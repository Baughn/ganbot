//! Configuration for image-generation models

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, trace};

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
    pub prompt_defaults: PromptDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptDefaults {
    pub positive_prepend: Option<String>,
    pub negative_prepend: Option<String>,
    pub positive_append: Option<String>,
    pub negative_append: Option<String>,
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
        use_torch_compile: Option<bool>,
        two_stage: Option<bool>,
        upscale_factor: Option<f32>,
        stage2_denoise: Option<f32>,
        stage2_sampler: Option<String>,
        stage2_scheduler: Option<String>,
    },
}

/// Internal structs used during loading and inheritance - these can have None values
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadingModelsConfig {
    pub default: String,
    pub aliases: HashMap<String, String>,
    pub templates: HashMap<String, LoadingModel>,
    pub models: HashMap<String, LoadingModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadingModel {
    pub name: Option<String>,
    pub inherit: Option<String>,
    pub description: Option<String>,
    pub backend: Option<LoadingBackend>,
    pub prompt_defaults: Option<LoadingPromptDefaults>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadingPromptDefaults {
    pub positive_prepend: Option<String>,
    pub negative_prepend: Option<String>,
    pub positive_append: Option<String>,
    pub negative_append: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
        use_torch_compile: Option<bool>,
        two_stage: Option<bool>,
        upscale_factor: Option<f32>,
        stage2_denoise: Option<f32>,
        stage2_sampler: Option<String>,
        stage2_scheduler: Option<String>,
    },
}

/// Load model configuration from models.toml
pub fn load_models_config() -> Result<ModelsConfig> {
    load_models_config_from_path("models.toml")
}

/// Apply inheritance from parent to child LoadingModel
fn apply_inheritance(child: &mut LoadingModel, parent: &LoadingModel) -> Result<()> {
    // Apply parent values to child where child has None
    if child.name.is_none() {
        child.name = parent.name.clone();
    }
    if child.description.is_none() {
        child.description = parent.description.clone();
    }

    // Merge prompt_defaults
    match (&parent.prompt_defaults, &mut child.prompt_defaults) {
        (Some(parent_defaults), Some(child_defaults)) => {
            // Child has prompt_defaults, merge with parent
            if child_defaults.positive_prepend.is_none() {
                child_defaults.positive_prepend = parent_defaults.positive_prepend.clone();
            }
            if child_defaults.negative_prepend.is_none() {
                child_defaults.negative_prepend = parent_defaults.negative_prepend.clone();
            }
            if child_defaults.positive_append.is_none() {
                child_defaults.positive_append = parent_defaults.positive_append.clone();
            }
            if child_defaults.negative_append.is_none() {
                child_defaults.negative_append = parent_defaults.negative_append.clone();
            }
        }
        (Some(parent_defaults), None) => {
            // Child has no prompt_defaults, inherit from parent
            child.prompt_defaults = Some(parent_defaults.clone());
        }
        (None, _) => {
            // Parent has no prompt_defaults, keep child's (if any)
        }
    }

    // For backend, merge fields
    match (&parent.backend, &mut child.backend) {
        (Some(parent_backend), Some(child_backend)) => {
            // Both have backends, merge StableDiffusion fields
            match (parent_backend, child_backend) {
                (
                    LoadingBackend::StableDiffusion {
                        checkpoint: p_checkpoint,
                        vae: p_vae,
                        cfg: p_cfg,
                        sampler: p_sampler,
                        scheduler: p_scheduler,
                        steps: p_steps,
                        resolution: p_resolution,
                        use_torch_compile: p_use_torch_compile,
                        two_stage: p_two_stage,
                        upscale_factor: p_upscale_factor,
                        stage2_denoise: p_stage2_denoise,
                        stage2_sampler: p_stage2_sampler,
                        stage2_scheduler: p_stage2_scheduler,
                    },
                    LoadingBackend::StableDiffusion {
                        checkpoint: c_checkpoint,
                        vae: c_vae,
                        cfg: c_cfg,
                        sampler: c_sampler,
                        scheduler: c_scheduler,
                        steps: c_steps,
                        resolution: c_resolution,
                        use_torch_compile: c_use_torch_compile,
                        two_stage: c_two_stage,
                        upscale_factor: c_upscale_factor,
                        stage2_denoise: c_stage2_denoise,
                        stage2_sampler: c_stage2_sampler,
                        stage2_scheduler: c_stage2_scheduler,
                    },
                ) => {
                    if c_checkpoint.is_none() {
                        *c_checkpoint = p_checkpoint.clone();
                    }
                    if c_vae.is_none() {
                        *c_vae = p_vae.clone();
                    }
                    if c_cfg.is_none() {
                        *c_cfg = *p_cfg;
                    }
                    if c_sampler.is_none() {
                        *c_sampler = p_sampler.clone();
                    }
                    if c_scheduler.is_none() {
                        *c_scheduler = p_scheduler.clone();
                    }
                    if c_steps.is_none() {
                        *c_steps = *p_steps;
                    }
                    if c_resolution.is_none() {
                        *c_resolution = *p_resolution;
                    }
                    if c_use_torch_compile.is_none() {
                        *c_use_torch_compile = *p_use_torch_compile;
                    }
                    if c_two_stage.is_none() {
                        *c_two_stage = *p_two_stage;
                    }
                    if c_upscale_factor.is_none() {
                        *c_upscale_factor = *p_upscale_factor;
                    }
                    if c_stage2_denoise.is_none() {
                        *c_stage2_denoise = *p_stage2_denoise;
                    }
                    if c_stage2_sampler.is_none() {
                        *c_stage2_sampler = p_stage2_sampler.clone();
                    }
                    if c_stage2_scheduler.is_none() {
                        *c_stage2_scheduler = p_stage2_scheduler.clone();
                    }
                }
                (LoadingBackend::NanoBanana, LoadingBackend::StableDiffusion { .. }) => {
                    bail!("Can't inherit from NanoBanana to StableDiffusion")
                }
                (LoadingBackend::StableDiffusion { .. }, LoadingBackend::NanoBanana) => {
                    bail!("Can't inherit from StableDiffusion to NanoBanana")
                }
                (LoadingBackend::NanoBanana, LoadingBackend::NanoBanana) => {
                    // Both are NanoBanana, nothing to inherit
                }
            }
        }
        (Some(parent_backend), None) => {
            // Child has no backend, inherit parent's entirely
            child.backend = Some(parent_backend.clone());
        }
        (None, _) => {
            // Parent has no backend, keep child's (if any)
        }
    }

    Ok(())
}

/// Load model configuration from a specific path (used for testing)
pub fn load_models_config_from_path(path: &str) -> Result<ModelsConfig> {
    info!("Loading models configuration from {}", path);
    let content =
        std::fs::read_to_string(path).with_context(|| format!("Failed to read {}", path))?;

    let mut loading_config: LoadingModelsConfig =
        toml::from_str(&content).with_context(|| format!("Failed to parse {}", path))?;

    // First, apply template-to-template inheritance
    let mut resolved_templates = loading_config.templates.clone();
    for (name, template) in resolved_templates.iter_mut() {
        if let Some(inherit_from) = &template.inherit {
            debug!("Template '{}' inheriting from '{}'", name, inherit_from);
            let parent_template =
                loading_config
                    .templates
                    .get(inherit_from)
                    .with_context(|| {
                        format!(
                            "Template '{}' inherits from unknown template '{}'",
                            name, inherit_from
                        )
                    })?;

            apply_inheritance(template, parent_template)?;
        }
    }

    // Apply inheritance - models inherit from templates
    let templates = resolved_templates;
    for (name, model) in loading_config.models.iter_mut() {
        debug!("Processing model '{}'", name);
        if let Some(inherit_from) = &model.inherit {
            debug!("  Inheriting from '{}'", inherit_from);
            let template = templates.get(inherit_from).with_context(|| {
                format!(
                    "Model '{}' inherits from unknown template '{}'",
                    name, inherit_from
                )
            })?;
            trace!("  Template: {:?}", template);

            apply_inheritance(model, template)?;
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
            Some(backend) => match backend {
                LoadingBackend::NanoBanana => Backend::NanoBanana,
                LoadingBackend::StableDiffusion {
                    checkpoint,
                    vae,
                    cfg,
                    sampler,
                    scheduler,
                    steps,
                    resolution,
                    use_torch_compile,
                    two_stage,
                    upscale_factor,
                    stage2_denoise,
                    stage2_sampler,
                    stage2_scheduler,
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
                        use_torch_compile: use_torch_compile.clone(),
                        two_stage: two_stage.clone(),
                        upscale_factor: upscale_factor.clone(),
                        stage2_denoise: stage2_denoise.clone(),
                        stage2_sampler: stage2_sampler.clone(),
                        stage2_scheduler: stage2_scheduler.clone(),
                    }
                }
            }
            None => {
                return Err(anyhow::anyhow!("Model '{}' has no backend after inheritance processing", name));
            }
        };

        let prompt_defaults = match &loading_model.prompt_defaults {
            Some(loading_defaults) => PromptDefaults {
                positive_prepend: loading_defaults.positive_prepend.clone(),
                negative_prepend: loading_defaults.negative_prepend.clone(),
                positive_append: loading_defaults.positive_append.clone(),
                negative_append: loading_defaults.negative_append.clone(),
            },
            None => PromptDefaults {
                positive_prepend: None,
                negative_prepend: None,
                positive_append: None,
                negative_append: None,
            },
        };

        let model = Model {
            name: model_name.clone(),
            description: loading_model.description.clone(),
            backend,
            prompt_defaults,
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
            use_torch_compile,
            two_stage,
            upscale_factor,
            stage2_denoise,
            stage2_sampler,
            stage2_scheduler,
        } = &child_model.backend
        {
            assert_eq!(checkpoint, "child.safetensors"); // Override
            assert_eq!(vae, &None); // Not specified, should be None
            assert_eq!(cfg, &7.5); // Inherited
            assert_eq!(sampler, "euler"); // Inherited
            assert_eq!(scheduler, "normal"); // Inherited
            assert_eq!(steps, &25); // Inherited
            assert_eq!(resolution, &(512, 512)); // Inherited
            assert_eq!(use_torch_compile, &None); // Not specified
            assert_eq!(two_stage, &None); // Not specified
            assert_eq!(upscale_factor, &None); // Not specified
            assert_eq!(stage2_denoise, &None); // Not specified
            assert_eq!(stage2_sampler, &None); // Not specified
            assert_eq!(stage2_scheduler, &None); // Not specified
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
