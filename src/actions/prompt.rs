use anyhow::{Context as _, Error, Result, bail};
use kameo::{Actor, prelude::Message};
use rand::RngCore as _;
use tracing::{debug, info, trace};

use crate::{
    config::models::{self, Model, ModelsConfig},
    fuzzy::{FuzzyResult, find_fuzzy_match},
    messages::{chat::NanoBanana, imagen::Generate},
    network::{
        comfyui::{self, api::KSamplerParams, net::ComfyUIClient},
        openrouter::OpenRouter,
    },
    persistence::{
        images::{GalleryInput, upload_gallery, upload_image_with_generation},
        user::{AddGeneratedImage, GetAlias, GetDefaultPrompt, UserActor},
    },
    supervisor::Supervisor,
};

pub mod parse;

/// Image generation actor for the !prompt command
#[derive(Actor)]
pub struct PromptActor {
    user_actor: kameo::actor::ActorRef<UserActor>,
}

#[derive(Debug)]
pub struct PromptResult {
    pub text: String,
    pub image_url: Option<String>,
    pub correction_message: Option<String>,
}

impl Message<String> for PromptActor {
    type Reply = Result<PromptResult, Error>;

    async fn handle(
        &mut self,
        msg: String,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        debug!("PromptActor received message: {}", msg);

        // Parse the user's prompt first
        let mut prompt = Generate::from_str(&msg)?;

        // Parse default settings (if any).
        let default_prompt = self
            .user_actor
            .ask(GetDefaultPrompt)
            .await
            .context("Failed to get default prompt")?
            .map(|base_str| Generate::from_str(&base_str))
            .transpose()
            .context("Failed to parse default prompt")?;

        // Have we got any aliases?
        let alias = prompt
            .alias
            .clone()
            .or(default_prompt.as_ref().and_then(|d| d.alias.clone()));
        if let Some(alias_name) = &alias {
            // Apply alias settings prior to defaults, so aliases override defaults.
            let alias = self
                .user_actor
                .ask(GetAlias(alias_name.clone()))
                .await
                .context("Failed to get alias")?;
            if let Some(alias_str) = alias {
                let alias = Generate::from_str(&alias_str)
                    .with_context(|| format!("Failed to parse alias '{}'", alias_name))?;
                prompt = Self::merge_prompt_settings(prompt, alias);
                debug!("Merged alias settings into prompt");
            } else {
                bail!(
                    "Alias '{}' not found. Use !config alias {} [settings] to create it.",
                    alias_name,
                    alias_name
                );
            }
        }

        // Apply the default settings (if any).
        if let Some(default_prompt) = default_prompt {
            prompt = Self::merge_prompt_settings(prompt, default_prompt);
            debug!("Merged default settings into prompt");
        }

        info!("Final parsed prompt: {:?}", prompt);

        self.process_generate(prompt).await
    }
}

impl Message<Generate> for PromptActor {
    type Reply = Result<PromptResult, Error>;

    async fn handle(
        &mut self,
        prompt: Generate,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        debug!("PromptActor received Generate: {:?}", prompt);
        self.process_generate(prompt).await
    }
}

#[derive(Debug)]
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

impl PromptActor {
    /// Common processing for all generation requests, i.e. prompt extension and model resolution.
    async fn process_generate(&mut self, mut prompt: Generate) -> Result<PromptResult, Error> {
        // Get models config and look up the model
        let models_config = Supervisor::models_config().await;
        let (model, correction_message) =
            self.resolve_model(&prompt.prompt, &models_config, prompt.model.as_deref())?;
        info!("Using model: {:?}", model);

        // Extend the prompt with information from the model defaults.
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
                prompt.negative_prompt.unwrap_or_default()
            ));
        }
        if let Some(neg_append) = &model.prompt_defaults.negative_append {
            prompt.negative_prompt = Some(format!(
                "{}. {}",
                prompt.negative_prompt.unwrap_or_default(),
                neg_append
            ));
        }

        let mut prompt_result = match &model.backend {
            models::Backend::NanoBanana => self.nanobanana(prompt).await,
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
                self.comfyui(ComfyParams {
                    count: prompt
                        .num_images
                        .unwrap_or(model.prompt_defaults.count.unwrap_or(2))
                        .clamp(1, 6),
                    prompt,
                    model_name: &model.name,
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
                })
                .await
            }
        }?;

        // Add correction message if there was one
        prompt_result.correction_message = correction_message;
        Ok(prompt_result)
    }

    /// Stable Diffusion, Qwen-Image, etc. via ComfyUI
    async fn comfyui<'a>(&self, params: ComfyParams<'a>) -> Result<PromptResult> {
        let client = ComfyUIClient::new();
        let mut graph = comfyui::api::Graph::new();

        // Never do two-stage if img2img is requested
        let two_stage = params.two_stage && params.prompt.references.img2img.is_none();

        // Replace model parameters with those from the prompt if specified.
        let seed = params.prompt.seed.unwrap_or(rand::rng().next_u64());
        let num_images = params.count;
        let mut width = params.resolution.0;
        let mut height = params.resolution.1;

        // Resolution selection logic based on user input
        if let Some(w) = params.prompt.width {
            width = w;
        }
        if let Some(h) = params.prompt.height {
            height = h;
        }

        // If user specified exact dimensions, use them as-is (no snapping)
        if params.prompt.width.is_some() || params.prompt.height.is_some() {
            // User provided explicit dimensions - use them directly
            width = width.clamp(256, 2048);
            height = height.clamp(256, 2048);
            trace!("Using user-specified dimensions: {}x{}", width, height);
        } else {
            // User didn't specify dimensions - use aspect ratio logic or default to 1:1
            let aspect = params.prompt.aspect.unwrap_or((1, 1)); // Default to 1:1 if no aspect specified

            if let Some(resolutions) = params.resolutions {
                // Model has specific allowed resolutions - find the best match
                let (selected_width, selected_height) = find_best_resolution(aspect, resolutions);
                trace!(
                    "Selected resolution {}x{} from allowed set for aspect ratio {:?}",
                    selected_width, selected_height, aspect
                );
                width = selected_width;
                height = selected_height;
            } else {
                // No specific resolutions - calculate dimensions normally
                trace!("Calculating dimensions for aspect ratio {:?}", aspect);
                (width, height) = calculate_dimensions(aspect, width, height);
                width = width.clamp(256, 2048);
                height = height.clamp(256, 2048);
                trace!("Calculated dimensions: {}x{}", width, height);
            }
        }
        let steps = params.prompt.steps.unwrap_or(params.steps).clamp(1, 150);

        // Load model, CLIP, and VAE
        let (mut model, clip, vae) = match params.checkpoint {
            // graph.checkpoint_loader(params.checkpoint);
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

        // Apply TorchCompile if enabled
        if params.use_torch_compile {
            model = graph.torch_compile_model(&model, "inductor");
        }

        let positive = graph.clip_text_encode(&clip, &params.prompt.prompt);
        let negative = graph.clip_text_encode(
            &clip,
            &params.prompt.negative_prompt.clone().unwrap_or_default(),
        );

        // Handle img2img mode vs text2img mode
        let latent = if let Some(ref input_image) = params.prompt.references.img2img {
            // img2img mode: encode the input image to latent space
            info!(
                "Using img2img mode with input image: {}x{}",
                input_image.width(),
                input_image.height()
            );

            // Confirm that denoise strength is set
            if let Some(strength) = params.prompt.references.img2img_strength {
                if !(0.0..=1.0).contains(&strength) {
                    bail!("--denoise parameter must be between 0.0 and 1.0");
                }
            } else {
                bail!("--denoise parameter is required for img2img generation");
            }

            // Load and encode the input image
            let loaded_image = graph.load_image_from_rgb(input_image);
            graph.vae_encode(&vae, &loaded_image)
        } else {
            // text2img mode: use empty latent
            graph.empty_latent_image(width, height, num_images)
        };

        // Determine denoise strength based on mode
        let denoise = if let Some(strength) = params.prompt.references.img2img_strength {
            strength.clamp(0.0, 1.0)
        } else {
            1.0
        };
        trace!("Using denoise strength: {}", denoise);

        let final_samples = if two_stage {
            // Stage 1: Initial sampling with half steps
            let stage1_params = KSamplerParams {
                sampler: params.sampler.to_string(),
                scheduler: params.scheduler.to_string(),
                steps: steps / 2,
                cfg: params.cfg,
                seed,
                denoise,
            };
            let stage1_samples =
                graph.ksampler(&model, &positive, &negative, &latent, stage1_params);

            // Stage 2: Upscale and refine
            let upscaled_latent =
                graph.latent_upscaler(&stage1_samples, "SDXL", params.upscale_factor);

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
            // Single-stage workflow
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

        // Capture the workflow before executing
        let workflow = graph.build();

        debug!("Submitting graph to ComfyUI");
        let images = client
            .execute_workflow(workflow.clone(), None)
            .await
            .context("while executing graph on ComfyUI")?;
        debug!("Graph execution completed");

        // Make a gallery from the images.
        let title = params.prompt.raw_prompt.clone();
        let subtitle = format!("Model: {}, Seed: {}", params.model_name, seed);
        let gallery = upload_gallery(GalleryInput {
            title,
            subtitle,
            images,
            workflow: Some(workflow),
            backend: Some("StableDiffusion".to_string()),
            generation_request: Some(params.prompt.clone()),
        })
        .await
        .context("while uploading image gallery")?;
        info!(
            "Successfully generated and uploaded image gallery: {}",
            gallery.0
        );

        // Record the generated image in user's history
        let _ = self
            .user_actor
            .tell(AddGeneratedImage {
                url: gallery.0.clone(),
                prompt: params.prompt.raw_prompt.clone(),
                model: Some(params.model_name.to_string()),
                backend: "StableDiffusion".to_string(),
            })
            .send()
            .await;

        Ok(PromptResult {
            text: "".to_string(),
            image_url: Some(gallery.0),
            correction_message: None,
        })
    }

    /// Calls Gemini 2.5-flah-image-preview (NanoBanana) via OpenRouter
    async fn nanobanana(&self, generate_request: Generate) -> Result<PromptResult> {
        // Adjust prompt based on whether we're editing an existing image
        let formatted_prompt = if generate_request.references.img2img.is_some() {
            format!(
                "Edit this image according to these instructions: {}\nAlways generate an edited image. In addition to the image, comment on the changes in the style of a hard-boiled noir detective.",
                generate_request.prompt
            )
        } else {
            format!(
                "Generate an image: {}\nAlways generate an image. In addition to the image, comment on it in the style of a hard-boiled noir detective.",
                generate_request.prompt
            )
        };

        // Get the OpenRouter instance
        let router = OpenRouter::get().context("while fetching OpenRouter instance")?;

        // Generate response using NanoBanana
        let response = router
            .ask(NanoBanana {
                origin: "prompt command".to_string(),
                prompt: formatted_prompt.clone(),
                input_image: generate_request.references.img2img.clone(),
            })
            .await
            .context("while generating response with NanoBanana")?;

        // Upload the image if one was generated
        let image_url = if let Some(image) = response.image {
            // Create a workflow object representing the NanoBanana request
            let workflow = serde_json::json!({
                "model": "gemini-2.5-flash-image-preview",
                "original_prompt": generate_request.prompt,
                "raw_prompt": generate_request.raw_prompt,
                "formatted_prompt": formatted_prompt,
                "timestamp": chrono::Utc::now().to_rfc3339()
            });

            let url = upload_image_with_generation(
                image,
                Some(workflow),
                Some("NanoBanana".to_string()),
                Some(generate_request.clone()),
            )
            .await
            .context("while uploading generated image")?;
            info!("Successfully generated and uploaded image: {}", url);
            Some(url)
        } else {
            info!("No image generated, text-only response");
            None
        };

        // Record the generated image in user's history if one was generated
        if let Some(ref url) = image_url {
            let _ = self
                .user_actor
                .tell(AddGeneratedImage {
                    url: url.clone(),
                    prompt: generate_request.raw_prompt.clone(),
                    model: Some("gemini-2.5-flash-image-preview".to_string()),
                    backend: "NanoBanana".to_string(),
                })
                .send()
                .await;
        }

        Ok(PromptResult {
            text: response.text,
            image_url,
            correction_message: None,
        })
    }
}

impl PromptActor {
    pub async fn new(user_actor: kameo::actor::ActorRef<UserActor>) -> Self {
        Self { user_actor }
    }

    /// Merge base settings (from alias or defaults) into a prompt.
    /// User's settings in the prompt take precedence.
    fn merge_prompt_settings(mut prompt: Generate, base: Generate) -> Generate {
        // Merge prompt text
        if !base.prompt.is_empty() {
            prompt.prompt = format!("{}. {}", prompt.prompt, base.prompt);
        }

        // Merge negative prompt
        if let Some(neg) = base.negative_prompt {
            if prompt.negative_prompt.is_none() {
                prompt.negative_prompt = Some(neg);
            } else {
                // If user has a negative prompt, append the base
                let user_neg = prompt.negative_prompt.unwrap_or_default();
                prompt.negative_prompt = Some(format!("{}, {}", user_neg, neg));
            }
        }

        // Merge optional settings - only apply base where user hasn't specified a value
        if prompt.num_images.is_none() {
            prompt.num_images = base.num_images;
        }
        if prompt.width.is_none() {
            prompt.width = base.width;
        }
        if prompt.height.is_none() {
            prompt.height = base.height;
        }
        if prompt.aspect.is_none() {
            prompt.aspect = base.aspect;
        }
        if prompt.model.is_none() {
            prompt.model = base.model;
        }
        if prompt.seed.is_none() {
            prompt.seed = base.seed;
        }
        if prompt.steps.is_none() {
            prompt.steps = base.steps;
        }
        if prompt.references.img2img_strength.is_none() {
            prompt.references.img2img_strength = base.references.img2img_strength;
        }

        prompt
    }

    /// Resolve model name to Model, handling aliases and defaults with fuzzy matching
    fn resolve_model<'a>(
        &self,
        prompt: &str,
        config: &'a ModelsConfig,
        model_name: Option<&str>,
    ) -> Result<(&'a Model, Option<String>)> {
        // Use default model if none specified
        let mut model_name = model_name.unwrap_or(&config.default);
        if model_name == "auto" {
            // Determine default based on comma levels.
            let commacity = prompt.matches(',').count() as f32 / prompt.len() as f32;
            if commacity >= 0.04 {
                model_name = &config.default_tagged;
            } else {
                model_name = &config.default_english;
            }
            info!(
                "Auto-selected model '{}' based on prompt commacity of {:.3}",
                model_name, commacity
            );
        }

        // Resolve alias if it exists
        let resolved_model_name = config
            .aliases
            .get(model_name)
            .map(String::as_str)
            .unwrap_or(model_name);

        // First try exact match
        if let Some(model) = config.models.get(resolved_model_name) {
            return Ok((model, None));
        }

        // If no exact match, try fuzzy matching on all model names and aliases
        let mut candidates: Vec<(&str, &Model)> = config
            .models
            .iter()
            .map(|(name, model)| (name.as_str(), model))
            .collect();

        // Add aliases to candidates
        for (alias, target) in &config.aliases {
            if let Some(model) = config.models.get(target) {
                candidates.push((alias.as_str(), model));
            }
        }

        let fuzzy_result = find_fuzzy_match(resolved_model_name, candidates);

        match fuzzy_result {
            FuzzyResult::Exact(model) => Ok((model, None)),
            FuzzyResult::Corrected {
                corrected,
                original,
            } => {
                let message = format!(
                    "Corrected model name '{}' to '{}'",
                    original, corrected.name
                );
                Ok((corrected, Some(message)))
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
}

/// Calculate dimensions based on aspect ratio and base dimensions, maintaining the pixel count.
fn calculate_dimensions(aspect: (u32, u32), base_width: u32, base_height: u32) -> (u32, u32) {
    let pixel_count = base_width * base_height;
    let aspect_ratio = aspect.0 as f32 / aspect.1 as f32;
    let height = (pixel_count as f32 / aspect_ratio).sqrt().round() as u32;
    let width = (height as f32 * aspect_ratio).round() as u32;

    // Make dimensions multiples of 64
    let width = (width / 64) * 64;
    let height = (height / 64) * 64;
    (width, height)
}

/// Find the best resolution from the allowed set that matches the desired aspect ratio.
fn find_best_resolution(
    desired_aspect: (u32, u32),
    allowed_resolutions: &Vec<(u32, u32)>,
) -> (u32, u32) {
    let desired_ratio = desired_aspect.0 as f32 / desired_aspect.1 as f32;

    let mut best_resolution = allowed_resolutions[0];
    let mut best_score = f32::INFINITY;

    for &(width, height) in allowed_resolutions {
        let resolution_ratio = width as f32 / height as f32;

        // Score based on how close the aspect ratio is
        let aspect_diff = (resolution_ratio - desired_ratio).abs();

        // Prefer resolutions with aspect ratios closer to the desired one
        let score = aspect_diff;

        if score < best_score {
            best_score = score;
            best_resolution = (width, height);
        }
    }

    best_resolution
}
