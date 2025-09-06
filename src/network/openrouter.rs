/// OpenRouter API module.
/// This mostly wraps the openrouter_api crate with some convenience methods, such as a conversation actor.
use anyhow::{Context as _, Result, bail};
use kameo::actor::ActorRef;
use kameo::prelude::*;
use kameo::registry::ACTOR_REGISTRY;
use openrouter_api::OpenRouterClient;
use openrouter_api::models::structured::JsonSchemaConfig;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use tracing::{error, info, instrument};

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

impl<T> Message<chat::Structured<T>> for OpenRouter
where
    T: DeserializeOwned + Reply + 'static + Send,
{
    type Reply = ForwardedReply<chat::Structured<T>, Result<T>>;

    #[instrument(skip_all)]
    async fn handle(
        &mut self,
        msg: chat::Structured<T>,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        tracing::info!("Received structured request from {}", msg.origin);

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

fn select_model(purpose: &chat::Purpose, config: &OpenrouterConfig) -> String {
    match purpose {
        chat::Purpose::Chat => config.chat_model.clone(),
        chat::Purpose::Image => config.image_model.clone(),
    }
}

impl<T> Message<chat::Structured<T>> for ConversationActor
where
    T: DeserializeOwned + Reply + 'static + Send,
{
    type Reply = Result<T>;

    async fn handle(
        &mut self,
        msg: chat::Structured<T>,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        info!(
            "Handling structured request with purpose: {:?}",
            msg.purpose
        );

        // Combine all text parts into a single message
        // TODO: Handle cacheable vs uncacheable parts differently in the future
        let message_content = msg
            .text
            .iter()
            .map(|part| match part {
                chat::Part::Cacheable(text) | chat::Part::Uncacheable(text) => text.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n");

        let model = select_model(&msg.purpose, &self.config);
        tracing::debug!(
            "Using model: {} for structured request from {}",
            model,
            msg.origin
        );

        // Initialize the OpenRouter client for this request
        let client = OpenRouterClient::production(
            self.config.token.clone(),
            "GANBot".to_string(),
            "https://github.com/Baughn/ganbot3-rs".to_string(),
        )
        .context("while creating OpenRouter client")?;

        // Create a structured chat completion request
        let structured_api = client
            .structured()
            .context("while getting structured API")?;

        let result = structured_api
            .generate(
                &model,
                vec![openrouter_api::Message {
                    role: "user".to_string(),
                    content: message_content,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                }],
                JsonSchemaConfig {
                    name: "Response".to_string(),
                    schema: msg.schema,
                    strict: true,
                },
            )
            .await;

        match result {
            Ok(response) => {
                return Ok(response);
            }
            Err(e) => {
                error!("OpenRouter structured API error response: {:?}", e);
                // This should be JSON. Attempt the extract $.error.message.
                match e {
                    openrouter_api::Error::ApiError {
                        code,
                        message,
                        metadata,
                    } => {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&message) {
                            if let Some(msg) = json
                                .get("error")
                                .and_then(|err| err.get("message"))
                                .and_then(|m| m.as_str())
                            {
                                bail!("OpenRouter API error: {}", msg);
                            }
                        }
                    }
                    other => {
                        bail!("OpenRouter API error: {:?}", other);
                    }
                }
                // Fallback to logging a generic error.
                bail!("OpenRouter API request failed");
            }
        }
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
        info!("Handling oneshot request with purpose: {:?}", msg.purpose);

        // Combine all text parts into a single message
        // TODO: Handle cacheable vs uncacheable parts differently in the future
        let message_content = msg
            .text
            .iter()
            .map(|part| match part {
                chat::Part::Cacheable(text) | chat::Part::Uncacheable(text) => text.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n");

        let model = select_model(&msg.purpose, &self.config);
        tracing::debug!("Using model: {} for request from {}", model, msg.origin);

        // Initialize the OpenRouter client for this request
        let client = OpenRouterClient::production(
            self.config.token.clone(),
            "GANBot".to_string(),
            "https://github.com/Baughn/ganbot3-rs".to_string(),
        )
        .context("while creating OpenRouter client")?;

        // Create a schemaless chat completion request
        let request = openrouter_api::ChatCompletionRequest {
            model,
            messages: vec![openrouter_api::Message {
                role: "user".to_string(),
                content: message_content,
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            stream: Some(false),
            response_format: None,
            tools: None,
            provider: None,
            models: None,
            transforms: None,
        };

        // Send the request to OpenRouter API
        let chat_api = client
            .chat()
            .map_err(|e| anyhow::anyhow!("Failed to get chat API: {}", e))?;

        let chat_response = chat_api.chat_completion(request).await;

        match chat_response {
            Ok(response) => {
                info!("Chat response: {response:?}");
                // Extract the response text from the first choice
                if let Some(choice) = response.choices.first() {
                    Ok(chat::OneshotResponse {
                        text: choice.message.content.clone(),
                    })
                } else {
                    tracing::error!("No choices in OpenRouter response");
                    Err(anyhow::anyhow!("No response choices from OpenRouter"))
                }
            }
            Err(e) => {
                error!("OpenRouter API error response: {:?}", e);
                // This should be JSON. Attempt the extract $.error.message.
                match e {
                    openrouter_api::Error::ApiError {
                        code,
                        message,
                        metadata,
                    } => {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&message) {
                            if let Some(msg) = json
                                .get("error")
                                .and_then(|err| err.get("message"))
                                .and_then(|m| m.as_str())
                            {
                                bail!("OpenRouter API error: {}", msg);
                            }
                        }
                    }
                    other => {
                        bail!("OpenRouter API error: {:?}", other);
                    }
                }
                // Fallback to logging a generic error.
                bail!("OpenRouter API request failed");
            }
        }
    }
}
