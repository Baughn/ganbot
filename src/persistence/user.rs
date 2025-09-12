use std::collections::HashMap;

use anyhow::{Context as _, Error, Result};
use kameo::{
    Actor,
    actor::ActorRef,
    error::Infallible,
    prelude::{Context, Message},
    registry::ACTOR_REGISTRY,
};
use redis::{AsyncTypedCommands, aio::ConnectionManager};
use serde::{Deserialize, Serialize};
use serenity::{all::UserId as DiscordUserId, json};
use tracing::info;

use crate::util;

/// User manager actor, responsible for loading and starting User actors.
pub struct UserManager {
    connection: ConnectionManager,
    loaded_users: HashMap<UserId, ActorRef<UserActor>>,
}

/// Fetch or create a user.
#[derive(Debug, Clone)]
pub(crate) struct GetUser(pub UserId, pub UserName);

/// User actor, representing a user in the system.
pub struct UserActor {
    user: User,
    redis: ConnectionManager,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct User {
    pub id: UserId,
    pub username: UserName,
    pub selected_image_url: Option<String>,
    pub default_prompt: Option<String>,
}

type UserName = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum UserId {
    Irc(String),
    Discord(DiscordUserId),
}

/// Update the username of the user.
struct UpdateUsername(pub UserName);

/// Generated image metadata for storage in Redis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedImage {
    pub url: String,
    pub prompt: String,
    pub timestamp: String, // ISO 8601 datetime string
    pub model: Option<String>,
    pub backend: String, // "StableDiffusion" or "NanoBanana"
}

/// Add a generated image to the user's history
#[derive(Debug, Clone)]
pub struct AddGeneratedImage {
    pub url: String,
    pub prompt: String,
    pub model: Option<String>,
    pub backend: String,
}

/// Set the selected image URL for the user
#[derive(Debug, Clone)]
pub struct SetSelectedImage(pub String);

/// Get the selected image URL for the user
#[derive(Debug, Clone)]
pub struct GetSelectedImage;

/// Set the default prompt text for the user
#[derive(Debug, Clone)]
pub struct SetDefaultPrompt(pub Option<String>);

/// Get the default prompt text for the user
#[derive(Debug, Clone)]
pub struct GetDefaultPrompt;

#[derive(Debug, Clone)]
pub struct GetUserId;

impl UserManager {
    pub fn get() -> Result<ActorRef<UserManager>> {
        let user_manager = ACTOR_REGISTRY
            .lock()
            .unwrap()
            .get::<UserManager, str>("user_manager")
            .context("while fetching UserManager")?
            .context("while fetching UserManager")?;
        Ok(user_manager)
    }
}

impl Actor for UserManager {
    type Args = ConnectionManager;
    type Error = Infallible;

    async fn on_start(
        args: Self::Args,
        actor_ref: kameo::prelude::ActorRef<Self>,
    ) -> Result<Self, Self::Error> {
        ACTOR_REGISTRY
            .lock()
            .unwrap()
            .insert("user_manager", actor_ref.clone());
        Ok(Self {
            connection: args,
            loaded_users: HashMap::new(),
        })
    }
}

impl Message<GetUser> for UserManager {
    type Reply = Result<ActorRef<UserActor>>;

    async fn handle(&mut self, msg: GetUser, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let user = self.loaded_users.get(&msg.0).cloned();
        if let Some(user) = user {
            return Ok(user);
        }
        // Not loaded. Let's check Redis.
        info!("Loading user from Redis: {:?}", msg.0);
        let key = msg.0.key();
        let user = util::retry(|| async {
            let mut conn = self.connection.clone();
            conn.get(key.clone()).await.context("Redis get failed")
        })
        .await?;

        // Deserialize or create new.
        let user = if let Some(user) = user {
            // Found in Redis, spawn actor.
            info!("Found existing user in Redis: {:?}", user);
            json::from_str(user).context("while deserializing user from Redis")?
        } else {
            // New user.
            info!("Creating new user: {:?}", msg.0);
            User {
                id: msg.0.clone(),
                username: msg.1.clone(),
                selected_image_url: None,
                default_prompt: None,
            }
        };

        let user_ref = UserActor::spawn(UserActor {
            user,
            redis: self.connection.clone(),
        });
        user_ref.tell(UpdateUsername(msg.1)).send().await?;

        self.loaded_users.insert(msg.0.clone(), user_ref.clone());
        Ok(user_ref)
    }
}

impl Actor for UserActor {
    type Args = UserActor;
    type Error = Error;

    async fn on_start(
        args: Self::Args,
        actor_ref: kameo::prelude::ActorRef<Self>,
    ) -> Result<Self, Self::Error> {
        Ok(args)
    }
}

impl Message<UpdateUsername> for UserActor {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        msg: UpdateUsername,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if self.user.username != msg.0 {
            self.user.username = msg.0;
            self.persist().await?;
        }
        Ok(())
    }
}

impl Message<AddGeneratedImage> for UserActor {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        msg: AddGeneratedImage,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let generated_image = GeneratedImage {
            url: msg.url,
            prompt: msg.prompt,
            timestamp: chrono::Utc::now().to_rfc3339(),
            model: msg.model,
            backend: msg.backend,
        };

        let key = format!("user:images:{}", self.user.id.key());
        let value = json::to_string(&generated_image)
            .context("while serializing generated image to JSON")?;
        let score = chrono::Utc::now().timestamp_millis() as f64;

        info!("Adding generated image to user history: {}", key);

        crate::util::retry(|| async {
            let mut conn = self.redis.clone();
            conn.zadd(key.clone(), value.clone(), score)
                .await
                .context("Redis ZADD failed")
        })
        .await
        .context("while adding generated image to Redis")?;

        Ok(())
    }
}

impl Message<SetSelectedImage> for UserActor {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        msg: SetSelectedImage,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.user.selected_image_url = Some(msg.0);
        self.persist().await?;
        Ok(())
    }
}

impl Message<GetSelectedImage> for UserActor {
    type Reply = Result<Option<String>>;

    async fn handle(
        &mut self,
        _msg: GetSelectedImage,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(self.user.selected_image_url.clone())
    }
}

impl Message<SetDefaultPrompt> for UserActor {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        msg: SetDefaultPrompt,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.user.default_prompt = msg.0;
        self.persist().await?;
        Ok(())
    }
}

impl Message<GetDefaultPrompt> for UserActor {
    type Reply = Result<Option<String>>;

    async fn handle(
        &mut self,
        _msg: GetDefaultPrompt,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(self.user.default_prompt.clone())
    }
}

impl Message<GetUserId> for UserActor {
    type Reply = Result<UserId>;

    async fn handle(
        &mut self,
        _msg: GetUserId,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(self.user.id.clone())
    }
}

impl UserActor {
    /// Persist the user to Redis.
    /// This is called whenever the user is updated.
    #[tracing::instrument(skip_all, fields(user_id = %self.user.id.key()))]
    async fn persist(&mut self) -> Result<()> {
        info!("Persisting user to Redis");
        let key = self.user.id.key();
        let value = json::to_string(&self.user).context("while serializing user to JSON")?;
        util::retry(|| async {
            let mut conn = self.redis.clone();
            conn.set(key.clone(), value.clone())
                .await
                .context("Redis set failed")
        })
        .await
        .context("while saving user to Redis")?;
        Ok(())
    }
}

impl UserId {
    pub fn key(&self) -> String {
        match self {
            UserId::Irc(nick) => format!("user:irc:{}", nick),
            UserId::Discord(id) => format!("user:discord:{}", id),
        }
    }
}
