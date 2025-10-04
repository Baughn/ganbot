use anyhow::{Context, Result};

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

#[tokio::main]
async fn main() -> Result<()> {
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
