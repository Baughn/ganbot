use anyhow::{Context as _, Error, Result, bail};
use kameo::{Actor, prelude::Message};
use tracing::{debug, info};

use crate::{
    messages::chat::NanoBanana, network::openrouter::OpenRouter, persistence::images::upload_image,
};

pub mod parse;

/// Image generation actor for the !prompt command
#[derive(Actor)]
pub(crate) struct PromptActor;

pub struct Prompt(pub String);

#[derive(Debug)]
pub struct PromptResult {
    pub image_url: String,
}

impl Message<String> for PromptActor {
    type Reply = Result<PromptResult, Error>;

    async fn handle(
        &mut self,
        msg: String,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        debug!("PromptActor received message: {}", msg);

        // Validate the prompt
        let prompt = msg.trim();
        if prompt.is_empty() {
            bail!("Please provide a prompt for image generation.");
        }

        // Make sure we're actually asking for an image.
        // NOTE: We should only do this for NanoBanana.
        let prompt = format!("Generate an image: {}", prompt);

        info!("Generating image for prompt: {}", prompt);

        // Get the OpenRouter instance
        let router = OpenRouter::get().context("while fetching OpenRouter instance")?;

        // Generate the image using NanoBanana
        let image_response = router
            .ask(NanoBanana {
                origin: "prompt command".to_string(),
                prompt: prompt.to_string(),
            })
            .await
            .context("while generating image with NanoBanana")?;

        // Upload the image and get the URL
        let image_url = upload_image(image_response)
            .await
            .context("while uploading generated image")?;

        info!("Successfully generated and uploaded image: {}", image_url);

        Ok(PromptResult { image_url })
    }
}

impl PromptActor {
    pub async fn new() -> Self {
        Self
    }
}
