use anyhow::{Context as _, Error, Result, bail};
use image::RgbImage;
use kameo::{Actor, prelude::Message};
use redis::AsyncTypedCommands;
use serde::{Deserialize, Serialize};

use crate::{messages::chat::Oneshot, network::openrouter::OpenRouter, supervisor::Supervisor};

/// Combination game actor.
/// This represents a single instance of the !combine command,
/// which takes 2 to 3 words and combines them into a new word.
#[derive(Actor)]
pub(crate) struct CombineActor {
    redis: redis::aio::MultiplexedConnection,
}

pub struct Combine(pub String);
#[derive(Debug, Serialize, Deserialize)]
pub struct CombineResult {
    pub result: String,
    pub reasoning: String,
    pub image_url: String,
}

#[derive(Deserialize)]
struct CombineChatResponse {
    result: String,
    reasoning: String,
    image_prompt: String,
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
        // Check cache first.
        if let Some(cached) = self.get_from_cache(&words.0, &words.1).await? {
            return Ok(cached);
        }
        let result = self.combine(&words.0, &words.1).await?;
        self.set_cache(&words.0, &words.1, &result).await?;
        Ok(result)
    }
}

impl CombineActor {
    pub async fn new() -> Self {
        let redis = Supervisor::redis().await;
        Self { redis }
    }

    async fn combine(&self, word1: &str, word2: &str) -> Result<CombineResult> {
        let router = OpenRouter::get().context("while fetching OpenRouter instance")?;
        let response = router.ask(Oneshot {
            purpose: crate::messages::chat::Purpose::Chat,
            origin: "Combination game".to_string(),
            text: vec![
                crate::messages::chat::Part::Cacheable(CONSTANT_PROMPT.to_string()),
                crate::messages::chat::Part::Uncacheable(format!(
                    "Now, combine these words: {} + {}",
                    word1, word2
                )),
            ],
        });
        let response = response.await.context("while asking OpenRouter")?;
        let response_text = response.text.trim();
        let parsed: CombineChatResponse = serde_json::from_str(response_text).context(format!(
            "Failed to parse response as JSON: {}\nResponse was:\n{}",
            serde_json::to_string_pretty(&response_text).unwrap_or_default(),
            response_text
        ))?;

        todo!()
    }

    fn cache_key(word1: &str, word2: &str) -> String {
        format!("combine:{}:{}", word1.to_lowercase(), word2.to_lowercase())
    }

    async fn get_from_cache(
        &mut self,
        word1: &str,
        word2: &str,
    ) -> Result<Option<CombineResult>, Error> {
        let key = Self::cache_key(word1, word2);
        let cached = self.redis.get(&key).await?;
        if let Some(cached_str) = cached {
            let result: CombineResult = serde_json::from_str(&cached_str)?;
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    async fn set_cache(
        &mut self,
        word1: &str,
        word2: &str,
        result: &CombineResult,
    ) -> Result<(), Error> {
        let key = Self::cache_key(word1, word2);
        let value = serde_json::to_string(result)?;
        self.redis.set(&key, value).await?;
        Ok(())
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
