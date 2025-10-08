use anyhow::{Context, Result};
use clap::Parser;
use kameo::Actor as _;
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

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
    let config = config::load().context("while loading initial configuration")?;
    let supervisor_ref = Supervisor::spawn(config);
    info!("Application initialized successfully");

    // Wait for... a !restart probably.
    supervisor_ref.wait_for_shutdown().await;
    info!("Application shutdown complete");
    Ok(())
}
