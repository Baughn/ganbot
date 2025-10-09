use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as AnyhowContext, Result};
use kameo::prelude::*;
use serenity::{
    Client,
    all::{
        self, ButtonStyle, Command, CommandDataOptionValue, CommandInteraction, CommandOptionType,
        ComponentInteraction, CreateActionRow, CreateAllowedMentions, CreateButton, CreateCommand,
        CreateCommandOption, CreateInputText, CreateInteractionResponse,
        CreateInteractionResponseFollowup, CreateMessage, CreateModal, EditMessage, EventHandler,
        GatewayIntents, Http, InputTextStyle, Interaction, InteractionResponseFlags,
        ModalInteraction,
    },
    async_trait,
    builder::CreateInteractionResponseMessage,
    model::{channel::MessageFlags, gateway::Ready, id::ApplicationId},
};
use tokio::task::JoinHandle;
use tracing::{Level, debug, error, info, instrument, trace, warn};

use crate::{
    actions::{
        ActionId, ActionOrigin, ActionPayload, ActionResponse, ActionStatus, SubmitAction,
        broker::{ActionBroker, RegisterDiscord},
    },
    config::global::DiscordConfig,
    persistence::{
        images::{
            AssociateMessage, GalleryMetadata, GalleryRegistry, GetGallery, GetGalleryByMessage,
            delete_image,
        },
        user::{GetUser, UserActor, UserId, UserManager},
    },
};

const GALLERY_BUTTON_PREFIX: &str = "gallery";
const PROMPT_RETRY_PREFIX: &str = "prompt_retry";
const PROMPT_DELETE_PREFIX: &str = "prompt_delete";
const PROMPT_DELETE_CONFIRM_PREFIX: &str = "prompt_delete_confirm";
const DISCORD_MESSAGE_LIMIT: usize = 2000;

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

#[derive(Debug, Clone)]
pub struct BrokerActionUpdate {
    pub id: ActionId,
    pub origin: ActionOrigin,
    pub status: ActionStatus,
}

#[derive(Debug, Clone)]
struct RegisterProgressActor {
    action_id: ActionId,
    actor: ActorRef<DiscordProgressActor>,
}

#[derive(Debug, Clone)]
struct ProgressActorFinished {
    action_id: ActionId,
}

enum DiscordCommandJob {
    Prompt {
        ctx: all::Context,
        command: CommandInteraction,
        input: String,
    },
    Dream {
        ctx: all::Context,
        command: CommandInteraction,
        input: String,
    },
}

#[derive(Actor)]
struct DiscordProgressActor {
    http: Arc<Http>,
    gallery_registry: ActorRef<GalleryRegistry>,
    parent: WeakActorRef<DiscordActor>,
    action_id: ActionId,
    last_origin: Option<ActionOrigin>,
    fallback_preface: Option<String>,
}

impl DiscordProgressActor {
    fn new(
        http: Arc<Http>,
        gallery_registry: ActorRef<GalleryRegistry>,
        parent: WeakActorRef<DiscordActor>,
        action_id: ActionId,
        origin: ActionOrigin,
    ) -> Self {
        let fallback_preface = match &origin {
            ActionOrigin::Discord {
                progress_message, ..
            } => progress_message.clone(),
            _ => None,
        };

        Self {
            http,
            gallery_registry,
            parent,
            action_id,
            last_origin: Some(origin),
            fallback_preface,
        }
    }

    fn resolve_context(&mut self, update: &BrokerActionUpdate) -> Option<(u64, u64, u64, String)> {
        if let ActionOrigin::Discord {
            channel_id,
            message_id,
            user_id,
            progress_message,
            ..
        } = &update.origin
        {
            let preface = progress_message
                .clone()
                .or_else(|| self.fallback_preface.clone())
                .unwrap_or_else(|| format!("Action `{}`", update.id));
            self.fallback_preface = Some(preface.clone());
            self.last_origin = Some(update.origin.clone());
            return Some((*channel_id, *message_id, *user_id, preface));
        }

        if let Some(ActionOrigin::Discord {
            channel_id,
            message_id,
            user_id,
            progress_message,
            ..
        }) = &self.last_origin
        {
            let preface = progress_message
                .clone()
                .or_else(|| self.fallback_preface.clone())
                .unwrap_or_else(|| format!("Action `{}`", update.id));
            return Some((*channel_id, *message_id, *user_id, preface));
        }

        None
    }

    async fn handle_update(
        &mut self,
        update: BrokerActionUpdate,
        ctx: &mut Context<Self, ()>,
    ) -> Result<()> {
        trace_http_ratelimiter(&self.http, "discord_progress_actor.handle_update").await;

        let Some((channel_id, message_id, user_id, preface)) = self.resolve_context(&update) else {
            trace!(action = %update.id, "Dropping broker update with no Discord context");
            return Ok(());
        };

        match update.status {
            ActionStatus::Queued => {
                update_progress_message_impl(
                    &self.http,
                    channel_id,
                    message_id,
                    Some(preface),
                    format!("Status: queued (`{}`)", update.id),
                )
                .await
            }
            ActionStatus::Started => {
                update_progress_message_impl(
                    &self.http,
                    channel_id,
                    message_id,
                    Some(preface),
                    "Status: submitted to generator; waiting for backend scheduling".to_string(),
                )
                .await
            }
            ActionStatus::Progress(progress) => {
                let status_line = if let Some(percent) = progress.percent {
                    let percent = percent.clamp(0.0, 100.0);
                    format!("Status: {:.0}% - {}", percent, progress.message)
                } else {
                    format!("Status: {}", progress.message)
                };
                update_progress_message_impl(
                    &self.http,
                    channel_id,
                    message_id,
                    Some(preface),
                    status_line,
                )
                .await
            }
            ActionStatus::Completed(completed) => {
                delete_progress_message_impl(&self.http, channel_id, message_id).await;
                send_completion_message_impl(
                    &self.http,
                    &self.gallery_registry,
                    channel_id,
                    user_id,
                    completed.response,
                    Some(preface),
                )
                .await?;
                self.finish(ctx).await;
                Ok(())
            }
            ActionStatus::Failed(failure) => {
                delete_progress_message_impl(&self.http, channel_id, message_id).await;
                send_failure_message_impl(
                    &self.http,
                    channel_id,
                    user_id,
                    failure.error,
                    failure.retry_scheduled,
                    Some(preface),
                )
                .await?;
                self.finish(ctx).await;
                Ok(())
            }
        }
    }

    async fn finish(&self, ctx: &mut Context<Self, ()>) {
        if let Some(parent) = self.parent.upgrade() {
            if let Err(err) = parent
                .tell(ProgressActorFinished {
                    action_id: self.action_id,
                })
                .send()
                .await
            {
                warn!(action = %self.action_id, "Failed to notify Discord actor about completion: {err:#}");
            }
        }

        ctx.stop();
    }
}

impl Message<BrokerActionUpdate> for DiscordProgressActor {
    type Reply = ();

    #[instrument(skip_all, name = "discord_progress_actor.handle_update_msg", fields(action = %msg.id))]
    async fn handle(
        &mut self,
        msg: BrokerActionUpdate,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if let Err(err) = self.handle_update(msg, ctx).await {
            error!(action = %self.action_id, "Failed to process broker update: {err:#}");
        }
    }
}

#[derive(Actor)]
struct DiscordCommandActor {
    http: Arc<Http>,
    broker: ActorRef<ActionBroker>,
    user_manager: ActorRef<UserManager>,
    application_id: u64,
    gallery_registry: ActorRef<GalleryRegistry>,
    parent: WeakActorRef<DiscordActor>,
}

impl DiscordCommandActor {
    fn new(
        http: Arc<Http>,
        broker: ActorRef<ActionBroker>,
        user_manager: ActorRef<UserManager>,
        application_id: u64,
        gallery_registry: ActorRef<GalleryRegistry>,
        parent: WeakActorRef<DiscordActor>,
    ) -> Self {
        Self {
            http,
            broker,
            user_manager,
            application_id,
            gallery_registry,
            parent,
        }
    }

    async fn handle_job(&mut self, job: DiscordCommandJob) -> Result<()> {
        match job {
            DiscordCommandJob::Prompt {
                ctx,
                command,
                input,
            } => {
                if let Err(err) = self
                    .handle_generation_command(
                        &ctx,
                        &command,
                        input.as_str(),
                        "Prompt",
                        |user_id, user_name, input| ActionPayload::Prompt {
                            user_id,
                            user_name,
                            input,
                        },
                    )
                    .await
                {
                    self.handle_command_error(&ctx, &command, "Prompt", &input, err)
                        .await;
                }
                Ok(())
            }
            DiscordCommandJob::Dream {
                ctx,
                command,
                input,
            } => {
                if let Err(err) = self
                    .handle_generation_command(
                        &ctx,
                        &command,
                        input.as_str(),
                        "Dream",
                        |user_id, user_name, input| ActionPayload::Dream {
                            user_id,
                            user_name,
                            input,
                        },
                    )
                    .await
                {
                    self.handle_command_error(&ctx, &command, "Dream", &input, err)
                        .await;
                }
                Ok(())
            }
        }
    }

    async fn handle_generation_command<F>(
        &mut self,
        discord_ctx: &all::Context,
        command: &CommandInteraction,
        input: &str,
        label: &str,
        payload_factory: F,
    ) -> Result<()>
    where
        F: Fn(UserId, String, String) -> ActionPayload,
    {
        // Ensure the user actor is instantiated so downstream actions have state ready.
        let _ =
            get_user_actor_impl(&self.user_manager, command.user.id, &command.user.name).await?;

        let preface = format!("**{}**: {}", label, input);
        let initial_content = format!("{preface}\nStatus: preparing request…");

        let followup_builder = CreateInteractionResponseFollowup::new()
            .content(initial_content)
            .allowed_mentions(CreateAllowedMentions::new());

        let followup = command
            .create_followup(&discord_ctx.http, followup_builder)
            .await
            .context("while publishing progress message")?;

        let origin = ActionOrigin::Discord {
            application_id: self.application_id,
            guild_id: command.guild_id.map(|id| id.get()),
            channel_id: followup.channel_id.get(),
            message_id: followup.id.get(),
            user_id: command.user.id.get(),
            progress_message: Some(preface.clone()),
        };

        let payload = payload_factory(
            UserId::Discord(command.user.id),
            command.user.name.clone(),
            input.to_string(),
        );

        let submit_origin = origin.clone();

        match self
            .broker
            .ask(SubmitAction::new(submit_origin, payload))
            .await
        {
            Ok(action_id) => {
                if self
                    .spawn_progress_actor(action_id, origin.clone())
                    .await
                    .is_none()
                {
                    update_progress_message_impl(
                        &self.http,
                        followup.channel_id.get(),
                        followup.id.get(),
                        Some(preface),
                        format!("Status: queued (`{action_id}`)"),
                    )
                    .await
                    .with_context(|| "while acknowledging queued request")?;
                }
            }
            Err(err) => {
                delete_progress_message_impl(
                    &self.http,
                    followup.channel_id.get(),
                    followup.id.get(),
                )
                .await;
                send_failure_message_impl(
                    &self.http,
                    followup.channel_id.get(),
                    command.user.id.get(),
                    format!("failed to queue request: {err:#}"),
                    false,
                    Some(preface),
                )
                .await?;
            }
        }

        Ok(())
    }

    async fn spawn_progress_actor(
        &self,
        action_id: ActionId,
        origin: ActionOrigin,
    ) -> Option<ActorRef<DiscordProgressActor>> {
        let actor = DiscordProgressActor::spawn(DiscordProgressActor::new(
            self.http.clone(),
            self.gallery_registry.clone(),
            self.parent.clone(),
            action_id,
            origin.clone(),
        ));

        if let Some(parent) = self.parent.upgrade() {
            if let Err(err) = parent
                .tell(RegisterProgressActor {
                    action_id,
                    actor: actor.clone(),
                })
                .send()
                .await
            {
                warn!(action = %action_id, "Failed to register progress actor: {err:#}");
                return None;
            }
        } else {
            warn!(action = %action_id, "Discord actor unavailable when registering progress actor");
            return None;
        }

        if let Err(err) = actor
            .tell(BrokerActionUpdate {
                id: action_id,
                origin,
                status: ActionStatus::Queued,
            })
            .send()
            .await
        {
            warn!(action = %action_id, "Failed to send initial queued update: {err:#}");
        }

        Some(actor)
    }

    async fn handle_command_error(
        &self,
        discord_ctx: &all::Context,
        command: &CommandInteraction,
        label: &str,
        input: &str,
        err: anyhow::Error,
    ) {
        error!(
            ?label,
            user_id = command.user.id.get(),
            "Failed to process Discord {label} command: {err:#}"
        );

        let content = format!(
            "An error occurred while processing **{}** (`{}`):\n{:#}",
            label, input, err
        );

        let followup_builder = CreateInteractionResponseFollowup::new()
            .content(content)
            .flags(MessageFlags::EPHEMERAL)
            .allowed_mentions(CreateAllowedMentions::new());

        if let Err(send_err) = command
            .create_followup(&discord_ctx.http, followup_builder)
            .await
        {
            error!("Failed to deliver error followup: {send_err:#}");
        }
    }
}

impl Message<DiscordCommandJob> for DiscordCommandActor {
    type Reply = ();

    #[instrument(skip_all, name = "discord_command_actor.handle_job_msg")]
    async fn handle(
        &mut self,
        job: DiscordCommandJob,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if let Err(err) = self.handle_job(job).await {
            error!("Discord command actor failed: {err:#}");
        }
    }
}

pub struct DiscordActor {
    config: DiscordConfig,
    http: Arc<Http>,
    client_task: JoinHandle<()>,
    user_manager: ActorRef<UserManager>,
    broker: ActorRef<ActionBroker>,
    gallery_registry: ActorRef<GalleryRegistry>,
    progress_sessions: HashMap<ActionId, ActorRef<DiscordProgressActor>>,
}

struct Handler {
    actor_ref: ActorRef<DiscordActor>,
}

struct ResponseSegment {
    prefix: &'static str,
    text: String,
    truncatable: bool,
}

impl ResponseSegment {
    fn new(prefix: &'static str, text: String, truncatable: bool) -> Self {
        Self {
            prefix,
            text,
            truncatable,
        }
    }

    fn len(&self) -> usize {
        self.prefix.len() + self.text.len()
    }

    fn render(&self) -> String {
        format!("{}{}", self.prefix, self.text)
    }
}

async fn update_progress_message_impl(
    http: &Arc<Http>,
    channel_id: u64,
    message_id: u64,
    preface: Option<String>,
    status_line: String,
) -> Result<()> {
    let mut lines = Vec::new();
    if let Some(preface) = preface {
        lines.push(preface);
    }
    lines.push(status_line);
    let content = lines.join("\n");

    let edit = EditMessage::new().content(content);

    all::ChannelId::new(channel_id)
        .edit_message(http, message_id, edit)
        .await
        .with_context(|| "while updating Discord progress message")?;
    Ok(())
}

async fn delete_progress_message_impl(http: &Arc<Http>, channel_id: u64, message_id: u64) {
    if let Err(err) = all::ChannelId::new(channel_id)
        .delete_message(http, message_id)
        .await
    {
        warn!(
            channel_id,
            message_id, "Failed to delete Discord progress message: {err:#}"
        );
    }
}

async fn send_completion_message_impl(
    http: &Arc<Http>,
    gallery_registry: &ActorRef<GalleryRegistry>,
    channel_id: u64,
    user_id: u64,
    response: ActionResponse,
    preface: Option<String>,
) -> Result<()> {
    let ActionResponse {
        lines: response_lines,
        gallery,
        ..
    } = response;

    let mut lines = Vec::with_capacity(response_lines.len() + 1);
    lines.push(format!("<@{}>", user_id));
    if let Some(_preface) = preface {
        // For now, ignore it!
        // The preface contains the prompt, which is duplicated inside the image.
    }
    lines.extend(response_lines);
    let content = lines.join("\n");

    let user = all::UserId::new(user_id);
    let allowed_mentions = CreateAllowedMentions::new().users(vec![user]);

    let mut builder = CreateMessage::new()
        .content(content)
        .allowed_mentions(allowed_mentions);

    let mut gallery_to_associate: Option<String> = None;

    if let Some(gallery_ref) = &gallery {
        match gallery_registry
            .ask(GetGallery(gallery_ref.id.clone()))
            .await
            .context("while loading gallery metadata for completion message")?
        {
            Some(metadata) => {
                let mut rows = vec![
                    // Delete & retry buttons
                    DiscordActor::build_prompt_action_buttons(&metadata.id, user_id),
                ];
                rows.extend(DiscordActor::build_gallery_components(&metadata));

                builder = builder.components(rows);
                gallery_to_associate = Some(metadata.id);
            }
            None => {
                warn!(gallery_id = %gallery_ref.id, "Gallery metadata missing; omitting buttons");
            }
        }
    }

    let message = all::ChannelId::new(channel_id)
        .send_message(http, builder)
        .await
        .with_context(|| "while sending Discord completion message")?;

    if let Some(gallery_id) = gallery_to_associate {
        if let Err(err) = gallery_registry
            .ask(AssociateMessage {
                gallery_id,
                channel_id,
                message_id: message.id.get(),
            })
            .await
            .context("while associating gallery with message")
        {
            warn!("Failed to associate gallery message: {err:#}");
        }
    }

    Ok(())
}

async fn send_failure_message_impl(
    http: &Arc<Http>,
    channel_id: u64,
    user_id: u64,
    error: String,
    retry_scheduled: bool,
    preface: Option<String>,
) -> Result<()> {
    let mut lines = Vec::new();
    lines.push(format!("<@{}>", user_id));
    if let Some(preface) = preface {
        lines.push(preface);
    }
    lines.push(format!("Status: failed – {error}"));
    if retry_scheduled {
        lines.push("A retry has been scheduled.".to_string());
    }
    let content = lines.join("\n");

    let user = all::UserId::new(user_id);
    let allowed_mentions = CreateAllowedMentions::new().users(vec![user]);
    let builder = CreateMessage::new()
        .content(content)
        .allowed_mentions(allowed_mentions);

    all::ChannelId::new(channel_id)
        .send_message(http, builder)
        .await
        .with_context(|| "while sending Discord failure message")?;
    Ok(())
}

async fn get_user_actor_impl(
    user_manager: &ActorRef<UserManager>,
    discord_user_id: all::UserId,
    discord_user_name: &str,
) -> Result<ActorRef<UserActor>> {
    let user_id = UserId::Discord(discord_user_id);
    let user = user_manager
        .ask(GetUser(user_id.clone(), discord_user_name.to_string()))
        .await
        .with_context(|| format!("while retrieving user {user_id:?}"))?;

    Ok(user)
}

async fn trace_http_ratelimiter(http: &Arc<Http>, label: &str) {
    let Some(ratelimiter) = http.ratelimiter.as_ref() else {
        trace!(%label, "http ratelimiter not configured");
        return;
    };

    let routes = ratelimiter.routes();
    let buckets = {
        let guard = routes.read().await;
        guard
            .iter()
            .map(|(bucket, limiter)| (*bucket, Arc::clone(limiter)))
            .collect::<Vec<_>>()
    };

    if buckets.is_empty() {
        trace!(%label, "http ratelimiter has no tracked buckets");
        return;
    }

    for (bucket, limiter) in buckets {
        let ratelimit = limiter.lock().await;
        let reset_after_ms = ratelimit.reset_after().map(|d| d.as_millis() as i64);
        let reset_epoch_s = ratelimit
            .reset()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        let reset_in_ms = ratelimit
            .reset()
            .and_then(|t| t.duration_since(SystemTime::now()).ok())
            .map(|d| d.as_millis() as i64);

        if ratelimit.remaining() == 0 {
            debug!(
                %label,
                bucket = ?bucket,
                remaining = ratelimit.remaining(),
                limit = ratelimit.limit(),
                reset_after_ms,
                reset_epoch_s,
                reset_in_ms,
                "http ratelimiter bucket empty",
            );
        }
        trace!(
            %label,
            bucket = ?bucket,
            remaining = ratelimit.remaining(),
            limit = ratelimit.limit(),
            reset_after_ms,
            reset_epoch_s,
            reset_in_ms,
            "http ratelimiter bucket state",
        );
    }
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
        let (http, client_task) = connect_discord(actor_ref.clone(), &args).await?;
        let user_manager =
            UserManager::get().with_context(|| "while retrieving user manager actor")?;
        let broker = ActionBroker::get().with_context(|| "while retrieving action broker actor")?;
        let gallery_registry =
            GalleryRegistry::get().with_context(|| "while retrieving gallery registry actor")?;

        if let Err(err) = broker
            .tell(RegisterDiscord {
                actor: actor_ref.clone(),
            })
            .send()
            .await
        {
            warn!("Failed to register Discord actor with broker: {err:#}");
        }

        Ok(Self {
            config: args,
            http,
            client_task,
            user_manager,
            broker,
            gallery_registry,
            progress_sessions: HashMap::new(),
        })
    }

    async fn on_stop(
        &mut self,
        _actor_ref: WeakActorRef<Self>,
        _reason: ActorStopReason,
    ) -> std::result::Result<(), Self::Error> {
        self.client_task.abort();
        Ok(())
    }
}

async fn connect_discord(
    actor_ref: ActorRef<DiscordActor>,
    config: &DiscordConfig,
) -> Result<(Arc<Http>, JoinHandle<()>)> {
    info!("Connecting to Discord...");
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let client = Client::builder(&config.token, intents)
        .event_handler(Handler { actor_ref })
        .application_id(ApplicationId::new(config.application_id))
        .await
        .context("Failed to create Discord client")?;

    let http = client.http.clone();
    let mut runner = client;
    let application_id = config.application_id;
    let handle = tokio::spawn(async move {
        if let Err(err) = runner.start().await {
            error!("Discord client {application_id} exited: {err:#}");
        }
    });

    Ok((http, handle))
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
                CreateCommandOption::new(
                    CommandOptionType::String,
                    "request",
                    "What should the detective dream about?",
                )
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

    #[instrument(skip_all, name = "discord_actor.handle_interaction_msg")]
    async fn handle(
        &mut self,
        event: DiscordInteraction,
        ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if let Err(err) = self.handle_interaction(ctx, event).await {
            error!("Failed to handle Discord interaction: {err:#}");
        }
    }
}

impl kameo::prelude::Message<BrokerActionUpdate> for DiscordActor {
    type Reply = ();

    #[instrument(skip_all, name = "discord_actor.handle_broker_update_msg", fields(action = %msg.id))]
    async fn handle(
        &mut self,
        msg: BrokerActionUpdate,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if let Err(err) = self.handle_broker_update(msg).await {
            error!("Failed to apply broker update: {err:#}");
        }
    }
}

impl kameo::prelude::Message<RegisterProgressActor> for DiscordActor {
    type Reply = ();

    #[instrument(skip_all, name = "discord_actor.register_progress_actor", fields(action = %msg.action_id))]
    async fn handle(
        &mut self,
        msg: RegisterProgressActor,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.progress_sessions.insert(msg.action_id, msg.actor);
    }
}

impl kameo::prelude::Message<ProgressActorFinished> for DiscordActor {
    type Reply = ();

    #[instrument(skip_all, name = "discord_actor.progress_actor_finished", fields(action = %msg.action_id))]
    async fn handle(
        &mut self,
        msg: ProgressActorFinished,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.progress_sessions.remove(&msg.action_id);
    }
}

impl DiscordActor {
    async fn handle_broker_update(&mut self, update: BrokerActionUpdate) -> Result<()> {
        trace_http_ratelimiter(&self.http, "discord_actor.handle_broker_update").await;

        if let Some(session) = self.progress_sessions.get(&update.id).cloned() {
            if let Err(err) = session.tell(update.clone()).send().await {
                warn!(action = %update.id, "Progress actor unavailable: {err:#}");
                self.progress_sessions.remove(&update.id);
            }
            return Ok(());
        }

        trace!(action = %update.id, "No registered progress actor for update");
        Ok(())
    }

    async fn handle_interaction(
        &mut self,
        ctx: &mut Context<Self, ()>,
        event: DiscordInteraction,
    ) -> Result<()> {
        trace_http_ratelimiter(&self.http, "discord_actor.handle_interaction").await;
        match event {
            DiscordInteraction::Command {
                ctx: discord_ctx,
                command,
            } => self.handle_command(ctx, discord_ctx, command).await,
            DiscordInteraction::Component {
                ctx: discord_ctx,
                component,
            } => self.handle_component(ctx, discord_ctx, component).await,
            DiscordInteraction::Modal {
                ctx: discord_ctx,
                modal,
            } => self.handle_modal(ctx, discord_ctx, modal).await,
        }
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn handle_command(
        &mut self,
        ctx: &mut Context<Self, ()>,
        discord_ctx: all::Context,
        command: CommandInteraction,
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

        let command_name = command.data.name.clone();
        let command_name_str = command_name.as_str();

        match command_name_str {
            "prompt" => {
                let prompt = read_string_option(&command, "prompt")
                    .ok_or_else(|| anyhow::anyhow!("Prompt text is required"))?;
                if let Err(err) = command.defer(&discord_ctx.http).await {
                    error!(
                        command = command_name_str,
                        user_id = command.user.id.get(),
                        "Failed to defer Discord interaction: {err:#}"
                    );
                    return Ok(());
                }
                self.dispatch_command_actor(
                    ctx,
                    DiscordCommandJob::Prompt {
                        ctx: discord_ctx,
                        command,
                        input: prompt,
                    },
                )
                .await
            }
            "dream" => {
                let request = read_string_option(&command, "request")
                    .ok_or_else(|| anyhow::anyhow!("Dream request is required"))?;
                if let Err(err) = command.defer(&discord_ctx.http).await {
                    error!(
                        command = command_name_str,
                        user_id = command.user.id.get(),
                        "Failed to defer Discord interaction: {err:#}"
                    );
                    return Ok(());
                }
                self.dispatch_command_actor(
                    ctx,
                    DiscordCommandJob::Dream {
                        ctx: discord_ctx,
                        command,
                        input: request,
                    },
                )
                .await
            }
            "select" => {
                let url = read_string_option(&command, "url")
                    .ok_or_else(|| anyhow::anyhow!("Image URL is required"))?;
                if let Err(err) = command.defer(&discord_ctx.http).await {
                    error!(
                        command = command_name_str,
                        user_id = command.user.id.get(),
                        "Failed to defer Discord interaction: {err:#}"
                    );
                    return Ok(());
                }
                self.handle_select_command(ctx, &discord_ctx, &command, url)
                    .await
            }
            other => {
                warn!(?other, "Unhandled command");
                Ok(())
            }
        }
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn handle_component(
        &mut self,
        ctx: &mut Context<Self, ()>,
        discord_ctx: all::Context,
        component: ComponentInteraction,
    ) -> Result<()> {
        let custom_id = component.data.custom_id.as_str();

        if custom_id.starts_with(GALLERY_BUTTON_PREFIX) {
            return self.handle_gallery_component(&component).await;
        }

        if custom_id.starts_with(PROMPT_RETRY_PREFIX) {
            return self.handle_retry_button(&component).await;
        }

        if custom_id.starts_with(PROMPT_DELETE_CONFIRM_PREFIX) {
            return self
                .handle_delete_confirm_button(ctx, &discord_ctx, &component)
                .await;
        }

        if custom_id.starts_with(PROMPT_DELETE_PREFIX) {
            return self.handle_delete_button(&component).await;
        }

        if custom_id == "delete_cancel" {
            // Replace the confirmation message with a simple notice so Discord
            // doesn't reject the update for being empty.
            let response = CreateInteractionResponseMessage::new()
                .content("Deletion cancelled.")
                .components(vec![]); // Remove buttons

            return component
                .create_response(
                    &self.http,
                    CreateInteractionResponse::UpdateMessage(response),
                )
                .await
                .with_context(|| "while dismissing delete confirmation");
        }

        warn!(custom_id, "Unhandled component interaction");
        self.respond_component_message(&component, "This control is not implemented yet.")
            .await
    }

    async fn dispatch_command_actor(
        &mut self,
        ctx: &mut Context<Self, ()>,
        job: DiscordCommandJob,
    ) -> Result<()> {
        let parent = ctx.actor_ref().downgrade();
        let actor = DiscordCommandActor::spawn(DiscordCommandActor::new(
            self.http.clone(),
            self.broker.clone(),
            self.user_manager.clone(),
            self.config.application_id,
            self.gallery_registry.clone(),
            parent,
        ));

        actor
            .tell(job)
            .send()
            .await
            .context("while dispatching Discord command actor")
    }

    fn build_gallery_response_content(metadata: &GalleryMetadata, image_index: usize) -> String {
        let base_line = format!(
            "U{} → {}",
            image_index + 1,
            metadata.image_urls[image_index]
        );
        let mut segments = vec![ResponseSegment::new("", base_line.clone(), true)];

        if let Some(prompt) = metadata
            .display_prompts
            .get(image_index)
            .filter(|p| !p.trim().is_empty())
        {
            segments.push(ResponseSegment::new("Prompt: ", prompt.clone(), true));
        }

        if let Some(command) = metadata
            .prompts
            .get(image_index)
            .filter(|p| !p.trim().is_empty())
        {
            segments.push(ResponseSegment::new("Command: ", command.clone(), true));
        }

        Self::assemble_segments(segments, base_line)
    }

    fn assemble_segments(mut segments: Vec<ResponseSegment>, base_line: String) -> String {
        if segments.is_empty() {
            return Self::truncate_text(&base_line, DISCORD_MESSAGE_LIMIT);
        }

        let mut made_progress = true;
        while Self::segments_total_len(&segments) > DISCORD_MESSAGE_LIMIT && made_progress {
            made_progress = false;

            for prefix in ["Command: ", "Prompt: ", ""] {
                if Self::segments_total_len(&segments) <= DISCORD_MESSAGE_LIMIT {
                    break;
                }

                if let Some(idx) = segments
                    .iter()
                    .position(|segment| segment.truncatable && segment.prefix == prefix)
                {
                    let max_total_len =
                        Self::max_segment_len(&segments, idx, DISCORD_MESSAGE_LIMIT);
                    if Self::truncate_segment(&mut segments[idx], max_total_len) {
                        made_progress = true;
                    }
                }
            }
        }

        segments.retain(|segment| !(segment.prefix != "" && segment.text.is_empty()));

        if Self::segments_total_len(&segments) > DISCORD_MESSAGE_LIMIT {
            let mut truncated_base = Self::truncate_text(&base_line, DISCORD_MESSAGE_LIMIT);
            let omission = "Prompt data omitted to fit Discord's 2000 character limit.";
            if !truncated_base.is_empty()
                && truncated_base.len() + 1 + omission.len() <= DISCORD_MESSAGE_LIMIT
            {
                truncated_base.push('\n');
                truncated_base.push_str(omission);
            }
            return truncated_base;
        }

        segments
            .into_iter()
            .map(|segment| segment.render())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn segments_total_len(segments: &[ResponseSegment]) -> usize {
        if segments.is_empty() {
            return 0;
        }
        let content_len: usize = segments.iter().map(ResponseSegment::len).sum();
        content_len + (segments.len() - 1)
    }

    fn max_segment_len(segments: &[ResponseSegment], idx: usize, limit: usize) -> usize {
        let current_total = Self::segments_total_len(segments);
        let segment_len = segments[idx].len();
        if current_total <= limit {
            segment_len
        } else {
            let others_len = current_total.saturating_sub(segment_len);
            if others_len >= limit {
                0
            } else {
                limit - others_len
            }
        }
    }

    fn truncate_segment(segment: &mut ResponseSegment, max_total_len: usize) -> bool {
        let current_len = segment.len();
        if max_total_len >= current_len {
            return false;
        }

        let prefix_len = segment.prefix.len();
        if max_total_len <= prefix_len {
            if !segment.text.is_empty() {
                segment.text.clear();
                return true;
            }
            return false;
        }

        let max_text_len = max_total_len - prefix_len;
        if segment.text.len() <= max_text_len {
            return false;
        }

        if max_text_len == 0 {
            segment.text.clear();
            return true;
        }

        let ellipsis = "...";
        if max_text_len <= ellipsis.len() {
            segment.text = ".".repeat(max_text_len);
            return true;
        }

        let allowed = max_text_len - ellipsis.len();
        let truncated = Self::truncate_text(&segment.text, allowed);
        segment.text = truncated;
        segment.text.push_str(ellipsis);
        true
    }

    fn truncate_text(text: &str, max_bytes: usize) -> String {
        if text.len() <= max_bytes {
            return text.to_string();
        }
        if max_bytes == 0 {
            return String::new();
        }

        let mut end = max_bytes.min(text.len());
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].to_string()
    }

    async fn handle_gallery_component(&self, component: &ComponentInteraction) -> Result<()> {
        let Some((gallery_id, image_index)) =
            Self::parse_gallery_custom_id(&component.data.custom_id)
        else {
            return self
                .respond_component_message(component, "Unable to parse gallery control.")
                .await;
        };

        let metadata = match self
            .gallery_registry
            .ask(GetGallery(gallery_id.clone()))
            .await
            .context("while loading gallery metadata")?
        {
            Some(metadata) => metadata,
            None => {
                return self
                    .respond_component_message(component, "This gallery is no longer available.")
                    .await;
            }
        };

        let channel_id = component.message.channel_id.get();
        let message_id = component.message.id.get();

        match self
            .gallery_registry
            .ask(GetGalleryByMessage {
                channel_id,
                message_id,
            })
            .await
            .context("while validating gallery/message association")?
        {
            Some(mapped) if mapped.id == metadata.id => {}
            Some(_) => {
                warn!(
                    custom_id = %component.data.custom_id,
                    "Gallery/message association mismatch"
                );
            }
            None => {
                warn!(
                    message_id,
                    channel_id, "No gallery association found for component message"
                );
            }
        }

        let owner = UserId::Discord(component.user.id);
        if metadata.owner_id != owner {
            return self
                .respond_component_message(
                    component,
                    "Only the original author can extract images from this gallery.",
                )
                .await;
        }

        if image_index >= metadata.image_urls.len() {
            return self
                .respond_component_message(component, "That image is no longer available.")
                .await;
        }

        let content = Self::build_gallery_response_content(&metadata, image_index);
        self.respond_component_message(component, &content).await
    }

    async fn respond_component_message(
        &self,
        component: &ComponentInteraction,
        content: &str,
    ) -> Result<()> {
        let response = CreateInteractionResponseMessage::new()
            .content(content.to_owned())
            .flags(InteractionResponseFlags::EPHEMERAL);

        component
            .create_response(&self.http, CreateInteractionResponse::Message(response))
            .await
            .with_context(|| "while responding to component interaction")
    }

    async fn handle_retry_button(&self, component: &ComponentInteraction) -> Result<()> {
        let Some((gallery_id, expected_user_id)) =
            Self::parse_prompt_action_custom_id(&component.data.custom_id)
        else {
            return self
                .respond_component_message(component, "Unable to parse retry button.")
                .await;
        };

        // Verify that the user clicking the button is the original user
        if component.user.id.get() != expected_user_id {
            return self
                .respond_component_message(
                    component,
                    "Only the original author can retry this prompt.",
                )
                .await;
        }

        // Get gallery metadata to retrieve the original prompt
        let metadata = match self
            .gallery_registry
            .ask(GetGallery(gallery_id.clone()))
            .await
            .context("while loading gallery metadata")?
        {
            Some(metadata) => metadata,
            None => {
                return self
                    .respond_component_message(component, "This gallery is no longer available.")
                    .await;
            }
        };

        // Get the original prompt from display_prompts
        let original_prompt = metadata
            .display_prompts
            .first()
            .map(|s| s.as_str())
            .unwrap_or("");

        // Create a modal with the prompt pre-filled
        let modal_custom_id = format!(
            "{}:{}:{}",
            PROMPT_RETRY_PREFIX, gallery_id, expected_user_id
        );
        let input = CreateInputText::new(InputTextStyle::Paragraph, "Prompt", "prompt_input")
            .value(original_prompt)
            .placeholder("Enter your image generation prompt")
            .required(true);

        let modal = CreateModal::new(modal_custom_id, "Retry Prompt")
            .components(vec![CreateActionRow::InputText(input)]);

        component
            .create_response(&self.http, CreateInteractionResponse::Modal(modal))
            .await
            .with_context(|| "while showing retry modal")
    }

    async fn handle_delete_button(&self, component: &ComponentInteraction) -> Result<()> {
        let Some((gallery_id, expected_user_id)) =
            Self::parse_prompt_action_custom_id(&component.data.custom_id)
        else {
            return self
                .respond_component_message(component, "Unable to parse delete button.")
                .await;
        };

        // Verify that the user clicking the button is the original user
        if component.user.id.get() != expected_user_id {
            return self
                .respond_component_message(
                    component,
                    "Only the original author can delete this image.",
                )
                .await;
        }

        // Show confirmation buttons
        let channel_id = component.message.channel_id.get();
        let message_id = component.message.id.get();

        let confirm_button = CreateButton::new(format!(
            "{}:{}:{}:{}",
            PROMPT_DELETE_CONFIRM_PREFIX, gallery_id, channel_id, message_id
        ))
        .style(ButtonStyle::Danger)
        .label("Yes, delete");

        let cancel_button = CreateButton::new("delete_cancel")
            .style(ButtonStyle::Secondary)
            .label("Cancel");

        let buttons = CreateActionRow::Buttons(vec![confirm_button, cancel_button]);

        let response = CreateInteractionResponseMessage::new()
            .content("Are you sure you want to delete this image?")
            .components(vec![buttons])
            .flags(InteractionResponseFlags::EPHEMERAL);

        component
            .create_response(&self.http, CreateInteractionResponse::Message(response))
            .await
            .with_context(|| "while showing delete confirmation")
    }

    async fn handle_delete_confirm_button(
        &self,
        _ctx: &mut Context<Self, ()>,
        _discord_ctx: &all::Context,
        component: &ComponentInteraction,
    ) -> Result<()> {
        let Some((gallery_id, channel_id, message_id)) =
            Self::parse_delete_confirm_custom_id(&component.data.custom_id)
        else {
            return self
                .respond_component_message(component, "Unable to parse delete button.")
                .await;
        };

        // Delete the message the delete button was on
        if let Err(err) = all::ChannelId::new(channel_id)
            .delete_message(&self.http, message_id)
            .await
        {
            warn!(
                channel_id,
                message_id, "Failed to delete prompt message for gallery {gallery_id}: {err:#}"
            );
        }

        // Delete the image(s)
        let user_id_key = UserId::Discord(component.user.id).key();
        match delete_image(&gallery_id, &user_id_key).await {
            Ok(result) => {
                let response = CreateInteractionResponseMessage::new()
                    .content(result.message)
                    .components(vec![]);

                component
                    .create_response(
                        &self.http,
                        CreateInteractionResponse::UpdateMessage(response),
                    )
                    .await
                    .with_context(|| "while confirming deletion")
            }
            Err(err) => {
                let error_msg = format!("Failed to delete image: {:#}", err);
                error!("{}", error_msg);
                self.respond_component_message(component, &error_msg).await
            }
        }
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn handle_modal(
        &mut self,
        ctx: &mut Context<Self, ()>,
        discord_ctx: all::Context,
        modal: ModalInteraction,
    ) -> Result<()> {
        let custom_id = modal.data.custom_id.as_str();

        if custom_id.starts_with(PROMPT_RETRY_PREFIX) {
            return self.handle_retry_modal(ctx, &discord_ctx, &modal).await;
        }

        warn!(custom_id = %modal.data.custom_id, "Unhandled modal submission");
        Ok(())
    }

    async fn handle_retry_modal(
        &mut self,
        ctx: &mut Context<Self, ()>,
        discord_ctx: &all::Context,
        modal: &ModalInteraction,
    ) -> Result<()> {
        // Parse the custom_id to extract gallery_id and user_id
        let Some((_gallery_id, expected_user_id)) =
            Self::parse_prompt_action_custom_id(&modal.data.custom_id)
        else {
            return self
                .respond_modal_error(modal, "Unable to parse retry modal.")
                .await;
        };

        // Verify that the user submitting the modal is the original user
        if modal.user.id.get() != expected_user_id {
            return self
                .respond_modal_error(modal, "Only the original author can retry this prompt.")
                .await;
        }

        // Extract the edited prompt from the modal
        let edited_prompt = modal
            .data
            .components
            .iter()
            .flat_map(|row| &row.components)
            .find_map(|component| {
                if let all::ActionRowComponent::InputText(text) = component {
                    if text.custom_id == "prompt_input" {
                        return text.value.as_ref().map(|s| s.as_str());
                    }
                }
                None
            })
            .unwrap_or("");

        if edited_prompt.trim().is_empty() {
            return self
                .respond_modal_error(modal, "Prompt cannot be empty.")
                .await;
        }

        if let Err(err) =
            get_user_actor_impl(&self.user_manager, modal.user.id, &modal.user.name).await
        {
            error!(
                user_id = modal.user.id.get(),
                "Failed to prepare user state for retry: {err:#}"
            );
            return self
                .respond_modal_error(
                    modal,
                    "Unable to prepare your retry right now. Please try again in a moment.",
                )
                .await;
        }

        // Defer the modal response
        modal
            .create_response(
                &discord_ctx.http,
                CreateInteractionResponse::Defer(
                    CreateInteractionResponseMessage::new()
                        .flags(InteractionResponseFlags::empty()),
                ),
            )
            .await
            .context("while deferring retry modal response")?;

        let preface = format!("**Prompt**: {}", edited_prompt);

        // Submit a new action with the edited prompt
        let payload = ActionPayload::Prompt {
            user_id: UserId::Discord(modal.user.id),
            user_name: modal.user.name.clone(),
            input: edited_prompt.to_string(),
        };

        // Create a followup message for progress tracking
        let followup_builder = CreateInteractionResponseFollowup::new()
            .content(format!("{preface}\nStatus: preparing request…"))
            .allowed_mentions(CreateAllowedMentions::new());

        let followup = modal
            .create_followup(&discord_ctx.http, followup_builder)
            .await
            .context("while creating retry followup message")?;

        // Update origin with the actual followup message ID
        let origin = ActionOrigin::Discord {
            application_id: self.config.application_id,
            guild_id: modal.guild_id.map(|id| id.get()),
            channel_id: followup.channel_id.get(),
            message_id: followup.id.get(),
            user_id: modal.user.id.get(),
            progress_message: Some(preface.clone()),
        };

        let submit_origin = origin.clone();

        match self
            .broker
            .ask(SubmitAction::new(submit_origin, payload))
            .await
        {
            Ok(action_id) => {
                if self
                    .spawn_progress_actor_for_origin(ctx, action_id, origin)
                    .await
                    .is_none()
                {
                    update_progress_message_impl(
                        &self.http,
                        followup.channel_id.get(),
                        followup.id.get(),
                        Some(preface.clone()),
                        format!("Status: queued (`{action_id}`)"),
                    )
                    .await
                    .with_context(|| "while acknowledging queued request")?;
                }
                Ok(())
            }
            Err(err) => {
                error!("Failed to submit retry action: {:#}", err);
                delete_progress_message_impl(
                    &self.http,
                    followup.channel_id.get(),
                    followup.id.get(),
                )
                .await;
                send_failure_message_impl(
                    &self.http,
                    followup.channel_id.get(),
                    modal.user.id.get(),
                    format!("failed to queue request: {err:#}"),
                    false,
                    Some(preface),
                )
                .await?;
                Ok(())
            }
        }
    }

    async fn respond_modal_error(&self, modal: &ModalInteraction, content: &str) -> Result<()> {
        let response = CreateInteractionResponseMessage::new()
            .content(content.to_owned())
            .flags(InteractionResponseFlags::EPHEMERAL);

        modal
            .create_response(&self.http, CreateInteractionResponse::Message(response))
            .await
            .with_context(|| "while responding to modal with error")
    }

    async fn spawn_progress_actor_for_origin(
        &mut self,
        ctx: &mut Context<Self, ()>,
        action_id: ActionId,
        origin: ActionOrigin,
    ) -> Option<ActorRef<DiscordProgressActor>> {
        let parent = ctx.actor_ref().downgrade();
        let actor = DiscordProgressActor::spawn(DiscordProgressActor::new(
            self.http.clone(),
            self.gallery_registry.clone(),
            parent,
            action_id,
            origin.clone(),
        ));

        self.progress_sessions.insert(action_id, actor.clone());

        if let Err(err) = actor
            .tell(BrokerActionUpdate {
                id: action_id,
                origin,
                status: ActionStatus::Queued,
            })
            .send()
            .await
        {
            warn!(action = %action_id, "Failed to send initial queued update: {err:#}");
            self.progress_sessions.remove(&action_id);
            return None;
        }

        Some(actor)
    }

    fn parse_gallery_custom_id(custom_id: &str) -> Option<(String, usize)> {
        let mut parts = custom_id.split(':');
        let prefix = parts.next()?;
        if prefix != GALLERY_BUTTON_PREFIX {
            return None;
        }
        let gallery_id = parts.next()?.to_string();
        let index_str = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        let index = index_str.parse().ok()?;
        Some((gallery_id, index))
    }

    fn parse_prompt_action_custom_id(custom_id: &str) -> Option<(String, u64)> {
        let mut parts = custom_id.split(':');
        let prefix = parts.next()?;
        if prefix != PROMPT_RETRY_PREFIX && prefix != PROMPT_DELETE_PREFIX {
            return None;
        }
        let gallery_id = parts.next()?.to_string();
        let user_id_str = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        let user_id = user_id_str.parse().ok()?;
        Some((gallery_id, user_id))
    }

    fn parse_delete_confirm_custom_id(custom_id: &str) -> Option<(String, u64, u64)> {
        let mut parts = custom_id.split(':');
        let prefix = parts.next()?;
        if prefix != PROMPT_DELETE_CONFIRM_PREFIX {
            return None;
        }
        let gallery_id = parts.next()?.to_string();
        let channel_id = parts.next()?.parse().ok()?;
        let message_id = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some((gallery_id, channel_id, message_id))
    }

    fn build_gallery_components(metadata: &GalleryMetadata) -> Vec<CreateActionRow> {
        let mut rows = Vec::new();
        let mut image_index = 0usize;

        for &count in &metadata.layout.row_counts {
            if count == 0 {
                continue;
            }

            let mut remaining = count as usize;
            while remaining > 0 && rows.len() < 5 {
                let take = remaining.min(5);
                let mut buttons = Vec::new();

                for _ in 0..take {
                    image_index += 1;
                    let label = format!("U{}", image_index);
                    let custom_id = format!(
                        "{}:{}:{}",
                        GALLERY_BUTTON_PREFIX,
                        metadata.id,
                        image_index - 1
                    );
                    buttons.push(
                        CreateButton::new(custom_id)
                            .style(ButtonStyle::Secondary)
                            .label(label),
                    );
                }

                rows.push(CreateActionRow::Buttons(buttons));
                remaining -= take;
            }

            if rows.len() >= 5 {
                break;
            }
        }

        rows
    }

    fn build_prompt_action_buttons(gallery_id: &str, user_id: u64) -> CreateActionRow {
        let delete_button = CreateButton::new(format!(
            "{}:{}:{}",
            PROMPT_DELETE_PREFIX, gallery_id, user_id
        ))
        .style(ButtonStyle::Danger)
        .label("🗑️ Delete");

        let retry_button = CreateButton::new(format!(
            "{}:{}:{}",
            PROMPT_RETRY_PREFIX, gallery_id, user_id
        ))
        .style(ButtonStyle::Primary)
        .label("🔄 Retry");

        let help_button = CreateButton::new_link("https://ganbot.brage.info/").label("❓ Help");

        CreateActionRow::Buttons(vec![delete_button, retry_button, help_button])
    }

    async fn handle_select_command(
        &mut self,
        _ctx: &mut Context<Self, ()>,
        _discord_ctx: &all::Context,
        _command: &CommandInteraction,
        _url: String,
    ) -> Result<()> {
        todo!()
    }
}
