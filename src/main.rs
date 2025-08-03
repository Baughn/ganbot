use tracing::{debug, info, trace, error};
use tracing_subscriber::EnvFilter;

mod config;
mod network;

fn main() {
    // Initialize tracing with environment-based configuration
    // RUST_LOG env var controls log levels (e.g., RUST_LOG=debug or RUST_LOG=ganbot3=trace,warn)
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(true)
        .with_line_number(true)
        .with_level(true)
        .init();

    info!("Starting ganbot3");
    debug!("Debug logging enabled");
    trace!("Trace logging enabled");

    // Load configuration
    let config = match config::load() {
        Ok(config) => {
            info!("Configuration loaded successfully");
            debug!("Config: {:#?}", config);
            config
        }
        Err(e) => {
            error!("Failed to load configuration: {}", e);
            std::process::exit(1);
        }
    };
    
    // Your application code here
    info!("Application initialized successfully with {} IRC server(s)", config.irc.len());
}
