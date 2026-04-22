//! Tests for the OpenAI Tower module.
//!
//! Three scales:
//!   1. Layer isolation — `service_fn` probes with one layer under test.
//!   2. HTTP replay — `wiremock` server returning canned bodies. Fixtures live
//!      in `tests/fixtures/openai/` and are regenerated (costs real money) by
//!      the `#[ignore]`d test at the bottom. Absent fixtures are replaced with
//!      a synthetic minimal response so `cargo test` works out-of-the-box.
//!   3. Live API regeneration — `#[ignore]`d; run manually to refresh fixtures.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine as _;
use image::{DynamicImage, ImageFormat, RgbImage};
use serde_json::json;
use tower::retry::{Policy, RetryLayer};
use tower::timeout::TimeoutLayer;
use tower::{ServiceBuilder, ServiceExt, service_fn};
use wiremock::matchers::{body_partial_json, header, method, path as path_matcher};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{
    DEFAULT_BASE_URL, ImageRequest, ImageResponse, OpenAIError, OpenAIRetryPolicy, TraceLayer,
    build_image_service_at,
};
use crate::config::global::OpenaiConfig;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fake_req() -> ImageRequest {
    ImageRequest {
        origin: "test".into(),
        model: "gpt-image-2".into(),
        prompt: "a cat".into(),
        size: Some("1024x1024".into()),
        quality: Some("low".into()),
        input_image: None,
    }
}

fn tiny_red_rgb() -> RgbImage {
    RgbImage::from_pixel(1, 1, image::Rgb([255, 0, 0]))
}

fn cfg_test() -> OpenaiConfig {
    OpenaiConfig {
        token: "test-token".into(),
    }
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("openai")
}

/// Return the bytes of a fixture file, or — if the fixture is missing — build
/// a synthetic minimal `images/generations` response body with a 1x1 red PNG
/// embedded as `b64_json`. Lets the test suite pass before the user has run
/// the `#[ignore]` regeneration test.
fn fixture_or_synthetic(name: &str) -> Vec<u8> {
    let path = fixtures_dir().join(name);
    if path.exists() {
        std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {path:?}: {e}"))
    } else {
        synthetic_response_body()
    }
}

fn synthetic_response_body() -> Vec<u8> {
    let mut png = Vec::new();
    DynamicImage::ImageRgb8(tiny_red_rgb())
        .write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
        .unwrap();
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    json!({
        "created": 0,
        "data": [{ "b64_json": b64 }]
    })
    .to_string()
    .into_bytes()
}

// ---------------------------------------------------------------------------
// 1. Layer isolation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retry_policy_retries_5xx_up_to_max() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();

    let probe = service_fn(move |_req: ImageRequest| {
        let hits = hits_clone.clone();
        async move {
            hits.fetch_add(1, Ordering::SeqCst);
            Err::<ImageResponse, _>(OpenAIError::Api {
                status: 503,
                message: "busy".into(),
            })
        }
    });

    let svc = ServiceBuilder::new()
        .layer(RetryLayer::new(OpenAIRetryPolicy::with_base_delay(
            3,
            Duration::ZERO,
        )))
        .service(probe);

    let err = svc.oneshot(fake_req()).await.unwrap_err();
    assert!(matches!(err, OpenAIError::Api { status: 503, .. }));
    assert_eq!(hits.load(Ordering::SeqCst), 4, "1 initial + 3 retries");
}

#[tokio::test]
async fn retry_policy_does_not_retry_4xx() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();

    let probe = service_fn(move |_req: ImageRequest| {
        let hits = hits_clone.clone();
        async move {
            hits.fetch_add(1, Ordering::SeqCst);
            Err::<ImageResponse, _>(OpenAIError::Api {
                status: 400,
                message: "bad request".into(),
            })
        }
    });

    let svc = ServiceBuilder::new()
        .layer(RetryLayer::new(OpenAIRetryPolicy::with_base_delay(
            3,
            Duration::ZERO,
        )))
        .service(probe);

    let err = svc.oneshot(fake_req()).await.unwrap_err();
    assert!(matches!(err, OpenAIError::Api { status: 400, .. }));
    assert_eq!(hits.load(Ordering::SeqCst), 1, "4xx should not retry");
}

#[tokio::test]
async fn retry_policy_retries_429() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();

    let probe = service_fn(move |_req: ImageRequest| {
        let hits = hits_clone.clone();
        async move {
            hits.fetch_add(1, Ordering::SeqCst);
            Err::<ImageResponse, _>(OpenAIError::Api {
                status: 429,
                message: "rate".into(),
            })
        }
    });

    let svc = ServiceBuilder::new()
        .layer(RetryLayer::new(OpenAIRetryPolicy::new(2)))
        .service(probe);

    let _ = svc.oneshot(fake_req()).await;
    assert_eq!(hits.load(Ordering::SeqCst), 3, "1 initial + 2 retries");
}

#[tokio::test]
async fn retry_policy_succeeds_after_transient_failures() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();

    let probe = service_fn(move |_req: ImageRequest| {
        let hits = hits_clone.clone();
        async move {
            let n = hits.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(OpenAIError::Api {
                    status: 503,
                    message: "busy".into(),
                })
            } else {
                Ok(ImageResponse {
                    image: tiny_red_rgb(),
                })
            }
        }
    });

    let svc = ServiceBuilder::new()
        .layer(RetryLayer::new(OpenAIRetryPolicy::with_base_delay(
            3,
            Duration::ZERO,
        )))
        .service(probe);

    let resp = svc.oneshot(fake_req()).await.unwrap();
    assert_eq!(resp.image.dimensions(), (1, 1));
    assert_eq!(hits.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn timeout_layer_fires_on_slow_service() {
    // Use a real tokio runtime (not paused) because TimeoutLayer is built on
    // a real-time timer that `start_paused` plus `Timeout` together don't
    // advance correctly without `tokio::time::advance` plumbing.
    let probe = service_fn(|_req: ImageRequest| async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok::<ImageResponse, OpenAIError>(ImageResponse {
            image: tiny_red_rgb(),
        })
    });

    let svc = ServiceBuilder::new()
        .map_err(super::box_to_openai_error)
        .layer(TimeoutLayer::new(Duration::from_millis(20)))
        .service(probe);

    let err = svc.oneshot(fake_req()).await.unwrap_err();
    assert!(matches!(err, OpenAIError::Timeout), "got {err:?}");
}

#[tokio::test]
async fn retry_policy_clones_request_unchanged() {
    // Prove clone_request returns the same request contents.
    let policy = OpenAIRetryPolicy::new(1);
    let req = fake_req();
    let cloned: ImageRequest =
        <OpenAIRetryPolicy as Policy<ImageRequest, ImageResponse, OpenAIError>>::clone_request(
            &policy, &req,
        )
        .expect("policy returns Some");
    assert_eq!(req.model, cloned.model);
    assert_eq!(req.prompt, cloned.prompt);
    assert_eq!(req.size, cloned.size);
}

// ---------------------------------------------------------------------------
// 2. Wiremock HTTP replay (synthetic bodies by default; golden data if present)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn generations_sends_size_and_decodes_response() {
    let body = fixture_or_synthetic("generate_1024x1024_response.json");
    let mock = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_matcher("/v1/images/generations"))
        .and(body_partial_json(
            json!({ "size": "1024x1024", "model": "gpt-image-2" }),
        ))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
        .expect(1)
        .mount(&mock)
        .await;

    let svc = build_image_service_at(&cfg_test(), &mock.uri());
    let resp = svc.oneshot(fake_req()).await.expect("happy path");
    // Image decodes; precise dimensions depend on the fixture, so just assert
    // something non-empty came back.
    assert!(resp.image.width() > 0 && resp.image.height() > 0);
}

#[tokio::test]
async fn generations_non_success_surfaces_api_error_message() {
    let mock = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_matcher("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": { "message": "invalid size", "type": "user_error", "code": "bad_size" }
        })))
        .mount(&mock)
        .await;

    let svc = build_image_service_at(&cfg_test(), &mock.uri());
    let err = svc.oneshot(fake_req()).await.unwrap_err();
    match err {
        OpenAIError::Api { status, message } => {
            assert_eq!(status, 400);
            assert!(message.contains("invalid size"), "got: {message}");
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[tokio::test]
async fn generations_retries_then_succeeds_on_transient_5xx() {
    let body = fixture_or_synthetic("generate_1024x1024_response.json");
    let mock = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_matcher("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path_matcher("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
        .mount(&mock)
        .await;

    let svc = build_image_service_at(&cfg_test(), &mock.uri());
    let resp = svc.oneshot(fake_req()).await.expect("eventual success");
    assert!(resp.image.width() > 0);
}

#[tokio::test]
async fn edits_uses_multipart() {
    let body = synthetic_response_body(); // edits fixture isn't in scope initially
    let mock = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_matcher("/v1/images/edits"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
        .expect(1)
        .mount(&mock)
        .await;

    let svc = build_image_service_at(&cfg_test(), &mock.uri());
    let mut req = fake_req();
    req.input_image = Some(Arc::new(tiny_red_rgb()));
    let resp = svc.oneshot(req).await.expect("edit");
    assert_eq!(resp.image.dimensions(), (1, 1));
}

// Compile-time sanity check: the custom `TraceLayer` wraps a `service_fn`
// cleanly. If the bounds on `TraceLayer`/`TraceService` break, this test
// stops compiling.
#[tokio::test]
async fn trace_layer_composes() {
    let probe = service_fn(|_req: ImageRequest| async {
        Ok::<ImageResponse, OpenAIError>(ImageResponse {
            image: tiny_red_rgb(),
        })
    });
    let svc = ServiceBuilder::new()
        .layer(TraceLayer::new("test"))
        .service(probe);
    let _ = svc.oneshot(fake_req()).await.unwrap();
}

// ---------------------------------------------------------------------------
// 3. Live API regeneration
//
// Run manually:
//   cargo test -p ganbot --bin ganbot regenerate_openai_fixtures -- --ignored
// Reads the token from `config-local.toml`, hits the real API once, writes:
//   tests/fixtures/openai/generate_1024x1024_response.json   (raw API body)
//   tests/fixtures/openai/generate_1024x1024_image.png       (decoded image)
// The other tests in this file use those fixtures if present, otherwise fall
// back to a synthetic minimal response.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "hits the live OpenAI API; run with --ignored"]
async fn regenerate_openai_fixtures() {
    let cfg = load_openai_config_from_local();
    assert!(
        !cfg.token.is_empty(),
        "openai.token missing from config-local.toml"
    );

    // Use reqwest directly (not our Tower service) so we capture the raw
    // server response bytes exactly as sent — golden data, not round-tripped.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1/images/generations", DEFAULT_BASE_URL))
        .bearer_auth(&cfg.token)
        .json(&json!({
            "model": "gpt-image-2",
            "prompt": "a single red pixel on pure white background, minimal, flat",
            "size": "1024x1024",
            "quality": "low",
            "n": 1,
        }))
        .send()
        .await
        .expect("send to OpenAI");

    let status = resp.status();
    let bytes = resp.bytes().await.expect("read body");
    assert!(
        status.is_success(),
        "API returned {status}: {}",
        String::from_utf8_lossy(&bytes)
    );

    let dir = fixtures_dir();
    std::fs::create_dir_all(&dir).expect("mkdir fixtures");

    std::fs::write(dir.join("generate_1024x1024_response.json"), &bytes)
        .expect("write response fixture");

    // Also pull the image out for byte-level inspection later.
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("parse JSON");
    let b64 = parsed["data"][0]["b64_json"]
        .as_str()
        .expect("b64_json present");
    let img_bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .expect("decode base64");
    std::fs::write(dir.join("generate_1024x1024_image.png"), &img_bytes)
        .expect("write image fixture");

    println!("wrote fixtures to {}", dir.display());
}

/// Live-API parameter probe.
///
/// The v1/images/generations API for `gpt-image-*` is fussier than the docs
/// imply — `response_format`, for example, is rejected with HTTP 400. This
/// test walks a matrix of parameter combinations against the live endpoint
/// and reports which succeed. Keep each cell cheap (`quality=low`,
/// `size=1024x1024`) so a full run costs roughly one image per combination.
///
/// Run manually:
///   cargo test -p ganbot --bin ganbot probe_openai_parameters -- --ignored --nocapture
#[tokio::test]
#[ignore = "hits the live OpenAI API many times; run with --ignored --nocapture"]
async fn probe_openai_parameters() {
    let cfg = load_openai_config_from_local();
    assert!(!cfg.token.is_empty(), "openai.token missing");

    let client = reqwest::Client::new();
    let base_body = json!({
        "model": "gpt-image-2",
        "prompt": "a single red pixel on white",
        "size": "1024x1024",
        "quality": "low",
        "n": 1,
    });

    // Each probe is (label, body-mutator). Empty mutation = baseline.
    type Mut = Box<dyn Fn(&mut serde_json::Value)>;
    let probes: Vec<(&str, Mut)> = vec![
        ("baseline", Box::new(|_| {})),
        (
            "with response_format=b64_json",
            Box::new(|b| {
                b["response_format"] = json!("b64_json");
            }),
        ),
        (
            "with response_format=url",
            Box::new(|b| {
                b["response_format"] = json!("url");
            }),
        ),
        (
            "with output_format=png",
            Box::new(|b| {
                b["output_format"] = json!("png");
            }),
        ),
        (
            "size=1536x1024",
            Box::new(|b| {
                b["size"] = json!("1536x1024");
            }),
        ),
        (
            "size=2048x2048",
            Box::new(|b| {
                b["size"] = json!("2048x2048");
            }),
        ),
        (
            "size=3840x2160 quality=high",
            Box::new(|b| {
                b["size"] = json!("3840x2160");
                b["quality"] = json!("high");
            }),
        ),
        (
            "quality=medium",
            Box::new(|b| {
                b["quality"] = json!("medium");
            }),
        ),
        (
            "model=gpt-image-1",
            Box::new(|b| {
                b["model"] = json!("gpt-image-1");
            }),
        ),
        (
            "model=gpt-image-1-mini",
            Box::new(|b| {
                b["model"] = json!("gpt-image-1-mini");
            }),
        ),
    ];

    println!("\n=== OpenAI parameter probe ===");
    for (label, mutate) in probes.iter() {
        let mut body = base_body.clone();
        mutate(&mut body);
        let resp = client
            .post(format!("{}/v1/images/generations", DEFAULT_BASE_URL))
            .bearer_auth(&cfg.token)
            .json(&body)
            .send()
            .await
            .expect("send");
        let status = resp.status();
        let bytes = resp.bytes().await.expect("bytes");
        let short: String = if status.is_success() {
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
            let has_b64 = json["data"][0]["b64_json"].is_string();
            let has_url = json["data"][0]["url"].is_string();
            format!("OK (b64_json={has_b64}, url={has_url})")
        } else {
            // Pull the message out succinctly.
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
            json["error"]["message"]
                .as_str()
                .unwrap_or_else(|| std::str::from_utf8(&bytes).unwrap_or("<binary>"))
                .chars()
                .take(160)
                .collect()
        };
        println!("  {:<40}  {}  {}", label, status, short);
    }
}

fn load_openai_config_from_local() -> OpenaiConfig {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config-local.toml");
    let content = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    #[derive(serde::Deserialize)]
    struct Partial {
        openai: OpenaiConfig,
    }
    let partial: Partial = toml::from_str(&content).expect("parse config-local.toml [openai]");
    partial.openai
}
