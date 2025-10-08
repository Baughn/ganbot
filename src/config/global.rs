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
    pub discord: Vec<DiscordConfig>,
    // Web server configuration
    #[serde(default)]
    pub webserver: Option<WebServerConfig>,
    #[serde(default)]
    pub model_gallery: ModelGalleryConfig,
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
    #[serde(default = "default_dream_model")]
    pub dream_model: String,
    #[serde(default = "default_cheap_models")]
    pub cheap_model: Vec<String>,
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
    pub sasl_username: Option<String>,
    pub sasl_password: Option<String>,
    #[serde(default = "default_bang")]
    pub command_prefix: String,
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq, Hash, Default)]
#[serde(deny_unknown_fields)]
pub struct DiscordConfig {
    pub token: String,
    pub application_id: u64,
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq, Hash)]
#[serde(deny_unknown_fields)]
pub struct WebServerConfig {
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq, Hash, Default)]
#[serde(deny_unknown_fields)]
pub struct ModelGalleryConfig {
    #[serde(default)]
    pub prompts: Vec<String>,
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

fn default_dream_model() -> String {
    default_chat_model()
}

fn default_cheap_models() -> Vec<String> {
    vec![default_chat_model()]
}

fn default_bind_address() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8080
}

impl Default for OpenrouterConfig {
    fn default() -> Self {
        Self {
            token: String::new(),
            chat_model: default_chat_model(),
            image_model: default_image_model(),
            dream_model: default_dream_model(),
            cheap_model: default_cheap_models(),
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
            sasl_username: None,
            sasl_password: None,
            command_prefix: "!".to_string(),
        }
    }
}

impl Default for WebServerConfig {
    fn default() -> Self {
        Self {
            bind_address: default_bind_address(),
            port: default_port(),
        }
    }
}
