/// OpenRouter API module.
/// Provides an actor-based wrapper around the OpenRouter HTTP API with conversation helpers.
pub mod api;
pub mod structured;

use self::structured::JsonSchemaConfig;
use anyhow::{Context as _, Result, anyhow, bail};
use kameo::actor::ActorRef;
use kameo::prelude::*;
use kameo::registry::ACTOR_REGISTRY;
use serde::de::DeserializeOwned;
use tracing::{debug, info, instrument};

use self::api::{
    CompletionRequest, ImageMimeKind, OpenRouterApi, OpenRouterApiConfig, RequestImage,
    StructuredRequest,
};

use crate::config::global::OpenrouterConfig;
use crate::messages::chat::{self, NanoBanana, NanoBananaResponse};

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

    #[instrument(name = "OpenRouter.oneshot", skip_all)]
    async fn handle(
        &mut self,
        msg: chat::Oneshot,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        tracing::info!("Received oneshot request from {}", msg.origin);

        // Spawn a new conversation actor to handle this specific request
        let actor_ref = ConversationActor::spawn_link(ctx.actor_ref(), self.config.clone()).await;

        ctx.forward(&actor_ref, msg).await
    }
}

impl<T> Message<chat::Structured<T>> for OpenRouter
where
    T: DeserializeOwned + Reply + 'static + Send,
{
    type Reply = ForwardedReply<chat::Structured<T>, Result<T>>;

    #[instrument(name = "OpenRouter.structured", skip_all)]
    async fn handle(
        &mut self,
        msg: chat::Structured<T>,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        tracing::info!("Received structured request from {}", msg.origin);

        // Spawn a new conversation actor to handle this specific request
        let actor_ref = ConversationActor::spawn_link(ctx.actor_ref(), self.config.clone()).await;

        ctx.forward(&actor_ref, msg).await
    }
}

impl Message<NanoBanana> for OpenRouter {
    type Reply = ForwardedReply<NanoBanana, Result<NanoBananaResponse>>;

    #[instrument(name = "OpenRouter.nanobanana", skip_all)]
    async fn handle(
        &mut self,
        msg: NanoBanana,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        info!("Received NanoBanana request from {}", msg.origin);

        // Spawn a new conversation actor to handle this specific request
        let actor_ref = ConversationActor::spawn_link(ctx.actor_ref(), self.config.clone()).await;

        ctx.forward(&actor_ref, msg).await
    }
}

struct ConversationActor {
    config: OpenrouterConfig,
    api: ActorRef<OpenRouterApi>,
}

impl Actor for ConversationActor {
    type Args = OpenrouterConfig;
    type Error = anyhow::Error;

    async fn on_start(config: Self::Args, actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        info!("Starting ConversationActor for OpenRouter");

        let api = OpenRouterApi::spawn_link(
            &actor_ref,
            OpenRouterApiConfig {
                token: config.token.clone(),
                client: None,
            },
        )
        .await;

        Ok(Self { config, api })
    }
}

fn select_models(purpose: &chat::Purpose, config: &OpenrouterConfig) -> Vec<String> {
    let primary = match purpose {
        chat::Purpose::Chat => config.chat_model.clone(),
        chat::Purpose::Dream => config.dream_model.clone(),
    };

    let mut models = config.cheap_model.clone();

    // Prefer the configured model, but fall back to any of the cheap models
    // if the preferred model is unavailable.
    if models.is_empty() {
        models.push(primary);
    } else if !models.iter().any(|m| m == &primary) {
        models.insert(0, primary);
    }

    models
}

fn select_image_models(config: &OpenrouterConfig) -> Vec<String> {
    vec![config.image_model.clone()]
}

fn combine_parts(parts: Vec<chat::Part>) -> String {
    let mut segments = Vec::with_capacity(parts.len());

    for part in parts {
        let text = match part {
            chat::Part::Cacheable(text) | chat::Part::Uncacheable(text) => text,
        };
        segments.push(text);
    }

    segments.join("\n")
}

impl<T> Message<chat::Structured<T>> for ConversationActor
where
    T: DeserializeOwned + Reply + 'static + Send,
{
    type Reply = Result<T>;

    #[instrument(name = "ConversationActor.structured", skip_all)]
    async fn handle(
        &mut self,
        msg: chat::Structured<T>,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let chat::Structured {
            purpose,
            origin,
            text,
            schema,
            ..
        } = msg;

        info!("Handling structured request with purpose: {:?}", purpose);

        let prompt = combine_parts(text);
        let models = select_models(&purpose, &self.config);

        debug!(origin = %origin, models = ?models, "Dispatching structured OpenRouter request");

        let schema = JsonSchemaConfig {
            name: "Response".to_string(),
            schema,
            strict: true,
        };

        let request =
            StructuredRequest::<T>::new(Some(origin.clone()), models, prompt, schema, None);

        self.api
            .ask(request)
            .await
            .map_err(|err| anyhow!("OpenRouter structured request failed: {err:#}"))
    }
}

impl Message<chat::Oneshot> for ConversationActor {
    type Reply = Result<chat::OneshotResponse>;

    #[instrument(name = "ConversationActor.oneshot", skip_all)]
    async fn handle(
        &mut self,
        msg: chat::Oneshot,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let chat::Oneshot {
            purpose,
            origin,
            text,
        } = msg;

        info!("Handling oneshot request with purpose: {:?}", purpose);

        let prompt = combine_parts(text);
        let models = select_models(&purpose, &self.config);

        debug!(origin = %origin, models = ?models, "Dispatching oneshot OpenRouter request");

        let request = CompletionRequest {
            origin: Some(origin.clone()),
            models,
            text: Some(prompt),
            image: None,
            expect_image: false,
        };

        let response = self
            .api
            .ask(request)
            .await
            .map_err(|err| anyhow!("OpenRouter oneshot request failed: {err:#}"))?;

        if let Some(ref model) = response.model {
            debug!(model = %model, "OpenRouter selected model");
        }

        let text = response
            .text
            .ok_or_else(|| anyhow!("OpenRouter response missing text content"))?;

        Ok(chat::OneshotResponse { text })
    }
}

impl Message<NanoBanana> for ConversationActor {
    type Reply = Result<NanoBananaResponse>;

    #[instrument(name = "ConversationActor.nanobanana", skip_all)]
    async fn handle(
        &mut self,
        msg: NanoBanana,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let NanoBanana {
            origin,
            prompt,
            input_image,
        } = msg;

        info!("Handling NanoBanana image request");

        let models = select_image_models(&self.config);

        debug!(origin = %origin, models = ?models, "Dispatching NanoBanana OpenRouter request");

        let image_attachment = input_image.map(|image| RequestImage::Data {
            name: Some("input-image".to_string()),
            mime: ImageMimeKind::Jpeg,
            image,
        });

        let request = CompletionRequest {
            origin: Some(origin.clone()),
            models,
            text: Some(prompt),
            image: image_attachment,
            expect_image: true,
        };

        let response = self
            .api
            .ask(request)
            .await
            .map_err(|err| anyhow!("OpenRouter NanoBanana request failed: {err:#}"))?;

        if let Some(ref model) = response.model {
            debug!(model = %model, "OpenRouter selected model");
        }

        let text = response
            .text
            .unwrap_or_else(|| "Generated an image for you.".to_string());

        Ok(NanoBananaResponse {
            text,
            image: response.image,
        })
    }
}
