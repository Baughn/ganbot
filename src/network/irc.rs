use anyhow::{Context, Result, bail};
use futures::StreamExt;
use irc::client::prelude::{Client, Command, Config};
use kameo::prelude::*;
use std::collections::HashMap;
use tokio::sync::oneshot;
use tokio::time::{Duration, Instant, timeout};

use crate::config::global::IrcConfig;

pub struct IrcActor {
    name: String,
    config: IrcConfig,
    /// Handle to the IRC client task
    loop_aborter: tokio::task::AbortHandle,
    /// Handle for sending IRC messages
    sender: irc::client::Sender,
}

impl Actor for IrcActor {
    type Args = IrcConfig;
    type Error = anyhow::Error;

    #[tracing::instrument(skip_all, fields(server = %state.server))]
    async fn on_start(state: Self::Args, actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        tracing::info!("Starting IRC actor for server: {}", state.server);

        // Clone config for the client loop
        let config = state.clone();

        // Spawn the IRC client loop
        let (sender_tx, sender_rx) = oneshot::channel();
        let actor_ref_clone = actor_ref.clone();
        let connection_task = tokio::spawn(async move {
            client_loop(config, sender_tx, actor_ref_clone).await;
        });
        tracing::info!("Started IRC actor for server: {}", state.server);

        // Wait for the sender from the client task
        let sender = match tokio::time::timeout(Duration::from_secs(15), sender_rx).await {
            Ok(Ok(sender)) => {
                tracing::info!("Received IRC sender from client task");
                sender
            }
            Ok(Err(_)) => {
                tracing::error!("Client task dropped sender without sending");
                connection_task.abort();
                bail!("Failed to receive IRC sender from client task");
            }
            Err(_) => {
                tracing::error!("Timeout waiting for IRC sender from client task");
                connection_task.abort();
                bail!("Timeout waiting for IRC connection");
            }
        };

        // Kill actor if the connection task fails
        let loop_aborter = connection_task.abort_handle();
        tokio::spawn(async move {
            connection_task.await.unwrap_or_else(|e| {
                tracing::error!("IRC connection task failed: {}", e);
            });
            actor_ref.kill();
        });

        Ok(IrcActor {
            name: state.server.clone(),
            config: state,
            loop_aborter,
            sender,
        })
    }
}

/// Handler for messages sent by the client loop to the actor.
impl Message<IrcMessage> for IrcActor {
    type Reply = ();

    #[tracing::instrument(skip_all, fields(server = %self.name))]
    async fn handle(&mut self, msg: IrcMessage, ctx: &mut kameo::message::Context<Self, ()>) {
        // Handle incoming IRC messages here
        match msg {
            IrcMessage::Connected => {
                tracing::info!("IRC connected");
            }
            IrcMessage::PrivMsg(privmsg) => {
                tracing::debug!("IRC PRIVMSG from {}: {}", privmsg.user, privmsg.message);
                // So what do we do with this message?
                if let Some(command) = privmsg.message.strip_prefix(&self.config.command_prefix) {
                    tracing::info!("Received command: {}", command);
                    match command {
                        "ping" => {
                            tracing::debug!("Sending pong to {}", privmsg.user);
                            if let Err(e) = self.sender.send_privmsg(
                                privmsg.channel.as_deref().unwrap_or(&self.config.nick),
                                "pong".to_string(),
                            ) {
                                tracing::error!("Failed to send pong: {}", e);
                            } else {
                                tracing::info!("Sent pong to {}", privmsg.user);
                            }
                        }
                        _ => {
                            tracing::warn!("Unknown command: {}", command);
                        }
                    }
                } else {
                    // Chat-db stuff goes here.
                }
            }
            IrcMessage::Error(err) => {
                tracing::error!("IRC error: {}", err);
            }
        }
    }
}

/// Messages sent by the client loop to the actor.
#[derive(Debug)]
enum IrcMessage {
    Connected,
    PrivMsg(PrivMsg),
    Error(String),
}

#[derive(Debug)]
struct PrivMsg {
    /// The channel this message was sent to, if any.
    /// If this is a private message, this will be None.
    channel: Option<String>,
    user: String,
    message: String,
}

/// Tracks a message being buffered for potential joining with subsequent messages
struct BufferedMessage {
    content: String,
    last_updated: Instant,
}

/// The main IRC client loop that handles connecting, receiving messages, and sending them to the actor.
/// Exits on any error, including disconnection.
#[tracing::instrument(skip_all, fields(server = %config.server))]
async fn client_loop(
    config: IrcConfig,
    sender_tx: oneshot::Sender<irc::client::Sender>,
    actor_ref: ActorRef<IrcActor>,
) {
    let err = match client_loop_inner(config, sender_tx, actor_ref.clone()).await {
        Ok(_) => "IRC client loop completed successfully?!".to_string(),
        Err(e) => {
            format!("IRC client loop error: {}", e)
        }
    };
    tracing::error!("{}", err);
    // Notify the actor of the error
    actor_ref
        .tell(IrcMessage::Error(err))
        .await
        .expect("Failed to send error message to actor");
}

async fn client_loop_inner(
    config: IrcConfig,
    sender_tx: oneshot::Sender<irc::client::Sender>,
    actor_ref: ActorRef<IrcActor>,
) -> Result<()> {
    tracing::info!("Connecting to IRC server: {}", config.server);

    // Create IRC client configuration
    let irc_config = Config {
        server: Some(config.server.clone()),
        port: Some(config.port),
        use_tls: Some(config.tls),
        nickname: Some(config.nick.clone()),
        channels: config.channels.clone(),
        ..Default::default()
    };

    // Try to connect
    let mut client = tokio::time::timeout(Duration::from_secs(10), async {
        let client = Client::from_config(irc_config)
            .await
            .context("while creating IRC client")?;

        // Identify to the server
        client
            .identify()
            .context("while identifying to IRC server")?;

        Ok::<Client, anyhow::Error>(client)
    })
    .await??;

    // Send the client sender back to the actor
    let client_sender = client.sender();
    if sender_tx.send(client_sender).is_err() {
        bail!("Failed to send IRC sender back to actor - receiver dropped");
    }
    tracing::info!("IRC client sender sent to actor, starting message loop");

    // Handle NickServ authentication if configured
    if let Some(ref password) = config.nickserv_password {
        tracing::debug!("Authenticating with NickServ");
        if let Err(e) = client.send_privmsg("NickServ", format!("IDENTIFY {}", password)) {
            tracing::warn!("Failed to authenticate with NickServ: {}", e);
        }
    }

    // Send connected message
    actor_ref
        .tell(IrcMessage::Connected)
        .await
        .context("while sending connected message to actor")?;
    tracing::info!("Successfully connected to IRC server");

    // Buffer for joining split messages
    let mut message_buffer: HashMap<(String, Option<String>), BufferedMessage> = HashMap::new();

    // Main message handling loop
    let mut stream = client.stream()?;

    loop {
        // Use a short timeout to regularly check buffer ages
        match timeout(Duration::from_millis(50), stream.next()).await {
            Ok(Some(msg)) => {
                let msg = msg?;
                tracing::trace!("Received IRC message: {:?}", msg);

                match &msg.command {
                    Command::PRIVMSG(target, text) => {
                        let channel = if target.starts_with('#') || target.starts_with('&') {
                            Some(target.clone())
                        } else {
                            None
                        };

                        let user = msg.source_nickname().unwrap_or("unknown").to_string();

                        let key = (user, channel);

                        // Check if we already have a buffered message from this user/channel
                        if let Some(buffered) = message_buffer.get_mut(&key) {
                            // Append to existing message and update timestamp
                            buffered.content.push(' ');
                            buffered.content.push_str(text);
                            buffered.last_updated = Instant::now();
                            tracing::trace!("Appending to buffered message from {:?}", key);
                        } else {
                            // Start a new buffered message
                            message_buffer.insert(
                                key.clone(),
                                BufferedMessage {
                                    content: text.clone(),
                                    last_updated: Instant::now(),
                                },
                            );
                            tracing::trace!("Starting new buffered message from {:?}", key);
                        }
                    }
                    _ => {
                        tracing::trace!("Unhandled IRC message");
                    }
                }
            }
            Ok(None) => {
                // Stream ended - flush all remaining messages before exiting
                for ((user, channel), buffered) in message_buffer.drain() {
                    let privmsg = PrivMsg {
                        channel,
                        user,
                        message: buffered.content,
                    };
                    actor_ref
                        .tell(IrcMessage::PrivMsg(privmsg))
                        .await
                        .context("while sending final PRIVMSG to actor")?;
                }
                bail!("IRC stream ended");
            }
            Err(_) => {
                // Timeout - check if any messages need flushing
            }
        }

        // Check and flush messages that have been idle for 500ms
        let now = Instant::now();
        let mut to_flush = Vec::new();

        for (key, buffered) in &message_buffer {
            if now.duration_since(buffered.last_updated) >= Duration::from_millis(500) {
                to_flush.push(key.clone());
            }
        }

        // Flush old messages
        for key in to_flush {
            if let Some(buffered) = message_buffer.remove(&key) {
                let (user, channel) = key;
                let privmsg = PrivMsg {
                    channel,
                    user,
                    message: buffered.content,
                };
                tracing::debug!("Flushing buffered message: {:?}", privmsg);
                actor_ref
                    .tell(IrcMessage::PrivMsg(privmsg))
                    .await
                    .context("while sending PRIVMSG to actor")?;
            }
        }
    }
}
