//! Configuration for image-generation models

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, trace};

/// Public API structs - guaranteed to have all required fields populated
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    pub default: String,
    pub default_english: String,
    pub default_tagged: String,
    pub aliases: HashMap<String, String>,
    pub models: HashMap<String, Model>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub backend: Backend,
    pub prompt_defaults: PromptDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptDefaults {
    pub positive_prepend: Option<String>,
    pub negative_prepend: Option<String>,
    pub positive_append: Option<String>,
    pub negative_append: Option<String>,
    pub count: Option<u32>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Backend {
    NanoBanana,
    ComfyUI {
        checkpoint: Checkpoint,
        cfg: f32,
        sampler: String,
        scheduler: String,
        steps: u32,
        resolution: (u32, u32),
        resolutions: Option<Vec<(u32, u32)>>,
        use_torch_compile: Option<bool>,
        two_stage: Option<bool>,
        upscale_factor: Option<f32>,
        stage2_denoise: Option<f32>,
        stage2_sampler: Option<String>,
        stage2_scheduler: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum Checkpoint {
    Combined(String),
    Split {
        unet: String,
        vae: String,
        clip: String,
    },
}

/// Internal structs used during loading and inheritance - these can have None values
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadingModelsConfig {
    pub default: String,
    pub default_english: String,
    pub default_tagged: String,
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
    pub tags: Option<Vec<String>>,
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
    pub count: Option<u32>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum LoadingBackend {
    NanoBanana,
    ComfyUI {
        checkpoint: Option<String>,
        unet: Option<String>,
        vae: Option<String>,
        clip: Option<String>,
        cfg: Option<f32>,
        sampler: Option<String>,
        scheduler: Option<String>,
        steps: Option<u32>,
        resolution: Option<(u32, u32)>,
        resolutions: Option<Vec<String>>,
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
    inherit_if_none(&mut child.name, &parent.name);
    inherit_if_none(&mut child.description, &parent.description);
    inherit_if_none(&mut child.tags, &parent.tags);

    // Merge prompt_defaults
    match (&parent.prompt_defaults, &mut child.prompt_defaults) {
        (Some(parent_defaults), Some(child_defaults)) => {
            child_defaults.inherit_from(parent_defaults);
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
            child_backend.inherit_from(parent_backend)?;
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

fn inherit_if_none<T: Clone>(child: &mut Option<T>, parent: &Option<T>) {
    if child.is_none() {
        *child = parent.clone();
    }
}

impl LoadingPromptDefaults {
    fn inherit_from(&mut self, parent: &Self) {
        inherit_if_none(&mut self.positive_prepend, &parent.positive_prepend);
        inherit_if_none(&mut self.negative_prepend, &parent.negative_prepend);
        inherit_if_none(&mut self.positive_append, &parent.positive_append);
        inherit_if_none(&mut self.negative_append, &parent.negative_append);
    }
}

impl LoadingBackend {
    fn inherit_from(&mut self, parent: &Self) -> Result<()> {
        match (parent, self) {
            (
                LoadingBackend::ComfyUI {
                    checkpoint: p_checkpoint,
                    unet: p_unet,
                    vae: p_vae,
                    clip: p_clip,
                    cfg: p_cfg,
                    sampler: p_sampler,
                    scheduler: p_scheduler,
                    steps: p_steps,
                    resolution: p_resolution,
                    resolutions: p_resolutions,
                    use_torch_compile: p_use_torch_compile,
                    two_stage: p_two_stage,
                    upscale_factor: p_upscale_factor,
                    stage2_denoise: p_stage2_denoise,
                    stage2_sampler: p_stage2_sampler,
                    stage2_scheduler: p_stage2_scheduler,
                },
                LoadingBackend::ComfyUI {
                    checkpoint: c_checkpoint,
                    unet: c_unet,
                    vae: c_vae,
                    clip: c_clip,
                    cfg: c_cfg,
                    sampler: c_sampler,
                    scheduler: c_scheduler,
                    steps: c_steps,
                    resolution: c_resolution,
                    resolutions: c_resolutions,
                    use_torch_compile: c_use_torch_compile,
                    two_stage: c_two_stage,
                    upscale_factor: c_upscale_factor,
                    stage2_denoise: c_stage2_denoise,
                    stage2_sampler: c_stage2_sampler,
                    stage2_scheduler: c_stage2_scheduler,
                },
            ) => {
                inherit_if_none(c_checkpoint, p_checkpoint);
                inherit_if_none(c_unet, p_unet);
                inherit_if_none(c_clip, p_clip);
                inherit_if_none(c_vae, p_vae);
                inherit_if_none(c_cfg, p_cfg);
                inherit_if_none(c_sampler, p_sampler);
                inherit_if_none(c_scheduler, p_scheduler);
                inherit_if_none(c_steps, p_steps);
                inherit_if_none(c_resolution, p_resolution);
                inherit_if_none(c_resolutions, p_resolutions);
                inherit_if_none(c_use_torch_compile, p_use_torch_compile);
                inherit_if_none(c_two_stage, p_two_stage);
                inherit_if_none(c_upscale_factor, p_upscale_factor);
                inherit_if_none(c_stage2_denoise, p_stage2_denoise);
                inherit_if_none(c_stage2_sampler, p_stage2_sampler);
                inherit_if_none(c_stage2_scheduler, p_stage2_scheduler);
                Ok(())
            }
            (LoadingBackend::NanoBanana, LoadingBackend::ComfyUI { .. }) => {
                bail!("Can't inherit from NanoBanana to ComfyUI")
            }
            (LoadingBackend::ComfyUI { .. }, LoadingBackend::NanoBanana) => {
                bail!("Can't inherit from ComfyUI to NanoBanana")
            }
            (LoadingBackend::NanoBanana, LoadingBackend::NanoBanana) => {
                // Both are NanoBanana, nothing to inherit
                Ok(())
            }
        }
    }
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
                LoadingBackend::ComfyUI {
                    checkpoint,
                    unet,
                    vae,
                    clip,
                    cfg,
                    sampler,
                    scheduler,
                    steps,
                    resolution,
                    resolutions,
                    use_torch_compile,
                    two_stage,
                    upscale_factor,
                    stage2_denoise,
                    stage2_sampler,
                    stage2_scheduler,
                } => Backend::ComfyUI {
                    checkpoint: match (checkpoint, unet, vae, clip) {
                        (Some(ckpt), None, None, None) => Checkpoint::Combined(ckpt.clone()),
                        (None, Some(u), Some(v), Some(c)) => Checkpoint::Split {
                            unet: u.clone(),
                            vae: v.clone(),
                            clip: c.clone(),
                        },
                        _ => {
                            return Err(anyhow::anyhow!(
                                "Model '{}' ComfyUI backend must have either 'checkpoint' or all of 'unet', 'vae', and 'clip'",
                                name
                            ));
                        }
                    },
                    cfg: cfg.ok_or_else(|| {
                        anyhow::anyhow!(
                            "Model '{}' ComfyUI backend is missing required field 'cfg'",
                            name
                        )
                    })?,
                    sampler: sampler
                        .as_ref()
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Model '{}' ComfyUI backend is missing required field 'sampler'",
                                name
                            )
                        })?
                        .clone(),
                    scheduler: scheduler
                        .as_ref()
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Model '{}' ComfyUI backend is missing required field 'scheduler'",
                                name
                            )
                        })?
                        .clone(),
                    steps: steps.ok_or_else(|| {
                        anyhow::anyhow!(
                            "Model '{}' ComfyUI backend is missing required field 'steps'",
                            name
                        )
                    })?,
                    resolution: resolution.ok_or_else(|| {
                        anyhow::anyhow!(
                            "Model '{}' ComfyUI backend is missing required field 'resolution'",
                            name
                        )
                    })?,
                    resolutions: if let Some(res_strings) = resolutions {
                        let parsed_resolutions: Result<Vec<(u32, u32)>, _> = res_strings
                            .iter()
                            .map(|res_str| {
                                let parts: Vec<&str> = res_str.split('x').collect();
                                if parts.len() != 2 {
                                    return Err(anyhow::anyhow!(
                                        "Invalid resolution format '{}', expected 'WIDTHxHEIGHT'",
                                        res_str
                                    ));
                                }
                                let width: u32 = parts[0].parse().with_context(|| {
                                    format!("Invalid width in resolution '{}'", res_str)
                                })?;
                                let height: u32 = parts[1].parse().with_context(|| {
                                    format!("Invalid height in resolution '{}'", res_str)
                                })?;
                                Ok((width, height))
                            })
                            .collect();
                        Some(parsed_resolutions.with_context(|| {
                            format!("Failed to parse resolutions for model '{}'", name)
                        })?)
                    } else {
                        None
                    },
                    use_torch_compile: *use_torch_compile,
                    two_stage: *two_stage,
                    upscale_factor: *upscale_factor,
                    stage2_denoise: *stage2_denoise,
                    stage2_sampler: stage2_sampler.clone(),
                    stage2_scheduler: stage2_scheduler.clone(),
                },
            },
            None => {
                return Err(anyhow::anyhow!(
                    "Model '{}' has no backend after inheritance processing",
                    name
                ));
            }
        };

        let prompt_defaults = match &loading_model.prompt_defaults {
            Some(loading_defaults) => PromptDefaults {
                positive_prepend: loading_defaults.positive_prepend.clone(),
                negative_prepend: loading_defaults.negative_prepend.clone(),
                positive_append: loading_defaults.positive_append.clone(),
                negative_append: loading_defaults.negative_append.clone(),
                count: loading_defaults.count,
            },
            None => PromptDefaults {
                positive_prepend: None,
                negative_prepend: None,
                positive_append: None,
                negative_append: None,
                count: None,
            },
        };

        let model = Model {
            name: model_name.clone(),
            description: loading_model.description.clone(),
            tags: loading_model.tags.clone().unwrap_or_default(),
            backend,
            prompt_defaults,
        };

        models.insert(name.clone(), model);
    }

    Ok(ModelsConfig {
        default: loading_config.default,
        default_english: loading_config.default_english,
        default_tagged: loading_config.default_tagged,
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
default_english = "simple"
default_tagged = "simple"

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
        assert!(model.tags.is_empty());
        assert!(matches!(model.backend, Backend::NanoBanana));
    }

    #[test]
    fn test_model_inheritance_from_template() {
        let content = r#"
default = "child"
default_english = "child"
default_tagged = "child"

[aliases]

[templates.base]
name = "base_template"
description = "Base template"

[templates.base.backend]
ComfyUI = { cfg = 7.5, sampler = "euler", scheduler = "normal", steps = 25, resolution = [512, 512] }

[models.child]
name = "child_model"
inherit = "base"

[models.child.backend]
ComfyUI = { checkpoint = "child.safetensors" }
"#;

        let temp_file = create_temp_models_file(content);
        let config = load_models_config_from_path(temp_file.path().to_str().unwrap())
            .expect("Failed to load config");

        let child_model = &config.models["child"];
        assert_eq!(child_model.name, "child_model");
        assert_eq!(child_model.description, Some("Base template".to_string())); // Inherited
        assert!(child_model.tags.is_empty());

        if let Backend::ComfyUI {
            checkpoint,
            cfg,
            sampler,
            scheduler,
            steps,
            resolution,
            resolutions: _,
            use_torch_compile,
            two_stage,
            upscale_factor,
            stage2_denoise,
            stage2_sampler,
            stage2_scheduler,
        } = &child_model.backend
        {
            assert_eq!(
                checkpoint,
                &Checkpoint::Combined("child.safetensors".to_string())
            );
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
            panic!("Expected ComfyUI backend");
        }
    }

    #[test]
    fn test_validation_missing_required_fields() {
        let content = r#"
default = "incomplete"
default_english = "incomplete"
default_tagged = "incomplete"

[aliases]

[templates]

[models.incomplete]
name = "incomplete"
description = "Missing required fields"

[models.incomplete.backend]
ComfyUI = { checkpoint = "incomplete.safetensors" }
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
default_english = "orphan"
default_tagged = "orphan"

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
default_english = "nameless"
default_tagged = "nameless"

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
    fn test_actual_models_toml_parses() {
        let result = load_models_config();
        assert!(
            result.is_ok(),
            "Failed to parse actual models.toml: {:?}",
            result.unwrap_err()
        );
    }
}
