/// OpenRouter API module.
/// This mostly wraps the openrouter_api crate with some convenience methods, such as a conversation actor.
use anyhow::{Context as _, Result, bail};
use kameo::actor::ActorRef;
use kameo::prelude::*;
use kameo::registry::ACTOR_REGISTRY;
use openrouter_api::OpenRouterClient;
use openrouter_api::models::structured::JsonSchemaConfig;
use serde::de::DeserializeOwned;
use serde_json::json;
use tracing::{error, info, instrument};

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

impl Message<NanoBanana> for OpenRouter {
    type Reply = ForwardedReply<NanoBanana, Result<NanoBananaResponse>>;

    #[instrument(skip_all)]
    async fn handle(
        &mut self,
        msg: NanoBanana,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        info!("Received NanoBanana request from {}", msg.origin);

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
            Ok(response) => Ok(response),
            Err(e) => {
                error!("OpenRouter structured API error response: {:?}", e);
                // This should be JSON. Attempt the extract $.error.message.
                match e {
                    openrouter_api::Error::ApiError {
                        code,
                        message,
                        metadata,
                    } => {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&message)
                            && let Some(msg) = json
                                .get("error")
                                .and_then(|err| err.get("message"))
                                .and_then(|m| m.as_str())
                        {
                            bail!("OpenRouter API error: {}", msg);
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
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&message)
                            && let Some(msg) = json
                                .get("error")
                                .and_then(|err| err.get("message"))
                                .and_then(|m| m.as_str())
                        {
                            bail!("OpenRouter API error: {}", msg);
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

impl Message<NanoBanana> for ConversationActor {
    type Reply = Result<NanoBananaResponse>;

    #[instrument(skip_all)]
    async fn handle(
        &mut self,
        msg: NanoBanana,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        info!("Handling NanoBanana image request");

        // openrouter_api doesn't support these yet, so time to get our hands dirty.
        let url = "https://openrouter.ai/api/v1/chat/completions";
        let client = reqwest::Client::new();
        let model = "google/gemini-2.5-flash-image-preview";
        let payload = json!({
            "model": model,
            "messages": [
                {
                    "role": "user",
                    "content": msg.prompt,
                }
            ],
            "modalities": ["text", "image"],
        });

        for backoff in [0, 5, 30] {
            if backoff > 0 {
                info!("Waiting {} seconds before retrying...", backoff);
                tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
            }

            let resp = client
                .post(url)
                .bearer_auth(&self.config.token)
                .json(&payload)
                .send()
                .await;

            match resp {
                Err(e) => {
                    error!("HTTP request error: {:?}", e);
                    continue;
                }
                Ok(resp) => {
                    if !resp.status().is_success() {
                        error!("OpenRouter API returned error status: {}", resp.status());
                        let body = resp.text().await.unwrap_or_default();
                        error!("Response body: {}", body);
                        continue;
                    }

                    let body = resp.text().await.unwrap_or_default();
                    let json: serde_json::Value = match serde_json::from_str(&body) {
                        Ok(j) => j,
                        Err(e) => {
                            error!("Failed to parse JSON response: {:?}", e);
                            continue;
                        }
                    };

                    // Expecting something like:
                    // {
                    //   "choices": [
                    //     {
                    //       "message": {
                    //         "role": "assistant",
                    //         "content": "I've generated a beautiful sunset image for you.",
                    //         "images": [
                    //           {
                    //             "type": "image_url",
                    //             "image_url": {
                    //               "url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAA..."
                    //             }
                    //           }
                    //         ]
                    //       }
                    //     }
                    //   ]
                    // }

                    // Extract text content
                    let text_content = json
                        .get("choices")
                        .and_then(|choices| choices.get(0))
                        .and_then(|choice| choice.get("message"))
                        .and_then(|message| message.get("content"))
                        .and_then(|content| content.as_str())
                        .unwrap_or("Generated an image for you.")
                        .to_string();

                    // Extract optional image
                    let image = if let Some(image_data) = json
                        .get("choices")
                        .and_then(|choices| choices.get(0))
                        .and_then(|choice| choice.get("message"))
                        .and_then(|message| message.get("images"))
                        .and_then(|images| images.get(0))
                        .and_then(|image| image.get("image_url"))
                        .and_then(|img_url| img_url.get("url"))
                        .and_then(|url| url.as_str())
                    {
                        // Expecting a data URL like "data:image/png;base64,...."
                        if let Some(base64_data) = image_data.strip_prefix("data:image/png;base64,")
                        {
                            use base64::Engine;
                            match base64::engine::general_purpose::STANDARD.decode(base64_data) {
                                Ok(image_bytes) => match image::load_from_memory(&image_bytes) {
                                    Ok(img) => {
                                        let rgb_image = img.to_rgb8();
                                        info!("Successfully generated image via OpenRouter");
                                        Some(rgb_image)
                                    }
                                    Err(e) => {
                                        error!("Failed to decode image from bytes: {:?}", e);
                                        None
                                    }
                                },
                                Err(e) => {
                                    error!("Failed to decode base64 image data: {:?}", e);
                                    None
                                }
                            }
                        } else {
                            error!("Image URL is not a valid data URL");
                            None
                        }
                    } else {
                        None
                    };

                    return Ok(NanoBananaResponse {
                        text: text_content,
                        image,
                    });
                }
            }
        }
        bail!("Failed to get response from OpenRouter after retries");
    }
}
