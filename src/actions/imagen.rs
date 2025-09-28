use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use image::RgbImage;
use kameo::{Actor, Reply, actor::ActorRef, message::Context, prelude::Message};
use rand::RngCore as _;
use tracing::{debug, info, trace};

use crate::{
    actions::ActionProgressEmitter,
    config::models::{self, Model, ModelsConfig},
    fuzzy::{FuzzyResult, find_fuzzy_match},
    messages::imagen::Generate,
    network::{
        comfyui::{self, api::KSamplerParams, net::ComfyUIClient},
        openrouter::OpenRouter,
    },
    persistence::user::{GetAlias, GetDefaultPrompt, UserActor},
};

/// Actor responsible for running image generation backends without performing persistence.
#[derive(Actor, Default)]
pub struct ImagenActor;

/// Message to run an image generation request against the resolved model backend.
pub struct GenerateImages {
    pub prompt: Generate,
    pub model: Model,
    pub progress: Option<ActionProgressEmitter>,
}

/// Backend classification for generated images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImagenBackend {
    NanoBanana,
    StableDiffusion,
}

impl ImagenBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NanoBanana => "NanoBanana",
            Self::StableDiffusion => "StableDiffusion",
        }
    }
}

/// Raw image generation result returned to higher-level actions for processing.
#[derive(Reply, Debug)]
pub struct ImagenResponse {
    pub images: Vec<Arc<RgbImage>>,
    pub text: Option<String>,
    pub workflow: Option<serde_json::Value>,
    pub backend: ImagenBackend,
    pub model_name: String,
    pub seed: Option<u64>,
}

impl Message<GenerateImages> for ImagenActor {
    type Reply = Result<ImagenResponse>;

    async fn handle(
        &mut self,
        msg: GenerateImages,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let GenerateImages {
            prompt,
            model,
            progress,
        } = msg;

        match &model.backend {
            models::Backend::NanoBanana => generate_nanobanana(prompt, &model).await,
            models::Backend::ComfyUI {
                checkpoint,
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
            } => {
                let count = prompt
                    .num_images
                    .unwrap_or(model.prompt_defaults.count.unwrap_or(2))
                    .clamp(1, 6);

                let params = ComfyParams {
                    prompt,
                    model_name: &model.name,
                    count,
                    checkpoint,
                    cfg: *cfg,
                    sampler: sampler.as_str(),
                    scheduler: scheduler.as_str(),
                    steps: *steps,
                    resolution: *resolution,
                    resolutions: resolutions.as_ref(),
                    use_torch_compile: use_torch_compile.unwrap_or(false),
                    two_stage: two_stage.unwrap_or(false),
                    upscale_factor: upscale_factor.unwrap_or(1.5),
                    stage2_denoise: stage2_denoise.unwrap_or(0.5),
                    stage2_sampler: stage2_sampler.as_deref().unwrap_or("euler"),
                    stage2_scheduler: stage2_scheduler.as_deref().unwrap_or("beta"),
                };

                generate_comfyui(params, progress.as_ref()).await
            }
        }
    }
}

/// Apply user-configured defaults and aliases to a prompt prior to generation.
pub async fn hydrate_prompt(
    mut prompt: Generate,
    user_actor: &ActorRef<UserActor>,
) -> Result<Generate> {
    let default_prompt = user_actor
        .ask(GetDefaultPrompt)
        .await
        .context("Failed to get default prompt")?
        .map(|base_str| Generate::from_str(&base_str))
        .transpose()
        .context("Failed to parse default prompt")?;

    // Pick an --alias from the prompt, or else from the default prompt if that has one.
    let alias = prompt
        .alias
        .clone()
        .or(default_prompt.as_ref().and_then(|d| d.alias.clone()));

    if let Some(alias_name) = &alias {
        let alias_value = user_actor
            .ask(GetAlias(alias_name.clone()))
            .await
            .context("Failed to get alias")?;

        let alias_str = alias_value.ok_or_else(|| {
            anyhow::anyhow!(
                "Alias '{}' not found. Use !config alias {} [settings] to create it.",
                alias_name,
                alias_name
            )
        })?;

        let alias_prompt = Generate::from_str(&alias_str)
            .with_context(|| format!("Failed to parse alias '{}'", alias_name))?;
        prompt = merge_prompt_settings(prompt, alias_prompt);
        debug!("Merged alias settings into prompt");
    }

    if let Some(default_prompt) = default_prompt {
        prompt = merge_prompt_settings(prompt, default_prompt);
        debug!("Merged default settings into prompt");
    }

    debug!("Final prompt: {prompt:?}");
    Ok(prompt)
}

/// Apply model-specific defaults (prepend/append text) to the prompt.
pub fn apply_model_defaults(prompt: &mut Generate, model: &Model) {
    if let Some(prepend) = &model.prompt_defaults.positive_prepend {
        prompt.prompt = format!("{}. {}", prepend, prompt.prompt);
    }
    if let Some(append) = &model.prompt_defaults.positive_append {
        prompt.prompt = format!("{}. {}", prompt.prompt, append);
    }
    if let Some(neg_prepend) = &model.prompt_defaults.negative_prepend {
        prompt.negative_prompt = Some(format!(
            "{}. {}",
            neg_prepend,
            prompt.negative_prompt.clone().unwrap_or_default()
        ));
    }
    if let Some(neg_append) = &model.prompt_defaults.negative_append {
        prompt.negative_prompt = Some(format!(
            "{}. {}",
            prompt.negative_prompt.clone().unwrap_or_default(),
            neg_append
        ));
    }
}

/// Merge prompt settings from `base` into `prompt`, allowing `prompt` to override.
pub fn merge_prompt_settings(mut prompt: Generate, base: Generate) -> Generate {
    let Generate {
        prompt: base_prompt,
        negative_prompt: base_negative,
        num_images: base_num_images,
        aspect: base_aspect,
        width: base_width,
        height: base_height,
        model: base_model,
        seed: base_seed,
        steps: base_steps,
        ..
    } = base;

    if !base_prompt.trim().is_empty() {
        prompt.prompt = append_clause(&prompt.prompt, &base_prompt, ". ");
    }

    if let Some(base_negative) = base_negative {
        if !base_negative.trim().is_empty() {
            let combined_negative = match prompt.negative_prompt.take() {
                Some(existing_negative) if !existing_negative.trim().is_empty() => {
                    append_clause(&existing_negative, &base_negative, ", ")
                }
                _ => base_negative.trim().to_string(),
            };
            prompt.negative_prompt = Some(combined_negative);
        }
    }

    if prompt.seed.is_none() {
        prompt.seed = base_seed;
    }
    if prompt.width.is_none() {
        prompt.width = base_width;
    }
    if prompt.height.is_none() {
        prompt.height = base_height;
    }
    if prompt.aspect.is_none() {
        prompt.aspect = base_aspect;
    }
    if prompt.steps.is_none() {
        prompt.steps = base_steps;
    }
    if prompt.model.is_none() {
        prompt.model = base_model;
    }
    if prompt.num_images.is_none() {
        prompt.num_images = base_num_images;
    }

    prompt
}

fn append_clause(existing: &str, addition: &str, default_separator: &str) -> String {
    let existing_trimmed = existing.trim();
    let addition_trimmed = addition.trim();

    if existing_trimmed.is_empty() {
        return addition_trimmed.to_string();
    }
    if addition_trimmed.is_empty() {
        return existing_trimmed.to_string();
    }

    let separator = match existing_trimmed.chars().rev().find(|c| !c.is_whitespace()) {
        Some('.') | Some('!') | Some('?') | Some(',') => " ",
        _ => default_separator,
    };

    let mut combined = existing_trimmed.to_string();
    combined.push_str(separator);
    combined.push_str(addition_trimmed);
    combined
}

#[cfg(test)]
mod tests {
    use super::merge_prompt_settings;
    use crate::messages::imagen::{Generate, References};

    fn generate_with_prompt(prompt: &str) -> Generate {
        Generate {
            raw_prompt: prompt.to_string(),
            prompt: prompt.to_string(),
            negative_prompt: None,
            num_images: None,
            aspect: None,
            width: None,
            height: None,
            model: None,
            seed: None,
            steps: None,
            references: References {
                img2img: None,
                img2img_strength: None,
                context: Vec::new(),
            },
            alias: None,
        }
    }

    #[test]
    fn defaults_trail_user_prompt() {
        let prompt = generate_with_prompt("A wizard reading");
        let base = generate_with_prompt("best quality, trending on artstation");

        let merged = merge_prompt_settings(prompt, base);

        assert_eq!(
            merged.prompt,
            "A wizard reading. best quality, trending on artstation"
        );
    }

    #[test]
    fn base_prompt_replaces_when_user_prompt_empty() {
        let prompt = generate_with_prompt("");
        let base = generate_with_prompt("photorealistic portrait");

        let merged = merge_prompt_settings(prompt, base);

        assert_eq!(merged.prompt, "photorealistic portrait");
    }

    #[test]
    fn negative_prompts_are_concatenated_with_separator() {
        let mut prompt = generate_with_prompt("A wizard reading");
        prompt.negative_prompt = Some("nsfw, lowres".to_string());

        let mut base = generate_with_prompt("Defaults");
        base.negative_prompt = Some("bad anatomy".to_string());

        let merged = merge_prompt_settings(prompt, base);

        assert_eq!(
            merged.negative_prompt.as_deref(),
            Some("nsfw, lowres, bad anatomy")
        );
    }

    #[test]
    fn base_negative_prompt_used_when_user_prompt_is_blank() {
        let mut prompt = generate_with_prompt("User prompt");
        prompt.negative_prompt = Some("   ".to_string());

        let mut base = generate_with_prompt("Defaults");
        base.negative_prompt = Some("blurry, watermark".to_string());

        let merged = merge_prompt_settings(prompt, base);

        assert_eq!(merged.negative_prompt.as_deref(), Some("blurry, watermark"));
    }

    #[test]
    fn base_values_act_as_fallbacks() {
        let mut prompt = generate_with_prompt("A wizard reading");
        prompt.width = Some(1024);
        prompt.height = None;
        prompt.steps = None;
        prompt.model = Some("user-model".to_string());
        prompt.num_images = None;
        prompt.seed = Some(1234);
        prompt.aspect = Some((16, 9));

        let mut base = generate_with_prompt("Defaults");
        base.width = Some(512);
        base.height = Some(768);
        base.steps = Some(25);
        base.model = Some("base-model".to_string());
        base.num_images = Some(3);
        base.seed = Some(9999);
        base.aspect = Some((1, 1));

        let merged = merge_prompt_settings(prompt, base);

        assert_eq!(merged.width, Some(1024));
        assert_eq!(merged.height, Some(768));
        assert_eq!(merged.steps, Some(25));
        assert_eq!(merged.model.as_deref(), Some("user-model"));
        assert_eq!(merged.num_images, Some(3));
        assert_eq!(merged.seed, Some(1234));
        assert_eq!(merged.aspect, Some((16, 9)));
    }
}

/// Resolve the requested model token to actual configuration, with fuzzy matching support.
pub fn resolve_model(
    prompt_text: &str,
    config: &ModelsConfig,
    target_model: Option<&str>,
) -> Result<(Model, Option<String>)> {
    let mut resolved_model_name = target_model.map(|s| s.to_string()).unwrap_or_else(|| {
        if prompt_text.to_lowercase().contains("english") {
            config.default_english.clone()
        } else {
            config.default.clone()
        }
    });

    if let Some(alias_target) = config.aliases.get(&resolved_model_name) {
        resolved_model_name = alias_target.clone();
    }

    if resolved_model_name == "auto" {
        resolved_model_name = select_auto_model(prompt_text, config);
    }

    let mut candidates: Vec<(&str, &Model)> = config
        .models
        .iter()
        .map(|(name, model)| (name.as_str(), model))
        .collect();

    for (alias, target) in &config.aliases {
        if let Some(model) = config.models.get(target) {
            candidates.push((alias.as_str(), model));
        }
    }

    let fuzzy_result = find_fuzzy_match(&resolved_model_name, candidates);

    match fuzzy_result {
        FuzzyResult::Exact(model) => Ok((model.clone(), None)),
        FuzzyResult::Corrected {
            corrected,
            original,
        } => {
            let message = format!(
                "Corrected model name '{}' to '{}'",
                original, corrected.name
            );
            Ok((corrected.clone(), Some(message)))
        }
        FuzzyResult::Suggestions {
            candidates,
            original,
        } => {
            let suggestions: Vec<String> = candidates
                .into_iter()
                .map(|model| model.name.clone())
                .collect();
            bail!(
                "Model '{}' not found. Did you mean: {}?",
                original,
                suggestions.join(", ")
            )
        }
        FuzzyResult::NotFound { original } => {
            bail!("Model '{}' not found in configuration", original)
        }
    }
}

fn select_auto_model(prompt_text: &str, config: &ModelsConfig) -> String {
    let prompt_len = prompt_text.len().max(1);
    let comma_count = prompt_text.matches(',').count();
    let commacity = comma_count as f32 / prompt_len as f32;

    let selected_model = if commacity >= 0.04 {
        config.default_tagged.clone()
    } else {
        config.default_english.clone()
    };

    info!(
        "Auto-selected model '{}' based on prompt commacity of {:.3}",
        selected_model, commacity
    );

    selected_model
}

async fn generate_nanobanana(prompt: Generate, model: &Model) -> Result<ImagenResponse> {
    let formatted_prompt = if prompt.references.img2img.is_some() {
        format!(
            "Edit this image according to these instructions: {}\nAlways generate an edited image. In addition to the image, comment on the changes in the style of a hard-boiled noir detective.",
            prompt.prompt
        )
    } else {
        format!(
            "Generate an image: {}\nAlways generate an image. In addition to the image, comment on it in the style of a hard-boiled noir detective.",
            prompt.prompt
        )
    };

    let router = OpenRouter::get().context("while fetching OpenRouter instance")?;

    let response = router
        .ask(crate::messages::chat::NanoBanana {
            origin: "prompt command".to_string(),
            prompt: formatted_prompt.clone(),
            input_image: prompt.references.img2img.clone(),
        })
        .await
        .context("while generating response with NanoBanana")?;

    let workflow = serde_json::json!({
        "model": model.name,
        "original_prompt": prompt.prompt,
        "raw_prompt": prompt.raw_prompt,
        "formatted_prompt": formatted_prompt,
        "timestamp": chrono::Utc::now().to_rfc3339()
    });

    let images = response
        .image
        .map(|img| vec![Arc::new(img)])
        .unwrap_or_default();

    Ok(ImagenResponse {
        images,
        text: Some(response.text),
        workflow: Some(workflow),
        backend: ImagenBackend::NanoBanana,
        model_name: model.name.clone(),
        seed: None,
    })
}

struct ComfyParams<'a> {
    prompt: Generate,
    model_name: &'a str,
    count: u32,
    checkpoint: &'a models::Checkpoint,
    cfg: f32,
    sampler: &'a str,
    scheduler: &'a str,
    steps: u32,
    resolution: (u32, u32),
    resolutions: Option<&'a Vec<(u32, u32)>>,
    use_torch_compile: bool,
    two_stage: bool,
    upscale_factor: f32,
    stage2_denoise: f32,
    stage2_sampler: &'a str,
    stage2_scheduler: &'a str,
}

async fn generate_comfyui(
    params: ComfyParams<'_>,
    progress: Option<&ActionProgressEmitter>,
) -> Result<ImagenResponse> {
    let progress_emitter = progress.cloned();
    if let Some(emitter) = &progress_emitter {
        emitter.progress(Some(0.0), "Preparing generation workflow…");
    }

    let client = ComfyUIClient::new();
    let mut graph = comfyui::api::Graph::new();

    let prompt = params.prompt;

    let two_stage = params.two_stage && prompt.references.img2img.is_none();
    let seed = prompt.seed.unwrap_or(rand::rng().next_u64());
    let num_images = params.count;
    let mut width = params.resolution.0;
    let mut height = params.resolution.1;

    if let Some(w) = prompt.width {
        width = w;
    }
    if let Some(h) = prompt.height {
        height = h;
    }

    if prompt.width.is_some() || prompt.height.is_some() {
        width = width.clamp(256, 2048);
        height = height.clamp(256, 2048);
        trace!("Using user-specified dimensions: {}x{}", width, height);
    } else {
        let aspect = prompt.aspect.unwrap_or((1, 1));

        if let Some(resolutions) = params.resolutions {
            let (selected_width, selected_height) = find_best_resolution(aspect, resolutions);
            debug!(
                "Selected resolution {}x{} from allowed set for aspect ratio {:?}",
                selected_width, selected_height, aspect
            );
            width = selected_width;
            height = selected_height;
        } else {
            trace!("Calculating dimensions for aspect ratio {:?}", aspect);
            (width, height) = calculate_dimensions(aspect, width, height);
            width = width.clamp(256, 2048);
            height = height.clamp(256, 2048);
            trace!("Calculated dimensions: {}x{}", width, height);
        }
    }

    let steps = prompt.steps.unwrap_or(params.steps).clamp(1, 150);

    let (mut model, clip, vae) = match params.checkpoint {
        models::Checkpoint::Combined(name) => graph.checkpoint_loader(name),
        models::Checkpoint::Split { unet, clip, vae } => {
            let clip_type = match unet.split('/').next().unwrap() {
                "qwen" => "qwen_image",
                _ => bail!("Unknown CLIP type for checkpoint: {}", unet),
            };
            (
                graph.unet_loader(unet),
                graph.clip_loader_with_type(clip, clip_type),
                graph.vae_loader(vae),
            )
        }
    };

    if params.use_torch_compile {
        model = graph.torch_compile_model(&model, "inductor");
    }

    let positive = graph.clip_text_encode(&clip, &prompt.prompt);
    let negative =
        graph.clip_text_encode(&clip, &prompt.negative_prompt.clone().unwrap_or_default());

    let latent = if let Some(input_image) = prompt.references.img2img.as_ref() {
        info!(
            "Using img2img mode with input image: {}x{}",
            input_image.width(),
            input_image.height()
        );

        if let Some(strength) = prompt.references.img2img_strength {
            if !(0.0..=1.0).contains(&strength) {
                bail!("--denoise parameter must be between 0.0 and 1.0");
            }
        } else {
            bail!("--denoise parameter is required for img2img generation");
        }

        let loaded_image = graph.load_image_from_rgb(input_image);
        graph.vae_encode(&vae, &loaded_image)
    } else {
        graph.empty_latent_image(width, height, num_images)
    };

    let denoise = if let Some(strength) = prompt.references.img2img_strength {
        strength.clamp(0.0, 1.0)
    } else {
        1.0
    };
    trace!("Using denoise strength: {}", denoise);

    let final_samples = if two_stage {
        let stage1_params = KSamplerParams {
            sampler: params.sampler.to_string(),
            scheduler: params.scheduler.to_string(),
            steps: steps / 2,
            cfg: params.cfg,
            seed,
            denoise,
        };
        let stage1_samples = graph.ksampler(&model, &positive, &negative, &latent, stage1_params);

        let upscaled_latent = graph.latent_upscaler(&stage1_samples, "SDXL", params.upscale_factor);

        let stage2_params = KSamplerParams {
            sampler: params.stage2_sampler.to_string(),
            scheduler: params.stage2_scheduler.to_string(),
            steps: steps / 2,
            cfg: params.cfg,
            seed,
            denoise: params.stage2_denoise,
        };
        graph.ksampler(
            &model,
            &positive,
            &negative,
            &upscaled_latent,
            stage2_params,
        )
    } else {
        let params = KSamplerParams {
            sampler: params.sampler.to_string(),
            scheduler: params.scheduler.to_string(),
            steps,
            cfg: params.cfg,
            seed,
            denoise,
        };
        graph.ksampler(&model, &positive, &negative, &latent, params)
    };

    let images = graph.vae_decode(&vae, &final_samples);
    graph.save_images(&images, params.model_name);

    let workflow = graph.build();

    let progress_callback = progress_emitter.as_ref().map(|emitter| {
        let emitter = emitter.clone();
        Box::new(move |fraction: f32, stage: Option<&str>| {
            handle_comfy_progress(&emitter, fraction, stage);
        }) as comfyui::net::ProgressCallback
    });

    debug!("Submitting graph to ComfyUI");
    let images = client
        .execute_workflow(workflow.clone(), progress_callback)
        .await
        .context("while executing graph on ComfyUI")?;
    debug!("Graph execution completed");

    let images = images.into_iter().map(Arc::new).collect();

    Ok(ImagenResponse {
        images,
        text: None,
        workflow: Some(workflow),
        backend: ImagenBackend::StableDiffusion,
        model_name: params.model_name.to_string(),
        seed: Some(seed),
    })
}

fn handle_comfy_progress(emitter: &ActionProgressEmitter, fraction: f32, stage: Option<&str>) {
    let percent = (fraction * 100.0).clamp(0.0, 100.0);

    if let Some(stage) = stage {
        if let Some(position) = stage.strip_prefix("queued:") {
            let message = match position.parse::<u32>() {
                Ok(0) => "Queued on ComfyUI (starting imminently)".to_string(),
                Ok(pos) => format!("Queued on ComfyUI (approximately {} job(s) ahead)", pos),
                Err(_) => "Queued on ComfyUI".to_string(),
            };
            emitter.progress(Some(0.0), message);
            return;
        }

        if let Some(remaining) = stage.strip_prefix("queue_remaining:") {
            let message = match remaining.parse::<u32>() {
                Ok(0) => "Backend queue cleared; starting soon".to_string(),
                Ok(count) => format!("Backend queue now has {} job(s) ahead", count),
                Err(_) => "Backend queue updated".to_string(),
            };
            emitter.progress(None, message);
            return;
        }

        match stage {
            "cached" => {
                emitter.progress(Some(percent.max(90.0)), "Using cached result from ComfyUI");
                return;
            }
            "completed" => {
                emitter.progress(Some(100.0), "Generation completed");
                return;
            }
            other if !other.is_empty() => {
                emitter.progress(
                    Some(percent),
                    format!("Executing {}", prettify_node_name(other)),
                );
                return;
            }
            _ => {}
        }
    }

    emitter.progress(Some(percent), format!("Progress {:.0}%", percent));
}

fn prettify_node_name(raw: &str) -> String {
    let mut result = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '_' {
            result.push(' ');
            continue;
        }

        if ch.is_uppercase() && result.ends_with(|c: char| c.is_lowercase()) {
            result.push(' ');
        }

        if result.is_empty() {
            result.push(ch.to_ascii_uppercase());
            continue;
        }

        result.push(ch);
    }

    if result.is_empty() {
        raw.to_string()
    } else {
        result
    }
}

fn calculate_dimensions(aspect: (u32, u32), base_width: u32, base_height: u32) -> (u32, u32) {
    let pixel_count = base_width * base_height;
    let aspect_ratio = aspect.0 as f32 / aspect.1 as f32;
    let height = (pixel_count as f32 / aspect_ratio).sqrt().round() as u32;
    let width = (height as f32 * aspect_ratio).round() as u32;

    let width = (width / 64) * 64;
    let height = (height / 64) * 64;
    (width, height)
}

fn find_best_resolution(
    desired_aspect: (u32, u32),
    allowed_resolutions: &Vec<(u32, u32)>,
) -> (u32, u32) {
    let desired_ratio = desired_aspect.0 as f32 / desired_aspect.1 as f32;

    let mut best_resolution = allowed_resolutions[0];
    let mut best_score = f32::INFINITY;

    for &(width, height) in allowed_resolutions {
        let resolution_ratio = width as f32 / height as f32;
        let aspect_diff = (resolution_ratio - desired_ratio).abs();

        if aspect_diff < best_score {
            best_score = aspect_diff;
            best_resolution = (width, height);
        }
    }

    best_resolution
}
