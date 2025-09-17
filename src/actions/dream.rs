use anyhow::{Context as _, Error, Result, bail};
use futures::future;
use kameo::{Actor, Reply, prelude::Message};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};

use crate::{
    actions::prompt::{PromptActor, PromptResult},
    messages::{
        chat::{Oneshot, Part, Purpose},
        imagen::Generate,
    },
    network::openrouter::OpenRouter,
    persistence::{
        images::{GalleryWithIndividualPromptsInput, upload_gallery_with_individual_prompts},
        user::{AddGeneratedImage, UserActor},
    },
    supervisor::Supervisor,
};

/// Dream command actor that generates image prompts with hard-boiled detective personality
#[derive(Actor)]
pub struct DreamActor {
    user_actor: kameo::actor::ActorRef<UserActor>,
}

#[derive(Debug, Serialize, Deserialize, Reply)]
struct DreamPromptResponse {
    image_prompt: String,
}

impl Message<String> for DreamActor {
    type Reply = Result<PromptResult, Error>;

    async fn handle(
        &mut self,
        msg: String,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        debug!("DreamActor received message: {}", msg);

        // Parse the user's prompt like !prompt does
        let mut prompt = Generate::from_str(&msg)?;

        // Resolve the requested model (defaulting to the dream alias) and ensure it's supported
        let models_config = Supervisor::models_config().await;
        let mut requested_model_token = prompt
            .model
            .clone()
            .unwrap_or_else(|| "dream".to_string());
        let (selected_model, correction_message) = PromptActor::resolve_model(
            &prompt.prompt,
            &models_config,
            Some(requested_model_token.as_str()),
        )?;

        let has_english_tag = selected_model.tags.iter().any(|tag| tag == "english");
        if !has_english_tag {
            bail!(
                "Model '{}' is not supported for !dream yet; please pick an English-friendly model.",
                selected_model.name
            );
        }

        if correction_message.is_some() {
            requested_model_token = selected_model.name.clone();
        }

        let display_model_name = requested_model_token.clone();
        prompt.model = Some(requested_model_token.clone());

        // Get the original user request for later commentary
        let original_request = prompt.prompt.clone();

        // Get the number of images to generate (defaults to 2 if not specified)
        let num_images = prompt.num_images.unwrap_or(2).clamp(1, 6) as usize;

        // Step 1: Generate multiple unique image prompts in parallel
        let router = OpenRouter::get().context("while fetching OpenRouter instance")?;

        let prompt_futures = (0..num_images).map(|i| {
            let router = router.clone();
            let original_request = original_request.clone();
            async move {
                let detective_prompt = format!(
                    "Transform the following request into a detailed image generation prompt.
                    Add theme, lighting, and that distinctive story feel. \n\
                    If there is a picture, copy the style of the picture unless requested otherwise.\n
                    Create a unique variation - don't repeat exactly the same elements.\n
                    Output nothing except for the image prompt.\n\n\
                    Original request: {} (Variation {})",
                    original_request, i + 1
                );

                router
                    .ask(Oneshot {
                        purpose: Purpose::Dream,
                        origin: "dream".to_string(),
                        text: vec![Part::Uncacheable(detective_prompt)],
                    })
                    .await
                    .map(|response| response.text)
            }
        });

        let generated_prompts = future::try_join_all(prompt_futures)
            .await
            .context("while generating image prompts")?;

        info!("Generated {} unique image prompts", generated_prompts.len());

        // Step 2: Generate images for each prompt in parallel
        let model_for_generation = requested_model_token.clone();
        let image_generation_futures = generated_prompts.iter().map(|image_prompt| {
            let user_actor = self.user_actor.clone();
            let actor_ref = _ctx.actor_ref().clone();
            let image_prompt = image_prompt.clone();
            let model_for_generation = model_for_generation.clone();
            async move {
                let prompt_actor =
                    PromptActor::spawn_link(&actor_ref, PromptActor::new(user_actor).await).await;

                let image_prompt_with_model =
                    format!("{} -m {} -c 1", image_prompt, model_for_generation);
                prompt_actor
                    .ask(image_prompt_with_model)
                    .await
                    .context("while generating image")
                    .map(|result| (image_prompt, result))
            }
        });

        let image_results = future::try_join_all(image_generation_futures)
            .await
            .context("while generating images")?;

        // Step 3: Create gallery with individual prompts
        let mut image_prompts: Vec<(String, Arc<image::RgbImage>)> = Vec::new();
        let mut has_images = false;

        for (prompt_text, result) in &image_results {
            if let Some(images) = &result.images {
                if images.len() != 1 {
                    bail!("Expected 1 image per prompt, got {}", images.len());
                }
                let first_image = images.first().unwrap();
                image_prompts.push((prompt_text.clone(), first_image.clone()));
                has_images = true;
            }
        }

        // Create and upload the gallery if we have images
        let gallery_url = if !image_prompts.is_empty() {
            let gallery_input = GalleryWithIndividualPromptsInput {
                image_prompts,
                workflow: None, // TODO: Grab the workflow from image #1 somehow.
                backend: Some("ComfyUI".to_string()),
                generation_request: Some(prompt.clone()),
            };

            let title = original_request.clone();
            let subtitle = format!("Model: {}", display_model_name);

            let url = upload_gallery_with_individual_prompts(title, subtitle, gallery_input)
                .await
                .context("while uploading gallery with individual prompts")?;

            // Record the generated gallery in user's history
            let _ = self
                .user_actor
                .tell(AddGeneratedImage {
                    url: url.clone(),
                    prompt: original_request.clone(),
                    model: Some(display_model_name.clone()),
                    backend: "StableDiffusion".to_string(),
                })
                .send()
                .await;

            info!(
                "Successfully created gallery with individual prompts: {}",
                url
            );
            Some(url)
        } else {
            None
        };

        // Generate commentary based on results
        if has_images && gallery_url.is_some() {
            let gallery_url = gallery_url.unwrap();
            let commentary_prompt = format!(
                "As a hard-boiled detective, provide commentary on this case. \
                The client requested: '{}'\n\
                I generated {} unique interpretations, each with its own angle on the case.\n\
                The evidence has been compiled here: {}\n\n\
                Give a brief, noir-style commentary on how this multi-faceted investigation turned out.",
                original_request, num_images, gallery_url
            );

            let commentary = router
                .ask(Oneshot {
                    purpose: Purpose::Chat,
                    origin: "dream commentary".to_string(),
                    text: vec![Part::Uncacheable(commentary_prompt)],
                })
                .await
                .context("while generating commentary")?;

            Ok(PromptResult {
                text: commentary.text,
                image_url: Some(gallery_url),
                images: None, // We've already uploaded them as a gallery
                correction_message: correction_message.clone(),
            })
        } else {
            // No images were generated
            let commentary_prompt = format!(
                "As a hard-boiled detective, provide commentary on this case. \
                The client requested: '{}'\n\
                I tried {} different approaches, but all leads went cold - no images were generated.\n\n\
                Give a brief, noir-style commentary on this failed investigation.",
                original_request, num_images
            );

            let commentary = router
                .ask(Oneshot {
                    purpose: Purpose::Chat,
                    origin: "dream commentary".to_string(),
                    text: vec![Part::Uncacheable(commentary_prompt)],
                })
                .await
                .context("while generating commentary")?;

            Ok(PromptResult {
                text: commentary.text,
                image_url: None,
                images: None,
                correction_message: correction_message.clone(),
            })
        }
    }
}

impl DreamActor {
    pub async fn new(user_actor: kameo::actor::ActorRef<UserActor>) -> Self {
        Self { user_actor }
    }
}
