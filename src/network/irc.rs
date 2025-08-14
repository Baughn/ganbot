use anyhow::{Context, Result, bail};
use irc::client::prelude::{Client, Command, Config};
use kameo::message::StreamMessage;
use kameo::prelude::*;
use kameo_actors::broker::{Broker, Publish};
use std::collections::HashMap;
use tokio::spawn;
use tokio::sync::OnceCell;
use tokio::time::{Duration, Instant};
use tracing::{error, info, instrument, trace};

use crate::actions::{self, Action};
use crate::config::global::IrcConfig;

pub struct IrcActor {
    name: String,
    config: IrcConfig,
    client: OnceCell<Client>,
    /// Buffer for joining split messages
    /// Keyed by (user, channel)
    message_buffer: HashMap<(String, Option<String>), BufferedMessage>,
    broker: actions::Broker,
}

#[derive(Debug, Clone)]
struct PrivMsg {
    channel: Option<String>,
    user: String,
    message: String,
}

struct BufferedMessage {
    content: String,
    last_updated: Instant,
}

struct Connect;

struct ProcessBufferedMessages;

impl Actor for IrcActor {
    type Args = (IrcConfig, actions::Broker);
    type Error = anyhow::Error;

    #[instrument(skip_all, fields(server = %args.0.server))]
    async fn on_start(
        args: Self::Args,
        actor_ref: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        tracing::info!("Starting IRC actor for server: {}", args.0.server);
        actor_ref.tell(Connect).try_send().unwrap();
        Ok(IrcActor {
            name: args.0.server.clone(),
            config: args.0,
            client: OnceCell::new(),
            message_buffer: HashMap::new(),
            broker: args.1,
        })
    }
}

impl IrcActor {
    async fn process_privmsg(&mut self, privmsg: PrivMsg) {
        // First off, is this a command?
        if let Some(command) = privmsg.message.strip_prefix(&self.config.command_prefix) {
            // Handle command logic here
            info!("Processing command: {}", privmsg.message);
            let (command, args) = command.split_once(' ').unwrap_or((command, ""));
            match command {
                "ping" => {
                    // Example command: respond to ping
                    self.broker
                        .tell(Publish {
                            topic: format!(
                                "command/{}/{}/ping",
                                privmsg.user,
                                privmsg.channel.unwrap_or("".to_string())
                            ),
                            message: Action::Ping,
                        })
                        .await
                        .unwrap();
                }
                _ => {
                    error!("Unknown command: {}", command);
                }
            }
        }
    }
}

impl Message<Connect> for IrcActor {
    type Reply = Result<()>;

    #[instrument(skip_all, fields(server = %self.name))]
    async fn handle(
        &mut self,
        msg: Connect,
        ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        tracing::info!("Connecting to IRC server: {}", self.config.server);
        if self.client.get().is_some() {
            bail!("Already connected to IRC server: {}", self.config.server);
        }

        // Create IRC client configuration
        let irc_config = Config {
            server: Some(self.config.server.clone()),
            port: Some(self.config.port),
            use_tls: Some(self.config.tls),
            nickname: Some(self.config.nick.clone()),
            channels: self.config.channels.clone(),
            ..Default::default()
        };

        // Try to connect
        let mut client = tokio::time::timeout(Duration::from_secs(20), async {
            Client::from_config(irc_config)
                .await
                .context("while creating IRC client")
        })
        .await??;

        // Identify to the server
        client
            .identify()
            .context("while identifying to IRC server")?;

        let stream = client.stream()?;
        ctx.actor_ref().attach_stream(stream, "start", "end");

        // Identify with NickServ if password is provided
        if let Some(password) = &self.config.nickserv_password {
            tracing::debug!("Authenticating with NickServ");
            client
                .send_privmsg("nickserv", format!("IDENTIFY {}", password))
                .context("while sending NickServ IDENTIFY command")?;
        }

        // Store the client in the actor
        self.client
            .set(client)
            .context("while setting IRC client")?;
        tracing::info!("Connected to IRC server: {}", self.config.server);

        Ok(())
    }
}

impl
    Message<
        StreamMessage<Result<irc::proto::Message, irc::error::Error>, &'static str, &'static str>,
    > for IrcActor
{
    type Reply = ();

    #[instrument(skip_all, fields(server = %self.name))]
    async fn handle(
        &mut self,
        msg: StreamMessage<Result<irc::proto::Message, irc::error::Error>, &str, &str>,
        ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        trace!("Received IRC message: {:?}", msg);

        match msg {
            StreamMessage::Next(Ok(message)) => {
                if let Err(e) = handle_irc_message(message, self, ctx).await {
                    error!("Error processing IRC message: {}", e);
                }
            }
            StreamMessage::Next(Err(e)) => {
                error!("Error in IRC stream: {}", e);
            }
            _ => {}
        }
    }
}

async fn handle_irc_message(
    message: irc::proto::Message,
    actor: &mut IrcActor,
    ctx: &mut kameo::prelude::Context<IrcActor, ()>,
) -> Result<()> {
    trace!("Handling IRC message: {:?}", message);

    match message.command {
        Command::PRIVMSG(ref target, ref text) => {
            let channel = if target.starts_with('#') {
                Some(target.clone())
            } else {
                None
            };

            let user = message.source_nickname().unwrap_or("unknown").to_string();
            let key = (user.clone(), channel.clone());

            // Check if we already have a buffered message from this user/channel
            if let Some(buffered) = actor.message_buffer.get_mut(&key) {
                // Append to existing message and update timestamp
                buffered.content.push(' ');
                buffered.content.push_str(&text);
                buffered.last_updated = Instant::now();
                trace!("Appending to buffered message from {:?}", key);
            } else {
                // Start a new buffered message
                actor.message_buffer.insert(
                    key.clone(),
                    BufferedMessage {
                        content: text.clone(),
                        last_updated: Instant::now(),
                    },
                );
                trace!("Starting new buffered message from {:?}", key);
            }

            // Either way, process the buffered messages in 500ms or so.
            let actor_ref = ctx.actor_ref().clone();
            spawn(async move {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let _ = actor_ref.tell(ProcessBufferedMessages).try_send();
            });
        }
        _ => {
            trace!("Unhandled IRC command: {:?}", message.command);
        }
    }

    Ok(())
}

impl Message<ProcessBufferedMessages> for IrcActor {
    type Reply = ();

    #[instrument(skip_all, fields(server = %self.name))]
    async fn handle(
        &mut self,
        _msg: ProcessBufferedMessages,
        ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        trace!("Processing buffered messages");

        let now = Instant::now();
        let mut to_flush = Vec::new();

        // Check and flush messages that have been idle for 500ms
        for (key, buffered) in &self.message_buffer {
            if now.duration_since(buffered.last_updated) >= Duration::from_millis(500) {
                to_flush.push(key.clone());
            }
        }

        // Process old messages
        for key in to_flush {
            if let Some(buffered) = self.message_buffer.remove(&key) {
                let (user, channel) = key;
                let privmsg = PrivMsg {
                    channel,
                    user,
                    message: buffered.content,
                };
                trace!("Flushing buffered message: {:?}", privmsg);
                self.process_privmsg(privmsg).await;
            }
        }
    }
}
