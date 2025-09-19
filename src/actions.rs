pub mod ask;
pub mod broker;
pub mod combine;
pub mod config;
pub mod delete;
pub mod dream;
pub mod edit;
pub mod prompt;
pub mod select;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::persistence::user::UserId;

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
        guild_id: Option<u64>,
        channel_id: u64,
        message_id: u64,
        user_id: u64,
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
}

impl ActionResponse {
    pub fn single_line(message: String, reply_privately: bool) -> Self {
        Self {
            lines: vec![message],
            reply_privately,
        }
    }
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
