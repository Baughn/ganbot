/// Global configuration from config.toml & environment variables
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct Config {
    // Backend configurations
    pub invokeai: InvokeaiConfig,
    pub openrouter: OpenrouterConfig,
    // Client configurations
    pub irc: Vec<IrcConfig>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct InvokeaiConfig {
    pub url: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct OpenrouterConfig {
    pub token: String,
    // Per-purpose model choices will be listed here.
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq, Hash)]
pub struct IrcConfig {
    pub server: String,
    #[serde(default = "default_true")]
    pub tls: bool,
    pub port: u16,
    pub channels: Vec<String>,
    pub nick: String,
    pub nickserv_password: Option<String>,
    #[serde(default = "default_bang")]
    pub command_prefix: String,
}

fn default_true() -> bool {
    true
}

fn default_bang() -> String {
    "!".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            invokeai: InvokeaiConfig::default(),
            openrouter: OpenrouterConfig::default(),
            irc: vec![],
        }
    }
}

impl Default for InvokeaiConfig {
    fn default() -> Self {
        Self { url: String::new() }
    }
}

impl Default for OpenrouterConfig {
    fn default() -> Self {
        Self {
            token: String::new(),
        }
    }
}

impl Default for IrcConfig {
    fn default() -> Self {
        Self {
            server: String::new(),
            tls: true,
            port: 6667,
            channels: vec![],
            nick: String::new(),
            nickserv_password: None,
            command_prefix: "!".to_string(),
        }
    }
}
