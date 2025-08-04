use anyhow::{bail, Context, Result};
use kameo::prelude::*;
use tokio::sync::SetOnce;
use irc::client::prelude::{Client, Config, Command};
use futures::StreamExt;

use crate::config::global::IrcConfig;



pub struct IrcActor {
    name: String,
    /// Handle to the IRC client task
    connection_task: tokio::task::JoinHandle<()>,
    /// Handle for sending IRC messages
    sender: irc::client::Sender,
}

impl Actor for IrcActor {
    type Args = IrcConfig;
    type Error = anyhow::Error;

    #[tracing::instrument(skip(actor_ref))]
    async fn on_start(state: Self::Args, actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        tracing::info!("Starting IRC actor for server: {}", state.server);
        
        // Clone config for the client loop
        let config = state.clone();
        
        // Spawn the IRC client loop
        let sender_mbox = tokio::sync::SetOnce::new();
        let sender_mbox_clone = sender_mbox.clone();
        let connection_task = tokio::spawn(async move {
            client_loop(config, sender_mbox_clone, actor_ref).await;
        });
        
        Ok(IrcActor {
            name: state.server,
            connection_task,
            sender: sender_mbox.into_inner().expect("Sender should be set by client loop"),
        })
    }
}


/// Handler for messages sent by the client loop to the actor.
impl Message<IrcMessage> for IrcActor {
    type Reply = ();

    #[tracing::instrument(skip_all, fields(server = %self.name))]
    async fn handle(
        &mut self,
        msg: IrcMessage,
        ctx: &mut kameo::message::Context<Self, ()>
    ) {
        // Handle incoming IRC messages here
        match msg {
            IrcMessage::Connected => {
                tracing::info!("IRC connected");
            }
            IrcMessage::PrivMsg(privmsg) => {
                tracing::info!(
                    "IRC PRIVMSG from {}: {}",
                    privmsg.user,
                    privmsg.message
                );
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


/// The main IRC client loop that handles connecting, receiving messages, and sending them to the actor.
/// Exits on any error, including disconnection.
#[tracing::instrument(fields(server = %config.server))]
async fn client_loop(
    config: IrcConfig,
    sender_mbox: tokio::sync::SetOnce<irc::client::Sender>,
    actor_ref: ActorRef<IrcActor>,
) {
    let err = match client_loop_inner(config, sender_mbox, actor_ref.clone()).await {
        Ok(_) => {
            "IRC client loop completed successfully?!".to_string()
        }
        Err(e) => {
            format!("IRC client loop error: {}", e)
        }
    };
    tracing::error!("{}", err);
    // Notify the actor of the error
    actor_ref.tell(IrcMessage::Error(err)).await
        .expect("Failed to send error message to actor");
}


async fn client_loop_inner(
    config: IrcConfig,
    sender_mbox: SetOnce<irc::client::Sender>,
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
    let mut client = Client::from_config(irc_config).await
        .context("while creating IRC client")?;
    
    // Identify to the server
    client.identify()
        .context("while identifying to IRC server")?;

    // Send connected message
    actor_ref.tell(IrcMessage::Connected).await
        .context("while sending connected message to actor")?;
    tracing::info!("Successfully connected to IRC server");
    
    // Handle NickServ authentication if configured
    if let Some(ref password) = config.nickserv_password {
        tracing::debug!("Authenticating with NickServ");
        if let Err(e) = client.send_privmsg("NickServ", format!("IDENTIFY {}", password)) {
            tracing::warn!("Failed to authenticate with NickServ: {}", e);
        }
    }
    
    // Main message handling loop
    sender_mbox.set(client.sender())
        .expect("Failed to set sender mbox");
    let mut stream = client.stream()?;
    while let Some(msg) = stream.next().await.transpose()? {
        tracing::trace!("Received IRC message: {:?}", msg);
        
        match &msg.command {
            Command::PRIVMSG(target, text) => {
                let channel = if target.starts_with('#') || target.starts_with('&') {
                    Some(target.clone())
                } else {
                    None
                };
                
                let user = msg.source_nickname()
                    .unwrap_or("unknown")
                    .to_string();
                
                let privmsg = PrivMsg {
                    channel,
                    user,
                    message: text.clone(),
                };

                actor_ref.tell(IrcMessage::PrivMsg(privmsg)).await
                    .context("while sending PRIVMSG to actor")?;
            },
            _ => {
                tracing::trace!("Unhandled IRC message");
            }
        }
    }

    bail!("IRC client loop exited unexpectedly");
}
