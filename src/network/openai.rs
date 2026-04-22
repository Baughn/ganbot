//! Tower-based OpenAI client.
//!
//! This is a deliberate contrast to `network::openrouter`: no Kameo actor, no
//! registry lookup, no message types. The public surface is a typed Tower
//! `Service`, type-erased through `BoxCloneService` so callers don't have to
//! name a `ServiceBuilder` stack type.
//!
//! The layers composed in `build_image_service_at`:
//! 1. `TraceLayer`  — tracing span + duration logging per request
//! 2. `map_err`     — convert `tower::timeout::error::Elapsed` back into `OpenAIError::Timeout`
//! 3. `TimeoutLayer`— hard deadline per attempt (after retries)
//! 4. `RetryLayer`  — exponential backoff on transient errors (5xx, 429, transport)
//!
//! Order means request flows top-to-bottom, responses/errors bottom-to-top, so
//! the Retry sits innermost against `OpenAIHttp` and sees typed `OpenAIError`s
//! rather than the `BoxError` produced by `TimeoutLayer`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine as _;
use image::{DynamicImage, RgbImage};
use reqwest::Client;
use serde::Deserialize;
use tower::retry::{Policy, RetryLayer};
use tower::timeout::TimeoutLayer;
use tower::util::BoxCloneService;
use tower::{BoxError, Service, ServiceBuilder};
use tracing::{Instrument, info, info_span, warn};

use crate::config::global::OpenaiConfig;

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum OpenAIError {
    #[error("transport: {0}")]
    Transport(Arc<reqwest::Error>),
    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },
    #[error("bad response: {0}")]
    BadResponse(String),
    #[error("timeout")]
    Timeout,
    #[error("encode input image: {0}")]
    EncodeImage(String),
}

impl From<reqwest::Error> for OpenAIError {
    fn from(e: reqwest::Error) -> Self {
        OpenAIError::Transport(Arc::new(e))
    }
}

// ---------------------------------------------------------------------------
// Public request / response
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ImageRequest {
    pub origin: String,
    pub model: String,
    pub prompt: String,
    pub size: Option<String>,
    pub quality: Option<String>,
    pub input_image: Option<Arc<RgbImage>>,
}

#[derive(Clone, Debug)]
pub struct ImageResponse {
    pub image: RgbImage,
}

/// Type-erased handle callers hold. `clone()` is cheap (refcount bumps).
pub type ImageService = BoxCloneService<ImageRequest, ImageResponse, OpenAIError>;

// ---------------------------------------------------------------------------
// Core HTTP service
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct OpenAIHttp {
    client: Client,
    token: Arc<str>,
    base_url: Arc<str>,
}

impl OpenAIHttp {
    fn new(token: &str, base_url: &str) -> Self {
        let client = Client::builder()
            .user_agent("GANBot/3 (https://github.com/Baughn/ganbot-rs)")
            .build()
            .expect("reqwest client builds with static config");
        Self {
            client,
            token: Arc::from(token),
            base_url: Arc::from(base_url.trim_end_matches('/')),
        }
    }

    async fn call_generations(&self, req: &ImageRequest) -> Result<ImageResponse, OpenAIError> {
        // NOTE: `gpt-image-*` always returns `b64_json`; it rejects the
        // `response_format` parameter with HTTP 400 "Unknown parameter".
        // This diverges from the legacy DALL-E API.
        let url = format!("{}/v1/images/generations", self.base_url);
        let mut body = serde_json::Map::new();
        body.insert("model".into(), serde_json::json!(req.model));
        body.insert("prompt".into(), serde_json::json!(req.prompt));
        body.insert("n".into(), serde_json::json!(1));
        if let Some(sz) = &req.size {
            body.insert("size".into(), serde_json::json!(sz));
        }
        if let Some(q) = &req.quality {
            body.insert("quality".into(), serde_json::json!(q));
        }

        let resp = self
            .client
            .post(url)
            .bearer_auth(self.token.as_ref())
            .json(&body)
            .send()
            .await?;
        decode_images_response(resp).await
    }

    async fn call_edits(
        &self,
        req: &ImageRequest,
        input: &RgbImage,
    ) -> Result<ImageResponse, OpenAIError> {
        use reqwest::multipart::{Form, Part};

        let url = format!("{}/v1/images/edits", self.base_url);

        let mut png = Vec::new();
        DynamicImage::ImageRgb8(input.clone())
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| OpenAIError::EncodeImage(e.to_string()))?;
        let image_part = Part::bytes(png)
            .file_name("input.png")
            .mime_str("image/png")
            .map_err(|e| OpenAIError::EncodeImage(e.to_string()))?;

        // NOTE: like /v1/images/generations, edits on `gpt-image-*` always
        // return `b64_json`; no `response_format` field.
        let mut form = Form::new()
            .text("model", req.model.clone())
            .text("prompt", req.prompt.clone())
            .part("image", image_part);
        if let Some(sz) = &req.size {
            form = form.text("size", sz.clone());
        }
        if let Some(q) = &req.quality {
            form = form.text("quality", q.clone());
        }

        let resp = self
            .client
            .post(url)
            .bearer_auth(self.token.as_ref())
            .multipart(form)
            .send()
            .await?;
        decode_images_response(resp).await
    }
}

async fn decode_images_response(resp: reqwest::Response) -> Result<ImageResponse, OpenAIError> {
    let status = resp.status();
    let bytes = resp.bytes().await?;
    if !status.is_success() {
        return Err(OpenAIError::Api {
            status: status.as_u16(),
            message: extract_error_message(&bytes),
        });
    }
    let parsed: ImagesResponseBody = serde_json::from_slice(&bytes)
        .map_err(|e| OpenAIError::BadResponse(format!("response not valid JSON: {e}")))?;
    let datum = parsed
        .data
        .into_iter()
        .next()
        .ok_or_else(|| OpenAIError::BadResponse("empty data array".into()))?;
    let b64 = datum
        .b64_json
        .ok_or_else(|| OpenAIError::BadResponse("no b64_json in response".into()))?;
    let img_bytes = base64::engine::general_purpose::STANDARD
        .decode(&b64)
        .map_err(|e| OpenAIError::BadResponse(format!("bad base64: {e}")))?;
    let dyn_img = image::load_from_memory(&img_bytes)
        .map_err(|e| OpenAIError::BadResponse(format!("decode image: {e}")))?;
    Ok(ImageResponse {
        image: dyn_img.to_rgb8(),
    })
}

#[derive(Deserialize)]
struct ImagesResponseBody {
    data: Vec<ImagesResponseDatum>,
}

#[derive(Deserialize)]
struct ImagesResponseDatum {
    b64_json: Option<String>,
}

fn extract_error_message(bytes: &[u8]) -> String {
    #[derive(Deserialize)]
    struct Outer {
        error: Inner,
    }
    #[derive(Deserialize)]
    struct Inner {
        message: String,
    }
    if let Ok(e) = serde_json::from_slice::<Outer>(bytes) {
        e.error.message
    } else {
        String::from_utf8_lossy(bytes).chars().take(200).collect()
    }
}

impl Service<ImageRequest> for OpenAIHttp {
    type Response = ImageResponse;
    type Error = OpenAIError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: ImageRequest) -> Self::Future {
        let this = self.clone();
        Box::pin(async move {
            if let Some(img) = req.input_image.clone() {
                this.call_edits(&req, &img).await
            } else {
                this.call_generations(&req).await
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Retry policy
//
// A Tower `Policy` is called after each response. Return `Some(future)` to
// retry (the future yields the next policy state — here, with the counter
// decremented and the backoff doubled). Return `None` to stop.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct OpenAIRetryPolicy {
    attempts_left: u32,
    next_delay: Duration,
}

impl OpenAIRetryPolicy {
    pub fn new(max_attempts: u32) -> Self {
        Self::with_base_delay(max_attempts, Duration::from_millis(500))
    }

    /// Construct with a custom starting backoff. Useful for tests that want
    /// retries to be effectively instant.
    pub fn with_base_delay(max_attempts: u32, base_delay: Duration) -> Self {
        Self {
            attempts_left: max_attempts,
            next_delay: base_delay,
        }
    }
}

impl<Req: Clone, Res> Policy<Req, Res, OpenAIError> for OpenAIRetryPolicy {
    type Future = Pin<Box<dyn Future<Output = Self> + Send>>;

    fn retry(&self, _: &Req, result: Result<&Res, &OpenAIError>) -> Option<Self::Future> {
        let err = result.err()?;
        if self.attempts_left == 0 || !is_retriable(err) {
            return None;
        }
        let delay = self.next_delay;
        let next = Self {
            attempts_left: self.attempts_left - 1,
            next_delay: self.next_delay * 2,
        };
        Some(Box::pin(async move {
            tokio::time::sleep(delay).await;
            next
        }))
    }

    fn clone_request(&self, req: &Req) -> Option<Req> {
        Some(req.clone())
    }
}

fn is_retriable(e: &OpenAIError) -> bool {
    match e {
        OpenAIError::Transport(_) => true,
        OpenAIError::Api { status, .. } => *status >= 500 || *status == 429,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// TraceLayer — custom demo layer
//
// Shows what a trivial custom layer looks like: a `Layer` that wraps an inner
// `Service`, plus the `Service` impl that logs around the inner call. Uses
// `tracing::Instrument` so events inside the future inherit the span.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct TraceLayer {
    name: &'static str,
}

impl TraceLayer {
    pub fn new(name: &'static str) -> Self {
        Self { name }
    }
}

impl<S> tower::Layer<S> for TraceLayer {
    type Service = TraceService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        TraceService {
            inner,
            name: self.name,
        }
    }
}

#[derive(Clone)]
pub struct TraceService<S> {
    inner: S,
    name: &'static str,
}

pub trait TraceMeta {
    fn trace_model(&self) -> &str;
    fn trace_origin(&self) -> &str;
}

impl TraceMeta for ImageRequest {
    fn trace_model(&self) -> &str {
        &self.model
    }
    fn trace_origin(&self) -> &str {
        &self.origin
    }
}

impl<S, R> Service<R> for TraceService<S>
where
    S: Service<R>,
    S::Future: Send + 'static,
    S::Error: std::fmt::Display + Send + 'static,
    S::Response: Send + 'static,
    R: TraceMeta,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: R) -> Self::Future {
        let span = info_span!(
            "openai",
            endpoint = self.name,
            model = %req.trace_model(),
            origin = %req.trace_origin(),
        );
        let fut = self.inner.call(req);
        Box::pin(
            async move {
                let start = std::time::Instant::now();
                let result = fut.await;
                let elapsed_ms = start.elapsed().as_millis() as u64;
                match &result {
                    Ok(_) => info!(elapsed_ms, "ok"),
                    Err(e) => warn!(elapsed_ms, error = %e, "failed"),
                }
                result
            }
            .instrument(span),
        )
    }
}

// ---------------------------------------------------------------------------
// Public builders
// ---------------------------------------------------------------------------

pub fn build_image_service(cfg: &OpenaiConfig) -> ImageService {
    build_image_service_at(cfg, DEFAULT_BASE_URL)
}

// ---------------------------------------------------------------------------
// Global handle (mirrors the `OpenRouter::get()` pattern used elsewhere in
// ganbot3). ganbot5 will replace this with an `AppContext`.
// ---------------------------------------------------------------------------

// `BoxCloneService` is `Send` but not `Sync`, so we can't park it directly in
// a `OnceLock`. A `Mutex` bridges the gap; lock contention is irrelevant since
// we only hold the guard long enough to `.clone()` the service (a refcount bump).
static OPENAI_IMAGE: std::sync::OnceLock<std::sync::Mutex<ImageService>> =
    std::sync::OnceLock::new();

/// Install the process-wide image service. Call once at startup.
pub fn set_image_service(svc: ImageService) {
    if OPENAI_IMAGE.set(std::sync::Mutex::new(svc)).is_err() {
        tracing::warn!("openai image service already initialised; ignoring repeat");
    }
}

/// Retrieve the global image service. Returns `None` if `set_image_service`
/// has not been called (e.g. tests that didn't initialise it).
pub fn image_service() -> Option<ImageService> {
    OPENAI_IMAGE.get().map(|m| {
        m.lock()
            .expect("openai image service mutex poisoned")
            .clone()
    })
}

/// Same as [`build_image_service`] but targets a configurable base URL.
/// Intended for tests that point at a local `wiremock` server.
pub fn build_image_service_at(cfg: &OpenaiConfig, base_url: &str) -> ImageService {
    let base = OpenAIHttp::new(&cfg.token, base_url);

    let svc = ServiceBuilder::new()
        .layer(TraceLayer::new("openai.image"))
        .map_err(box_to_openai_error)
        .layer(TimeoutLayer::new(Duration::from_secs(480)))
        .layer(RetryLayer::new(OpenAIRetryPolicy::new(3)))
        .service(base);

    BoxCloneService::new(svc)
}

fn box_to_openai_error(e: BoxError) -> OpenAIError {
    if e.is::<tower::timeout::error::Elapsed>() {
        return OpenAIError::Timeout;
    }
    match e.downcast::<OpenAIError>() {
        Ok(boxed) => *boxed,
        Err(other) => OpenAIError::BadResponse(format!("unexpected error: {}", other)),
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;
