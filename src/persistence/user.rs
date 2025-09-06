use std::collections::HashMap;

use anyhow::{Context as _, Error, Result};
use kameo::{
    Actor,
    actor::ActorRef,
    error::Infallible,
    prelude::{Context, Message},
    registry::ACTOR_REGISTRY,
};
use redis::{
    AsyncTypedCommands,
    aio::{ConnectionManager, MultiplexedConnection},
};
use serde::{Deserialize, Serialize};
use serenity::{all::UserId as DiscordUserId, json};
use tracing::info;

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
}

type UserName = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) enum UserId {
    IRC(String),
    Discord(DiscordUserId),
}

/// Update the username of the user.
struct UpdateUsername(pub UserName);

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
        let user = self.connection.get(key.clone()).await?;

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

impl UserActor {
    /// Persist the user to Redis.
    /// This is called whenever the user is updated.
    #[tracing::instrument(skip_all, fields(user_id = %self.user.id.key()))]
    async fn persist(&mut self) -> Result<()> {
        info!("Persisting user to Redis");
        let key = self.user.id.key();
        let value = json::to_string(&self.user).context("while serializing user to JSON")?;
        self.redis
            .set(key, value)
            .await
            .context("while saving user to Redis")?;
        Ok(())
    }
}

impl UserId {
    fn key(&self) -> String {
        match self {
            UserId::IRC(nick) => format!("user:irc:{}", nick),
            UserId::Discord(id) => format!("user:discord:{}", id),
        }
    }
}
