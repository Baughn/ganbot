use anyhow::{Context as _, Error, Result};
use kameo::{Actor, prelude::Message};
use std::sync::Arc;
use tracing::{debug, info};

use crate::{
    actions::imagen::{self, GenerateImages, ImagenActor, ImagenBackend, ImagenResponse},
    messages::imagen::Generate,
    persistence::{
        images::{GalleryImageInput, GalleryInput, upload_gallery, upload_image_with_generation},
        user::{AddGeneratedImage, UserActor},
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
    pub images: Option<Vec<Arc<image::RgbImage>>>,
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

        let prompt = imagen::hydrate_prompt(Generate::from_str(&msg)?, &self.user_actor).await?;
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
        let prompt = imagen::hydrate_prompt(prompt, &self.user_actor).await?;
        self.process_generate(prompt).await
    }
}

impl PromptActor {
    pub async fn new(user_actor: kameo::actor::ActorRef<UserActor>) -> Self {
        Self { user_actor }
    }

    async fn process_generate(&mut self, mut prompt: Generate) -> Result<PromptResult, Error> {
        let models_config = Supervisor::models_config().await;
        let (model, correction_message) =
            imagen::resolve_model(&prompt.prompt, &models_config, prompt.model.as_deref())?;

        imagen::apply_model_defaults(&mut prompt, &model);

        let response = ImagenActor::spawn(ImagenActor::default())
            .ask(GenerateImages {
                prompt: prompt.clone(),
                model,
            })
            .await
            .context("while generating images")?;

        self.upload_and_format_response(prompt, response, correction_message)
            .await
    }

    async fn upload_and_format_response(
        &self,
        prompt: Generate,
        response: ImagenResponse,
        correction_message: Option<String>,
    ) -> Result<PromptResult> {
        let ImagenResponse {
            images,
            text,
            workflow,
            backend,
            model_name,
            seed,
        } = response;

        let mut image_url = None;
        let mut images_opt = if images.is_empty() {
            None
        } else {
            Some(images.clone())
        };

        if !images.is_empty() {
            match backend {
                ImagenBackend::StableDiffusion => {
                    let gallery_images: Vec<GalleryImageInput> = images
                        .iter()
                        .cloned()
                        .map(|image| GalleryImageInput { image, title: None })
                        .collect();

                    let subtitle = seed
                        .map(|seed| format!("Model: {}, Seed: {}", model_name, seed))
                        .unwrap_or_else(|| format!("Model: {}", model_name));

                    let (gallery_url, _) = upload_gallery(GalleryInput {
                        title: Some(prompt.raw_prompt.clone()),
                        subtitle: Some(subtitle),
                        images: gallery_images,
                        workflow: workflow.clone(),
                        backend: Some(backend.as_str().to_string()),
                        generation_request: Some(prompt.clone()),
                    })
                    .await
                    .context("while uploading image gallery")?;
                    info!(
                        "Successfully generated and uploaded image gallery: {}",
                        gallery_url
                    );

                    let _ = self
                        .user_actor
                        .tell(AddGeneratedImage {
                            url: gallery_url.clone(),
                            prompt: prompt.raw_prompt.clone(),
                            model: Some(model_name.clone()),
                            backend: backend.as_str().to_string(),
                        })
                        .send()
                        .await;

                    image_url = Some(gallery_url);
                }
                ImagenBackend::NanoBanana => {
                    if let Some(first_image) = images.first() {
                        let url = upload_image_with_generation(
                            Arc::clone(first_image),
                            workflow.clone(),
                            Some(backend.as_str().to_string()),
                            Some(prompt.clone()),
                        )
                        .await
                        .context("while uploading generated image")?;
                        info!("Successfully generated and uploaded image: {}", url);

                        let _ = self
                            .user_actor
                            .tell(AddGeneratedImage {
                                url: url.clone(),
                                prompt: prompt.raw_prompt.clone(),
                                model: Some(model_name.clone()),
                                backend: backend.as_str().to_string(),
                            })
                            .send()
                            .await;

                        image_url = Some(url);
                    } else {
                        info!("No image generated, text-only response");
                        images_opt = None;
                    }
                }
            }
        } else if matches!(backend, ImagenBackend::NanoBanana) {
            info!("No image generated, text-only response");
        }

        Ok(PromptResult {
            text: text.unwrap_or_default(),
            image_url,
            images: images_opt,
            correction_message,
        })
    }
}
