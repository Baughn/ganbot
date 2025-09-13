//! Help utilities for formatting and displaying bot information

use crate::config::models::{Backend, ModelsConfig};
use crate::supervisor::Supervisor;
use anyhow::Result;

/// Structured information about available models
#[derive(Debug, Clone)]
pub struct ModelsHelp {
    pub default: String,
    pub aliases: Vec<(String, String)>, // (alias, target)
    pub models: Vec<ModelInfo>,
}

/// Information about a specific model
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub name: String,
    pub description: Option<String>,
    pub backend_info: BackendInfo,
}

/// Backend configuration information
#[derive(Debug, Clone)]
pub enum BackendInfo {
    NanoBanana,
    ComfyUI {
        checkpoint: String,
        sampler: String,
        steps: u32,
        resolution: (u32, u32),
        cfg: f32,
        scheduler: String,
    },
}

/// Fetches and structures model configuration information
pub async fn get_models_help() -> Result<ModelsHelp> {
    // Fetch the models configuration from supervisor
    let models_config = Supervisor::models_config().await;

    // Convert to structured help data
    Ok(models_config_to_help(models_config))
}

/// Convert ModelsConfig to ModelsHelp structure
fn models_config_to_help(config: ModelsConfig) -> ModelsHelp {
    // Convert aliases to sorted vector of tuples
    let mut aliases: Vec<(String, String)> = config.aliases.into_iter().collect();
    aliases.sort_by(|a, b| a.0.cmp(&b.0));

    // Convert models to ModelInfo structs
    let mut models: Vec<ModelInfo> = config
        .models.into_values().map(|model| {
            let backend_info = match model.backend {
                Backend::NanoBanana => BackendInfo::NanoBanana,
                Backend::ComfyUI {
                    checkpoint,
                    cfg,
                    sampler,
                    scheduler,
                    steps,
                    resolution,
                    ..
                } => BackendInfo::ComfyUI {
                    checkpoint: match &checkpoint {
                        crate::config::models::Checkpoint::Combined(name) => name.clone(),
                        crate::config::models::Checkpoint::Split {
                            unet,
                            clip: _,
                            vae: _,
                        } => unet.clone(),
                    },
                    sampler,
                    steps,
                    resolution,
                    cfg,
                    scheduler,
                },
            };

            ModelInfo {
                name: model.name,
                description: model.description,
                backend_info,
            }
        })
        .collect();

    // Sort models by name for consistent display
    models.sort_by(|a, b| a.name.cmp(&b.name));

    ModelsHelp {
        default: config.default,
        aliases,
        models,
    }
}
