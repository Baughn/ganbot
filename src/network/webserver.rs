use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use kameo::{Actor, actor::ActorRef, registry::ACTOR_REGISTRY};
use redis::aio::ConnectionManager;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};
use tracing::{debug, error, info};

use crate::{
    actions::imagen::{GenerateImagesRequest, submit_generation},
    config::{
        global::{ModelGalleryConfig, WebServerConfig},
        models::Backend,
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
}

/// Shared application state
#[derive(Clone)]
struct AppState {
    gallery_config: ModelGalleryConfig,
    redis: ConnectionManager,
}

impl WebServer {
    pub fn new(
        config: WebServerConfig,
        gallery_config: ModelGalleryConfig,
        redis: ConnectionManager,
    ) -> Self {
        Self {
            config,
            gallery_config,
            redis,
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

        axum::serve(listener, app)
            .await
            .context("Web server error")?;

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

        let listener = TcpListener::bind(socket_addr)
            .await
            .context("Failed to bind TCP listener")?;

        // Clone values for the spawned task
        let server_config = config.clone();
        let server_gallery_config = gallery_config.clone();
        let server_redis = redis.clone();

        // Clone actor ref to stop it on error
        let actor_ref_clone = actor_ref.clone();

        // Spawn the server in a separate task and get the handle
        let server_handle = tokio::spawn(async move {
            let server = WebServer::new(server_config, server_gallery_config, server_redis);
            server.run(listener).await
        });

        // Spawn a monitoring task that watches for panics or errors
        tokio::spawn(async move {
            match server_handle.await {
                Ok(Ok(())) => {
                    info!("Web server exited cleanly");
                }
                Ok(Err(e)) => {
                    error!("Web server failed: {:#}", e);
                    let _ = actor_ref_clone.stop_gracefully().await;
                }
                Err(e) => {
                    error!("Web server panicked: {:#}", e);
                    let _ = actor_ref_clone.stop_gracefully().await;
                }
            }
        });

        // Spawn background task to pre-generate missing gallery images
        let pregen_gallery_config = gallery_config.clone();
        let pregen_redis = redis.clone();
        tokio::spawn(async move {
            // Get image host config
            let image_host_config = Supervisor::image_host().await;
            pre_generate_gallery_task(
                pregen_gallery_config,
                pregen_redis,
                image_host_config.base_url,
            )
            .await;
        });

        // Return an instance for the actor
        Ok(Self {
            config,
            gallery_config,
            redis,
        })
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
async fn model_gallery_handler(State(mut state): State<AppState>) -> impl IntoResponse {
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
    // Set "Recommended" as default if it exists, otherwise "All"
    let default_tag = if tags.contains(&"recommended".to_string()) {
        "recommended"
    } else {
        "all"
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
        table_rows.push_str(&format!("<td class=\"model-label\">{}</td>", model_name));

        for prompt in &state.gallery_config.prompts {
            // Build a Generate struct and apply model defaults
            let mut generate = Generate {
                raw_prompt: prompt.clone(),
                prompt: prompt.clone(),
                negative_prompt: None,
                num_images: Some(1),
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
            };
            crate::actions::imagen::apply_model_defaults(&mut generate, model);

            let image_url = get_gallery_image(
                &mut state.redis,
                model_name,
                &generate,
                &image_host_config.base_url,
            )
            .await;

            table_rows.push_str(&format!(
                r#"<td><a href="{0}" class="gallery-link"><img src="{0}" alt="{1} - {2}" class="gallery-img" /></a></td>"#,
                image_url, model_name, prompt
            ));
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

/// Generate a stable cache key for a model+prompt combination
fn gallery_cache_key(model_name: &str, generate: &Generate) -> String {
    let mut hasher = Sha256::new();
    // Serialize the entire Generate struct to ensure all parameters are included
    let json = serde_json::to_string(generate).unwrap_or_default();
    hasher.update(json.as_bytes());
    let hash = hasher.finalize();
    format!("gallery:cache:{}:{:x}", model_name, hash)
}

/// Get cached gallery image URL, or return placeholder
async fn get_gallery_image(
    redis: &mut ConnectionManager,
    model_name: &str,
    generate: &Generate,
    image_host_base_url: &str,
) -> String {
    let cache_key = gallery_cache_key(model_name, generate);

    match redis::cmd("GET")
        .arg(&cache_key)
        .query_async::<String>(redis)
        .await
    {
        Ok(uuid) => {
            debug!("Gallery cache hit: {} -> {}", cache_key, uuid);
            format!("{}/{}.jpg", image_host_base_url.trim_end_matches('/'), uuid)
        }
        Err(_) => {
            debug!("Gallery cache miss: {}", cache_key);
            PLACEHOLDER_URL.to_string()
        }
    }
}

/// Store gallery image UUID in cache
async fn set_gallery_image(
    redis: &mut ConnectionManager,
    model_name: &str,
    generate: &Generate,
    image_uuid: &str,
) -> Result<()> {
    let cache_key = gallery_cache_key(model_name, generate);

    redis::cmd("SET")
        .arg(&cache_key)
        .arg(image_uuid)
        .query_async::<()>(redis)
        .await
        .context("Failed to set gallery cache")?;

    info!("Cached gallery image: {} -> {}", cache_key, image_uuid);
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
            let mut generate = Generate {
                raw_prompt: prompt.clone(),
                prompt: prompt.clone(),
                negative_prompt: None,
                num_images: Some(1),
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
            };
            crate::actions::imagen::apply_model_defaults(&mut generate, model);

            let cache_key = gallery_cache_key(model_name, &generate);
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

        // Upload the image
        if result.images.is_empty() {
            error!(
                "No images generated for {} / {}",
                model_name, generate_request.raw_prompt
            );
            continue;
        }

        let image = result.images[0].clone();
        let backend_str = result.backend.as_str().to_string();
        let image_url = match upload_image_with_generation(
            image,
            result.workflow,
            Some(backend_str),
            Some(generate_request.clone()),
        )
        .await
        {
            Ok(url) => url,
            Err(e) => {
                error!("Failed to upload gallery image: {:#}", e);
                continue;
            }
        };

        // Extract UUID from URL
        let uuid = image_url
            .trim_start_matches(&image_host_base_url)
            .trim_start_matches('/')
            .trim_end_matches(".jpg");

        // Store in cache
        if let Err(e) = set_gallery_image(&mut redis, model_name, generate_request, uuid).await {
            error!("Failed to cache gallery image: {:#}", e);
        }

        info!("Gallery: Generated {}/{} successfully", progress, total);
    }

    info!("Gallery pre-generation task completed");
}
