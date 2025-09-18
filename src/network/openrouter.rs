/// OpenRouter API module.
/// This mostly wraps the openrouter_api crate with some convenience methods, such as a conversation actor.
#[path = "openrouter/api.rs"]
pub mod api;
use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use kameo::actor::ActorRef;
use kameo::prelude::*;
use kameo::registry::ACTOR_REGISTRY;
use openrouter_api::OpenRouterClient;
use openrouter_api::models::structured::JsonSchemaConfig;
use regex::Regex;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::io::Cursor;
use tracing::{debug, error, info, instrument};
use url;

use crate::config::global::OpenrouterConfig;
use crate::messages::chat::{self, NanoBanana, NanoBananaResponse};

/// Convert an RgbImage to a base64-encoded JPEG data URL
fn rgb_image_to_base64_data_url(img: &image::RgbImage) -> Result<String> {
    let mut buffer = Vec::new();
    let mut cursor = Cursor::new(&mut buffer);

    // Convert to JPEG format
    image::codecs::jpeg::JpegEncoder::new(&mut cursor)
        .encode(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgb8,
        )
        .context("Failed to encode image as JPEG")?;

    // Encode as base64
    let base64_string = base64::engine::general_purpose::STANDARD.encode(&buffer);

    Ok(format!("data:image/jpeg;base64,{}", base64_string))
}

/// Extract URLs from text using regex and url crate validation
fn extract_urls(text: &str) -> Vec<String> {
    let url_regex = Regex::new(r"https?://[^\s]+").expect("Invalid URL regex");

    url_regex
        .find_iter(text)
        .filter_map(|m| {
            let mut candidate = m.as_str();

            // Common trailing punctuation that's likely not part of the URL
            const TRAILING_PUNCT: &[char] =
                &[',', '.', ';', '!', '?', ')', ']', '}', '"', '\'', ':', '>'];

            // Always trim trailing punctuation first, even if the URL would parse as-is
            while !candidate.is_empty() && candidate.ends_with(TRAILING_PUNCT) {
                candidate = &candidate[..candidate.len() - 1];
            }

            // Now try parsing the cleaned URL
            if !candidate.is_empty() && url::Url::parse(candidate).is_ok() {
                Some(candidate.to_string())
            } else {
                None
            }
        })
        .collect()
}

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
        chat::Purpose::Dream => config.dream_model.clone(),
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
            "https://github.com/Baughn/ganbot-rs".to_string(),
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
                        code: _,
                        message,
                        metadata: _,
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
        let combined_text = msg
            .text
            .iter()
            .map(|part| match part {
                chat::Part::Cacheable(text) | chat::Part::Uncacheable(text) => text.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n");

        let model = select_model(&msg.purpose, &self.config);
        tracing::debug!("Using model: {} for request from {}", model, msg.origin);

        // Extract URLs from the text
        let urls = extract_urls(&combined_text);

        // Build message content array
        let mut message_content = Vec::new();

        // Add text content
        message_content.push(json!({
            "type": "text",
            "text": combined_text
        }));

        // Add image URLs
        for url in urls {
            info!("Including image from URL in chat: {}", url);
            message_content.push(json!({
                "type": "image_url",
                "text": format!("Reference image ({url})"),
                "image_url": {
                    "url": url
                }
            }));
        }
        debug!("Sending {message_content:#?}");

        // Use raw reqwest API for multi-part content support
        let url = "https://openrouter.ai/api/v1/chat/completions";
        let client = reqwest::Client::new();

        let payload = json!({
            "model": model,
            "messages": [{
                "role": "user",
                "content": message_content
            }]
        });

        let response = client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.config.token))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .context("Failed to send request to OpenRouter")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            bail!("OpenRouter API error: {} - {}", status, error_text);
        }

        let response_json: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse OpenRouter response as JSON")?;

        // Extract the text content from the response
        let text = response_json
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .ok_or_else(|| anyhow::anyhow!("Unexpected response format from OpenRouter"))?
            .to_string();

        Ok(chat::OneshotResponse { text })
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
        // Build the message content based on whether we have an input image
        let message_content = if let Some(input_image) = msg.input_image.as_ref() {
            // We have an input image - include it in the request
            let image_data_url = rgb_image_to_base64_data_url(input_image.as_ref())
                .context("Failed to convert input image to base64")?;

            json!([
                {
                    "type": "text",
                    "text": msg.prompt
                },
                {
                    "type": "image_url",
                    "image_url": {
                        "url": image_data_url
                    }
                }
            ])
        } else {
            // No input image - just text
            json!(msg.prompt)
        };

        let payload = json!({
            "model": model,
            "messages": [
                {
                    "role": "user",
                    "content": message_content,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_urls_with_comma() {
        let text = "Like https://brage.info/GAN/01994f64-92a9-7052-be8d-a1255a127d54.2.jpg, but on the moon";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec!["https://brage.info/GAN/01994f64-92a9-7052-be8d-a1255a127d54.2.jpg"]
        );
    }

    #[test]
    fn test_extract_urls_with_period() {
        let text = "Check out this image: https://example.com/image.jpg.";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://example.com/image.jpg"]);
    }

    #[test]
    fn test_extract_urls_in_parentheses() {
        let text = "The image (https://example.com/image.jpg) shows something interesting";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://example.com/image.jpg"]);
    }

    #[test]
    fn test_extract_multiple_urls() {
        let text = "First: https://example.com/1.jpg, second: https://example.com/2.jpg!";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec!["https://example.com/1.jpg", "https://example.com/2.jpg"]
        );
    }

    #[test]
    fn test_extract_url_with_query_params() {
        let text = "API endpoint: https://api.example.com/data?param=value&other=123, works great!";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec!["https://api.example.com/data?param=value&other=123"]
        );
    }

    #[test]
    fn test_extract_url_with_fragment() {
        let text = "Documentation at https://docs.example.com/guide#section-2.";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://docs.example.com/guide#section-2"]);
    }

    #[test]
    fn test_no_urls() {
        let text = "This text has no URLs whatsoever";
        let urls = extract_urls(text);
        assert!(urls.is_empty());
    }

    #[test]
    fn test_url_at_end_of_sentence() {
        let text = "Visit https://example.com.";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://example.com"]);
    }

    #[test]
    fn test_url_with_exclamation() {
        let text = "Amazing site: https://example.com!";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://example.com"]);
    }

    #[test]
    fn test_url_with_question_mark_outside() {
        let text = "Have you seen https://example.com?";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://example.com"]);
    }

    #[test]
    fn test_url_in_quotes() {
        let text = r#"He said "check out https://example.com" yesterday"#;
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://example.com"]);
    }

    #[test]
    fn test_url_with_trailing_slash() {
        let text = "Homepage: https://example.com/, pretty cool";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://example.com/"]);
    }

    #[test]
    fn test_http_and_https() {
        let text = "HTTP: http://example.com, HTTPS: https://secure.example.com.";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec!["http://example.com", "https://secure.example.com"]
        );
    }

    #[test]
    fn test_complex_gan_url() {
        // Real-world GAN bot URL format
        let text =
            "Generated: https://brage.info/GAN/550e8400-e29b-41d4-a716-446655440000.1.jpg, nice!";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec!["https://brage.info/GAN/550e8400-e29b-41d4-a716-446655440000.1.jpg"]
        );
    }

    #[test]
    fn test_url_with_semicolon() {
        let text = "Resources: https://example.com/docs; https://example.com/api";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec!["https://example.com/docs", "https://example.com/api"]
        );
    }
}
