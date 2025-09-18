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
    pub image_urls: Option<Vec<String>>,
    pub prompts: Option<Vec<String>>,
    pub display_prompts: Option<Vec<String>>,
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

        if prompt.model.is_none() {
            prompt.model = Some(model.name.clone());
        }

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
        let mut image_urls_opt: Option<Vec<String>> = None;
        let mut prompts_opt: Option<Vec<String>> = None;
        let mut display_prompts_opt: Option<Vec<String>> = None;

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

                    let (gallery_url, gallery_image_urls) = upload_gallery(GalleryInput {
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

                    image_urls_opt = Some(gallery_image_urls);
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

                        image_urls_opt = Some(vec![url.clone()]);
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

        let image_count = if !images.is_empty() {
            images.len()
        } else {
            image_urls_opt.as_ref().map(|urls| urls.len()).unwrap_or(0)
        };

        if image_count > 0 {
            let command_string = Self::command_string_for_prompt(&prompt, &model_name);
            prompts_opt = Some(vec![command_string; image_count]);
            display_prompts_opt = Some(vec![prompt.raw_prompt.clone(); image_count]);
        }

        Ok(PromptResult {
            text: text.unwrap_or_default(),
            image_url,
            images: images_opt,
            image_urls: image_urls_opt,
            prompts: prompts_opt,
            display_prompts: display_prompts_opt,
            correction_message,
        })
    }

    fn command_string_for_prompt(prompt: &Generate, resolved_model: &str) -> String {
        let mut parts = Vec::new();

        let raw = prompt.raw_prompt.trim();
        if !raw.is_empty() {
            parts.push(raw.to_string());
        }

        if !Self::raw_prompt_has_model_flag(prompt.raw_prompt.as_str()) {
            let model_token = prompt
                .model
                .as_deref()
                .unwrap_or(resolved_model)
                .to_string();
            parts.push(format!("-m {}", model_token));
        }

        parts.join(" ")
    }

    fn raw_prompt_has_model_flag(raw: &str) -> bool {
        raw.split_whitespace().any(|token| {
            matches!(token, "-m" | "--model")
                || (token.starts_with("--model=") && token.len() > "--model=".len())
                || (token.starts_with("-m") && !token.starts_with("--") && token.len() > 2)
        })
    }
}
