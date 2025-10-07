use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::Utc;
use kameo::{
    Actor,
    actor::{ActorRef, WeakActorRef},
    prelude::{Context as ActorContext, Message},
    registry::ACTOR_REGISTRY,
};
use redis::{AsyncCommands, aio::ConnectionManager};
use serde_json::json;
use tokio::task::JoinHandle;
use tracing::{error, info, instrument, warn};

use crate::actions::{
    ActionCompleted, ActionFailure, ActionLifecycleResult, ActionPayload, ActionProgressEmitter,
    ActionRequest, ActionResponse, ActionStatus, ActionUpdate, SubmitAction,
};
use crate::network::{
    discord::{BrokerActionUpdate, DiscordActor},
    irc::{BrokerActionDelivery, IrcActor},
};
use crate::persistence::{
    images::{GalleryMetadata, GalleryRegistry, RegisterGallery},
    user::{GetUser, UserManager},
};

const ACTIONS_PENDING_KEY: &str = "actions:pending";

pub struct ActionBroker {
    redis: ConnectionManager,
    irc_targets: HashMap<String, WeakActorRef<IrcActor>>,
    discord_target: Option<WeakActorRef<DiscordActor>>,
}

pub struct RegisterIrc {
    pub server: String,
    pub actor: ActorRef<IrcActor>,
}

pub struct RegisterDiscord {
    pub actor: ActorRef<DiscordActor>,
}

impl ActionBroker {
    pub fn get() -> Result<ActorRef<Self>> {
        ACTOR_REGISTRY
            .lock()
            .unwrap()
            .get::<Self, str>("action_broker")
            .context("while fetching ActionBroker")?
            .context("ActionBroker not registered")
    }

    async fn persist_action(&self, request: &ActionRequest) -> Result<()> {
        let mut conn = self.redis.clone();
        let payload = serde_json::to_string(request).context("while serializing action request")?;
        let score = request.inserted_at.timestamp_millis() as f64;
        let _: i64 = conn
            .zadd(ACTIONS_PENDING_KEY, payload, score)
            .await
            .context("while appending action to Redis sorted set")?;
        Ok(())
    }

    fn spawn_worker(
        request: ActionRequest,
        broker_ref: WeakActorRef<ActionBroker>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let progress = ActionProgressEmitter::new(&request, broker_ref.clone());
            progress.started();

            let status = match execute_action(&request, &progress).await {
                Ok(response) => ActionStatus::Completed(ActionCompleted { response }),
                Err(error) => ActionStatus::Failed(ActionFailure {
                    error: format!("{error:#}"),
                    retry_scheduled: false,
                }),
            };

            if let Some(broker) = broker_ref.upgrade() {
                if let Err(err) = broker
                    .tell(ActionLifecycleResult { request, status })
                    .send()
                    .await
                {
                    warn!("Failed to report completion of action: {err:#}");
                }
            }
        })
    }

    async fn recover_pending(&mut self, actor_ref: &ActorRef<Self>) -> Result<()> {
        let entries: Vec<String> = {
            let mut conn = self.redis.clone();
            conn.zrange(ACTIONS_PENDING_KEY, 0, -1)
                .await
                .context("while fetching pending actions from Redis")?
        };

        if entries.is_empty() {
            return Ok(());
        }

        info!(pending = entries.len(), "Recovering pending actions");

        for entry in entries {
            match serde_json::from_str::<ActionRequest>(&entry) {
                Ok(request) => {
                    let queued = ActionUpdate {
                        id: request.id,
                        origin: request.origin.clone(),
                        status: ActionStatus::Queued,
                    };
                    if let Err(err) = self.forward_update(queued).await {
                        warn!("Failed to forward queued update during recovery: {err:#}");
                    }

                    Self::spawn_worker(request, actor_ref.downgrade());
                }
                Err(err) => {
                    warn!("Dropping unreadable pending action: {err:#}");
                    let mut conn = self.redis.clone();
                    let _: i64 = conn
                        .zrem(ACTIONS_PENDING_KEY, entry)
                        .await
                        .context("while removing unreadable pending action")?;
                }
            }
        }

        Ok(())
    }

    async fn clear_pending(&self, request: &ActionRequest) -> Result<()> {
        let mut conn = self.redis.clone();
        let payload =
            serde_json::to_string(request).context("while serializing action removal payload")?;
        let _: i64 = conn
            .zrem(ACTIONS_PENDING_KEY, payload)
            .await
            .context("while removing action from pending set")?;
        Ok(())
    }

    async fn forward_update(&mut self, update: ActionUpdate) -> Result<()> {
        if let crate::actions::ActionOrigin::Irc { .. } = &update.origin {
            match &update.status {
                ActionStatus::Queued | ActionStatus::Started | ActionStatus::Progress(_) => {
                    // Placeholder: future progress routing.
                    Ok(())
                }
                ActionStatus::Completed(completed) => {
                    self.deliver_irc(update.id, &update.origin, &completed.response)
                        .await
                }
                ActionStatus::Failed(failure) => {
                    let reply_privately = matches!(
                        update.origin,
                        crate::actions::ActionOrigin::Irc {
                            reply_privately: true,
                            ..
                        }
                    );
                    let response = ActionResponse::single_line(
                        format!("Error: {}", failure.error),
                        reply_privately,
                    );
                    self.deliver_irc(update.id, &update.origin, &response).await
                }
            }
        } else if let crate::actions::ActionOrigin::Discord { .. } = &update.origin {
            self.deliver_discord(update).await
        } else {
            Ok(())
        }
    }

    async fn deliver_irc(
        &mut self,
        action_id: crate::actions::ActionId,
        origin: &crate::actions::ActionOrigin,
        response: &ActionResponse,
    ) -> Result<()> {
        let crate::actions::ActionOrigin::Irc {
            server,
            channel,
            nickname,
            reply_privately,
        } = origin
        else {
            return Ok(());
        };

        let target = self
            .irc_targets
            .get(server)
            .and_then(|actor| actor.upgrade());

        let Some(actor) = target else {
            warn!("No IRC actor registered for server {server}; dropping action {action_id}");
            return Ok(());
        };

        let delivery = BrokerActionDelivery {
            channel: channel.clone(),
            nickname: nickname.clone(),
            response: response.clone(),
            reply_privately: *reply_privately,
        };

        actor
            .tell(delivery)
            .send()
            .await
            .context("while delivering to IRC actor")
    }

    async fn deliver_discord(&mut self, update: ActionUpdate) -> Result<()> {
        let Some(actor) = self.discord_target.as_ref().and_then(WeakActorRef::upgrade) else {
            warn!("No Discord actor registered; dropping action {}", update.id);
            return Ok(());
        };

        actor
            .tell(BrokerActionUpdate {
                id: update.id,
                origin: update.origin,
                status: update.status,
            })
            .send()
            .await
            .context("while delivering action update to Discord actor")
    }

    async fn persist_terminal(&self, request: &ActionRequest, status: &ActionStatus) -> Result<()> {
        let mut conn = self.redis.clone();
        let payload = json!({
            "request": request,
            "status": status_as_json(status),
        });
        let _: i64 = redis::cmd("HSET")
            .arg("actions:terminal")
            .arg(request.id.to_string())
            .arg(serde_json::to_string(&payload).context("while serializing terminal action")?)
            .query_async(&mut conn)
            .await
            .context("while persisting terminal action state")?;
        Ok(())
    }
}

impl Actor for ActionBroker {
    type Args = ConnectionManager;
    type Error = anyhow::Error;

    #[instrument(skip_all)]
    async fn on_start(args: Self::Args, actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        actor_ref
            .register("action_broker")
            .expect("Failed to register ActionBroker");
        let mut broker = Self {
            redis: args,
            irc_targets: HashMap::new(),
            discord_target: None,
        };
        broker.recover_pending(&actor_ref).await?;
        Ok(broker)
    }
}

impl Message<SubmitAction> for ActionBroker {
    type Reply = Result<crate::actions::ActionId>;

    #[instrument(skip_all)]
    async fn handle(
        &mut self,
        msg: SubmitAction,
        ctx: &mut ActorContext<Self, Self::Reply>,
    ) -> Self::Reply {
        let request = ActionRequest::new(msg.origin.clone(), msg.payload);
        self.persist_action(&request).await?;

        let queued = ActionUpdate {
            id: request.id,
            origin: request.origin.clone(),
            status: ActionStatus::Queued,
        };
        if let Err(err) = self.forward_update(queued).await {
            warn!("Failed to forward queued update: {err:#}");
        }

        let broker_ref = ctx.actor_ref().downgrade();
        Self::spawn_worker(request.clone(), broker_ref);

        Ok(request.id)
    }
}

impl Message<ActionLifecycleResult> for ActionBroker {
    type Reply = ();

    #[instrument(skip_all)]
    async fn handle(
        &mut self,
        msg: ActionLifecycleResult,
        _ctx: &mut ActorContext<Self, Self::Reply>,
    ) -> Self::Reply {
        let update = ActionUpdate {
            id: msg.request.id,
            origin: msg.request.origin.clone(),
            status: msg.status.clone(),
        };
        if let Err(err) = self.forward_update(update).await {
            error!("Failed to forward action update: {err:#}");
        }

        if matches!(
            msg.status,
            ActionStatus::Completed(_) | ActionStatus::Failed(_)
        ) {
            if let Err(err) = self.clear_pending(&msg.request).await {
                warn!("Failed to clear pending action: {err:#}");
            }
            if let Err(err) = self.persist_terminal(&msg.request, &msg.status).await {
                warn!("Failed to persist terminal action state: {err:#}");
            }
        }
    }
}

impl Message<RegisterIrc> for ActionBroker {
    type Reply = ();

    #[instrument(skip_all, fields(server = %msg.server))]
    async fn handle(
        &mut self,
        msg: RegisterIrc,
        _ctx: &mut ActorContext<Self, Self::Reply>,
    ) -> Self::Reply {
        self.irc_targets.insert(msg.server, msg.actor.downgrade());
    }
}

impl Message<RegisterDiscord> for ActionBroker {
    type Reply = ();

    #[instrument(skip_all)]
    async fn handle(
        &mut self,
        msg: RegisterDiscord,
        _ctx: &mut ActorContext<Self, Self::Reply>,
    ) -> Self::Reply {
        self.discord_target = Some(msg.actor.downgrade());
    }
}

fn status_as_json(status: &ActionStatus) -> serde_json::Value {
    match status {
        ActionStatus::Queued => json!({ "state": "queued" }),
        ActionStatus::Started => json!({ "state": "started" }),
        ActionStatus::Progress(progress) => {
            let mut payload = serde_json::Map::new();
            payload.insert("state".to_string(), json!("progress"));
            payload.insert("message".to_string(), json!(progress.message));
            if let Some(percent) = progress.percent {
                payload.insert("percent".to_string(), json!(percent));
            }
            serde_json::Value::Object(payload)
        }
        ActionStatus::Completed(completed) => {
            let mut payload = serde_json::Map::new();
            payload.insert("state".to_string(), json!("completed"));
            payload.insert("lines".to_string(), json!(completed.response.lines));
            if let Some(gallery) = &completed.response.gallery {
                payload.insert("gallery_id".to_string(), json!(gallery.id));
            }
            serde_json::Value::Object(payload)
        }
        ActionStatus::Failed(failure) => json!({
            "state": "failed",
            "error": failure.error,
            "retry_scheduled": failure.retry_scheduled,
        }),
    }
}

async fn execute_action(
    request: &ActionRequest,
    progress: &ActionProgressEmitter,
) -> Result<ActionResponse> {
    match &request.payload {
        ActionPayload::Ask { question } => {
            let actor =
                crate::actions::ask::AskActor::spawn(crate::actions::ask::AskActor::new().await);
            let result = actor
                .ask(question.clone())
                .await
                .context("while executing ask action")?;
            Ok(ActionResponse::single_line(result.response, false))
        }
        ActionPayload::Combine { request } => {
            let actor = crate::actions::combine::CombineActor::spawn(
                crate::actions::combine::CombineActor::new().await,
            );
            let result = actor
                .ask(request.clone())
                .await
                .context("while executing combine action")?;
            let mut message = format!(
                "{}\n**{}**\n{}",
                result.image_url, result.result, result.reasoning
            );
            if let Some(correction) = result.correction_message {
                message = format!("{}\n({})", message, correction);
            }
            Ok(ActionResponse::single_line(message, false))
        }
        ActionPayload::Prompt {
            user_id,
            user_name,
            input,
        } => {
            let user_manager = UserManager::get().context("while fetching UserManager")?;
            let user_actor = user_manager
                .ask(GetUser(user_id.clone(), user_name.clone()))
                .await
                .context("while fetching user actor")?;
            execute_prompt(user_actor, user_id.clone(), input.clone(), progress).await
        }
        ActionPayload::Dream {
            user_id,
            user_name,
            input,
        } => {
            let user_manager = UserManager::get().context("while fetching UserManager")?;
            let user_actor = user_manager
                .ask(GetUser(user_id.clone(), user_name.clone()))
                .await
                .context("while fetching user actor")?;
            execute_dream(user_actor, user_id.clone(), input.clone(), progress).await
        }
    }
}

async fn execute_prompt(
    user_actor: ActorRef<crate::persistence::user::UserActor>,
    user_id: crate::persistence::user::UserId,
    input: String,
    progress: &ActionProgressEmitter,
) -> Result<ActionResponse> {
    let actor = crate::actions::prompt::PromptActor::spawn(
        crate::actions::prompt::PromptActor::new(user_actor, Some(progress.clone())).await,
    );
    let result = actor
        .ask(input)
        .await
        .context("while executing prompt action")?;
    let crate::actions::prompt::PromptResult {
        text,
        image_url,
        image_urls,
        prompts,
        display_prompts,
        gallery_id,
        gallery_layout,
        correction_message,
        ..
    } = result;

    let base_response = match image_url.as_ref() {
        Some(image_url) => format!("{} {}", text, image_url),
        None => format!("{}\n(No image)", text),
    };

    let response = if let Some(correction) = correction_message.as_ref() {
        format!("{}\n({})", base_response, correction)
    } else {
        base_response
    };

    let mut action_response = ActionResponse::single_line(response, false);

    if let (
        Some(gallery_id),
        Some(gallery_layout),
        Some(image_urls),
        Some(prompts),
        Some(display_prompts),
    ) = (
        gallery_id,
        gallery_layout,
        image_urls,
        prompts,
        display_prompts,
    ) {
        let metadata = GalleryMetadata {
            id: gallery_id.clone(),
            owner_id: user_id,
            gallery_url: image_url,
            image_urls,
            prompts,
            display_prompts,
            layout: gallery_layout,
            created_at: Utc::now().to_rfc3339(),
        };

        if let Ok(registry) = GalleryRegistry::get() {
            if let Err(err) = registry
                .ask(RegisterGallery { metadata })
                .await
                .context("while registering gallery metadata")
            {
                error!("Failed to register gallery metadata: {err:#}");
            }
        }

        action_response.gallery = Some(crate::actions::GalleryReference { id: gallery_id });
    }

    Ok(action_response)
}

async fn execute_dream(
    user_actor: ActorRef<crate::persistence::user::UserActor>,
    user_id: crate::persistence::user::UserId,
    input: String,
    progress: &ActionProgressEmitter,
) -> Result<ActionResponse> {
    let actor = crate::actions::dream::DreamActor::spawn(
        crate::actions::dream::DreamActor::new(user_actor, Some(progress.clone())).await,
    );
    let result = actor
        .ask(input)
        .await
        .context("while executing dream action")?;
    let crate::actions::prompt::PromptResult {
        text,
        image_url,
        image_urls,
        prompts,
        display_prompts,
        gallery_id,
        gallery_layout,
        correction_message,
        ..
    } = result;

    let base_response = match image_url.as_ref() {
        Some(image_url) => format!("{} {}", text, image_url),
        None => format!("{}\n(No image)", text),
    };

    let response = if let Some(correction) = correction_message.as_ref() {
        format!("{}\n({})", base_response, correction)
    } else {
        base_response
    };

    let mut action_response = ActionResponse::single_line(response, false);

    if let (
        Some(gallery_id),
        Some(gallery_layout),
        Some(image_urls),
        Some(prompts),
        Some(display_prompts),
    ) = (
        gallery_id,
        gallery_layout,
        image_urls,
        prompts,
        display_prompts,
    ) {
        let metadata = GalleryMetadata {
            id: gallery_id.clone(),
            owner_id: user_id,
            gallery_url: image_url,
            image_urls,
            prompts,
            display_prompts,
            layout: gallery_layout,
            created_at: Utc::now().to_rfc3339(),
        };

        if let Ok(registry) = GalleryRegistry::get() {
            if let Err(err) = registry
                .ask(RegisterGallery { metadata })
                .await
                .context("while registering dream gallery metadata")
            {
                error!("Failed to register dream gallery metadata: {err:#}");
            }
        }

        action_response.gallery = Some(crate::actions::GalleryReference { id: gallery_id });
    }

    Ok(action_response)
}
