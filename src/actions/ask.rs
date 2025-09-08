/// Ask command actor that forwards questions to AI with noir detective persona
use anyhow::{Context, Result, bail};
use kameo::{Actor, prelude::Message};
use tracing::{debug, info};

use crate::{
    messages::chat::{Oneshot, Part, Purpose},
    network::openrouter::OpenRouter,
};

/// Ask command actor that handles AI Q&A with noir detective persona
#[derive(Actor)]
pub(crate) struct AskActor;

#[derive(Debug)]
pub struct AskResult {
    pub response: String,
}

impl Message<String> for AskActor {
    type Reply = Result<AskResult>;

    async fn handle(
        &mut self,
        msg: String,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        debug!("AskActor received question: {}", msg);

        // Validate the question
        if msg.trim().is_empty() {
            bail!("Question cannot be empty");
        }

        info!("Processing question: {}", msg);

        // Get the OpenRouter instance
        let router = OpenRouter::get().context("while fetching OpenRouter instance")?;

        // Format the question with noir detective persona
        let noir_prompt = format!(
            "Answer the following question in the style of a hard-boiled noir detective. Be dramatic, cynical, and use classic noir language and metaphors: {}",
            msg.trim()
        );

        // Send to OpenRouter
        let response = router
            .ask(Oneshot {
                purpose: Purpose::Chat,
                origin: "ask command".to_string(),
                text: vec![Part::Uncacheable(noir_prompt)],
            })
            .await
            .context("while asking OpenRouter")?;

        Ok(AskResult {
            response: response.text,
        })
    }
}

impl AskActor {
    pub async fn new() -> Self {
        Self
    }
}
