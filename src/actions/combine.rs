use anyhow::{Error, bail};
use image::RgbImage;
use kameo::{Actor, actor::ActorRef, prelude::Message};

use crate::supervisor::Supervisor;

/// Combination game actor.
/// This represents a single instance of the !combine command,
/// which takes 2 to 3 words and combines them into a new word.
#[derive(Actor)]
pub struct CombineActor;

pub struct Combine(pub String);

pub struct CombineResult {
    pub result: String,
    pub reasoning: String,
    pub image: RgbImage,
}

const CONSTANT_PROMPT: &str = include_str!("../../prompts/combine_prompt.tmpl");

fn split_words(input: &str) -> Result<(String, String), Error> {
    let cleaned = input
        .chars()
        .map(|c| match c {
            ',' | '+' | '&' | '|' | '/' | ';' | ':' | '-' | '_' | '=' | '*' | '~' | '!' | '?'
            | '.' => ' ',
            c => c,
        })
        .collect::<String>();

    let words: Vec<String> = cleaned.split_whitespace().map(|s| s.to_string()).collect();

    match words.len() {
        2 => Ok((words[0].clone(), words[1].clone())),
        0 => bail!("No words provided. Please provide exactly 2 words to combine."),
        1 => bail!("Only one word provided. Please provide exactly 2 words to combine."),
        _ => bail!(
            "Too many words provided ({}). Please provide exactly 2 words to combine.",
            words.len()
        ),
    }
}

impl Message<String> for CombineActor {
    type Reply = Result<CombineResult, Error>;

    async fn handle(
        &mut self,
        msg: String,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let words = split_words(&msg)?;
        let redis = Supervisor::redis().await;
        // Check cache first.
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_words_basic() {
        assert_eq!(
            split_words("fire water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("hello world").unwrap(),
            ("hello".to_string(), "world".to_string())
        );
    }

    #[test]
    fn test_split_words_with_separators() {
        assert_eq!(
            split_words("fire, water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire + water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire&water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire|water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire/water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire-water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire_water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire:water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire;water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire=water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire*water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire~water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire!water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire?water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire.water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
    }

    #[test]
    fn test_split_words_multiple_separators() {
        assert_eq!(
            split_words("fire, + water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire & + water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire,,,water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
    }

    #[test]
    fn test_split_words_extra_spaces() {
        assert_eq!(
            split_words("  fire  water  ").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("fire    water").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
        assert_eq!(
            split_words("\tfire\twater\t").unwrap(),
            ("fire".to_string(), "water".to_string())
        );
    }

    #[test]
    fn test_split_words_preserve_case() {
        assert_eq!(
            split_words("Fire Water").unwrap(),
            ("Fire".to_string(), "Water".to_string())
        );
        assert_eq!(
            split_words("FIRE WATER").unwrap(),
            ("FIRE".to_string(), "WATER".to_string())
        );
        assert_eq!(
            split_words("FiRe WaTeR").unwrap(),
            ("FiRe".to_string(), "WaTeR".to_string())
        );
    }

    #[test]
    fn test_split_words_errors() {
        assert!(split_words("").is_err());
        assert!(split_words("   ").is_err());
        assert!(split_words("fire").is_err());
        assert!(split_words("fire water earth").is_err());
        assert!(split_words("fire water earth air").is_err());
        assert!(split_words(",,,").is_err());
        assert!(split_words("+++").is_err());
    }

    #[test]
    fn test_split_words_error_messages() {
        let err = split_words("").unwrap_err();
        assert!(err.to_string().contains("No words provided"));

        let err = split_words("fire").unwrap_err();
        assert!(err.to_string().contains("Only one word provided"));

        let err = split_words("fire water earth").unwrap_err();
        assert!(err.to_string().contains("Too many words provided (3)"));
    }
}
