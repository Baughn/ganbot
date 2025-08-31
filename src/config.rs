pub mod global;
use config::{Config as ConfigBuilder, ConfigError, Environment, File};
pub use global::Config;

pub fn load() -> Result<Config, ConfigError> {
    let mut builder = ConfigBuilder::builder();

    // Load from config.toml by default.
    builder = builder.add_source(File::with_name("config"));

    // Layer config-local.toml file if it exists
    builder = builder.add_source(File::with_name("config-local").required(false));

    // Layer environment variables with GANBOT_ prefix
    builder = builder.add_source(
        Environment::with_prefix("GANBOT")
            .prefix_separator("__")
            .separator("_"),
    );

    let config = builder.build()?;
    config.try_deserialize().map(|mut c: Config| {
        if c.openrouter.token.is_empty() {
            tracing::warn!("OpenRouter token is not set. Some features may not work.");
        } else if c.openrouter.token == "your-openrouter-token-here" {
            tracing::warn!("OpenRouter token is set to the default placeholder. Please update it in your configuration.");
            c.openrouter.token = String::new(); // Clear placeholder token
        }
        c
    })
}
