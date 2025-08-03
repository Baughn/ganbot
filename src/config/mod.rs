pub mod global;

use config::{Config as ConfigBuilder, ConfigError, Environment, File};
use std::path::Path;

pub use global::Config;

pub fn load() -> Result<Config, ConfigError> {
    let mut builder = ConfigBuilder::builder();

    // Load from config.toml if it exists
    if Path::new("config.toml").exists() {
        builder = builder.add_source(File::with_name("config"));
    }

    // Layer environment variables with GANBOT_ prefix
    builder = builder.add_source(
        Environment::with_prefix("GANBOT")
            .prefix_separator("_")
            .separator("__")
    );

    let config = builder.build()?;
    config.try_deserialize()
}