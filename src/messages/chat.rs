use crate::network::openrouter::structured::JsonSchemaDefinition;

/// Message types for chatbot interactions.

#[derive(Debug, Clone)]
pub enum Purpose {
    /// General chat style interactions, e.g. casual conversation.
    Chat,
    /// Dream prompt generation with hard-boiled detective personality.
    Dream,
}

#[derive(Debug, Clone)]
pub enum Part {
    Cacheable(String),
    Uncacheable(String),
}

/// Simple one-shot requests.
#[derive(Debug, Clone)]
pub struct Oneshot {
    /// Used to select the model for the request.
    pub purpose: Purpose,
    /// The origin of the message, i.e. "user@channel@server". For logging and debugging.
    pub origin: String,
    /// The text that should be sent to the chatbot API.
    pub text: Vec<Part>,
}

#[derive(Debug, Clone)]
pub struct OneshotResponse {
    /// The response text from the chatbot API.
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Structured<T> {
    /// Used to select the model for the request.
    pub purpose: Purpose,
    /// The origin of the message, i.e. "user@channel@server". For logging and debugging.
    pub origin: String,
    /// The text that should be sent to the chatbot API.
    pub text: Vec<Part>,
    /// JSON schema for structured response.
    pub schema: JsonSchemaDefinition,
    /// Phantom type for the expected response type.
    pub marker: std::marker::PhantomData<T>,
}

#[derive(Debug, Clone)]
pub struct NanoBanana {
    /// The origin of the message, i.e. "user@channel@server". For logging and debugging.
    pub origin: String,
    /// The image prompt.
    pub prompt: String,
    /// Optional input image for editing mode.
    pub input_image: Option<std::sync::Arc<image::RgbImage>>,
}

#[derive(Debug, Clone)]
pub struct NanoBananaResponse {
    /// The text response from the AI.
    pub text: String,
    /// Optional image if one was generated.
    pub image: Option<image::RgbImage>,
}
