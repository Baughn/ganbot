/// Image generation requests & responses.
use image::RgbImage;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Represents a user-initiated request to generate an image.
/// Any unset value will be based on defaults for the model.
#[derive(Clone, Serialize, Deserialize)]
pub struct Generate {
    /// The original, unparsed prompt.
    pub raw_prompt: String,
    /// The prompt for the image generation.
    pub prompt: String,
    /// Optional negative prompt to avoid certain features in the generated image.
    pub negative_prompt: Option<String>,
    /// The number of images to generate.
    pub num_images: Option<u32>,
    /// The requested aspect ratio of the generated image.
    pub aspect: Option<(u32, u32)>,
    /// The width of the generated image.
    pub width: Option<u32>,
    /// The height of the generated image.
    pub height: Option<u32>,
    /// The model to use for image generation.
    pub model: Option<String>,
    /// Optional seed for reproducibility.
    pub seed: Option<u64>,
    /// Number of inference steps to use.
    pub steps: Option<u32>,
    /// References to images that can be used as starting points or for context.
    pub references: References,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct References {
    /// A starting-point image for img2img generation.
    #[serde(skip)]
    pub img2img: Option<RgbImage>,
    pub img2img_strength: Option<f32>,
    /// A reference image for Kontext / Qwen-Image-Edit.
    #[serde(skip)]
    pub context: Vec<RgbImage>,
}

/// Represents a response containing the generated image.
/// This is typically sent back to the user or a channel.
#[derive(Clone)]
pub struct GeneratedImage {
    /// The generated image data.
    pub image: RgbImage,
    /// The original request that triggered this generation.
    pub request: Generate,
}

impl fmt::Debug for Generate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Generate")
            .field("raw_prompt", &self.raw_prompt)
            .field("prompt", &self.prompt)
            .field("negative_prompt", &self.negative_prompt)
            .field("num_images", &self.num_images)
            .field("aspect", &self.aspect)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("model", &self.model)
            .field("seed", &self.seed)
            .field("steps", &self.steps)
            .field("references", &self.references)
            .finish()
    }
}

impl fmt::Debug for References {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("References")
            .field(
                "img2img",
                &self
                    .img2img
                    .as_ref()
                    .map(|img| format!("RgbImage({}x{})", img.width(), img.height())),
            )
            .field("img2img_strength", &self.img2img_strength)
            .field(
                "context",
                &format!("[{} context images]", self.context.len()),
            )
            .finish()
    }
}

impl fmt::Debug for GeneratedImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeneratedImage")
            .field(
                "image",
                &format!("RgbImage({}x{})", self.image.width(), self.image.height()),
            )
            .field("request", &self.request)
            .finish()
    }
}
