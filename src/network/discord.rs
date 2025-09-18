use std::{
    collections::{HashMap, VecDeque},
    time::Instant,
};

use anyhow::{Context as AnyhowContext, Result};
use kameo::prelude::*;
use serenity::{
    Client,
    all::{
        self, ButtonStyle, Command, CommandDataOptionValue, CommandInteraction, CommandOptionType,
        ComponentInteraction, CreateCommand, CreateCommandOption, EventHandler,
        GatewayIntents, Http, Interaction, ModalInteraction,
    },
    async_trait,
    model::{
        gateway::Ready,
        id::ApplicationId,
    },
};
use tracing::{error, info, instrument, warn};

use crate::{
    actions::{broker::ActionBroker, prompt::PromptActor},
    config::global::DiscordConfig,
    persistence::user::{GetUser, UserActor, UserId, UserManager},
};

const MAX_GALLERY_STATES: usize = 128;
const SELECT_BUTTON_PREFIX: &str = "select";
const EDIT_BUTTON_PREFIX: &str = "edit";
const RETRY_BUTTON_PREFIX: &str = "retry";
const EDIT_MODAL_PREFIX: &str = "edit-modal";
const RETRY_MODAL_PREFIX: &str = "retry-modal";

#[derive(Clone)]
struct ButtonDescriptor {
    custom_id: String,
    label: String,
    style: ButtonStyle,
}

type ButtonLayout = Vec<Vec<ButtonDescriptor>>;

struct GalleryState {
    created_at: Instant,
    owner_id: all::UserId,
    owner_name: String,
    gallery_url: Option<String>,
    image_urls: Vec<String>,
    prompts: Vec<String>,
    display_prompts: Vec<String>,
}

enum DiscordInteraction {
    Command {
        ctx: all::Context,
        command: CommandInteraction,
    },
    Component {
        ctx: all::Context,
        component: ComponentInteraction,
    },
    Modal {
        ctx: all::Context,
        modal: ModalInteraction,
    },
}

/// Helper methods for sending error responses mostly.
impl DiscordInteraction {
    fn ctx(&self) -> &all::Context {
        match self {
            DiscordInteraction::Command { ctx, .. } => ctx,
            DiscordInteraction::Component { ctx, .. } => ctx,
            DiscordInteraction::Modal { ctx, .. } => ctx,
        }
    }

    fn interaction_user_id(&self) -> all::UserId {
        match self {
            DiscordInteraction::Command { command, .. } => command.user.id,
            DiscordInteraction::Component { component, .. } => component.user.id,
            DiscordInteraction::Modal { modal, .. } => modal.user.id,
        }
    }

    fn interaction_channel_id(&self) -> all::ChannelId {
        match self {
            DiscordInteraction::Command { command, .. } => command.channel_id,
            DiscordInteraction::Component { component, .. } => component.channel_id,
            DiscordInteraction::Modal { modal, .. } => modal.channel_id,
        }
    }
}

pub struct DiscordActor {
    config: DiscordConfig,
    client: Client,
    user_manager: ActorRef<UserManager>,
    broker: ActorRef<ActionBroker>,
}

struct Handler {
    actor_ref: ActorRef<DiscordActor>,
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: all::Context, ready: Ready) {
        info!("Discord bot connected as {}", ready.user.name);
        if let Err(err) = register_commands(&ctx.http).await {
            error!("Failed to register Discord commands: {err:#}");
        }

        let guild_ids: Vec<_> = ready.guilds.iter().map(|status| status.id).collect();

        if !guild_ids.is_empty() {
            info!(?guild_ids, "Joined guilds");
        }
    }

    async fn interaction_create(&self, ctx: all::Context, interaction: Interaction) {
        let event = match interaction {
            Interaction::Command(command) => Some(DiscordInteraction::Command { ctx, command }),
            Interaction::Component(component) => {
                Some(DiscordInteraction::Component { ctx, component })
            }
            Interaction::Modal(modal) => Some(DiscordInteraction::Modal { ctx, modal }),
            _ => None,
        };

        if let Some(event) = event {
            if let Err(err) = self.actor_ref.tell(event).send().await {
                error!("Discord actor is unavailable: {err:#}");
            }
        }
    }
}

impl Actor for DiscordActor {
    type Args = DiscordConfig;
    type Error = anyhow::Error;

    #[instrument(skip_all, fields(app_id = args.application_id))]
    async fn on_start(args: Self::Args, actor_ref: ActorRef<Self>) -> Result<Self> {
        info!("Starting Discord actor");
        let client = connect_discord(actor_ref.clone(), &args).await?;
        let user_manager =
            UserManager::get().with_context(|| "while retrieving user manager actor")?;

        Ok(Self {
            config: args,
            client,
            user_manager,
            broker: ActionBroker::get().with_context(|| "while retrieving action broker actor")?,
        })
    }
}

async fn connect_discord(
    actor_ref: ActorRef<DiscordActor>,
    config: &DiscordConfig,
) -> Result<Client> {
    info!("Connecting to Discord...");
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let mut client = serenity::Client::builder(&config.token, intents)
        .event_handler(Handler { actor_ref })
        .application_id(ApplicationId::new(config.application_id))
        .await
        .context("Failed to create Discord client")?;

    client
        .start()
        .await
        .context("Failed to start Discord client")?;

    Ok(client)
}

fn build_commands() -> Vec<CreateCommand> {
    vec![
        CreateCommand::new("prompt")
            .description("Generate an image from a prompt")
            .add_option(
                CreateCommandOption::new(CommandOptionType::String, "prompt", "Prompt text")
                    .required(true),
            ),
        CreateCommand::new("dream")
            .description("Let the detective dream up images")
            .add_option(
                CreateCommandOption::new(CommandOptionType::String, "dream", "What to dream")
                    .required(true),
            ),
        CreateCommand::new("select")
            .description("Select an image URL for editing")
            .add_option(
                CreateCommandOption::new(CommandOptionType::String, "url", "Image URL")
                    .required(true),
            ),
    ]
}

async fn register_commands(http: &Http) -> Result<()> {
    let commands = build_commands();
    Command::set_global_commands(http, commands)
        .await
        .context("while registering commands")?;
    Ok(())
}

impl kameo::prelude::Message<DiscordInteraction> for DiscordActor {
    type Reply = ();

    async fn handle(
        &mut self,
        event: DiscordInteraction,
        ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if let Err(err) = self.handle_interaction(ctx, &event).await {
            error!("Failed to handle Discord interaction: {err:#}");
            // Make an attempt to notify the user about the error.
            match event.interaction_channel_id().say(&event.ctx().http, 
                format!("An error occurred while processing your request. Please try again later.\n{err}")
            ).await {
                Ok(_) => {}
                Err(send_err) => {
                    error!("Failed to send error message to Discord: {send_err:#}");
                }
            }
        }
    }
}

impl DiscordActor {
    async fn handle_interaction(
        &mut self,
        ctx: &mut Context<Self, ()>,
        event: &DiscordInteraction,
    ) -> Result<()> {
        match event {
            DiscordInteraction::Command {
                ctx: discord_ctx,
                command,
            } => self.handle_command(ctx, discord_ctx, command).await,
            _ => todo!(),
            // DiscordInteraction::Component {
            //     ctx: discord_ctx,
            //     component,
            // } => self.handle_component(ctx, discord_ctx, component).await,
            // DiscordInteraction::Modal {
            //     ctx: discord_ctx,
            //     modal,
            // } => self.handle_modal(ctx, discord_ctx, modal).await,
        }
    }

    async fn handle_command(
        &mut self,
        ctx: &mut Context<Self, ()>,
        discord_ctx: &all::Context,
        command: &CommandInteraction,
    ) -> Result<()> {
        fn read_string_option(command: &CommandInteraction, name: &str) -> Option<String> {
            command
                .data
                .options
                .iter()
                .find(|opt| opt.name == name)
                .and_then(|opt| match &opt.value {
                    CommandDataOptionValue::String(s) => Some(s.clone()),
                    _ => None,
                })
        }

        match command.data.name.as_str() {
            "prompt" => {
                let prompt = read_string_option(&command, "prompt")
                    .ok_or_else(|| anyhow::anyhow!("Prompt text is required"))?;
                self.handle_prompt_command(ctx, &discord_ctx, command, prompt)
                    .await
            }
            "dream" => {
                let request = read_string_option(&command, "request")
                    .ok_or_else(|| anyhow::anyhow!("Dream request is required"))?;
                self.handle_dream_command(ctx, &discord_ctx, command, request)
                    .await
            }
            "select" => {
                let url = read_string_option(&command, "url")
                    .ok_or_else(|| anyhow::anyhow!("Image URL is required"))?;
                self.handle_select_command(ctx, &discord_ctx, command, url)
                    .await
            }
            other => {
                warn!(?other, "Unhandled command");
                Ok(())
            }
        }
    }

    async fn handle_prompt_command(
        &mut self,
        ctx: &mut Context<Self, ()>,
        discord_ctx: &all::Context,
        command: &CommandInteraction,
        input: String,
    ) -> Result<()> {
        command
            .defer(&discord_ctx.http)
            .await
            .context("while deferring prompt response")?;

        let user_actor = self
            .get_user_actor(command.user.id, &command.user.name)
            .await?;

        let prompt_actor =
            PromptActor::spawn_link(&ctx.actor_ref(), PromptActor::new(user_actor.clone()).await)
                .await;
        let result = prompt_actor
            .ask(input.clone())
            .await
            .context("while executing prompt command")?;

        todo!()
    }

    async fn handle_dream_command(
        &mut self,
        ctx: &mut Context<Self, ()>,
        discord_ctx: &all::Context,
        command: &CommandInteraction,
        request: String,
    ) -> Result<()> {
        todo!()
    }

    async fn handle_select_command(
        &mut self,
        ctx: &mut Context<Self, ()>,
        discord_ctx: &all::Context,
        command: &CommandInteraction,
        url: String,
    ) -> Result<()> {
        todo!()
    }

    async fn get_user_actor(
        &self,
        discord_user_id: all::UserId,
        discord_user_name: &str,
    ) -> Result<ActorRef<UserActor>> {
        let user_id = UserId::Discord(discord_user_id);
        let user = self
            .user_manager
            .ask(GetUser(user_id.clone(), discord_user_name.to_string()))
            .await
            .with_context(|| format!("while retrieving user {user_id:?}"))?;

        Ok(user)
    }
}
