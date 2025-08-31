/// OpenRouter API module.
/// This mostly wraps the openrouter_api crate with some convenience methods, such as a conversation actor.
use anyhow::{Context as _, Result, bail};
use kameo::actor::ActorRef;
use kameo::prelude::*;
use kameo::registry::ACTOR_REGISTRY;
use tracing::instrument;

use crate::config::global::OpenrouterConfig;
use crate::messages::chat;

/// Singleton actor that manages OpenRouter API access
pub struct OpenRouter {
    config: OpenrouterConfig,
}

impl Actor for OpenRouter {
    type Args = OpenrouterConfig;
    type Error = anyhow::Error;

    async fn on_start(config: Self::Args, actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        tracing::info!("Starting OpenRouter actor");
        if config.token.is_empty() {
            bail!("OpenRouter token must not be empty");
        }

        actor_ref
            .register("openrouter")
            .context("while registering OpenRouter actor")?;

        Ok(OpenRouter { config })
    }
}

impl OpenRouter {
    /// Get a reference to the global OpenRouter actor.
    pub fn get() -> Result<ActorRef<Self>> {
        let actor_ref = ACTOR_REGISTRY
            .lock()
            .unwrap()
            .get::<OpenRouter, str>("openrouter")
            .context("while getting OpenRouter actor from registry")?
            .context("OpenRouter actor not found in registry")?;
        Ok(actor_ref)
    }
}

impl Message<chat::Oneshot> for OpenRouter {
    type Reply = ForwardedReply<chat::Oneshot, Result<chat::OneshotResponse>>;

    #[instrument(skip_all)]
    async fn handle(
        &mut self,
        msg: chat::Oneshot,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        tracing::info!("Received oneshot request from {}", msg.origin);

        // Spawn a new conversation actor to handle this specific request
        let actor_ref = ConversationActor::spawn_link(&ctx.actor_ref(), self.config.clone()).await;

        ctx.forward(&actor_ref, msg).await
    }
}

struct ConversationActor {
    config: OpenrouterConfig,
}

/// Actor that handles a single conversation with OpenRouter
/// TODO: Implement conversation logic!
/// Right now only Oneshot requests are handled.
impl Actor for ConversationActor {
    type Args = OpenrouterConfig;
    type Error = anyhow::Error;

    async fn on_start(config: Self::Args, _actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        tracing::info!("Starting ConversationActor for OpenRouter");
        Ok(ConversationActor { config })
    }
}

impl Message<chat::Oneshot> for ConversationActor {
    type Reply = Result<chat::OneshotResponse>;

    #[instrument(skip_all)]
    async fn handle(
        &mut self,
        msg: chat::Oneshot,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        tracing::info!("Handling oneshot request with purpose: {:?}", msg.purpose);

        // Here you would implement the logic to interact with OpenRouter API
        // For now, we just echo back the text
        let response_text = msg
            .text
            .iter()
            .map(|part| match part {
                chat::Part::Cacheable(text) | chat::Part::Uncacheable(text) => text.clone(),
            })
            .collect::<Vec<_>>()
            .join(" ");

        Ok(chat::OneshotResponse {
            text: response_text,
        })
    }
}
