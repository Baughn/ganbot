use anyhow::{Context as _, Error, Result};
use kameo::{Actor, prelude::Message};
use rand::RngCore as _;
use tracing::{debug, info};

use crate::{
    config::models::{self, Model, ModelsConfig},
    messages::{chat::NanoBanana, imagen::Generate},
    network::{
        comfyui::{self, api::KSamplerParams, net::ComfyUIClient},
        openrouter::OpenRouter,
    },
    persistence::images::{GalleryInput, upload_gallery, upload_image},
    supervisor::Supervisor,
};

pub mod parse;

/// Image generation actor for the !prompt command
#[derive(Actor)]
pub(crate) struct PromptActor;

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
        let mut prompt = Generate::from_str(&msg)?;
        info!("Parsed prompt: {:?}", prompt);

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
                )
                .await
            }
        }?;
        Ok(prompt_result)
    }
}

impl PromptActor {
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
    ) -> Result<PromptResult> {
        let client = ComfyUIClient::new();
        let mut graph = comfyui::api::Graph::new();

        // Replace model parameters with those from the prompt if specified.
        let seed = prompt.seed.unwrap_or(rand::rng().next_u64());
        let num_images = prompt.num_images.unwrap_or(2).clamp(1, 6);
        let mut width = resolution.0;
        let mut height = resolution.1;
        if let Some(aspect) = prompt.aspect {
            (width, height) = calculate_dimensions(aspect, width, height);
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
        let (model, clip, vae) = graph.checkpoint_loader(checkpoint);
        let vae = if let Some(vae_name) = vae_name {
            graph.vae_loader(vae_name)
        } else {
            vae
        };
        let positive = graph.clip_text_encode(&clip, &prompt.prompt);
        let negative =
            graph.clip_text_encode(&clip, &prompt.negative_prompt.clone().unwrap_or_default());
        let latent = graph.empty_latent_image(resolution.0, resolution.1, num_images);

        let params = KSamplerParams {
            sampler: sampler.to_string(),
            scheduler: scheduler.to_string(),
            steps,
            cfg,
            seed,
            denoise: 1.0,
        };

        let samples = graph.ksampler(&model, &positive, &negative, &latent, params);
        let images = graph.vae_decode(&vae, &samples);
        graph.save_images(&images, model_name);

        debug!("Submitting graph to ComfyUI");
        let images = client
            .execute_graph(graph, None)
            .await
            .context("while executing graph on ComfyUI")?;
        debug!("Graph execution completed");

        // Make a gallery from the images.
        let title = format!("{}", prompt);
        let subtitle = format!("Model: {}, Seed: {}", model_name, seed);
        let gallery = upload_gallery(GalleryInput {
            title,
            subtitle,
            images,
        })
        .await
        .context("while uploading image gallery")?;
        info!(
            "Successfully generated and uploaded image gallery: {}",
            gallery.0
        );

        Ok(PromptResult {
            text: "".to_string(),
            image_url: Some(gallery.0),
        })
    }

    /// Calls Gemini 2.5-flah-image-preview (NanoBanana) via OpenRouter
    async fn nanobanana(&self, prompt: Generate) -> Result<PromptResult> {
        // Make sure we're actually asking for an image.
        let prompt = format!(
            "Generate an image: {}\nAlways generate an image. In addition to the image, comment on it in the style of a hard-boiled noir detective.",
            prompt.prompt
        );

        // Get the OpenRouter instance
        let router = OpenRouter::get().context("while fetching OpenRouter instance")?;

        // Generate response using NanoBanana
        let response = router
            .ask(NanoBanana {
                origin: "prompt command".to_string(),
                prompt: prompt.to_string(),
            })
            .await
            .context("while generating response with NanoBanana")?;

        // Upload the image if one was generated
        let image_url = if let Some(image) = response.image {
            let url = upload_image(image)
                .await
                .context("while uploading generated image")?;
            info!("Successfully generated and uploaded image: {}", url);
            Some(url)
        } else {
            info!("No image generated, text-only response");
            None
        };

        Ok(PromptResult {
            text: response.text,
            image_url,
        })
    }
}

impl PromptActor {
    pub async fn new() -> Self {
        Self
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
