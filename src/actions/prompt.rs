use anyhow::{Context as _, Error, Result, bail};
use kameo::{Actor, prelude::Message};
use rand::RngCore as _;
use tracing::{debug, info, trace};

use crate::{
    config::models::{self, Model, ModelsConfig},
    messages::{chat::NanoBanana, imagen::Generate},
    network::{
        comfyui::{self, api::KSamplerParams, net::ComfyUIClient},
        openrouter::OpenRouter,
    },
    persistence::{
        images::{GalleryInput, upload_gallery, upload_image_with_workflow},
        user::{AddGeneratedImage, UserActor},
    },
    supervisor::Supervisor,
};

pub mod parse;

/// Image generation actor for the !prompt command
#[derive(Actor)]
pub(crate) struct PromptActor {
    user_actor: kameo::actor::ActorRef<UserActor>,
}

pub struct Prompt(pub String);

#[derive(Debug)]
pub struct PromptResult {
    pub text: String,
    pub image_url: Option<String>,
}

impl Message<String> for PromptActor {
    type Reply = Result<PromptResult, Error>;

    async fn handle(
        &mut self,
        msg: String,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        debug!("PromptActor received message: {}", msg);

        // Parse the prompt
        let prompt = Generate::from_str(&msg)?;
        info!("Parsed prompt: {:?}", prompt);

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

impl PromptActor {
    async fn process_generate(&mut self, mut prompt: Generate) -> Result<PromptResult, Error> {
        // Get models config and look up the model
        let models_config = Supervisor::models_config().await;
        let model = self.resolve_model(&models_config, prompt.model.as_deref())?;
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

        let prompt_result = match &model.backend {
            models::Backend::NanoBanana => self.nanobanana(prompt).await,
            models::Backend::StableDiffusion {
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
                self.stable_diffusion(
                    prompt,
                    &model.name,
                    checkpoint.as_str(),
                    vae.as_deref(),
                    *cfg,
                    sampler.as_str(),
                    scheduler.as_str(),
                    *steps,
                    *resolution,
                    use_torch_compile.unwrap_or(false),
                    two_stage.unwrap_or(false),
                    upscale_factor.unwrap_or(1.5),
                    stage2_denoise.unwrap_or(0.5),
                    stage2_sampler.as_deref().unwrap_or("euler"),
                    stage2_scheduler.as_deref().unwrap_or("beta"),
                )
                .await
            }
        }?;
        Ok(prompt_result)
    }
    /// Stable Diffusion image generation (SDXL, etc.)
    #[allow(clippy::too_many_arguments)]
    async fn stable_diffusion(
        &self,
        prompt: Generate,
        model_name: &str,
        checkpoint: &str,
        vae_name: Option<&str>,
        cfg: f32,
        sampler: &str,
        scheduler: &str,
        steps: u32,
        resolution: (u32, u32),
        use_torch_compile: bool,
        two_stage: bool,
        upscale_factor: f32,
        stage2_denoise: f32,
        stage2_sampler: &str,
        stage2_scheduler: &str,
    ) -> Result<PromptResult> {
        let client = ComfyUIClient::new();
        let mut graph = comfyui::api::Graph::new();

        // Never do two-stage if img2img is requested
        let two_stage = two_stage && prompt.references.img2img.is_none();

        // Replace model parameters with those from the prompt if specified.
        let seed = prompt.seed.unwrap_or(rand::rng().next_u64());
        let num_images = prompt.num_images.unwrap_or(2).clamp(1, 6);
        let mut width = resolution.0;
        let mut height = resolution.1;
        if let Some(aspect) = prompt.aspect {
            trace!("Calculating dimensions for aspect ratio {:?}", aspect);
            (width, height) = calculate_dimensions(aspect, width, height);
            trace!("Calculated dimensions: {}x{}", width, height);
        }
        if let Some(w) = prompt.width {
            width = w;
        }
        if let Some(h) = prompt.height {
            height = h;
        }
        let width = width.clamp(256, 2048);
        let height = height.clamp(256, 2048);
        let steps = prompt.steps.unwrap_or(steps).clamp(1, 150);

        // Build workflow
        let (mut model, clip, vae) = graph.checkpoint_loader(checkpoint);

        // Apply TorchCompile if enabled
        if use_torch_compile {
            model = graph.torch_compile_model(&model, "inductor");
        }

        let vae = if let Some(vae_name) = vae_name {
            graph.vae_loader(vae_name)
        } else {
            vae
        };
        let positive = graph.clip_text_encode(&clip, &prompt.prompt);
        let negative =
            graph.clip_text_encode(&clip, &prompt.negative_prompt.clone().unwrap_or_default());

        // Handle img2img mode vs text2img mode
        let latent = if let Some(ref input_image) = prompt.references.img2img {
            // img2img mode: encode the input image to latent space
            info!(
                "Using img2img mode with input image: {}x{}",
                input_image.width(),
                input_image.height()
            );

            // For img2img, use the input image dimensions instead of specified dimensions
            let img_width = input_image.width();
            let img_height = input_image.height();

            // Confirm that denoise strength is set
            if let Some(strength) = prompt.references.img2img_strength {
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
        let denoise = if let Some(strength) = prompt.references.img2img_strength {
            strength.clamp(0.0, 1.0)
        } else {
            1.0
        };
        trace!("Using denoise strength: {}", denoise);

        let final_samples = if two_stage {
            // Stage 1: Initial sampling with half steps
            let stage1_params = KSamplerParams {
                sampler: sampler.to_string(),
                scheduler: scheduler.to_string(),
                steps: steps / 2,
                cfg,
                seed,
                denoise,
            };
            let stage1_samples =
                graph.ksampler(&model, &positive, &negative, &latent, stage1_params);

            // Stage 2: Upscale and refine
            let upscaled_latent = graph.latent_upscaler(&stage1_samples, "SDXL", upscale_factor);

            let stage2_params = KSamplerParams {
                sampler: stage2_sampler.to_string(),
                scheduler: stage2_scheduler.to_string(),
                steps: steps / 2,
                cfg,
                seed,
                denoise: stage2_denoise,
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
                sampler: sampler.to_string(),
                scheduler: scheduler.to_string(),
                steps,
                cfg,
                seed,
                denoise,
            };
            graph.ksampler(&model, &positive, &negative, &latent, params)
        };

        let images = graph.vae_decode(&vae, &final_samples);
        graph.save_images(&images, model_name);

        // Capture the workflow before executing
        let workflow = graph.build();

        debug!("Submitting graph to ComfyUI");
        let images = client
            .execute_workflow(workflow.clone(), None)
            .await
            .context("while executing graph on ComfyUI")?;
        debug!("Graph execution completed");

        // Make a gallery from the images.
        let title = prompt.raw_prompt.clone();
        let subtitle = format!("Model: {}, Seed: {}", model_name, seed);
        let gallery = upload_gallery(GalleryInput {
            title,
            subtitle,
            images,
            workflow: Some(workflow),
            backend: Some("StableDiffusion".to_string()),
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
                prompt: prompt.raw_prompt.clone(),
                model: Some(model_name.to_string()),
                backend: "StableDiffusion".to_string(),
            })
            .send()
            .await;

        Ok(PromptResult {
            text: "".to_string(),
            image_url: Some(gallery.0),
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

            let url =
                upload_image_with_workflow(image, Some(workflow), Some("NanoBanana".to_string()))
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
        })
    }
}

impl PromptActor {
    pub async fn new(user_actor: kameo::actor::ActorRef<UserActor>) -> Self {
        Self { user_actor }
    }

    /// Resolve model name to Model, handling aliases and defaults
    fn resolve_model<'a>(
        &self,
        config: &'a ModelsConfig,
        model_name: Option<&str>,
    ) -> Result<&'a Model> {
        // Use default model if none specified
        let model_name = model_name.unwrap_or(&config.default);
        // Resolve alias if it exists
        let model_name = config
            .aliases
            .get(model_name)
            .map(String::as_str)
            .unwrap_or(model_name);
        // Look up the model in the config
        config
            .models
            .get(model_name)
            .with_context(|| format!("model '{}' not found in configuration", model_name))
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
