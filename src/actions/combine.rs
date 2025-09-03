use anyhow::Error;
use image::RgbImage;
use kameo::{Actor, actor::ActorRef};

/// Combination game actor.
/// This represents a single instance of the !combine command,
/// which takes 2 to 3 words and combines them into a new word.
#[derive(Actor)]
pub struct Combine {
    args: String,
    requester: kameo::reply::ReplySender<CombineResult>,
}

pub struct CombineResult {
    pub result: String,
    pub reasoning: String,
    pub image: RgbImage,
}

const CONSTANT_PROMPT: &str = include_str!("../../prompts/combine_prompt.tmpl");
