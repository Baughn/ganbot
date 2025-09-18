use anyhow::{Context as _, Error, Result, bail};
use futures::future;
use kameo::{Actor, Reply, prelude::Message};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::{
    actions::{
        imagen::{self, GenerateImages, ImagenActor, ImagenBackend},
        prompt::PromptResult,
    },
    messages::{
        chat::{Oneshot, Part, Purpose},
        imagen::Generate,
    },
    network::openrouter::OpenRouter,
    persistence::{
        images::{GalleryImageInput, GalleryInput, upload_gallery},
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

    #[tracing::instrument(skip(self, _ctx, msg), fields(prompt_length = msg.len()))]
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
        let mut requested_model_token = prompt.model.clone().unwrap_or_else(|| "dream".to_string());
        let (selected_model, correction_message) = imagen::resolve_model(
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
        info!(requested_model = %requested_model_token, num_variations = num_images, "Starting dream image workflow");

        // Step 1: Generate multiple unique image prompts in parallel
        let router = OpenRouter::get().context("while fetching OpenRouter instance")?;
        info!(
            num_variations = num_images,
            "Requesting detective prompt variations from OpenRouter"
        );

        let prompt_futures = (0..num_images).map(|i| {
            let router = router.clone();
            let original_request = original_request.clone();
            async move {
                let detective_prompt = format!(
                    "Transform the following request into a detailed image generation prompt.
                    Add theme, lighting, and that distinctive story feel. 
                    If there is a picture, copy the style of the picture unless requested otherwise.

                    Create a unique variation - don't repeat exactly the same elements.

                    Output nothing except for the image prompt.

                    Original request: {} (Variation {})",
                    original_request,
                    i + 1
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

        info!(
            generated = generated_prompts.len(),
            "Received detective prompt variations"
        );

        // Step 2: Generate images for each prompt in parallel using the shared imagen actor
        let imagen_actor = ImagenActor::spawn(ImagenActor::default());
        let model_for_generation = requested_model_token.clone();
        let selected_model_clone = selected_model.clone();

        let image_generation_futures = generated_prompts.iter().enumerate().map(|(idx, image_prompt)| {
            let user_actor = self.user_actor.clone();
            let imagen_actor = imagen_actor.clone();
            let image_prompt = image_prompt.clone();
            let model_token = model_for_generation.clone();
            let model = selected_model_clone.clone();
            let variation = idx + 1;
            async move {
                let image_prompt_with_model = format!("{} -m {} -c 1", image_prompt, model_token);
                let mut prompt_preview: String = image_prompt.chars().take(160).collect();
                if image_prompt.len() > prompt_preview.len() {
                    prompt_preview.push_str("...");
                }
                let prompt_preview = prompt_preview.replace('\n', " ");
                debug!(variation, %model_token, prompt_preview = %prompt_preview, "Preparing imagen request for dream variation");
                let mut generate = imagen::hydrate_prompt(
                    Generate::from_str(&image_prompt_with_model)
                        .with_context(|| format!("while parsing generated prompt for variation {}", variation))?,
                    &user_actor,
                )
                .await
                .with_context(|| format!("while hydrating imagen prompt for variation {}", variation))?;
                debug!(
                    variation,
                    %model_token,
                    steps = ?generate.steps,
                    alias = ?generate.alias,
                    "Imagen prompt hydrated"
                );
                imagen::apply_model_defaults(&mut generate, &model);
                debug!(variation, %model_token, "Applied model defaults for imagen request");

                let response = imagen_actor
                    .ask(GenerateImages {
                        prompt: generate.clone(),
                        model,
                    })
                    .await
                    .map_err(|err| {
                        error!(variation, %model_token, error = ?err, "Imagen actor ask failed");
                        err
                    })
                    .with_context(|| format!("while generating image for variation {}", variation))?;
                debug!(variation, %model_token, backend = ?response.backend, image_count = response.images.len(), "Imagen actor responded");

                Ok::<_, Error>((image_prompt, image_prompt_with_model, response))
            }
        });

        let image_results = future::try_join_all(image_generation_futures)
            .await
            .context("while generating images")?;
        info!(
            num_responses = image_results.len(),
            "Completed dream image generation requests"
        );

        // Step 3: Create gallery with individual prompts
        let mut image_entries: Vec<(String, String, Arc<image::RgbImage>)> = Vec::new();
        let mut has_images = false;

        for (display_prompt, command_prompt, response) in &image_results {
            if response.images.len() != 1 {
                error!(
                    image_count = response.images.len(),
                    "Dream command expected one image per variation"
                );
                bail!("Expected 1 image per prompt, got {}", response.images.len());
            }

            let first_image = response.images.first().unwrap();
            image_entries.push((
                display_prompt.clone(),
                command_prompt.clone(),
                first_image.clone(),
            ));
            has_images = true;
        }

        // Create and upload the gallery if we have images
        let mut gallery_image_urls: Option<Vec<String>> = None;

        let gallery_url = if !image_entries.is_empty() {
            let gallery_images: Vec<GalleryImageInput> = image_entries
                .iter()
                .map(|(prompt_text, _, image)| GalleryImageInput {
                    image: image.clone(),
                    title: Some(prompt_text.clone()),
                })
                .collect();

            let title = original_request.clone();
            let subtitle = format!("Model: {}", display_model_name);

            let (url, image_urls) = upload_gallery(GalleryInput {
                title: Some(title),
                subtitle: Some(subtitle),
                images: gallery_images,
                workflow: None, // TODO: Grab the workflow from image #1 somehow.
                backend: Some(ImagenBackend::StableDiffusion.as_str().to_string()),
                generation_request: Some(prompt.clone()),
            })
            .await
            .context("while uploading dream gallery")?;

            gallery_image_urls = Some(image_urls);

            // Record the generated gallery in user's history
            let _ = self
                .user_actor
                .tell(AddGeneratedImage {
                    url: url.clone(),
                    prompt: original_request.clone(),
                    model: Some(display_model_name.clone()),
                    backend: ImagenBackend::StableDiffusion.as_str().to_string(),
                })
                .send()
                .await;

            info!(gallery = %url, total_images = image_results.len(), "Successfully created dream gallery");
            Some(url)
        } else {
            None
        };

        // Generate commentary based on results
        if has_images && gallery_url.is_some() {
            let gallery_url = gallery_url.unwrap();
            let commentary_prompt = format!(
                "As a hard-boiled detective, provide commentary on this case.                 The client requested: '{}'
                I generated {} unique interpretations, each with its own angle on the case.
                The evidence has been compiled here: {}

                Give a brief, noir-style commentary on how this multi-faceted investigation turned out.",
                original_request, num_images, gallery_url
            );

            let router = OpenRouter::get().context("while fetching OpenRouter instance")?;
            let commentary = router
                .ask(Oneshot {
                    purpose: Purpose::Chat,
                    origin: "dream commentary".to_string(),
                    text: vec![Part::Uncacheable(commentary_prompt)],
                })
                .await
                .context("while generating commentary")?;

            let images_vec: Vec<Arc<image::RgbImage>> = image_entries
                .iter()
                .map(|(_, _, image)| image.clone())
                .collect();
            let prompts_vec: Vec<String> = image_entries
                .iter()
                .map(|(_, command, _)| command.clone())
                .collect();
            let display_prompts_vec: Vec<String> = image_entries
                .iter()
                .map(|(display, _, _)| display.clone())
                .collect();

            Ok(PromptResult {
                text: commentary.text,
                image_url: Some(gallery_url),
                images: Some(images_vec),
                image_urls: gallery_image_urls,
                prompts: Some(prompts_vec),
                display_prompts: Some(display_prompts_vec),
                correction_message: correction_message.clone(),
            })
        } else {
            // No images were generated
            warn!("Dream workflow completed without any images");
            let commentary_prompt = format!(
                "As a hard-boiled detective, provide commentary on this case.                 The client requested: '{}'
                I tried {} different approaches, but all leads went cold - no images were generated.

                Give a brief, noir-style commentary on this failed investigation.",
                original_request, num_images
            );

            let router = OpenRouter::get().context("while fetching OpenRouter instance")?;
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
                image_url: gallery_url,
                images: None,
                image_urls: None,
                prompts: None,
                display_prompts: None,
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
