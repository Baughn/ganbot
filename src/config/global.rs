/// Global configuration from config.toml & environment variables

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    // Backend configurations
    pub invokeai: InvokeaiConfig,
    pub openrouter: OpenrouterConfig,
    // Client configurations
    pub irc: Vec<IrcConfig>,
}

#[derive(Debug, Deserialize)]
pub struct InvokeaiConfig {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct OpenrouterConfig {
    pub token: String,
    // Per-purpose model choices will be listed here.
}

#[derive(Debug, Deserialize)]
pub struct IrcConfig {
    pub server: String,
    #[serde(default = "default_true")]
    pub tls: bool,
    pub port: u16,
    pub channels: Vec<String>,
    pub nick: String,
    pub nickserv_password: Option<String>,
}

fn default_true() -> bool {
    true
}