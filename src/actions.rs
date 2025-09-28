pub mod ask;
pub mod broker;
pub mod combine;
pub mod config;
pub mod delete;
pub mod dream;
pub mod edit;
pub mod imagen;
pub mod prompt;
pub mod select;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use kameo::actor::WeakActorRef;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    actions::broker::ActionBroker, persistence::user::UserId, util::token_bucket::TokenBucket,
};

/// Unique identifier for an action invocation persisted in Redis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActionId(pub Uuid);

impl ActionId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl std::fmt::Display for ActionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Where the action originated so responses can be routed appropriately.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActionOrigin {
    Irc {
        server: String,
        channel: Option<String>,
        nickname: String,
        reply_privately: bool,
    },
    Discord {
        /// Discord application identifier so the broker can locate the actor instance.
        application_id: u64,
        /// Optional guild identifier; absent for direct messages.
        guild_id: Option<u64>,
        /// Channel that should receive progress and completion updates.
        channel_id: u64,
        /// Progress message identifier so the broker can update/delete it later.
        message_id: u64,
        /// Requesting user.
        user_id: u64,
        /// Static portion of the progress message that should be preserved across updates.
        progress_message: Option<String>,
    },
}

/// Action payload variants covering supported commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActionPayload {
    Ask {
        question: String,
    },
    Combine {
        request: String,
    },
    Prompt {
        user_id: UserId,
        user_name: String,
        input: String,
    },
    Dream {
        user_id: UserId,
        user_name: String,
        input: String,
    },
}

/// Persisted representation of a queued action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    pub id: ActionId,
    pub origin: ActionOrigin,
    pub payload: ActionPayload,
    pub retry_count: u32,
    pub inserted_at: DateTime<Utc>,
}

impl ActionRequest {
    pub fn new(origin: ActionOrigin, payload: ActionPayload) -> Self {
        Self {
            id: ActionId::new(),
            origin,
            payload,
            retry_count: 0,
            inserted_at: Utc::now(),
        }
    }
}

/// Runtime status update for an action sent back to the originating network actor.
#[derive(Debug, Clone)]
pub struct ActionUpdate {
    pub id: ActionId,
    pub origin: ActionOrigin,
    pub status: ActionStatus,
}

#[derive(Debug, Clone)]
pub enum ActionStatus {
    Queued,
    Started,
    Progress(ActionProgress),
    Completed(ActionCompleted),
    Failed(ActionFailure),
}

#[derive(Debug, Clone)]
pub struct ActionProgress {
    /// Progress percentage in the range 0.0-100.0 if available.
    pub percent: Option<f32>,
    /// Human-readable status text describing the current stage.
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ActionCompleted {
    pub response: ActionResponse,
}

#[derive(Debug, Clone)]
pub struct ActionFailure {
    pub error: String,
    pub retry_scheduled: bool,
}

#[derive(Debug, Clone)]
pub struct ActionResponse {
    pub lines: Vec<String>,
    pub reply_privately: bool,
    pub gallery: Option<GalleryReference>,
}

impl ActionResponse {
    pub fn single_line(message: String, reply_privately: bool) -> Self {
        Self {
            lines: vec![message],
            reply_privately,
            gallery: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GalleryReference {
    pub id: String,
}

/// Message emitted when a network actor wants to queue an action.
#[derive(Debug, Clone)]
pub struct SubmitAction {
    pub origin: ActionOrigin,
    pub payload: ActionPayload,
}

impl SubmitAction {
    pub fn new(origin: ActionOrigin, payload: ActionPayload) -> Self {
        Self { origin, payload }
    }
}

/// Message emitted by workers once an action completes or fails.
#[derive(Debug, Clone)]
pub struct ActionLifecycleResult {
    pub request: ActionRequest,
    pub status: ActionStatus,
}

#[derive(Clone)]
pub struct ActionProgressEmitter {
    request: Arc<ActionRequest>,
    broker: WeakActorRef<ActionBroker>,
    throttle: Arc<Mutex<ProgressThrottle>>,
}

impl ActionProgressEmitter {
    pub fn new(request: &ActionRequest, broker: WeakActorRef<ActionBroker>) -> Self {
        Self {
            request: Arc::new(request.clone()),
            broker,
            throttle: Arc::new(Mutex::new(ProgressThrottle::new())),
        }
    }

    pub fn started(&self) {
        self.spawn_status(ActionStatus::Started);
    }

    pub fn progress(&self, percent: Option<f32>, message: impl Into<String> + Send + 'static) {
        let message = message.into();
        let emitter = self.clone();
        tokio::spawn(async move {
            if !emitter.should_emit_progress(percent, &message).await {
                return;
            }
            emitter
                .send_status(ActionStatus::Progress(ActionProgress { percent, message }))
                .await;
        });
    }

    async fn send_status(&self, status: ActionStatus) {
        if let Some(broker) = self.broker.upgrade() {
            let request = (*self.request).clone();
            if let Err(err) = broker
                .tell(ActionLifecycleResult { request, status })
                .send()
                .await
            {
                tracing::warn!("Failed to emit action status update: {err:#}");
            }
        }
    }

    fn spawn_status(&self, status: ActionStatus) {
        let emitter = self.clone();
        tokio::spawn(async move {
            emitter.send_status(status).await;
        });
    }

    async fn should_emit_progress(&self, percent: Option<f32>, message: &str) -> bool {
        // Ensure we always surface completion updates even if they arrive quickly.
        let force_emit = percent.map(|p| p >= 99.9).unwrap_or(false);

        let mut throttle = self.throttle.lock().await;
        throttle.allow_emit(percent, force_emit, message)
    }
}

struct ProgressThrottle {
    bucket: TokenBucket,
    last_snapshot: Option<(Option<f32>, String)>,
}

impl ProgressThrottle {
    fn new() -> Self {
        Self {
            bucket: TokenBucket::new(1.0, 0.5),
            last_snapshot: None,
        }
    }

    fn allow_emit(&mut self, percent: Option<f32>, force_emit: bool, message: &str) -> bool {
        if force_emit {
            self.bucket.force_consume(1.0);
            self.record(percent, message);
            return true;
        }

        if self.is_duplicate(percent, message) {
            return false;
        }

        if !self.bucket.try_consume(1.0) {
            return false;
        }

        self.record(percent, message);
        true
    }

    fn is_duplicate(&self, percent: Option<f32>, message: &str) -> bool {
        match &self.last_snapshot {
            Some((last_percent, last_message)) if last_message == message => {
                match (*last_percent, percent) {
                    (Some(a), Some(b)) => (a - b).abs() < 0.5,
                    (None, None) => true,
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn record(&mut self, percent: Option<f32>, message: &str) {
        self.last_snapshot = Some((percent, message.to_string()));
    }
}
