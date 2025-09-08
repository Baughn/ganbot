use anyhow::{Context as _, Error, Result, bail};
use kameo::{Actor, Reply, prelude::Message};
use openrouter_api::models::structured::JsonSchemaDefinition;
use rand::seq::IndexedRandom as _;
use redis::AsyncTypedCommands;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, info};

use crate::{
    messages::chat::Structured, network::openrouter::OpenRouter, persistence::images::upload_image,
    supervisor::Supervisor, util,
};

/// Combination game actor.
/// This represents a single instance of the !combine command,
/// which takes 2 to 3 words and combines them into a new word.
#[derive(Actor)]
pub(crate) struct CombineActor {
    redis: redis::aio::ConnectionManager,
}

pub struct Combine(pub String);
#[derive(Debug, Serialize, Deserialize)]
pub struct CombineResult {
    pub result: String,
    pub reasoning: String,
    pub image_url: String,
}

#[derive(Deserialize, Reply)]
struct CombineChatResponse {
    result: String,
    reasoning: String,
    image_prompt: String,
}

const CONSTANT_PROMPT: &str = include_str!("../../prompts/combine_prompt.tmpl");

const ELEMENTS: &[&str] = &["air", "earth", "fire", "water"];

fn split_words(input: &str) -> Result<(String, String), Error> {
    let cleaned = input
        .to_ascii_lowercase()
        .chars()
        .map(|c| match c {
            ',' => ' ',
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
        debug!("CombineActor received message: {}", msg);
        let words = split_words(&msg)?;
        info!("Combining words: {} + {}", words.0, words.1);
        // Confirm that both words have been unlocked.
        for word in [&words.0, &words.1] {
            if !ELEMENTS.contains(&word.as_str()) && self.check_basis(word).await?.is_none() {
                bail!(
                    "The word '{}' has not been unlocked yet. Please use the basic elements: air, earth, fire, water.",
                    word
                );
            }
        }
        // Check cache.
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
        let focus = self.get_random_focus();
        info!("Using focus: {}", focus);
        let response = router.ask::<Structured<CombineChatResponse>>(Structured {
            purpose: crate::messages::chat::Purpose::Chat,
            origin: "combination game".to_string(),
            text: vec![
                crate::messages::chat::Part::Cacheable(CONSTANT_PROMPT.to_string()),
                crate::messages::chat::Part::Uncacheable(focus),
                crate::messages::chat::Part::Uncacheable(format!(
                    "Now, combine these words: {} + {}",
                    word1, word2
                )),
            ],
            schema: JsonSchemaDefinition {
                schema_type: "object".to_string(),
                properties: json!({
                    "result": {
                        "type": "string",
                        "description": "The new word or short concept. Must not contain spaces."
                    },
                    "reasoning": {
                        "type": "string",
                        "description": "An evocative explanation, one paragraph in length."
                    },
                    "image_prompt": {
                        "type": "string",
                        "description": "A clever image-generation prompt for a brushwork style illustration of the result, untitled."
                    }
                }).as_object().unwrap().clone(),
                required: vec![
                    "result".to_string(),
                    "reasoning".to_string(),
                    "image_prompt".to_string(),
                ].into(),
                additional_properties: None,
            },
            marker: std::marker::PhantomData,
        });
        let response: CombineChatResponse = response.await.context("while asking OpenRouter")?;

        // Generate image.
        let image_response = router
            .ask(crate::messages::chat::NanoBanana {
                origin: "combination game".to_string(),
                prompt: response.image_prompt,
                input_image: None,
            })
            .await
            .context("while asking OpenRouter for image")?;

        // And get the image URL.
        let image_url = if let Some(image) = image_response.image {
            upload_image(image).await.context("while uploading image")?
        } else {
            return Err(anyhow::anyhow!("No image generated for combination"));
        };

        Ok(CombineResult {
            result: response.result,
            reasoning: response.reasoning,
            image_url,
        })
    }

    /// Randomly returns a theme, e.g. scientific, metaphorical, poetic, etc.
    fn get_random_focus(&self) -> String {
        let themes = [
            "obvious",
            "obvious",
            "obvious",
            "obvious",
            "obvious",
            "obvious",
            "metaphorical",
            "obvious",
            "poetic",
            "basic",
            "ironic",
            "basic",
            "humorous",
            "scientific",
            "philosophical",
        ];
        themes
            .choose(&mut rand::rng())
            .map(|s| format!("Focus on a {} combination.", s))
            .unwrap()
    }

    fn cache_key_combine(_word1: &str, _word2: &str) -> String {
        "combine:combinations".to_string()
    }

    fn cache_key_basis(_word: &str) -> String {
        "combine:basis".to_string()
    }

    fn combination_field_name(word1: &str, word2: &str) -> String {
        format!("{}:{}", word1.to_lowercase(), word2.to_lowercase())
    }

    fn basis_field_name(word: &str) -> String {
        word.to_lowercase()
    }

    async fn get_from_cache(
        &mut self,
        word1: &str,
        word2: &str,
    ) -> Result<Option<CombineResult>, Error> {
        let key = Self::cache_key_combine(word1, word2);
        let field = Self::combination_field_name(word1, word2);
        let cached = util::retry(|| async {
            let mut conn = self.redis.clone();
            conn.hget(key.clone(), field.clone())
                .await
                .context("Redis hget failed")
        })
        .await?;
        if let Some(cached_str) = cached {
            let result: CombineResult = serde_json::from_str(&cached_str)?;
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    /// Check if this word has been constructed before. (And if so, return the combination that created it)
    async fn check_basis(&mut self, word: &str) -> Result<Option<String>> {
        let key = Self::cache_key_basis(word);
        let field = Self::basis_field_name(word);
        let source = util::retry(|| async {
            let mut conn = self.redis.clone();
            conn.hget(key.clone(), field.clone())
                .await
                .context("Redis hget failed")
        })
        .await?;
        Ok(source)
    }

    async fn set_cache(
        &mut self,
        word1: &str,
        word2: &str,
        result: &CombineResult,
    ) -> Result<(), Error> {
        let combination_key = Self::cache_key_combine(word1, word2);
        let combination_field = Self::combination_field_name(word1, word2);
        let basis_key = Self::cache_key_basis(&result.result);
        let basis_field = Self::basis_field_name(&result.result);
        let value = serde_json::to_string(result)?;
        util::retry(|| async {
            let mut conn = self.redis.clone();
            conn.hset(
                combination_key.clone(),
                combination_field.clone(),
                value.clone(),
            )
            .await
            .context("Redis hset failed")?;
            conn.hset(
                basis_key.clone(),
                basis_field.clone(),
                combination_field.clone(),
            )
            .await
            .context("Redis hset failed")
        })
        .await?;
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
