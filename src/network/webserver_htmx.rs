use anyhow::{Context, Result};
use askama::Template;
use askama_web::WebTemplate;
use axum::{
    Router,
    extract::{Json, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use axum_htmx::{HxPushUrl, HxRequest, HxResponseTrigger};
use kameo::{Actor, actor::ActorRef};
use redis::aio::ConnectionManager;
use socket2::{Domain, Socket, Type};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::{
    config::global::{ModelGalleryConfig, WebServerConfig},
    supervisor::Supervisor,
};

/// Shared application state for htmx gallery
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
    col: Option<u32>,
}

/// Request body for gallery regen endpoint
#[derive(serde::Deserialize)]
struct RegenRequest {
    model_name: String,
    prompt: String,
    style_name: String,
}

/// htmx-enabled web server actor
pub struct WebServerHtmx {
    gallery_config: ModelGalleryConfig,
    redis: ConnectionManager,
    shutdown_token: CancellationToken,
    enable_regen: bool,
}

impl WebServerHtmx {
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
            enable_regen: config.enable_regen,
        }
    }

    pub async fn run(self, listener: TcpListener) -> Result<()> {
        let local_addr = listener
            .local_addr()
            .context("Failed to get local address")?;

        info!("htmx web server listening on {}", local_addr);

        let state = AppState {
            gallery_config: self.gallery_config.clone(),
            redis: self.redis.clone(),
            enable_regen: self.enable_regen,
        };

        let app = Router::new()
            .route("/gallery-htmx", get(gallery_handler))
            .route("/gallery-htmx/modal/{model}/{prompt}", get(modal_handler))
            .route("/gallery-htmx/regen", post(regen_handler))
            .with_state(state);

        let shutdown_token = self.shutdown_token.clone();
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_token.cancelled().await;
                info!("htmx web server shutdown signal received");
            })
            .await
            .context("htmx web server error")?;

        info!("htmx web server stopped");
        Ok(())
    }
}

impl Actor for WebServerHtmx {
    type Args = (WebServerConfig, ModelGalleryConfig, ConnectionManager);
    type Error = anyhow::Error;

    async fn on_start(args: Self::Args, actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        let (config, gallery_config, redis) = args;

        // Bind listener with SO_REUSEADDR
        let addr = format!("{}:{}", config.bind_address, config.port);
        let socket_addr: SocketAddr = addr.parse().context("Failed to parse bind address")?;

        info!("Starting htmx web server on {}", socket_addr);

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

        let shutdown_token = CancellationToken::new();

        // Clone values for spawned task
        let server_config = config.clone();
        let server_gallery_config = gallery_config.clone();
        let server_redis = redis.clone();
        let server_shutdown_token = shutdown_token.clone();
        let actor_ref_clone = actor_ref.clone();

        // Spawn server in background
        tokio::spawn(async move {
            let server = WebServerHtmx::new(
                server_config,
                server_gallery_config,
                server_redis,
                server_shutdown_token,
            );
            match server.run(listener).await {
                Ok(()) => {
                    info!("htmx web server exited cleanly");
                }
                Err(e) => {
                    error!("htmx web server failed: {:#}", e);
                    let _ = actor_ref_clone.stop_gracefully().await;
                }
            }
        });

        Ok(Self {
            gallery_config,
            redis,
            shutdown_token,
            enable_regen: config.enable_regen,
        })
    }

    async fn on_stop(
        &mut self,
        _actor_ref: kameo::actor::WeakActorRef<Self>,
        reason: kameo::error::ActorStopReason,
    ) -> Result<(), Self::Error> {
        info!("WebServerHtmx stopping: {:?}", reason);
        self.shutdown_token.cancel();
        info!("WebServerHtmx cleanup complete");
        Ok(())
    }
}

// ============================================================================
// Route Handlers
// ============================================================================

/// Main gallery handler - returns full page or table partial depending on request
async fn gallery_handler(
    HxRequest(is_htmx): HxRequest,
    Query(query): Query<GalleryQuery>,
    State(state): State<AppState>,
) -> Response {
    // TODO: Implement gallery handler
    //
    // If is_htmx:
    //   - Return table partial (templates/gallery_htmx/table_body.html)
    //   - Apply tag/style filtering
    //   - Apply pagination (col parameter)
    //
    // If not is_htmx:
    //   - Return full page (templates/gallery_htmx/page.html)
    //   - Include filter buttons, table, pagination
    //
    // Use HxPushUrl to update browser URL on htmx requests

    (
        StatusCode::NOT_IMPLEMENTED,
        "TODO: Implement gallery handler",
    )
        .into_response()
}

/// Modal content handler - returns modal HTML for a specific model/prompt
async fn modal_handler(
    Path((model, prompt)): Path<(String, String)>,
    Query(query): Query<GalleryQuery>,
    State(state): State<AppState>,
) -> Response {
    // TODO: Implement modal handler
    //
    // 1. Look up model configuration from supervisor
    // 2. Find the gallery cell for this model + prompt
    // 3. Get image URLs from Redis cache
    // 4. Render modal template (templates/gallery_htmx/modal.html)
    // 5. Include navigation buttons with hx-get to adjacent models/prompts
    //
    // Use HxPushUrl to update browser URL with modal state

    (StatusCode::NOT_IMPLEMENTED, "TODO: Implement modal handler").into_response()
}

/// Regenerate gallery cell - returns updated cell HTML
async fn regen_handler(
    State(state): State<AppState>,
    Json(request): Json<RegenRequest>,
) -> Response {
    // TODO: Implement regen handler
    //
    // 1. Check if regen is enabled (state.enable_regen)
    // 2. Validate model exists
    // 3. Generate 4 new images (similar to existing webserver)
    // 4. Upload images and get UUIDs
    // 5. Update Redis cache
    // 6. Render cell template (templates/gallery_htmx/cell.html)
    // 7. Use HxResponseTrigger to show success message
    //
    // On error, return error response with appropriate status code

    (StatusCode::NOT_IMPLEMENTED, "TODO: Implement regen handler").into_response()
}

// ============================================================================
// Template Definitions (to be created)
// ============================================================================

// TODO: Create these templates in templates/gallery_htmx/
//
// - page.html         : Full page layout with htmx CDN
// - table.html        : Filter buttons + full table
// - table_body.html   : Just <tbody> for partial swaps
// - modal.html        : Modal content with navigation
// - cell.html         : Single gallery cell (4 images)

// ============================================================================
// Helper Functions (to be implemented as needed)
// ============================================================================

// TODO: Port these from webserver.rs as needed:
// - get_gallery_image()  : Fetch cached image URLs
// - set_gallery_image()  : Store image URLs in cache
// - build_gallery_generate() : Build Generate struct
// - apply_style_to_prompt() : Apply style prepend to prompt
// - gallery_cache_key() : Generate cache key
