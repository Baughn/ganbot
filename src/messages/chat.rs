/// Message types for chatbot interactions.

#[derive(Debug, Clone)]
pub enum Purpose {
    /// General chat style interactions, e.g. casual conversation.
    Chat,
    /// Image generation request enhancements, e.g. "Follow these rules to improve the following prompt".
    Image,
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
