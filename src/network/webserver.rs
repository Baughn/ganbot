use anyhow::{Context, Result};
use askama::Template;
use askama_web::WebTemplate;
use axum::{
    Router,
    extract::{Json, Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
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
        global::{GalleryStyle, ModelGalleryConfig, WebServerConfig},
        models::{Backend, Model},
    },
    messages::imagen::Generate,
    persistence::images::upload_image_with_generation,
    supervisor::{GetModelsConfig, Supervisor},
    util::image_compression::compress_jpeg,
};

/// Template for the index/home page
#[derive(Template, WebTemplate)]
#[template(path = "index.html")]
struct IndexTemplate {
    css_url: String,
}

/// Template for help pages
#[derive(Template, WebTemplate)]
#[template(path = "help.html")]
struct HelpTemplate {
    css_url: String,
    path: String,
    content: String,
}

/// Template for the model gallery page
#[derive(Template, WebTemplate)]
#[template(path = "gallery.html")]
struct GalleryTemplate {
    css_url: String,
    js_url: String,
    active_tag: String,
    tags: Vec<TagButton>,
    active_style: String,
    styles: Vec<StyleButton>,
    prompts: Vec<String>,
    models: Vec<ModelRow>,
    enable_regen: bool,
}

/// Tag filter button data
struct TagButton {
    name: String,
    display_name: String,
}

/// Style filter button data
struct StyleButton {
    name: String,
    display_name: String,
}

/// Model row data for the gallery table
struct ModelRow {
    name: String,
    tags: String,
    aliases: Vec<String>,
    cells: Vec<GalleryCell>,
}

/// Gallery cell data for each (model, prompt) combination
struct GalleryCell {
    urls_json: String,
    offset: u32,
    model_config: String,
    prompt: String,
    images: Vec<GalleryImage>,
}

/// Individual image data within a gallery cell
struct GalleryImage {
    url: String,
    opacity: &'static str,
    pointer_events: &'static str,
    index: usize,
    loading: &'static str,
}

/// Web server actor
pub struct WebServer {
    gallery_config: ModelGalleryConfig,
    redis: ConnectionManager,
    shutdown_token: CancellationToken,
    pregen_handle: Option<tokio::task::JoinHandle<()>>,
    enable_regen: bool,
}

/// Shared application state
#[derive(Clone)]
struct AppState {
    gallery_config: ModelGalleryConfig,
    redis: ConnectionManager,
    enable_regen: bool,
}

/// Query parameters for gallery filtering
#[derive(serde::Deserialize)]
struct GalleryQuery {
    tag: Option<String>,
    style: Option<String>,
}

/// Request body for gallery regen endpoint
#[derive(serde::Deserialize)]
struct GalleryRegenRequest {
    model_name: String,
    prompt: String,
    style_name: String,
}

/// Response for gallery regen endpoint
#[derive(serde::Serialize)]
struct GalleryRegenResponse {
    success: bool,
    urls: Vec<String>,
    error: Option<String>,
}

impl WebServer {
    pub fn new(
        config: WebServerConfig,
        gallery_config: ModelGalleryConfig,
        redis: ConnectionManager,
        shutdown_token: CancellationToken,
    ) -> Self {
        Self {
            gallery_config,
            redis,
            shutdown_token,
            pregen_handle: None,
            enable_regen: config.enable_regen,
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
            enable_regen: self.enable_regen,
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
            .route("/gallery/regen", axum::routing::post(gallery_regen_handler))
            .route(
                "/image/{scale}/{quality}/{uuid}",
                get(compressed_image_handler),
            )
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
            gallery_config,
            redis,
            shutdown_token,
            pregen_handle: Some(pregen_handle),
            enable_regen: config.enable_regen,
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

    IndexTemplate { css_url }
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
            let mut content = String::new();
            pulldown_cmark::html::push_html(&mut content, parser);

            HelpTemplate {
                css_url,
                path,
                content,
            }
            .into_response()
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
        let js_url = static_url("gallery.js").await;
        return GalleryTemplate {
            css_url,
            js_url,
            active_tag: "all".to_string(),
            tags: vec![],
            active_style: "default".to_string(),
            styles: vec![],
            prompts: vec![],
            models: vec![],
            enable_regen: state.enable_regen,
        }
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

    // Build tag buttons data
    let tag_buttons: Vec<TagButton> = tags
        .iter()
        .map(|tag| TagButton {
            name: tag.clone(),
            display_name: capitalize(tag),
        })
        .collect();

    // Build style buttons data
    let mut style_names: Vec<String> = state.gallery_config.styles.keys().cloned().collect();
    style_names.sort();

    let style_buttons: Vec<StyleButton> = style_names
        .iter()
        .map(|style| StyleButton {
            name: style.clone(),
            display_name: capitalize(style),
        })
        .collect();

    // Determine active style (default to "default")
    let active_style = if let Some(ref query_style) = query.style {
        // Validate that the style exists in our style list or is "default"
        if query_style == "default" || state.gallery_config.styles.contains_key(query_style) {
            query_style.as_str()
        } else {
            "default"
        }
    } else {
        "default"
    };

    // Build model rows data
    let mut model_rows = Vec::new();
    for (model_name, model) in &models {
        let tags_attr = model.tags.join(",");
        let aliases = model_aliases.get(*model_name).cloned().unwrap_or_default();

        let mut cells = Vec::new();
        for prompt in &state.gallery_config.prompts {
            // Apply style prepend to prompt
            let styled_prompt = apply_style_to_prompt(
                prompt,
                active_style,
                &state.gallery_config.styles,
                &model.tags,
            );

            // Build a Generate struct and apply model defaults
            let mut generate = build_gallery_generate(&styled_prompt, model_name);
            crate::actions::imagen::apply_model_defaults(&mut generate, model);

            let image_urls =
                get_gallery_image(&mut state.redis, model_name, model, &generate, active_style)
                    .await;

            // Generate a deterministic but varied cycle offset (0-39, representing 0-9.75s in 0.25s increments)
            // Based on hash of model+prompt to ensure different cells don't sync
            let offset_seed = format!("{}{}", model_name, prompt);
            let offset = offset_seed.bytes().map(|b| b as u32).sum::<u32>() % 40;

            // Serialize URLs as JSON for data attribute
            let urls_json = serde_json::to_string(&image_urls).unwrap_or_else(|_| "[]".to_string());

            // Build model config for modal display
            let model_config = build_model_config_json(model);

            // Build image data
            let images: Vec<GalleryImage> = image_urls
                .iter()
                .enumerate()
                .map(|(index, url)| {
                    let (opacity, pointer_events) = if index == 0 {
                        ("1", "auto")
                    } else {
                        ("0", "none")
                    };
                    let loading = if index == 0 { "eager" } else { "lazy" };
                    GalleryImage {
                        url: url.clone(),
                        opacity,
                        pointer_events,
                        index,
                        loading,
                    }
                })
                .collect();

            cells.push(GalleryCell {
                urls_json,
                offset,
                model_config,
                prompt: styled_prompt.clone(),
                images,
            });
        }

        model_rows.push(ModelRow {
            name: model_name.to_string(),
            tags: tags_attr,
            aliases,
            cells,
        });
    }

    // Render the template
    let css_url = static_url("style.css").await;
    let js_url = static_url("gallery.js").await;

    GalleryTemplate {
        css_url,
        js_url,
        active_tag: default_tag.to_string(),
        tags: tag_buttons,
        active_style: active_style.to_string(),
        styles: style_buttons,
        prompts: state.gallery_config.prompts.clone(),
        models: model_rows,
        enable_regen: state.enable_regen,
    }
    .into_response()
}

/// Handler for gallery regeneration endpoint
async fn gallery_regen_handler(
    State(mut state): State<AppState>,
    Json(request): Json<GalleryRegenRequest>,
) -> Response {
    // Check if regen is enabled
    if !state.enable_regen {
        return (
            StatusCode::FORBIDDEN,
            Json(GalleryRegenResponse {
                success: false,
                urls: vec![],
                error: Some("Gallery regeneration is not enabled".to_string()),
            }),
        )
            .into_response();
    }

    info!(
        "Regenerating gallery cell for model={}, prompt={}, style={}",
        request.model_name, request.prompt, request.style_name
    );

    // Get models configuration
    let supervisor = match ACTOR_REGISTRY
        .lock()
        .unwrap()
        .get::<Supervisor, str>("supervisor")
    {
        Ok(Some(sup)) => sup,
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GalleryRegenResponse {
                    success: false,
                    urls: vec![],
                    error: Some("Failed to access supervisor".to_string()),
                }),
            )
                .into_response();
        }
    };

    let models_config = match supervisor.ask(GetModelsConfig).await {
        Ok(reply) => reply.0,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GalleryRegenResponse {
                    success: false,
                    urls: vec![],
                    error: Some("Failed to fetch models configuration".to_string()),
                }),
            )
                .into_response();
        }
    };

    // Get the model
    let model = match models_config.models.get(&request.model_name) {
        Some(m) => m.clone(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(GalleryRegenResponse {
                    success: false,
                    urls: vec![],
                    error: Some(format!("Model '{}' not found", request.model_name)),
                }),
            )
                .into_response();
        }
    };

    // Build Generate request
    let mut generate = build_gallery_generate(&request.prompt, &request.model_name);
    crate::actions::imagen::apply_model_defaults(&mut generate, &model);

    // Calculate cache key
    let cache_key = gallery_cache_key(&request.model_name, &model, &generate, &request.style_name);

    // Get old image UUIDs from cache
    let old_uuids: Option<Vec<String>> = match redis::cmd("GET")
        .arg(&cache_key)
        .query_async::<Option<String>>(&mut state.redis)
        .await
    {
        Ok(Some(json_str)) => serde_json::from_str(&json_str).ok(),
        _ => None,
    };

    // Delete old cache entry
    let _: () = redis::cmd("DEL")
        .arg(&cache_key)
        .query_async(&mut state.redis)
        .await
        .unwrap_or_default();

    // Delete old image files if they exist
    if let Some(uuids) = old_uuids {
        let image_host_config = Supervisor::image_host().await;
        for uuid in &uuids {
            // Find all files with this UUID (the 4 individual images)
            let filename = format!("{}.jpg", uuid);
            let remote_path = format!(
                "{}:{}/{}",
                image_host_config.ssh_hostname, image_host_config.ssh_directory, filename
            );

            debug!("Deleting old gallery image: {}", remote_path);

            let output = tokio::process::Command::new("ssh")
                .arg("-o")
                .arg("StrictHostKeyChecking=no")
                .arg(&image_host_config.ssh_hostname)
                .arg("rm")
                .arg("-f")
                .arg(format!("{}/{}", image_host_config.ssh_directory, filename))
                .output()
                .await;

            if let Ok(output) = output {
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    error!("Failed to delete old image {}: {}", filename, stderr);
                }
            }

            // Also remove from Redis tracking
            let _: () = redis::cmd("ZREM")
                .arg("image:files")
                .arg(&filename)
                .query_async(&mut state.redis)
                .await
                .unwrap_or_default();
        }
    }

    // Generate new images
    info!("Generating new images for {}", request.model_name);
    let imagen_request = GenerateImagesRequest {
        prompt: generate.clone(),
        model: model.clone(),
        progress: None,
        batch: None,
    };

    let result = match submit_generation(imagen_request).await {
        Ok(response) => response,
        Err(e) => {
            error!("Failed to generate new images: {:#}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GalleryRegenResponse {
                    success: false,
                    urls: vec![],
                    error: Some(format!("Image generation failed: {}", e)),
                }),
            )
                .into_response();
        }
    };

    if result.images.len() < 4 {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(GalleryRegenResponse {
                success: false,
                urls: vec![],
                error: Some(format!("Expected 4 images but got {}", result.images.len())),
            }),
        )
            .into_response();
    }

    // Upload the new images
    let backend_str = result.backend.as_str().to_string();
    let mut new_uuids = Vec::new();
    let image_host_base_url = Supervisor::image_host().await.base_url;

    for image in result.images.iter().take(4) {
        match upload_image_with_generation(
            image.clone(),
            result.workflow.clone(),
            Some(backend_str.clone()),
            Some(generate.clone()),
        )
        .await
        {
            Ok(url) => {
                // Extract UUID from URL
                let uuid = url
                    .trim_start_matches(&image_host_base_url)
                    .trim_start_matches('/')
                    .trim_end_matches(".jpg")
                    .to_string();
                new_uuids.push(uuid);
            }
            Err(e) => {
                error!("Failed to upload image: {:#}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(GalleryRegenResponse {
                        success: false,
                        urls: vec![],
                        error: Some(format!("Image upload failed: {}", e)),
                    }),
                )
                    .into_response();
            }
        }
    }

    // Store new cache entry
    if let Err(e) = set_gallery_image(
        &mut state.redis,
        &request.model_name,
        &model,
        &generate,
        &request.style_name,
        &new_uuids,
    )
    .await
    {
        error!("Failed to cache new images: {:#}", e);
    }

    // Return the new URLs in the format expected by the frontend
    let new_urls: Vec<String> = new_uuids
        .iter()
        .map(|uuid| format!("/image/0.5/75/{}.jpg", uuid))
        .collect();

    info!(
        "Successfully regenerated gallery cell with {} images",
        new_urls.len()
    );

    (
        StatusCode::OK,
        Json(GalleryRegenResponse {
            success: true,
            urls: new_urls,
            error: None,
        }),
    )
        .into_response()
}

/// Path parameters for compressed image endpoint
#[derive(serde::Deserialize)]
struct ImagePathParams {
    uuid: String,
    scale: String,
    quality: String,
}

/// Handler for compressed image proxy endpoint
async fn compressed_image_handler(
    Path(params): Path<ImagePathParams>,
    State(mut state): State<AppState>,
) -> Response {
    // Strip .jpg suffix from uuid if present
    let uuid = params.uuid.strip_suffix(".jpg").unwrap_or(&params.uuid);

    // Parse scale and quality
    let scale: f32 = match params.scale.parse() {
        Ok(s) if s > 0.0 && s <= 2.0 => s,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Invalid scale parameter (must be 0.0 < scale <= 2.0)",
            )
                .into_response();
        }
    };

    let quality: u8 = match params.quality.parse() {
        Ok(q) if q > 0 && q <= 100 => q,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Invalid quality parameter (must be 1-100)",
            )
                .into_response();
        }
    };

    // Validate UUID format (basic check for enhanced UUIDv7 with optional suffix)
    // Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx[.N]
    let uuid_parts: Vec<&str> = uuid.split('.').collect();
    if uuid_parts.is_empty() || uuid_parts[0].len() < 36 {
        return (StatusCode::BAD_REQUEST, "Invalid UUID format").into_response();
    }

    // Build cache key
    let cache_key = format!(
        "image:compressed:{}:{}:{}",
        uuid, params.scale, params.quality
    );

    // Check Redis cache
    let cached: Option<Vec<u8>> = match redis::cmd("GET")
        .arg(&cache_key)
        .query_async(&mut state.redis)
        .await
    {
        Ok(data) => data,
        Err(e) => {
            error!("Redis error checking cache for {}: {:#}", cache_key, e);
            None
        }
    };

    if let Some(compressed_bytes) = cached {
        trace!("Image cache hit: {}", cache_key);
        return serve_jpeg_response(compressed_bytes);
    }

    // Cache miss - fetch original from remote host
    trace!("Image cache miss: {}", cache_key);
    let image_host_config = Supervisor::image_host().await;
    let original_url = format!(
        "{}/{}.jpg",
        image_host_config.base_url.trim_end_matches('/'),
        uuid
    );

    debug!("Fetching original image from: {}", original_url);

    // Fetch original JPEG
    let original_bytes = match reqwest::get(&original_url).await {
        Ok(response) => {
            if !response.status().is_success() {
                error!("Failed to fetch original image: HTTP {}", response.status());
                return (
                    StatusCode::NOT_FOUND,
                    "Original image not found on remote host",
                )
                    .into_response();
            }
            match response.bytes().await {
                Ok(bytes) => bytes.to_vec(),
                Err(e) => {
                    error!("Failed to read image bytes: {:#}", e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Failed to read image data",
                    )
                        .into_response();
                }
            }
        }
        Err(e) => {
            error!(
                "Failed to fetch original image from {}: {:#}",
                original_url, e
            );
            return (
                StatusCode::BAD_GATEWAY,
                "Failed to fetch image from remote host",
            )
                .into_response();
        }
    };

    // Compress the image
    let compressed_bytes = match compress_jpeg(&original_bytes, scale, quality) {
        Ok(bytes) => bytes,
        Err(e) => {
            error!("Failed to compress image {}: {:#}", uuid, e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to compress image",
            )
                .into_response();
        }
    };

    // Cache the compressed image with 1-day TTL
    if let Err(e) = redis::cmd("SETEX")
        .arg(&cache_key)
        .arg(86400) // 1 day TTL
        .arg(&compressed_bytes)
        .query_async::<()>(&mut state.redis)
        .await
    {
        error!("Failed to cache compressed image {}: {:#}", cache_key, e);
        // Continue serving the image even if caching fails
    } else {
        debug!(
            "Cached compressed image: {} ({} bytes)",
            cache_key,
            compressed_bytes.len()
        );
    }

    serve_jpeg_response(compressed_bytes)
}

/// Helper to build JPEG response with appropriate headers
fn serve_jpeg_response(jpeg_bytes: Vec<u8>) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        jpeg_bytes,
    )
        .into_response()
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
        model: Some(model_name.to_string()),
        ..Default::default()
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

/// Apply style prepend to a prompt based on model tags
fn apply_style_to_prompt(
    prompt: &str,
    style_name: &str,
    styles: &std::collections::HashMap<String, GalleryStyle>,
    model_tags: &[String],
) -> String {
    // "default" style means no prepending
    if style_name == "default" {
        return prompt.to_string();
    }

    // Get the style configuration
    let Some(style) = styles.get(style_name) else {
        return prompt.to_string();
    };

    // Determine which prepend to use based on model tags
    let prepend = if model_tags.contains(&"booru".to_string()) {
        &style.prepend_booru
    } else {
        &style.prepend_english
    };

    // Apply prepend if it's not empty
    if prepend.is_empty() {
        prompt.to_string()
    } else {
        format!("{}{}", prepend, prompt)
    }
}

/// Generate a stable cache key for a model+prompt combination with style
fn gallery_cache_key(
    model_name: &str,
    model: &Model,
    generate: &Generate,
    style_name: &str,
) -> String {
    let mut hasher = Sha256::new();

    // Include the style name to differentiate cached images by style
    hasher.update(style_name.as_bytes());

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
    style_name: &str,
) -> Vec<String> {
    let cache_key = gallery_cache_key(model_name, model, generate, style_name);

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
                    .map(|uuid| format!("/image/0.5/75/{}.jpg", uuid))
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
    style_name: &str,
    image_uuids: &[String],
) -> Result<()> {
    let cache_key = gallery_cache_key(model_name, model, generate, style_name);

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

    // Build list of all styles (default + configured styles)
    let mut all_styles = vec!["default".to_string()];
    all_styles.extend(gallery_config.styles.keys().cloned());

    // Collect all missing (model, prompt, style) combinations
    let mut missing = Vec::new();
    for (model_name, model) in &models {
        for style_name in &all_styles {
            for prompt in &gallery_config.prompts {
                // Apply style prepend to prompt
                let styled_prompt =
                    apply_style_to_prompt(prompt, style_name, &gallery_config.styles, &model.tags);

                // Build Generate struct and apply model defaults
                let mut generate = build_gallery_generate(&styled_prompt, model_name);
                crate::actions::imagen::apply_model_defaults(&mut generate, model);

                let cache_key = gallery_cache_key(model_name, model, &generate, style_name);
                match redis::cmd("EXISTS")
                    .arg(&cache_key)
                    .query_async::<u8>(&mut redis)
                    .await
                {
                    Ok(0) => {
                        missing.push((model_name.to_string(), style_name.clone(), generate));
                    }
                    Ok(_) => {
                        trace!("Gallery image already cached: {}", cache_key);
                    }
                    Err(e) => {
                        error!("Failed to check cache for {}: {:#}", cache_key, e);
                    }
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
    for (index, (model_name, style_name, generate_request)) in missing.iter().enumerate() {
        let progress = index + 1;
        info!(
            "Gallery: Generating {}/{} ({} / {} / {})",
            progress, total, model_name, style_name, generate_request.raw_prompt
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
                style_name,
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
