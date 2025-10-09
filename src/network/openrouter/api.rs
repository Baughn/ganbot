//! Minimal OpenRouter API wrapper that models the single-turn flows GANBot currently uses.
//! This home-grown client keeps the rest of the codebase unchanged while avoiding external
//! dependencies.
use std::sync::Arc;

use crate::network::openrouter::structured::*;
use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use image::RgbImage;
use kameo::{Actor, actor::ActorRef, prelude::*};
use reqwest_middleware::ClientWithMiddleware;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tracing::{debug, instrument, warn};

const OPENROUTER_CHAT_COMPLETIONS: &str = "https://openrouter.ai/api/v1/chat/completions";

/// Configuration used when starting the [`OpenRouterApi`] actor.
#[derive(Debug, Clone)]
pub struct OpenRouterApiConfig {
    pub token: String,
    pub client: Option<ClientWithMiddleware>,
}

/// Actor that exposes a minimal, single-turn OpenRouter client.
pub struct OpenRouterApi {
    client: ClientWithMiddleware,
    token: String,
}

impl Actor for OpenRouterApi {
    type Args = OpenRouterApiConfig;
    type Error = anyhow::Error;

    async fn on_start(config: Self::Args, _actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        let OpenRouterApiConfig { token, client } = config;

        if token.trim().is_empty() {
            bail!("OpenRouter token must not be empty");
        }

        let client = if let Some(client) = client {
            client
        } else {
            let base_client = reqwest::Client::builder()
                .user_agent("GANBot/3 (https://github.com/Baughn/ganbot-rs)")
                .build()
                .context("while building OpenRouter HTTP client")?;
            ClientWithMiddleware::from(base_client)
        };

        Ok(Self { client, token })
    }
}

/// MIME types supported for encoded image uploads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageMimeKind {
    Jpeg,
}

impl ImageMimeKind {
    fn as_str(&self) -> &'static str {
        match self {
            ImageMimeKind::Jpeg => "image/jpeg",
        }
    }
}

/// A named image attachment that can either reference raw pixel data or an existing URL.
#[derive(Debug, Clone)]
pub enum RequestImage {
    #[allow(dead_code)]
    Url { name: Option<String>, url: String },
    Data {
        name: Option<String>,
        mime: ImageMimeKind,
        image: Arc<RgbImage>,
    },
}

impl RequestImage {
    fn display_name(&self, fallback: &str) -> String {
        match self {
            RequestImage::Url { name, .. } | RequestImage::Data { name, .. } => {
                name.clone().unwrap_or_else(|| fallback.to_string())
            }
        }
    }
}

/// Request payload for a single-turn chat completion.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub origin: Option<String>,
    pub models: Vec<String>,
    pub text: Option<String>,
    pub image: Option<RequestImage>,
    /// When true, request both textual and image outputs.
    pub expect_image: bool,
}

impl CompletionRequest {
    pub fn validate(&self) -> Result<()> {
        if self.models.is_empty() {
            bail!("At least one model must be provided");
        }
        Ok(())
    }
}

/// Response payload for [`CompletionRequest`].
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionResponse {
    pub model: Option<String>,
    pub text: Option<String>,
    pub image: Option<RgbImage>,
}

/// Structured output request description.
#[derive(Debug, Clone)]
pub struct StructuredRequest<T> {
    pub origin: Option<String>,
    pub models: Vec<String>,
    pub text: String,
    pub schema: JsonSchemaConfig,
    pub image: Option<RequestImage>,
    phantom: std::marker::PhantomData<T>,
}

impl<T> StructuredRequest<T> {
    pub fn new(
        origin: Option<String>,
        models: Vec<String>,
        text: String,
        schema: JsonSchemaConfig,
        image: Option<RequestImage>,
    ) -> Self {
        Self {
            origin,
            models,
            text,
            schema,
            image,
            phantom: std::marker::PhantomData,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.models.is_empty() {
            bail!("At least one model must be provided");
        }
        if self.schema.name.trim().is_empty() {
            bail!("Structured output schema name must not be empty");
        }
        Ok(())
    }
}

impl Message<CompletionRequest> for OpenRouterApi {
    type Reply = Result<CompletionResponse>;

    #[instrument(name = "OpenRouterApi.completion", skip_all, fields(origin = msg.origin.as_deref().unwrap_or("unknown")))]
    async fn handle(
        &mut self,
        msg: CompletionRequest,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        msg.validate().context("invalid completion request")?;

        let extracted_urls = msg.text.as_deref().map(extract_urls).unwrap_or_default();

        if !extracted_urls.is_empty() {
            debug!(urls = ?extracted_urls, "Detected image URLs in prompt");
        }

        let content = build_user_content(msg.text.as_deref(), msg.image.as_ref(), &extracted_urls)?;

        let mut payload = json!({
            "messages": [
                {
                    "role": "user",
                    "content": content,
                }
            ],
            "models": msg.models,
            "stream": false,
        });

        if msg.expect_image {
            payload["modalities"] = json!(["text", "image"]);
        }

        let response_body = self
            .client
            .post(OPENROUTER_CHAT_COMPLETIONS)
            .bearer_auth(&self.token)
            .json(&payload)
            .send()
            .await
            .context("failed to call OpenRouter")?
            .error_for_status()
            .context("OpenRouter returned an error status")?
            .text()
            .await
            .context("failed to read OpenRouter response body")?;

        let response_json: Value = serde_json::from_str(&response_body)
            .context("failed to parse OpenRouter response body as JSON")?;

        let choice = response_json
            .get("choices")
            .and_then(|choices| choices.get(0))
            .ok_or_else(|| anyhow!("OpenRouter response did not contain any choices"))?;

        let message = choice
            .get("message")
            .ok_or_else(|| anyhow!("OpenRouter response missing message field"))?;

        let text = extract_text_from_message(message);
        let image = if msg.expect_image {
            extract_image_from_message(message)?
        } else {
            match extract_image_from_message(message) {
                Ok(img) => img,
                Err(err) => {
                    if !err.is::<MissingImageError>() {
                        warn!(error = ?err, "Failed to decode image payload");
                    }
                    None
                }
            }
        };

        let model = choice
            .get("model")
            .and_then(|model| model.as_str())
            .map(ToString::to_string);

        Ok(CompletionResponse { model, text, image })
    }
}

impl<T> Message<StructuredRequest<T>> for OpenRouterApi
where
    T: DeserializeOwned + Send + 'static,
{
    type Reply = Result<T>;

    #[instrument(name = "OpenRouterApi.structured", skip_all, fields(origin = msg.origin.as_deref().unwrap_or("unknown")))]
    async fn handle(
        &mut self,
        msg: StructuredRequest<T>,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        msg.validate().context("invalid structured request")?;

        let extracted_urls = extract_urls(&msg.text);
        if !extracted_urls.is_empty() {
            debug!(urls = ?extracted_urls, "Detected URLs in structured request");
        }

        let content =
            build_user_content(Some(msg.text.as_str()), msg.image.as_ref(), &extracted_urls)?;

        let mut payload = json!({
            "messages": [
                {
                    "role": "user",
                    "content": content,
                }
            ],
            "models": msg.models,
            "stream": false,
        });

        payload["response_format"] = json!({
            "type": "json_schema",
            "json_schema": {
                "name": msg.schema.name,
                "strict": msg.schema.strict,
                "schema": msg.schema.schema,
            }
        });

        let response_body = self
            .client
            .post(OPENROUTER_CHAT_COMPLETIONS)
            .bearer_auth(&self.token)
            .json(&payload)
            .send()
            .await
            .context("failed to call OpenRouter structured endpoint")?
            .error_for_status()
            .context("OpenRouter returned an error status for structured request")?
            .text()
            .await
            .context("failed to read OpenRouter structured response body")?;

        let response_json: Value = serde_json::from_str(&response_body)
            .context("failed to parse OpenRouter structured response")?;

        let choice = response_json
            .get("choices")
            .and_then(|choices| choices.get(0))
            .ok_or_else(|| anyhow!("OpenRouter structured response missing choices"))?;

        if let Some(model) = choice.get("model").and_then(|m| m.as_str()) {
            debug!(model = %model, "OpenRouter selected model for structured request");
        }

        let message = choice
            .get("message")
            .ok_or_else(|| anyhow!("OpenRouter structured response missing message"))?;

        let content_value = message
            .get("content")
            .ok_or_else(|| anyhow!("OpenRouter structured response missing content"))?;

        let json_payload = match content_value {
            Value::String(raw) => serde_json::from_str::<Value>(raw)
                .with_context(|| format!("failed to parse structured response as JSON: {raw}"))?,
            Value::Array(items) => {
                let mut candidate: Option<Value> = None;
                let mut last_err = None;

                for item in items {
                    if (item.get("type").and_then(|v| v.as_str()) == Some("output_text")
                        || item.get("type").and_then(|v| v.as_str()) == Some("text"))
                        && let Some(raw) = item.get("text").and_then(|v| v.as_str())
                    {
                        match serde_json::from_str::<Value>(raw) {
                            Ok(val) => {
                                candidate = Some(val);
                                break;
                            }
                            Err(err) => {
                                last_err = Some(err);
                            }
                        }
                    }
                }

                if let Some(val) = candidate {
                    val
                } else if let Some(err) = last_err {
                    return Err(anyhow!(
                        "failed to decode structured response from content array: {err}"
                    ));
                } else {
                    bail!("Structured response did not contain textual JSON content");
                }
            }
            other => bail!("Unexpected structured response content type: {other:?}"),
        };

        serde_json::from_value(json_payload)
            .context("failed to deserialize structured response into target type")
    }
}

fn build_user_content(
    text: Option<&str>,
    image: Option<&RequestImage>,
    extracted_urls: &[String],
) -> Result<Vec<Value>> {
    let mut content = Vec::new();

    if let Some(text) = text {
        content.push(json!({
            "type": "text",
            "text": text,
            "name": "prompt",
        }));
    }

    for (idx, url) in extracted_urls.iter().enumerate() {
        content.push(json!({
            "type": "image_url",
            "image_url": { "url": url },
            "name": format!("reference-{idx}"),
        }));
    }

    if let Some(image) = image {
        let display_name = image.display_name("input-image");
        match image {
            RequestImage::Url { name: _, url } => {
                content.push(json!({
                    "type": "image_url",
                    "image_url": { "url": url },
                    "name": display_name.clone(),
                }));
            }
            RequestImage::Data {
                name: _,
                mime,
                image,
            } => {
                let data_url = encode_image_to_data_url(image, *mime)?;
                content.push(json!({
                    "type": "image_url",
                    "image_url": { "url": data_url },
                    "name": display_name,
                }));
            }
        }
    }

    Ok(content)
}

fn encode_image_to_data_url(image: &RgbImage, mime: ImageMimeKind) -> Result<String> {
    let mut buffer = Vec::new();

    match mime {
        ImageMimeKind::Jpeg => {
            let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut buffer);
            encoder
                .encode(
                    image.as_raw(),
                    image.width(),
                    image.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .context("failed to encode image as JPEG")?;
        }
    }

    let base64_data = base64::engine::general_purpose::STANDARD.encode(buffer);
    Ok(format!("data:{};base64,{}", mime.as_str(), base64_data))
}

#[derive(Debug, thiserror::Error)]
#[error("no image payload present in response")]
struct MissingImageError;

fn extract_image_from_message(message: &Value) -> Result<Option<RgbImage>> {
    if let Some(images) = message.get("images").and_then(|v| v.as_array())
        && let Some(first) = images.first()
        && let Some(url) = first
            .get("image_url")
            .and_then(|img| img.get("url"))
            .and_then(|url| url.as_str())
    {
        return decode_image_payload(url).map(Some);
    }

    if let Some(content) = message.get("content").and_then(|v| v.as_array()) {
        for item in content {
            if let Some(kind) = item.get("type").and_then(|v| v.as_str()) {
                match kind {
                    "image_url" => {
                        if let Some(url) = item
                            .get("image_url")
                            .and_then(|img| img.get("url"))
                            .and_then(|url| url.as_str())
                        {
                            return decode_image_payload(url).map(Some);
                        }
                    }
                    "output_image" => {
                        if let Some(data) = item.get("image_base64").and_then(|v| v.as_str()) {
                            let mime = item
                                .get("mime_type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("image/png");
                            let data_url = format!("data:{};base64,{}", mime, data);
                            return decode_image_payload(&data_url).map(Some);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Err(MissingImageError.into())
}

fn decode_image_payload(data: &str) -> Result<RgbImage> {
    if let Some(stripped) = data.strip_prefix("data:") {
        let (meta, base64_part) = stripped
            .split_once(",")
            .ok_or_else(|| anyhow!("invalid data URL"))?;

        if !meta.ends_with(";base64") {
            bail!("data URL is not base64 encoded");
        }

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(base64_part)
            .context("failed to decode base64 image payload")?;

        let decoded = image::load_from_memory(&bytes)
            .context("failed to decode image payload into image buffer")?;

        Ok(decoded.to_rgb8())
    } else {
        bail!("unsupported image payload type: expected data URL");
    }
}

fn extract_text_from_message(message: &Value) -> Option<String> {
    match message.get("content") {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Array(items)) => {
            for item in items {
                let kind = item.get("type").and_then(|v| v.as_str());
                if (matches!(kind, Some("text") | Some("output_text")))
                    && let Some(text) = item.get("text").and_then(|v| v.as_str())
                {
                    return Some(text.to_string());
                }
            }
            None
        }
        _ => None,
    }
}

/// Extract URLs from a body of text.
pub fn extract_urls(text: &str) -> Vec<String> {
    let url_regex =
        regex::Regex::new(r"https?://[^\s]+").expect("URL regex must be valid at compile time");

    url_regex
        .find_iter(text)
        .filter_map(|m| {
            let mut candidate = m.as_str();

            const TRAILING_PUNCT: &[char] =
                &[',', '.', ';', '!', '?', ')', ']', '}', '"', '\'', ':', '>'];

            while !candidate.is_empty() && candidate.ends_with(TRAILING_PUNCT) {
                candidate = &candidate[..candidate.len() - 1];
            }

            if !candidate.is_empty() && url::Url::parse(candidate).is_ok() {
                Some(candidate.to_string())
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use kameo::Actor;
    use serde::Deserialize;
    use serde_json::Map;
    use std::path::PathBuf;

    use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
    use rvcr::{VCRMiddleware, VCRMode};

    #[test]
    fn extract_urls_handles_common_trailing_characters() {
        let text = "Look at https://brage.info/vacation.jpg, and also https://example.com/test.";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec![
                "https://brage.info/vacation.jpg".to_string(),
                "https://example.com/test".to_string()
            ]
        );
    }

    #[test]
    fn extract_urls_handles_parentheses_and_quotes() {
        let text = "(https://brage.info/vacation.jpg) and \"https://example.com/next\"";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec![
                "https://brage.info/vacation.jpg".to_string(),
                "https://example.com/next".to_string()
            ]
        );
    }

    fn openrouter_vcr_client(name: &str) -> Result<ClientWithMiddleware> {
        let mut mode = match std::env::var("GANBOT_OPENROUTER_VCR") {
            Ok(value) if value.eq_ignore_ascii_case("record") => VCRMode::Record,
            Ok(value) if value.eq_ignore_ascii_case("replay") => VCRMode::Replay,
            _ => VCRMode::Replay,
        };

        let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.push("tests");
        dir.push("vcr");

        let mut cassette_path = dir.clone();
        cassette_path.push(format!("{name}.vcr.json"));

        if matches!(mode, VCRMode::Replay) && !cassette_path.exists() {
            eprintln!(
                "Forcing record mode: cassette '{}' not found (set GANBOT_OPENROUTER_VCR=record to force capture)",
                cassette_path.display()
            );
            mode = VCRMode::Record;
        }

        std::fs::create_dir_all(&dir)
            .context("while ensuring directory for OpenRouter VCR cassettes exists")?;

        let middleware = VCRMiddleware::try_from(cassette_path.clone())
            .map_err(|err| anyhow!(err))?
            .with_mode(mode);

        let base_client = reqwest::Client::builder()
            .user_agent("GANBot/3 (https://github.com/Baughn/ganbot-rs)")
            .build()
            .context("while building reqwest client for OpenRouter VCR test")?;

        let client = ClientBuilder::new(base_client).with(middleware).build();
        Ok(client)
    }

    #[tokio::test]
    #[ignore]
    async fn completion_roundtrip_works_with_urls() -> Result<()> {
        let config = crate::config::load()?;
        if config.openrouter.token.is_empty() {
            eprintln!("Skipping OpenRouter integration test: token not configured");
            return Ok(());
        }

        let client = openrouter_vcr_client("completion_roundtrip")?;

        let models = if config.openrouter.cheap_model.is_empty() {
            vec![config.openrouter.chat_model.clone()]
        } else {
            config.openrouter.cheap_model.clone()
        };

        let actor = OpenRouterApi::spawn(OpenRouterApiConfig {
            token: config.openrouter.token.clone(),
            client: Some(client),
        });

        let response = actor
            .ask(CompletionRequest {
                origin: Some("test-url".into()),
                models: models.clone(),
                text: Some(
                    "Please describe the scene in this photo: https://brage.info/vacation.jpg."
                        .into(),
                ),
                image: None,
                expect_image: false,
            })
            .await?;

        assert!(response.text.is_some());
        Ok(())
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct CaptionResponse {
        caption: String,
    }

    #[tokio::test]
    #[ignore]
    async fn structured_roundtrip_returns_json() -> Result<()> {
        let config = crate::config::load()?;
        if config.openrouter.token.is_empty() {
            eprintln!("Skipping OpenRouter structured integration test: token not configured");
            return Ok(());
        }

        let client = openrouter_vcr_client("structured_roundtrip")?;

        let models = if config.openrouter.cheap_model.is_empty() {
            vec![config.openrouter.chat_model.clone()]
        } else {
            config.openrouter.cheap_model.clone()
        };

        let schema = JsonSchemaConfig {
            name: "CaptionResponse".to_string(),
            strict: true,
            schema: JsonSchemaDefinition {
                schema_type: "object".to_string(),
                properties: {
                    let mut map = Map::new();
                    map.insert(
                        "caption".to_string(),
                        json!({
                            "type": "string",
                            "description": "A concise caption for the vacation photo"
                        }),
                    );
                    map
                },
                required: Some(vec!["caption".to_string()]),
                additional_properties: Some(false),
            },
        };

        let actor = OpenRouterApi::spawn(OpenRouterApiConfig {
            token: config.openrouter.token.clone(),
            client: Some(client),
        });

        let response: CaptionResponse = actor
            .ask(StructuredRequest::new(
                Some("structured-test".into()),
                models,
                "Provide a JSON object with a caption for https://brage.info/vacation.jpg".into(),
                schema,
                None,
            ))
            .await?;

        assert!(!response.caption.is_empty());
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn structured_roundtrip_with_image_input() -> Result<()> {
        let config = crate::config::load()?;
        if config.openrouter.token.is_empty() {
            eprintln!("Skipping OpenRouter structured image test: token not configured");
            return Ok(());
        }

        let client = openrouter_vcr_client("structured_roundtrip_image")?;

        let models = if config.openrouter.cheap_model.is_empty() {
            vec![config.openrouter.chat_model.clone()]
        } else {
            config.openrouter.cheap_model.clone()
        };

        let schema = JsonSchemaConfig {
            name: "CaptionResponse".to_string(),
            strict: true,
            schema: JsonSchemaDefinition {
                schema_type: "object".to_string(),
                properties: {
                    let mut map = Map::new();
                    map.insert(
                        "caption".to_string(),
                        json!({
                            "type": "string",
                            "description": "A concise caption for the provided image"
                        }),
                    );
                    map
                },
                required: Some(vec!["caption".to_string()]),
                additional_properties: Some(false),
            },
        };

        let actor = OpenRouterApi::spawn(OpenRouterApiConfig {
            token: config.openrouter.token.clone(),
            client: Some(client),
        });

        let response: CaptionResponse = actor
            .ask(StructuredRequest::new(
                Some("structured-image-test".into()),
                models,
                "Provide a JSON caption for the attached image".into(),
                schema,
                Some(RequestImage::Url {
                    name: Some("vacation-photo".into()),
                    url: "https://brage.info/vacation.jpg".into(),
                }),
            ))
            .await?;

        assert!(!response.caption.is_empty());
        Ok(())
    }
}
