use anyhow::{Context as _, Error, Result};
use kameo::{Actor, prelude::Message};
use tracing::{debug, info};

use crate::{
    config::models::{self, Model, ModelsConfig},
    messages::{chat::NanoBanana, imagen::Generate},
    network::{
        comfyui::{self, api::KSamplerParams, net::ComfyUIClient},
        openrouter::OpenRouter,
    },
    persistence::images::upload_image,
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
        let prompt = Generate::from_str(&msg)?;
        info!("Parsed prompt: {:?}", prompt);

        // Get models config and look up the model
        let models_config = Supervisor::models_config().await;
        let model = self.resolve_model(&models_config, prompt.model.as_deref())?;
        info!("Using model: {:?}", model);

        match &model.backend {
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
        }
    }
}

impl PromptActor {
    /// Stable Diffusion image generation (SDXL, etc.)
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

        // Build workflow
        let (model, clip, vae) = graph.checkpoint_loader(checkpoint);
        let vae = if let Some(vae_name) = vae_name {
            graph.vae_loader(vae_name)
        } else {
            vae
        };
        let positive = graph.clip_text_encode(&clip, &prompt.prompt);
        let negative = graph.clip_text_encode(&clip, &prompt.negative_prompt.unwrap_or_default());
        let latent =
            graph.empty_latent_image(resolution.0, resolution.1, prompt.num_images.unwrap_or(2));

        let params = KSamplerParams {
            sampler: sampler.to_string(),
            scheduler: scheduler.to_string(),
            steps,
            cfg,
            seed: prompt.seed.unwrap_or(0),
            denoise: 1.0,
        };

        let samples = graph.ksampler(&model, &positive, &negative, &latent, params);
        let images = graph.vae_decode(&vae, &samples);
        graph.save_images(&images, model_name);

        debug!("Submitting graph to ComfyUI");
        let results = client
            .execute_graph(graph, None)
            .await
            .context("while executing graph on ComfyUI")?;
        debug!("Graph execution completed");

        // Make a gallery from the images.

        todo!()
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
        Ok(config
            .models
            .get(model_name)
            .with_context(|| format!("model '{}' not found in configuration", model_name))?)
    }
}
