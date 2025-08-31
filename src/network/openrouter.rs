/// OpenRouter API module.
/// This mostly wraps the openrouter_api crate with some convenience methods, such as a conversation actor.
use anyhow::{Context as _, Result, bail};
use kameo::actor::ActorRef;
use kameo::prelude::*;
use kameo::registry::ACTOR_REGISTRY;
use openrouter_api::OpenRouterClient;
use tracing::{error, instrument};

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

        // Combine all text parts into a single message
        let message_content = msg
            .text
            .iter()
            .map(|part| match part {
                chat::Part::Cacheable(text) | chat::Part::Uncacheable(text) => text.clone(),
            })
            .collect::<Vec<_>>()
            .join(" ");

        // Select the appropriate model based on the purpose
        let model = match msg.purpose {
            chat::Purpose::Chat => self.config.chat_model.clone(),
            chat::Purpose::Image => self.config.image_model.clone(),
        };

        tracing::debug!("Using model: {} for request from {}", model, msg.origin);

        // Initialize the OpenRouter client for this request
        let client = OpenRouterClient::production(
            self.config.token.clone(),
            "Ganbot3".to_string(),
            "https://github.com/svein/ganbot3-rs".to_string(),
        )
        .context("while creating OpenRouter client")?;

        // Create the chat completion request using the openrouter_api crate
        let request = openrouter_api::ChatCompletionRequest {
            model,
            messages: vec![openrouter_api::Message {
                role: "user".to_string(),
                content: message_content,
                name: None,
                tool_calls: None,
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

        match chat_api.chat_completion(request).await {
            Ok(response) => {
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
