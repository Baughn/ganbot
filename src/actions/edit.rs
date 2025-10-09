use anyhow::{Context as _, Error, Result, bail};
use kameo::{Actor, prelude::Message};
use std::sync::Arc;
use tracing::{debug, info};

use crate::{
    actions::prompt::{PromptActor, PromptResult},
    messages::imagen::{Generate, References},
    persistence::user::{GetSelectedImage, UserActor},
};

/// Actor for the !edit command - edits a previously selected image
#[derive(Actor)]
pub(crate) struct EditActor {
    user_actor: kameo::actor::ActorRef<UserActor>,
}

impl Message<String> for EditActor {
    type Reply = Result<PromptResult, Error>;

    async fn handle(
        &mut self,
        msg: String,
        ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        debug!("EditActor received edit instructions: {}", msg);

        // Get the selected image URL from the user
        let selected_url = self
            .user_actor
            .ask(GetSelectedImage)
            .await
            .context("while getting selected image URL")?;

        let selected_url = match selected_url {
            Some(url) => url,
            None => bail!("No image selected. Use !select <url> first to choose an image to edit."),
        };

        info!("Editing selected image: {}", selected_url);

        // Download the image from the URL
        let client = reqwest::Client::new();
        let response = client
            .get(&selected_url)
            .send()
            .await
            .context("while downloading selected image")?;

        if !response.status().is_success() {
            bail!("Failed to download image: HTTP {}", response.status());
        }

        let image_bytes = response
            .bytes()
            .await
            .context("while reading image bytes")?;

        // Convert bytes to RgbImage
        let dynamic_image =
            image::load_from_memory(&image_bytes).context("while decoding image")?;
        let rgb_image = dynamic_image.to_rgb8();

        info!(
            "Successfully downloaded and decoded image: {}x{}",
            rgb_image.width(),
            rgb_image.height()
        );

        // Parse the edit instructions using the same parser as !prompt
        let mut generate_request = Generate::from_str(&msg)?;
        info!("Parsed edit request: {:?}", generate_request);

        // Set the input image in the references
        generate_request.references = References {
            img2img: Some(Arc::new(rgb_image)),
            img2img_strength: generate_request.references.img2img_strength,
            context: Vec::new(),
        };

        // Create a PromptActor and delegate to it
        let prompt_actor = PromptActor::spawn_link(
            ctx.actor_ref(),
            PromptActor::new(self.user_actor.clone(), None).await,
        )
        .await;

        let result = prompt_actor.ask(generate_request).await?;

        info!("Edit completed successfully");
        Ok(result)
    }
}

impl EditActor {
    pub async fn new(user_actor: kameo::actor::ActorRef<UserActor>) -> Self {
        Self { user_actor }
    }
}
