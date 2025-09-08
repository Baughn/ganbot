/// Global configuration from config.toml & environment variables
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    // Backend configurations
    pub invokeai: InvokeaiConfig,
    pub openrouter: OpenrouterConfig,
    pub redis_url: String,
    pub image_host: ImageHostConfig,
    // Client configurations
    pub irc: Vec<IrcConfig>,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct InvokeaiConfig {
    pub url: String,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct ImageHostConfig {
    pub ssh_hostname: String,
    pub ssh_directory: String,
    pub base_url: String,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct OpenrouterConfig {
    pub token: String,
    // Per-purpose model choices will be listed here.
    #[serde(default = "default_chat_model")]
    pub chat_model: String,
    #[serde(default = "default_image_model")]
    pub image_model: String,
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq, Hash)]
#[serde(deny_unknown_fields)]
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

fn default_chat_model() -> String {
    "anthropic/claude-3-5-sonnet".to_string()
}

fn default_image_model() -> String {
    "openai/gpt-4o".to_string()
}

impl Default for OpenrouterConfig {
    fn default() -> Self {
        Self {
            token: String::new(),
            chat_model: default_chat_model(),
            image_model: default_image_model(),
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
