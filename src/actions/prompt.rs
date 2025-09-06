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

        // Validate the prompt
        let prompt = msg.trim();
        if prompt.is_empty() {
            bail!("Please provide a prompt for image generation.");
        }

        // Make sure we're actually asking for an image.
        // NOTE: We should only do this for NanoBanana.
        let prompt = format!(
            "Generate an image: {}\nAlways generate an image. In addition to the image, comment on it in the style of a hard-boiled noir detective.",
            prompt
        );

        info!("Generating image for prompt: {}", prompt);

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
}
