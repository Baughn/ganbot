use anyhow::{Context, Result};
use clap::Parser;
use fs2::FileExt;
use kameo::Actor as _;
use std::fs::{File, create_dir_all};
use tracing::{debug, info, trace};
use tracing_subscriber::{EnvFilter, fmt::format::FmtSpan};

use crate::supervisor::Supervisor;

mod actions;
mod config;
mod fuzzy;
mod help;
mod messages;
mod network;
mod persistence;
mod supervisor;
mod util;

#[derive(Parser, Debug)]
#[command(name = "ganbot")]
#[command(about = "IRC bot with AI capabilities and image generation")]
struct Args {
    /// Clear the model gallery cache and exit
    #[arg(long)]
    clear_gallery_cache: bool,

    /// Enable the regeneration button in the gallery
    #[arg(long)]
    enable_regen: bool,
}

/// Acquires an exclusive lock to prevent multiple instances from running.
/// Returns the lock file handle which must be kept alive for the program's duration.
fn acquire_instance_lock() -> Result<File> {
    let lock_dir = dirs::data_local_dir()
        .context("Failed to determine local data directory")?
        .join("ganbot");
    create_dir_all(&lock_dir).context("Failed to create lock directory")?;

    let lock_path = lock_dir.join("ganbot.lock");
    let lock_file = File::create(&lock_path).context("Failed to create lock file")?;

    lock_file.try_lock_exclusive().context(format!(
        "Another instance of ganbot is already running. Lock file: {}",
        lock_path.display()
    ))?;

    Ok(lock_file)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Acquire instance lock to prevent running multiple copies
    let lock_file = acquire_instance_lock().context("Failed to acquire instance lock")?;

    // Initialize tracing with environment-based configuration
    // RUST_LOG env var controls log levels (e.g., RUST_LOG=debug or RUST_LOG=ganbot=trace,warn)
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_ansi(true)
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_level(true)
        .init();

    // Handle clear-gallery-cache flag
    if args.clear_gallery_cache {
        info!("Clearing gallery cache...");
        let config = config::load().context("Failed to load configuration")?;

        // Connect to Redis
        let client = redis::Client::open(config.redis_url.as_str())
            .context("Failed to create Redis client")?;
        let mut conn = client
            .get_connection_manager()
            .await
            .context("Failed to connect to Redis")?;

        // Delete all gallery cache keys
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg("gallery:cache:*")
            .query_async(&mut conn)
            .await
            .context("Failed to query gallery cache keys")?;

        if keys.is_empty() {
            info!("No gallery cache keys found");
        } else {
            let count = keys.len();
            for key in keys {
                redis::cmd("DEL")
                    .arg(&key)
                    .query_async::<()>(&mut conn)
                    .await
                    .context(format!("Failed to delete key: {}", key))?;
            }
            info!("Cleared {} gallery cache entries", count);
        }

        return Ok(());
    }

    info!("Starting ganbot");
    debug!("Debug logging enabled");
    trace!("Trace logging enabled");

    // Initialize supervisor.
    let mut config = config::load().context("while loading initial configuration")?;
    // Apply runtime flags to config
    if let Some(webserver_config) = config.webserver.as_mut() {
        webserver_config.enable_regen = args.enable_regen;
    }
    // Build the OpenAI Tower service (demo of Tower layers; used by
    // `Backend::OpenAI` models in models.toml).
    if !config.openai.token.is_empty() {
        network::openai::set_image_service(network::openai::build_image_service(&config.openai));
        info!("OpenAI image service ready");
    } else {
        info!("openai.token absent; Backend::OpenAI models will fail if invoked");
    }
    let supervisor_ref = Supervisor::spawn(config);
    info!("Application initialized successfully");

    // Wait for... a !restart probably.
    supervisor_ref.wait_for_shutdown().await;
    info!("Application shutdown complete");

    drop(lock_file);
    Ok(())
}
