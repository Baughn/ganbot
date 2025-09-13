use anyhow::{Context as AnyhowContext, Result};
use kameo::prelude::*;
use serenity::{
    all::{Command, Interaction},
    async_trait,
    model::{gateway::Ready, id::ApplicationId},
    prelude::*,
};
use tracing::{error, info, instrument};

use crate::config::global::DiscordConfig;
use crate::persistence::user::UserManager;

pub struct DiscordActor {
    name: String,
    config: DiscordConfig,
    client: Client,
    user_manager: ActorRef<UserManager>,
}

// Internal messages
// (none so far)

// Event handler for Discord events
struct Handler {
    actor_ref: ActorRef<DiscordActor>,
}

#[async_trait]
impl EventHandler for Handler {
    async fn interaction_create(&self, ctx: serenity::prelude::Context, interaction: Interaction) {
        if let Interaction::Command(command) = interaction {
            let response = match command.data.name.as_str() {
                "ping" => "Pong!".to_string(),
                _ => "Unknown command".to_string(),
            };

            let builder = serenity::all::CreateInteractionResponse::Message(
                serenity::all::CreateInteractionResponseMessage::new().content(response),
            );

            if let Err(e) = command.create_response(&ctx.http, builder).await {
                error!("Failed to respond to Discord interaction: {}", e);
            }
        }
    }

    async fn ready(&self, ctx: serenity::prelude::Context, ready: Ready) {
        info!("Discord bot is connected as {}", ready.user.name);

        // Register slash commands
        let commands = vec![serenity::all::CreateCommand::new("ping").description("Ping the bot")];

        if let Err(e) = Command::set_global_commands(&ctx.http, commands).await {
            error!("Failed to register Discord slash commands: {}", e);
        } else {
            info!("Discord slash commands registered");
        }
    }
}

impl Actor for DiscordActor {
    type Args = DiscordConfig;
    type Error = anyhow::Error;

    #[instrument(skip_all, fields(bot_name = %args.token[..10]))]
    async fn on_start(args: Self::Args, actor_ref: ActorRef<Self>) -> Result<Self> {
        info!("Starting Discord actor");
        Ok(DiscordActor {
            name: "Discord".to_string(),
            client: connect_discord(actor_ref, &args).await?,
            config: args,
            user_manager: UserManager::get().with_context(|| "while getting UserManager")?,
        })
    }
}

async fn connect_discord(
    actor_ref: ActorRef<DiscordActor>,
    config: &DiscordConfig,
) -> Result<Client> {
    info!("Connecting to Discord...");

    // Create intents
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    // Create client
    let mut client = serenity::Client::builder(&config.token, intents)
        .event_handler(Handler { actor_ref })
        .application_id(ApplicationId::new(config.application_id))
        .await
        .context("Failed to create Discord client")?;

    // Start the client.
    client
        .start()
        .await
        .context("Failed to start Discord client")?;

    info!("Discord client started");
    Ok(client)
}
