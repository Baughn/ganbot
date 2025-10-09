use anyhow::{Context as _, Error, Result, bail};
use futures::future;
use kameo::{Actor, Reply, prelude::Message};
use rand::seq::IndexedRandom as _;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

#[derive(Clone, Copy)]
struct CommentarySeed {
    shared_thread: &'static str,
    clue_image: &'static str,
    signoff_hint: &'static str,
}

const COMMENTARY_SEEDS: &[CommentarySeed] = &[
    CommentarySeed {
        shared_thread: "the rain-slick promise that started the job",
        clue_image: "reflections smeared across midnight glass",
        signoff_hint: "the storm still hanging overhead",
    },
    CommentarySeed {
        shared_thread: "that cigarette ember of hope the client handed over",
        clue_image: "hazy neon humming outside the office blinds",
        signoff_hint: "smoke curling toward dawn",
    },
    CommentarySeed {
        shared_thread: "the heartbeat of a jazz trio down the block",
        clue_image: "shadows stretching long in the club lights",
        signoff_hint: "the last trumpet wail",
    },
    CommentarySeed {
        shared_thread: "footsteps echoing through the warehouse of memory",
        clue_image: "dust motes swirling in the projector beam",
        signoff_hint: "the hush before the reel snaps",
    },
    CommentarySeed {
        shared_thread: "the copper tang of truth hiding in the files",
        clue_image: "folders spread like tarot on the desk",
        signoff_hint: "the drawer closing with a click",
    },
];

use crate::{
    actions::{
        ActionProgressEmitter,
        imagen::{self, BatchInfo, GenerateImagesRequest, ImagenBackend},
        prompt::PromptResult,
    },
    messages::{
        chat::{Oneshot, Part, Purpose, Structured},
        imagen::Generate,
    },
    network::openrouter::{OpenRouter, structured::JsonSchemaDefinition},
    persistence::{
        images::{GalleryImageInput, GalleryInput, GalleryLayout, upload_gallery},
        user::{AddGeneratedImage, UserActor},
    },
    supervisor::Supervisor,
};

/// Dream command actor that generates image prompts with hard-boiled detective personality
#[derive(Actor)]
pub struct DreamActor {
    user_actor: kameo::actor::ActorRef<UserActor>,
    progress: Option<ActionProgressEmitter>,
}

#[derive(Debug, Serialize, Deserialize, Reply)]
struct DreamPromptResponse {
    image_prompt: String,
    aspect_ratio: String,
}

impl Message<String> for DreamActor {
    type Reply = Result<PromptResult, Error>;

    #[tracing::instrument(name = "DreamActor.generate", skip(self, _ctx, msg), fields(prompt_length = msg.len()))]
    async fn handle(
        &mut self,
        msg: String,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        debug!("DreamActor received message: {}", msg);

        if let Some(progress) = &self.progress {
            progress.progress(Some(1.0), "Drafting detective variations…");
        }

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

                    Also suggest an appropriate aspect ratio for this image based on the subject and composition.
                    Use landscape ratios (16:9, 2:1, etc.) for wide scenes, portrait ratios (9:16, 2:3, etc.) for tall subjects.
                    Avoid 1:1 square ratios unless the composition truly demands it.

                    Output JSON with two fields:
                    - image_prompt: the image generation prompt (text only, no flags or commands)
                    - aspect_ratio: suggested aspect ratio in \"width:height\" format (e.g., \"16:9\", \"3:2\", \"9:16\")

                    Original request: {} (Variation {})",
                    original_request,
                    i + 1
                );

                router
                    .ask(Structured::<DreamPromptResponse> {
                        purpose: Purpose::Dream,
                        origin: "dream".to_string(),
                        text: vec![Part::Uncacheable(detective_prompt)],
                        schema: dream_prompt_schema(),
                        marker: std::marker::PhantomData,
                    })
                    .await
            }
        });

        let generated_prompts = future::try_join_all(prompt_futures)
            .await
            .context("while generating image prompts")?;

        info!(
            generated = generated_prompts.len(),
            "Received detective prompt variations"
        );

        // Parse aspect ratios from all variations and calculate average
        let aspect_ratios: Vec<(u32, u32)> = generated_prompts
            .iter()
            .map(|response| parse_aspect_ratio(&response.aspect_ratio))
            .collect::<Result<Vec<_>>>()
            .context("while parsing aspect ratios from LLM responses")?;

        let averaged_aspect = calculate_average_aspect_ratio(&aspect_ratios)?;
        info!(
            aspect_ratios = ?aspect_ratios,
            averaged_aspect = ?averaged_aspect,
            "Calculated average aspect ratio from variations"
        );

        if let Some(progress) = &self.progress {
            progress.progress(Some(8.0), "Prompts locked in; dispatching to the lab…");
        }

        // Step 2: Generate images for each prompt in parallel via the shared imagen coordinator
        let model_for_generation = requested_model_token.clone();
        let selected_model_clone = selected_model.clone();

        let progress = self.progress.clone();
        let total_batches = generated_prompts.len() as u32;
        let image_generation_futures = generated_prompts.iter().enumerate().map(|(idx, response)| {
            let user_actor = self.user_actor.clone();
            let image_prompt = response.image_prompt.clone();
            let model_token = model_for_generation.clone();
            let model = selected_model_clone.clone();
            let variation = idx + 1;
            let progress = progress.clone();
            let aspect = averaged_aspect;
            async move {
                let mut prompt_preview: String = image_prompt.chars().take(160).collect();
                if image_prompt.len() > prompt_preview.len() {
                    prompt_preview.push_str("...");
                }
                let prompt_preview = prompt_preview.replace('\n', " ");
                debug!(variation, %model_token, prompt_preview = %prompt_preview, aspect = ?aspect, "Preparing imagen request for dream variation");

                // Build Generate struct directly with averaged aspect ratio
                let base_generate = Generate {
                    raw_prompt: image_prompt.clone(),
                    prompt: image_prompt.clone(),
                    negative_prompt: None,
                    num_images: Some(1),
                    aspect: Some(aspect),
                    width: None,
                    height: None,
                    model: Some(model_token.clone()),
                    seed: None,
                    steps: None,
                    references: crate::messages::imagen::References {
                        img2img: None,
                        img2img_strength: None,
                        context: Vec::new(),
                    },
                    alias: None,
                };

                let mut generate = imagen::hydrate_prompt(base_generate, &user_actor)
                    .await
                    .with_context(|| format!("while hydrating imagen prompt for variation {}", variation))?;
                debug!(
                    variation,
                    %model_token,
                    steps = ?generate.steps,
                    alias = ?generate.alias,
                    aspect = ?generate.aspect,
                    "Imagen prompt hydrated"
                );
                imagen::apply_model_defaults(&mut generate, &model);
                debug!(variation, %model_token, "Applied model defaults for imagen request");

                let batch = Some(BatchInfo {
                    position: variation as u32,
                    total: total_batches,
                });

                let response = imagen::submit_generation(GenerateImagesRequest {
                    prompt: generate.clone(),
                    model,
                    progress,
                    batch,
                })
                .await
                .map_err(|err| {
                    error!(variation, %model_token, error = ?err, "Imagen generation failed");
                    err
                })
                .with_context(|| format!("while generating image for variation {}", variation))?;
                debug!(variation, %model_token, backend = ?response.backend, image_count = response.images.len(), "Imagen actor responded");

                Ok::<_, Error>((image_prompt, response))
            }
        });

        let image_results = future::try_join_all(image_generation_futures)
            .await
            .context("while generating images")?;
        info!(
            num_responses = image_results.len(),
            "Completed dream image generation requests"
        );

        if let Some(progress) = &self.progress {
            progress.progress(Some(96.0), "Case closed; packaging evidence…");
        }

        // Step 3: Create gallery with individual prompts
        let mut image_entries: Vec<(String, Arc<image::RgbImage>)> = Vec::new();
        let mut has_images = false;

        for (display_prompt, response) in &image_results {
            if response.images.len() != 1 {
                error!(
                    image_count = response.images.len(),
                    "Dream command expected one image per variation"
                );
                bail!("Expected 1 image per prompt, got {}", response.images.len());
            }

            let first_image = response.images.first().unwrap();
            image_entries.push((display_prompt.clone(), first_image.clone()));
            has_images = true;
        }

        // Create and upload the gallery if we have images
        let mut gallery_image_urls: Option<Vec<String>> = None;
        let mut gallery_layout: Option<GalleryLayout> = None;
        let mut gallery_id: Option<String> = None;
        let gallery_url = if !image_entries.is_empty() {
            let gallery_images: Vec<GalleryImageInput> = image_entries
                .iter()
                .map(|(prompt_text, image)| GalleryImageInput {
                    image: image.clone(),
                    title: Some(prompt_text.clone()),
                })
                .collect();

            let title = original_request.clone();
            let subtitle = format!("Model: {}", display_model_name);

            let uploaded = upload_gallery(GalleryInput {
                title: Some(title),
                subtitle: Some(subtitle),
                images: gallery_images,
                workflow: None, // TODO: Grab the workflow from image #1 somehow.
                backend: Some(ImagenBackend::StableDiffusion.as_str().to_string()),
                generation_request: Some(prompt.clone()),
            })
            .await
            .context("while uploading dream gallery")?;

            gallery_image_urls = Some(uploaded.image_urls.clone());
            gallery_layout = Some(uploaded.layout.clone());
            gallery_id = Some(uploaded.id.clone());

            // Record the generated gallery in user's history
            let _ = self
                .user_actor
                .tell(AddGeneratedImage {
                    url: uploaded.gallery_url.clone(),
                    prompt: original_request.clone(),
                    model: Some(display_model_name.clone()),
                    backend: ImagenBackend::StableDiffusion.as_str().to_string(),
                })
                .send()
                .await;

            info!(gallery = %uploaded.gallery_url, total_images = image_results.len(), "Successfully created dream gallery");
            Some(uploaded.gallery_url)
        } else {
            None
        };

        // Generate commentary based on results
        let commentary_seed = {
            let mut rng = rand::rng();
            *COMMENTARY_SEEDS
                .choose(&mut rng)
                .unwrap_or(&COMMENTARY_SEEDS[0])
        };

        if has_images && gallery_url.is_some() {
            let gallery_url = gallery_url.unwrap();
            let commentary_prompt = format!(
                "You are a hard-boiled detective closing a case with a smoky, confident monologue.\n\
Speak in first person and open by naming the shared thread tying every clue together: {shared}.\n\
The client requested: '{request}'. Call the gallery the evidence locker, but avoid directly mentioning the url ({url}).\n\
You gathered {count} clues; thread them into one investigation, folding contrasts in naturally as angles glimpsed through {clue}.\n\
Write 1-3 paragraphs, no bullets or lists, and never use words like variation, version, or attempt.\n\
Sign off with a punchy one-liner that nods to {signoff}.\n\
Output only the monologue.",
                shared = commentary_seed.shared_thread,
                request = original_request,
                url = gallery_url,
                count = num_images,
                clue = commentary_seed.clue_image,
                signoff = commentary_seed.signoff_hint,
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

            // Build command prompts with the averaged aspect ratio for storage
            let prompts_vec: Vec<String> = image_entries
                .iter()
                .map(|(display_prompt, _)| {
                    format!(
                        "{} -m {} --ar {}:{} -c 1",
                        display_prompt, display_model_name, averaged_aspect.0, averaged_aspect.1
                    )
                })
                .collect();
            let display_prompts_vec: Vec<String> = image_entries
                .iter()
                .map(|(display, _)| display.clone())
                .collect();

            Ok(PromptResult {
                text: commentary.text,
                image_url: Some(gallery_url),
                image_urls: gallery_image_urls,
                prompts: Some(prompts_vec),
                display_prompts: Some(display_prompts_vec),
                gallery_id,
                gallery_layout,
                correction_message: correction_message.clone(),
            })
        } else {
            // No images were generated
            warn!("Dream workflow completed without any images");
            let commentary_prompt = format!(
                "You are a hard-boiled detective reflecting on a case that slipped through your fingers.\n\
Speak in first person, keep the voice smoky and steady, and anchor everything to the shared thread: {shared}.\n\
The client requested: '{request}'. You chased {count} leads, but each clue dissolved like {clue}.\n\
Deliver one tight paragraph with no bullets or lists, and never use words like variation, version, or attempt.\n\
Lament the empty evidence locker, yet sign off with a wry one-liner that nods to {signoff}.\n\
Output only the monologue.",
                shared = commentary_seed.shared_thread,
                request = original_request,
                count = num_images,
                clue = commentary_seed.clue_image,
                signoff = commentary_seed.signoff_hint,
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
                image_urls: None,
                prompts: None,
                display_prompts: None,
                gallery_id: None,
                gallery_layout: None,
                correction_message: correction_message.clone(),
            })
        }
    }
}

/// Create JSON schema for DreamPromptResponse
fn dream_prompt_schema() -> JsonSchemaDefinition {
    use serde_json::json;

    let mut properties = serde_json::Map::new();
    properties.insert(
        "image_prompt".to_string(),
        json!({
            "type": "string",
            "description": "The image generation prompt (text only, no flags or commands)"
        }),
    );
    properties.insert(
        "aspect_ratio".to_string(),
        json!({
            "type": "string",
            "description": "Suggested aspect ratio in 'width:height' format (e.g., '16:9', '3:2', '9:16')"
        }),
    );

    JsonSchemaDefinition {
        schema_type: "object".to_string(),
        properties,
        required: Some(vec!["image_prompt".to_string(), "aspect_ratio".to_string()]),
        additional_properties: Some(false),
    }
}

/// Parse an aspect ratio string like "16:9" into a tuple (16, 9)
fn parse_aspect_ratio(s: &str) -> Result<(u32, u32)> {
    let separators = [':', 'x', 'X', '/', '-'];

    for sep in separators {
        if s.contains(sep) {
            let parts: Vec<&str> = s.split(sep).collect();
            if parts.len() == 2 {
                let width = parts[0]
                    .trim()
                    .parse::<u32>()
                    .with_context(|| format!("Invalid aspect ratio width: {}", parts[0]))?;
                let height = parts[1]
                    .trim()
                    .parse::<u32>()
                    .with_context(|| format!("Invalid aspect ratio height: {}", parts[1]))?;

                if width == 0 || height == 0 {
                    bail!("Aspect ratio dimensions must be non-zero");
                }

                return Ok((width, height));
            }
        }
    }

    bail!(
        "Invalid aspect ratio format: {}. Expected format like 16:9, 16x9, or 16/9",
        s
    )
}

/// Calculate the average aspect ratio from multiple aspect ratios
fn calculate_average_aspect_ratio(ratios: &[(u32, u32)]) -> Result<(u32, u32)> {
    if ratios.is_empty() {
        bail!("Cannot calculate average aspect ratio from empty list");
    }

    // Convert each ratio to a decimal value, average them, then find closest common ratio
    let sum: f64 = ratios.iter().map(|(w, h)| *w as f64 / *h as f64).sum();
    let avg_ratio = sum / ratios.len() as f64;

    // Common aspect ratios to consider
    let common_ratios = [
        (1, 1),   // 1:1 square
        (4, 3),   // 4:3
        (3, 2),   // 3:2
        (16, 10), // 16:10
        (16, 9),  // 16:9
        (21, 9),  // 21:9 ultrawide
        (2, 1),   // 2:1
        (3, 4),   // 3:4 portrait
        (2, 3),   // 2:3 portrait
        (9, 16),  // 9:16 portrait
    ];

    // Find the closest common ratio
    let mut best_ratio = common_ratios[0];
    let mut best_diff = (common_ratios[0].0 as f64 / common_ratios[0].1 as f64 - avg_ratio).abs();

    for &ratio in &common_ratios[1..] {
        let ratio_value = ratio.0 as f64 / ratio.1 as f64;
        let diff = (ratio_value - avg_ratio).abs();
        if diff < best_diff {
            best_diff = diff;
            best_ratio = ratio;
        }
    }

    Ok(best_ratio)
}

impl DreamActor {
    pub async fn new(
        user_actor: kameo::actor::ActorRef<UserActor>,
        progress: Option<ActionProgressEmitter>,
    ) -> Self {
        Self {
            user_actor,
            progress,
        }
    }
}
