use anyhow::{Context as _, Error, Result, bail};
use kameo::{Actor, prelude::Message};
use tracing::{debug, info};

use crate::persistence::user::{SetSelectedImage, UserActor};

/// Actor for the !select command - stores an image URL for later editing
#[derive(Actor)]
pub(crate) struct SelectActor {
    user_actor: kameo::actor::ActorRef<UserActor>,
}

#[derive(Debug)]
pub struct SelectResult {
    pub message: String,
}

impl Message<String> for SelectActor {
    type Reply = Result<SelectResult, Error>;

    async fn handle(
        &mut self,
        msg: String,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        debug!("SelectActor received URL: {}", msg);

        let url = msg.trim();
        if url.is_empty() {
            bail!("No URL provided");
        }

        // Basic URL validation
        if !url.starts_with("http://") && !url.starts_with("https://") {
            bail!("URL must start with http:// or https://");
        }

        // Basic image URL validation (check for common image extensions)
        let lower_url = url.to_lowercase();
        let has_image_extension = [".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp"]
            .iter()
            .any(|ext| lower_url.contains(ext));

        if !has_image_extension {
            // Allow URLs without explicit extensions as they might still be images
            info!(
                "URL doesn't have explicit image extension, but allowing anyway: {}",
                url
            );
        }

        // Store the selected image URL
        self.user_actor
            .tell(SetSelectedImage(url.to_string()))
            .send()
            .await
            .context("while setting selected image URL")?;

        info!("Selected image URL set to: {}", url);

        Ok(SelectResult {
            message: "✓ Selected image".to_string(),
        })
    }
}

impl SelectActor {
    pub async fn new(user_actor: kameo::actor::ActorRef<UserActor>) -> Self {
        Self { user_actor }
    }
}
