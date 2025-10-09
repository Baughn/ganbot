use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use kameo::{Actor, actor::ActorRef, registry::ACTOR_REGISTRY};
use redis::aio::ConnectionManager;
use sha2::{Digest, Sha256};
use socket2::{Domain, Socket, Type};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower::ServiceBuilder;
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};
use tracing::{debug, error, info, trace};

use crate::{
    actions::imagen::{GenerateImagesRequest, submit_generation},
    config::{
        global::{ModelGalleryConfig, WebServerConfig},
        models::{Backend, Model},
    },
    messages::imagen::{Generate, References},
    persistence::images::upload_image_with_generation,
    supervisor::{GetModelsConfig, Supervisor},
};

/// Web server actor
pub struct WebServer {
    config: WebServerConfig,
    gallery_config: ModelGalleryConfig,
    redis: ConnectionManager,
    shutdown_token: CancellationToken,
    pregen_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Shared application state
#[derive(Clone)]
struct AppState {
    gallery_config: ModelGalleryConfig,
    redis: ConnectionManager,
}

/// Query parameters for gallery filtering
#[derive(serde::Deserialize)]
struct GalleryQuery {
    tag: Option<String>,
}

impl WebServer {
    pub fn new(
        config: WebServerConfig,
        gallery_config: ModelGalleryConfig,
        redis: ConnectionManager,
        shutdown_token: CancellationToken,
    ) -> Self {
        Self {
            config,
            gallery_config,
            redis,
            shutdown_token,
            pregen_handle: None,
        }
    }

    pub async fn run(self, listener: TcpListener) -> Result<()> {
        let local_addr = listener
            .local_addr()
            .context("Failed to get local address")?;

        info!("Web server listening on {}", local_addr);

        let state = AppState {
            gallery_config: self.gallery_config.clone(),
            redis: self.redis.clone(),
        };

        // Configure static file service with cache headers
        let static_service = ServiceBuilder::new()
            .layer(SetResponseHeaderLayer::if_not_present(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            ))
            .service(ServeDir::new("static"));

        let app = Router::new()
            .route("/", get(index_handler))
            .route("/help/{*path}", get(help_handler))
            .route("/gallery/models", get(model_gallery_handler))
            .nest_service("/static", static_service)
            .with_state(state);

        let shutdown_token = self.shutdown_token.clone();
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_token.cancelled().await;
                info!("Web server shutdown signal received");
            })
            .await
            .context("Web server error")?;

        info!("Web server stopped");
        Ok(())
    }
}

impl Actor for WebServer {
    type Args = (WebServerConfig, ModelGalleryConfig, ConnectionManager);
    type Error = anyhow::Error;

    async fn on_start(args: Self::Args, actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        let (config, gallery_config, redis) = args;

        // Bind the listener immediately to fail fast if port is unavailable
        let addr = format!("{}:{}", config.bind_address, config.port);
        let socket_addr: SocketAddr = addr.parse().context("Failed to parse bind address")?;

        info!("Starting web server on {}", socket_addr);

        // Create socket with SO_REUSEADDR
        let domain = if socket_addr.is_ipv4() {
            Domain::IPV4
        } else {
            Domain::IPV6
        };
        let socket = Socket::new(domain, Type::STREAM, None).context("Failed to create socket")?;
        socket
            .set_reuse_address(true)
            .context("Failed to set SO_REUSEADDR")?;
        socket
            .bind(&socket_addr.into())
            .context("Failed to bind socket")?;
        socket.listen(1024).context("Failed to listen on socket")?;
        socket
            .set_nonblocking(true)
            .context("Failed to set socket to non-blocking")?;

        let listener = TcpListener::from_std(socket.into())
            .context("Failed to create TcpListener from socket")?;

        // Create cancellation token for graceful shutdown
        let shutdown_token = CancellationToken::new();

        // Clone values for the spawned task
        let server_config = config.clone();
        let server_gallery_config = gallery_config.clone();
        let server_redis = redis.clone();
        let server_shutdown_token = shutdown_token.clone();

        // Clone actor ref to stop it on error
        let actor_ref_clone = actor_ref.clone();

        // Spawn the server in a separate task and monitor it
        tokio::spawn(async move {
            let server = WebServer::new(
                server_config,
                server_gallery_config,
                server_redis,
                server_shutdown_token,
            );
            match server.run(listener).await {
                Ok(()) => {
                    info!("Web server exited cleanly");
                }
                Err(e) => {
                    error!("Web server failed: {:#}", e);
                    let _ = actor_ref_clone.stop_gracefully().await;
                }
            }
        });

        // Spawn background task to pre-generate missing gallery images
        let pregen_gallery_config = gallery_config.clone();
        let pregen_redis = redis.clone();
        let pregen_handle = tokio::spawn(async move {
            // Get image host config
            let image_host_config = Supervisor::image_host().await;
            pre_generate_gallery_task(
                pregen_gallery_config,
                pregen_redis,
                image_host_config.base_url,
            )
            .await;
        });

        // Return an instance for the actor with shutdown token and pregen handle
        Ok(Self {
            config,
            gallery_config,
            redis,
            shutdown_token,
            pregen_handle: Some(pregen_handle),
        })
    }

    async fn on_stop(
        &mut self,
        _actor_ref: kameo::actor::WeakActorRef<Self>,
        reason: kameo::error::ActorStopReason,
    ) -> Result<(), Self::Error> {
        info!("WebServer stopping: {:?}", reason);

        // Cancel the shutdown token to gracefully stop the axum server
        self.shutdown_token.cancel();

        // Abort the pre-generation task if it's still running
        if let Some(handle) = self.pregen_handle.take() {
            handle.abort();
            info!("Aborted gallery pre-generation task");
        }

        info!("WebServer cleanup complete");
        Ok(())
    }
}

/// Handler for the home page
async fn index_handler() -> impl IntoResponse {
    let css_url = static_url("style.css").await;

    Html(format!(
        r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Ganbot</title>
    <link rel="stylesheet" href="{}">
</head>
<body>
    <nav class="sidebar">
        <h1>Ganbot</h1>
        <ul>
            <li><a href="/">Home</a></li>
            <li><a href="/gallery/models">Model Gallery</a></li>
            <li><a href="/help/commands">Help</a></li>
        </ul>
    </nav>
    <main class="content">
        <h1>Welcome to Ganbot</h1>
        <p>An IRC bot with AI capabilities and image generation.</p>
        <ul>
            <li><a href="/gallery/models">Browse Model Gallery</a></li>
            <li><a href="/help/commands">View Help</a></li>
        </ul>
    </main>
</body>
</html>
    "#,
        css_url
    ))
}

/// Handler for help pages
async fn help_handler(Path(path): Path<String>) -> Response {
    // Read markdown file from help/ directory
    let file_path = format!("help/{}.md", path);

    match tokio::fs::read_to_string(&file_path).await {
        Ok(markdown_content) => {
            let css_url = static_url("style.css").await;

            // Convert markdown to HTML
            let parser = pulldown_cmark::Parser::new(&markdown_content);
            let mut html_output = String::new();
            pulldown_cmark::html::push_html(&mut html_output, parser);

            // Wrap in HTML template
            let full_html = format!(
                r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Help - {}</title>
    <link rel="stylesheet" href="{}">
</head>
<body>
    <nav class="sidebar">
        <h1>Ganbot</h1>
        <ul>
            <li><a href="/">Home</a></li>
            <li><a href="/gallery/models">Model Gallery</a></li>
            <li><a href="/help/commands">Help</a></li>
        </ul>
    </nav>
    <main class="content">
        {}
    </main>
</body>
</html>
            "#,
                path, css_url, html_output
            );

            Html(full_html).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "Help page not found").into_response(),
    }
}

/// Handler for model gallery page
async fn model_gallery_handler(
    Query(query): Query<GalleryQuery>,
    State(mut state): State<AppState>,
) -> impl IntoResponse {
    // Get models from supervisor
    let supervisor = match ACTOR_REGISTRY
        .lock()
        .unwrap()
        .get::<Supervisor, str>("supervisor")
    {
        Ok(Some(sup)) => sup,
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to access models configuration",
            )
                .into_response();
        }
    };

    let models_config = match supervisor.ask(GetModelsConfig).await {
        Ok(reply) => reply.0,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch models").into_response();
        }
    };

    let image_host_config = Supervisor::image_host().await;

    // Filter to ComfyUI models
    let mut models: Vec<_> = models_config
        .models
        .iter()
        .filter(|(_, model)| matches!(model.backend, Backend::ComfyUI { .. }))
        .collect();

    models.sort_by_key(|(name, _)| *name);

    // Build reverse alias mapping: model_name -> Vec<alias>
    let mut model_aliases: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for (alias, model_name) in &models_config.aliases {
        model_aliases
            .entry(model_name.clone())
            .or_default()
            .push(alias.clone());
    }
    // Sort aliases for consistent display
    for aliases in model_aliases.values_mut() {
        aliases.sort();
    }

    if models.is_empty() || state.gallery_config.prompts.is_empty() {
        let css_url = static_url("style.css").await;
        return Html(format!(
            r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Model Gallery</title>
    <link rel="stylesheet" href="{}">
</head>
<body>
    <nav class="sidebar">
        <h1>Ganbot</h1>
        <ul>
            <li><a href="/">Home</a></li>
            <li><a href="/gallery/models">Model Gallery</a></li>
            <li><a href="/help/commands">Help</a></li>
        </ul>
    </nav>
    <main class="content">
        <h1>Model Gallery</h1>
        <p>No models or prompts configured for the gallery.</p>
    </main>
</body>
</html>
            "#,
            css_url
        ))
        .into_response();
    }

    // Collect all unique tags
    let mut all_tags = std::collections::HashSet::new();
    for (_, model) in &models {
        for tag in &model.tags {
            all_tags.insert(tag.clone());
        }
    }

    // Sort tags: "recommended" first, then alphabetically
    let mut tags: Vec<String> = all_tags.into_iter().collect();
    tags.sort_by(|a, b| {
        if a == "recommended" {
            std::cmp::Ordering::Less
        } else if b == "recommended" {
            std::cmp::Ordering::Greater
        } else {
            a.cmp(b)
        }
    });

    // Capitalize first letter for display
    fn capitalize(s: &str) -> String {
        let mut chars = s.chars();
        match chars.next() {
            None => String::new(),
            Some(first) => first.to_uppercase().chain(chars).collect(),
        }
    }

    // Build tag filter buttons HTML
    // Determine which tag should be active:
    // 1. Use query parameter if provided and valid
    // 2. Otherwise, default to "recommended" if it exists
    // 3. Fall back to "all"
    let default_tag = if let Some(ref query_tag) = query.tag {
        // Validate that the tag exists in our tag list or is "all"
        if query_tag == "all" || tags.contains(query_tag) {
            query_tag.as_str()
        } else {
            // Invalid tag in query, fall back to default
            if tags.contains(&"recommended".to_string()) {
                "recommended"
            } else {
                "all"
            }
        }
    } else {
        // No query parameter, use default
        if tags.contains(&"recommended".to_string()) {
            "recommended"
        } else {
            "all"
        }
    };

    let all_active = if default_tag == "all" { " active" } else { "" };
    let mut tag_buttons = format!(
        r#"<button class="tag-filter{}" data-tag="all">All</button>"#,
        all_active
    );

    for tag in &tags {
        let is_active = if tag == default_tag { " active" } else { "" };
        tag_buttons.push_str(&format!(
            r#"<button class="tag-filter{}" data-tag="{}">{}</button>"#,
            is_active,
            tag,
            capitalize(tag)
        ));
    }

    // Build table header with prompts
    let mut table_header = String::from("<tr><th>Model</th>");
    for prompt in &state.gallery_config.prompts {
        table_header.push_str(&format!("<th>{}</th>", prompt));
    }
    table_header.push_str("</tr>");

    // Build table rows
    let mut table_rows = String::new();
    for (model_name, model) in &models {
        let tags_attr = model.tags.join(",");
        table_rows.push_str(&format!(r#"<tr data-tags="{}">"#, tags_attr));

        // Build model label with aliases
        let mut label_html = format!("<td class=\"model-label\">{}", model_name);
        if let Some(aliases) = model_aliases.get(*model_name)
            && !aliases.is_empty()
        {
            label_html.push_str("<div class=\"model-aliases\">");
            label_html.push_str(&aliases.join(", "));
            label_html.push_str("</div>");
        }
        label_html.push_str("</td>");
        table_rows.push_str(&label_html);

        for prompt in &state.gallery_config.prompts {
            // Build a Generate struct and apply model defaults
            let mut generate = build_gallery_generate(prompt, model_name);
            crate::actions::imagen::apply_model_defaults(&mut generate, model);

            let image_urls = get_gallery_image(
                &mut state.redis,
                model_name,
                model,
                &generate,
                &image_host_config.base_url,
            )
            .await;

            // Generate a deterministic but varied cycle offset (0-39, representing 0-9.75s in 0.25s increments)
            // Based on hash of model+prompt to ensure different cells don't sync
            let offset_seed = format!("{}{}", model_name, prompt);
            let offset = offset_seed.bytes().map(|b| b as u32).sum::<u32>() % 40;

            // Serialize URLs as JSON for data attribute
            let urls_json = serde_json::to_string(&image_urls).unwrap_or_else(|_| "[]".to_string());
            let urls_json_escaped = urls_json.replace('"', "&quot;");

            // Build model config for modal display
            let model_config = build_model_config_json(model);
            let model_config_escaped = model_config.replace('"', "&quot;");

            // Build HTML with 4 images, first visible, rest hidden
            let mut cell_html = format!(
                r#"<td><div class="gallery-cell" data-urls="{}" data-cycle-offset="{}" data-model-config="{}">"#,
                urls_json_escaped, offset, model_config_escaped
            );

            for (index, url) in image_urls.iter().enumerate() {
                let (opacity, pointer_events) = if index == 0 {
                    ("1", "auto")
                } else {
                    ("0", "none")
                };
                cell_html.push_str(&format!(
                    r#"<a href="{}" class="gallery-link" style="opacity: {}; pointer-events: {};"><img src="{}" alt="{} - {}" class="gallery-img" data-index="{}" /></a>"#,
                    url, opacity, pointer_events, url, model_name, prompt, index
                ));
            }

            cell_html.push_str("</div></td>");
            table_rows.push_str(&cell_html);
        }

        table_rows.push_str("</tr>");
    }

    // Render the full HTML
    let css_url = static_url("style.css").await;
    let js_url = static_url("gallery.js").await;

    let html = format!(
        r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Model Gallery</title>
    <link rel="stylesheet" href="{}">
    <script src="{}" defer></script>
</head>
<body>
    <nav class="sidebar">
        <h1>Ganbot</h1>
        <ul>
            <li><a href="/">Home</a></li>
            <li><a href="/gallery/models">Model Gallery</a></li>
            <li><a href="/help/commands">Help</a></li>
        </ul>
    </nav>
    <main class="content">
        <h1>Model Gallery</h1>
        <div class="gallery-filters">
            {}
        </div>
        <div class="gallery-comparison">
            <table class="comparison-grid">
                <thead>
                    {}
                </thead>
                <tbody>
                    {}
                </tbody>
            </table>
        </div>
    </main>
</body>
</html>
        "#,
        css_url, js_url, tag_buttons, table_header, table_rows
    );

    Html(html).into_response()
}

/// Gallery cache helper functions
const PLACEHOLDER_URL: &str = "/static/placeholder.svg";

/// Calculate SHA256 hash of a file for cache busting
async fn calculate_file_hash(path: &str) -> Result<String> {
    let content = tokio::fs::read(path)
        .await
        .context(format!("Failed to read file: {}", path))?;

    let mut hasher = Sha256::new();
    hasher.update(&content);
    let hash = hasher.finalize();

    // Return first 8 characters of hex hash
    Ok(format!("{:x}", hash)[..8].to_string())
}

/// Generate a versioned static file URL with content hash for cache busting
async fn static_url(filename: &str) -> String {
    let path = format!("static/{}", filename);
    match calculate_file_hash(&path).await {
        Ok(hash) => format!("/static/{}?v={}", filename, hash),
        Err(e) => {
            error!("Failed to calculate hash for {}: {:#}", path, e);
            // Fallback to unversioned URL if hash calculation fails
            format!("/static/{}", filename)
        }
    }
}

/// Build a Generate struct for gallery images with canonical settings
fn build_gallery_generate(prompt: &str, model_name: &str) -> Generate {
    Generate {
        raw_prompt: prompt.to_string(),
        prompt: prompt.to_string(),
        negative_prompt: Some("nsfw".to_string()),
        num_images: Some(4),
        aspect: None,
        width: None,
        height: None,
        model: Some(model_name.to_string()),
        seed: None,
        steps: None,
        references: References {
            img2img: None,
            img2img_strength: None,
            context: Vec::new(),
        },
        alias: None,
    }
}

/// Build a JSON string of model configuration for display in the modal
fn build_model_config_json(model: &Model) -> String {
    let config_obj = match &model.backend {
        Backend::ComfyUI {
            checkpoint,
            cfg,
            sampler,
            scheduler,
            steps,
            resolution,
            two_stage,
            upscale_factor,
            stage2_denoise,
            stage2_sampler,
            stage2_scheduler,
            ..
        } => {
            let checkpoint_name = match checkpoint {
                crate::config::models::Checkpoint::Combined(name) => name.clone(),
                crate::config::models::Checkpoint::Split { unet, .. } => unet.clone(),
            };

            serde_json::json!({
                "name": model.name,
                "description": model.description,
                "checkpoint": checkpoint_name,
                "sampler": sampler,
                "scheduler": scheduler,
                "steps": steps,
                "resolution": format!("{}x{}", resolution.0, resolution.1),
                "cfg": cfg,
                "two_stage": two_stage,
                "upscale_factor": upscale_factor,
                "stage2_denoise": stage2_denoise,
                "stage2_sampler": stage2_sampler,
                "stage2_scheduler": stage2_scheduler,
            })
        }
        Backend::NanoBanana => {
            serde_json::json!({
                "name": model.name,
                "description": model.description,
                "backend": "NanoBanana (Gemini 2.5-flash-image-preview)",
            })
        }
    };

    config_obj.to_string()
}

/// Generate a stable cache key for a model+prompt combination
fn gallery_cache_key(model_name: &str, model: &Model, generate: &Generate) -> String {
    let mut hasher = Sha256::new();

    // Include the model configuration (backend and prompt_defaults)
    // These are the fields that affect image generation
    let model_config_json = serde_json::json!({
        "backend": model.backend,
        "prompt_defaults": model.prompt_defaults,
    });
    hasher.update(model_config_json.to_string().as_bytes());

    // Serialize the entire Generate struct to ensure all parameters are included
    let generate_json = serde_json::to_string(generate).unwrap_or_default();
    hasher.update(generate_json.as_bytes());

    let hash = hasher.finalize();
    format!("gallery:cache:{}:{:x}", model_name, hash)
}

/// Get cached gallery image URLs, or return placeholders
async fn get_gallery_image(
    redis: &mut ConnectionManager,
    model_name: &str,
    model: &Model,
    generate: &Generate,
    image_host_base_url: &str,
) -> Vec<String> {
    let cache_key = gallery_cache_key(model_name, model, generate);

    match redis::cmd("GET")
        .arg(&cache_key)
        .query_async::<String>(redis)
        .await
    {
        Ok(json_str) => match serde_json::from_str::<Vec<String>>(&json_str) {
            Ok(uuids) => {
                trace!("Gallery cache hit: {} -> {:?}", cache_key, uuids);
                uuids
                    .into_iter()
                    .map(|uuid| {
                        format!("{}/{}.jpg", image_host_base_url.trim_end_matches('/'), uuid)
                    })
                    .collect()
            }
            Err(_) => {
                debug!("Gallery cache invalid JSON: {}", cache_key);
                vec![PLACEHOLDER_URL.to_string(); 4]
            }
        },
        Err(_) => {
            trace!("Gallery cache miss: {}", cache_key);
            vec![PLACEHOLDER_URL.to_string(); 4]
        }
    }
}

/// Store gallery image UUIDs in cache
async fn set_gallery_image(
    redis: &mut ConnectionManager,
    model_name: &str,
    model: &Model,
    generate: &Generate,
    image_uuids: &[String],
) -> Result<()> {
    let cache_key = gallery_cache_key(model_name, model, generate);

    let uuids_json =
        serde_json::to_string(image_uuids).context("Failed to serialize image UUIDs")?;

    redis::cmd("SET")
        .arg(&cache_key)
        .arg(uuids_json)
        .query_async::<()>(redis)
        .await
        .context("Failed to set gallery cache")?;

    info!("Cached gallery images: {} -> {:?}", cache_key, image_uuids);
    Ok(())
}

/// Background task to pre-generate missing gallery images
async fn pre_generate_gallery_task(
    gallery_config: ModelGalleryConfig,
    mut redis: ConnectionManager,
    image_host_base_url: String,
) {
    info!("Starting gallery pre-generation task");

    // Get models configuration from supervisor
    let supervisor = match ACTOR_REGISTRY
        .lock()
        .unwrap()
        .get::<Supervisor, str>("supervisor")
    {
        Ok(Some(sup)) => sup,
        _ => {
            error!("Failed to find supervisor for gallery pre-generation");
            return;
        }
    };

    let models_config = match supervisor.ask(GetModelsConfig).await {
        Ok(reply) => reply.0,
        Err(e) => {
            error!("Failed to get models config for gallery: {:#}", e);
            return;
        }
    };

    // Filter to ComfyUI models only (for consistency)
    let models: Vec<_> = models_config
        .models
        .iter()
        .filter(|(_, model)| matches!(model.backend, Backend::ComfyUI { .. }))
        .collect();

    if models.is_empty() {
        info!("No ComfyUI models found for gallery");
        return;
    }

    if gallery_config.prompts.is_empty() {
        info!("No gallery prompts configured");
        return;
    }

    // Collect all missing (model, prompt) combinations
    let mut missing = Vec::new();
    for (model_name, model) in &models {
        for prompt in &gallery_config.prompts {
            // Build Generate struct and apply model defaults
            let mut generate = build_gallery_generate(prompt, model_name);
            crate::actions::imagen::apply_model_defaults(&mut generate, model);

            let cache_key = gallery_cache_key(model_name, model, &generate);
            match redis::cmd("EXISTS")
                .arg(&cache_key)
                .query_async::<u8>(&mut redis)
                .await
            {
                Ok(0) => {
                    missing.push((model_name.to_string(), generate));
                }
                Ok(_) => {
                    debug!("Gallery image already cached: {}", cache_key);
                }
                Err(e) => {
                    error!("Failed to check cache for {}: {:#}", cache_key, e);
                }
            }
        }
    }

    let total = missing.len();
    if total == 0 {
        info!("Gallery is fully populated (no missing images)");
        return;
    }

    info!("Gallery pre-generation: {} images to generate", total);

    // Generate each missing image serially
    for (index, (model_name, generate_request)) in missing.iter().enumerate() {
        let progress = index + 1;
        info!(
            "Gallery: Generating {}/{} ({} / {})",
            progress, total, model_name, generate_request.raw_prompt
        );

        // Resolve the model
        let model = match models_config.models.get(model_name) {
            Some(m) => m.clone(),
            None => {
                error!("Model not found: {}", model_name);
                continue;
            }
        };

        // Clone model for cache operations since it will be moved into the request
        let model_for_cache = model.clone();

        // Generate the image
        let imagen_request = GenerateImagesRequest {
            prompt: generate_request.clone(),
            model,
            progress: None,
            batch: None,
        };

        let result = match submit_generation(imagen_request).await {
            Ok(response) => response,
            Err(e) => {
                error!(
                    "Failed to generate gallery image for {} / {}: {:#}",
                    model_name, generate_request.raw_prompt, e
                );
                continue;
            }
        };

        // Upload all 4 images
        if result.images.len() < 4 {
            error!(
                "Expected 4 images but got {} for {} / {}",
                result.images.len(),
                model_name,
                generate_request.raw_prompt
            );
            continue;
        }

        let backend_str = result.backend.as_str().to_string();
        let mut uuids = Vec::new();

        for (idx, image) in result.images.iter().take(4).enumerate() {
            let image_url = match upload_image_with_generation(
                image.clone(),
                result.workflow.clone(),
                Some(backend_str.clone()),
                Some(generate_request.clone()),
            )
            .await
            {
                Ok(url) => url,
                Err(e) => {
                    error!(
                        "Failed to upload gallery image {}/4 for {} / {}: {:#}",
                        idx + 1,
                        model_name,
                        generate_request.raw_prompt,
                        e
                    );
                    continue;
                }
            };

            // Extract UUID from URL
            let uuid = image_url
                .trim_start_matches(&image_host_base_url)
                .trim_start_matches('/')
                .trim_end_matches(".jpg")
                .to_string();

            uuids.push(uuid);
        }

        // Only cache if we successfully uploaded all 4 images
        if uuids.len() == 4 {
            // Store in cache
            if let Err(e) = set_gallery_image(
                &mut redis,
                model_name,
                &model_for_cache,
                generate_request,
                &uuids,
            )
            .await
            {
                error!("Failed to cache gallery images: {:#}", e);
            }
        } else {
            error!(
                "Only uploaded {}/4 images for {} / {}, skipping cache",
                uuids.len(),
                model_name,
                generate_request.raw_prompt
            );
            continue;
        }

        info!("Gallery: Generated {}/{} successfully", progress, total);
    }

    info!("Gallery pre-generation task completed");
}
