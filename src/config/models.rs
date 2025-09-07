//! Configuration for image-generation models

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Public API structs - guaranteed to have all required fields populated
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    pub default: String,
    pub aliases: HashMap<String, String>,
    pub models: HashMap<String, Model>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    pub description: Option<String>,
    pub backend: Backend,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Backend {
    NanoBanana,
    StableDiffusion {
        checkpoint: String,
        vae: Option<String>,
        cfg: f32,
        sampler: String,
        scheduler: String,
        steps: u32,
        resolution: (u32, u32),
    },
}

/// Internal structs used during loading and inheritance - these can have None values
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LoadingModelsConfig {
    pub default: String,
    pub aliases: HashMap<String, String>,
    pub templates: HashMap<String, LoadingModel>,
    pub models: HashMap<String, LoadingModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LoadingModel {
    pub name: Option<String>,
    pub inherit: Option<String>,
    pub description: Option<String>,
    pub backend: LoadingBackend,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum LoadingBackend {
    NanoBanana,
    StableDiffusion {
        checkpoint: Option<String>,
        vae: Option<String>,
        cfg: Option<f32>,
        sampler: Option<String>,
        scheduler: Option<String>,
        steps: Option<u32>,
        resolution: Option<(u32, u32)>,
    },
}

/// Load model configuration from models.toml
pub fn load_models_config() -> Result<ModelsConfig> {
    load_models_config_from_path("models.toml")
}

/// Load model configuration from a specific path (used for testing)
pub fn load_models_config_from_path(path: &str) -> Result<ModelsConfig> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("Failed to read {}", path))?;

    let mut loading_config: LoadingModelsConfig =
        toml::from_str(&content).with_context(|| format!("Failed to parse {}", path))?;

    // Apply inheritance - models inherit from templates
    let templates = loading_config.templates.clone();
    for (name, model) in loading_config.models.iter_mut() {
        if let Some(inherit_from) = &model.inherit {
            let template = templates.get(inherit_from).with_context(|| {
                format!(
                    "Model '{}' inherits from unknown template '{}'",
                    name, inherit_from
                )
            })?;

            // Apply template values to model where model has None
            if model.name.is_none() {
                model.name = template.name.clone();
            }
            if model.description.is_none() {
                model.description = template.description.clone();
            }

            // For backend, merge StableDiffusion fields
            match (&template.backend, &mut model.backend) {
                (
                    LoadingBackend::StableDiffusion {
                        checkpoint: t_checkpoint,
                        vae: t_vae,
                        cfg: t_cfg,
                        sampler: t_sampler,
                        scheduler: t_scheduler,
                        steps: t_steps,
                        resolution: t_resolution,
                    },
                    LoadingBackend::StableDiffusion {
                        checkpoint: m_checkpoint,
                        vae: m_vae,
                        cfg: m_cfg,
                        sampler: m_sampler,
                        scheduler: m_scheduler,
                        steps: m_steps,
                        resolution: m_resolution,
                    },
                ) => {
                    if m_checkpoint.is_none() {
                        *m_checkpoint = t_checkpoint.clone();
                    }
                    if m_vae.is_none() {
                        *m_vae = t_vae.clone();
                    }
                    if m_cfg.is_none() {
                        *m_cfg = *t_cfg;
                    }
                    if m_sampler.is_none() {
                        *m_sampler = t_sampler.clone();
                    }
                    if m_scheduler.is_none() {
                        *m_scheduler = t_scheduler.clone();
                    }
                    if m_steps.is_none() {
                        *m_steps = *t_steps;
                    }
                    if m_resolution.is_none() {
                        *m_resolution = *t_resolution;
                    }
                }
                (LoadingBackend::NanoBanana, LoadingBackend::StableDiffusion { .. }) => {
                    // Can't inherit from NanoBanana to StableDiffusion
                }
                (LoadingBackend::StableDiffusion { .. }, LoadingBackend::NanoBanana) => {
                    // Can't inherit from StableDiffusion to NanoBanana
                }
                (LoadingBackend::NanoBanana, LoadingBackend::NanoBanana) => {
                    // Both are NanoBanana, nothing to inherit
                }
            }
        }
    }

    // Convert from loading structs to public structs while validating
    let mut models = HashMap::new();

    for (name, loading_model) in &loading_config.models {
        let model_name = loading_model
            .name
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Model '{}' is missing required field 'name'", name))?;

        let backend = match &loading_model.backend {
            LoadingBackend::NanoBanana => Backend::NanoBanana,
            LoadingBackend::StableDiffusion {
                checkpoint,
                vae,
                cfg,
                sampler,
                scheduler,
                steps,
                resolution,
            } => {
                Backend::StableDiffusion {
                    checkpoint: checkpoint.as_ref()
                        .ok_or_else(|| anyhow::anyhow!("Model '{}' StableDiffusion backend is missing required field 'checkpoint'", name))?.clone(),
                    vae: vae.clone(),
                    cfg: cfg
                        .ok_or_else(|| anyhow::anyhow!("Model '{}' StableDiffusion backend is missing required field 'cfg'", name))?,
                    sampler: sampler.as_ref()
                        .ok_or_else(|| anyhow::anyhow!("Model '{}' StableDiffusion backend is missing required field 'sampler'", name))?.clone(),
                    scheduler: scheduler.as_ref()
                        .ok_or_else(|| anyhow::anyhow!("Model '{}' StableDiffusion backend is missing required field 'scheduler'", name))?.clone(),
                    steps: steps
                        .ok_or_else(|| anyhow::anyhow!("Model '{}' StableDiffusion backend is missing required field 'steps'", name))?,
                    resolution: resolution
                        .ok_or_else(|| anyhow::anyhow!("Model '{}' StableDiffusion backend is missing required field 'resolution'", name))?,
                }
            }
        };

        let model = Model {
            name: model_name.clone(),
            description: loading_model.description.clone(),
            backend,
        };

        models.insert(name.clone(), model);
    }

    Ok(ModelsConfig {
        default: loading_config.default,
        aliases: loading_config.aliases,
        models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_temp_models_file(content: &str) -> NamedTempFile {
        let mut temp_file = NamedTempFile::new().expect("Failed to create temp file");
        temp_file
            .write_all(content.as_bytes())
            .expect("Failed to write to temp file");
        temp_file.flush().expect("Failed to flush temp file");
        temp_file
    }

    #[test]
    fn test_simple_model_without_inheritance() {
        let content = r#"
default = "simple"

[aliases]
"alias1" = "simple"

[templates]

[models.simple]
name = "simple"
description = "A simple test model"

[models.simple.backend]
NanoBanana = {}
"#;

        let temp_file = create_temp_models_file(content);
        let config = load_models_config_from_path(temp_file.path().to_str().unwrap())
            .expect("Failed to load config");

        assert_eq!(config.default, "simple");
        assert_eq!(config.aliases.get("alias1"), Some(&"simple".to_string()));
        assert!(config.models.contains_key("simple"));

        let model = &config.models["simple"];
        assert_eq!(model.name, "simple");
        assert_eq!(model.description, Some("A simple test model".to_string()));
        assert!(matches!(model.backend, Backend::NanoBanana));
    }

    #[test]
    fn test_model_inheritance_from_template() {
        let content = r#"
default = "child"

[aliases]

[templates.base]
name = "base_template"
description = "Base template"

[templates.base.backend]
StableDiffusion = { cfg = 7.5, sampler = "euler", scheduler = "normal", steps = 25, resolution = [512, 512] }

[models.child]
name = "child_model"
inherit = "base"

[models.child.backend]
StableDiffusion = { checkpoint = "child.safetensors" }
"#;

        let temp_file = create_temp_models_file(content);
        let config = load_models_config_from_path(temp_file.path().to_str().unwrap())
            .expect("Failed to load config");

        let child_model = &config.models["child"];
        assert_eq!(child_model.name, "child_model");
        assert_eq!(child_model.description, Some("Base template".to_string())); // Inherited

        if let Backend::StableDiffusion {
            checkpoint,
            vae,
            cfg,
            sampler,
            scheduler,
            steps,
            resolution,
        } = &child_model.backend
        {
            assert_eq!(checkpoint, "child.safetensors"); // Override
            assert_eq!(vae, &None); // Not specified, should be None
            assert_eq!(cfg, &7.5); // Inherited
            assert_eq!(sampler, "euler"); // Inherited
            assert_eq!(scheduler, "normal"); // Inherited
            assert_eq!(steps, &25); // Inherited
            assert_eq!(resolution, &(512, 512)); // Inherited
        } else {
            panic!("Expected StableDiffusion backend");
        }
    }

    #[test]
    fn test_validation_missing_required_fields() {
        let content = r#"
default = "incomplete"

[aliases]

[templates]

[models.incomplete]
name = "incomplete"
description = "Missing required fields"

[models.incomplete.backend]
StableDiffusion = { checkpoint = "incomplete.safetensors" }
"#;

        let temp_file = create_temp_models_file(content);
        let result = load_models_config_from_path(temp_file.path().to_str().unwrap());

        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("missing required field"));
    }

    #[test]
    fn test_missing_template_error() {
        let content = r#"
default = "orphan"

[aliases]

[templates]

[models.orphan]
name = "orphan"
inherit = "nonexistent"

[models.orphan.backend]
NanoBanana = {}
"#;

        let temp_file = create_temp_models_file(content);
        let result = load_models_config_from_path(temp_file.path().to_str().unwrap());

        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("inherits from unknown template"));
        assert!(error_msg.contains("nonexistent"));
    }

    #[test]
    fn test_invalid_toml_syntax() {
        let content = r#"
default = "broken
[this is not valid toml
"#;

        let temp_file = create_temp_models_file(content);
        let result = load_models_config_from_path(temp_file.path().to_str().unwrap());

        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("Failed to parse"));
    }

    #[test]
    fn test_missing_name_validation() {
        let content = r#"
default = "nameless"

[aliases]

[templates]

[models.nameless]
description = "No name field"

[models.nameless.backend]
NanoBanana = {}
"#;

        let temp_file = create_temp_models_file(content);
        let result = load_models_config_from_path(temp_file.path().to_str().unwrap());

        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("missing required field 'name'"));
    }

    #[test]
    fn test_vae_field_optional() {
        let content = r#"
default = "no_vae"

[aliases]

[templates]

[models.no_vae]
name = "no_vae"
description = "VAE field should be optional"

[models.no_vae.backend]
StableDiffusion = { checkpoint = "model.safetensors", cfg = 7.0, sampler = "euler", scheduler = "normal", steps = 30, resolution = [1024, 1024] }
"#;

        let temp_file = create_temp_models_file(content);
        let config = load_models_config_from_path(temp_file.path().to_str().unwrap())
            .expect("VAE should be optional");

        let model = &config.models["no_vae"];
        if let Backend::StableDiffusion { vae, .. } = &model.backend {
            assert_eq!(vae, &None); // VAE should be None and that should be OK
        } else {
            panic!("Expected StableDiffusion backend");
        }
    }
}
