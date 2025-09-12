use anyhow::{Context as _, Error, Result, bail};
use kameo::{Actor, prelude::Message};
use tracing::{debug, info};

use crate::{
    messages::imagen::Generate,
    persistence::user::{GetDefaultPrompt, SetDefaultPrompt, UserActor},
};

/// Actor for the !config command - manages user configuration settings
#[derive(Actor)]
pub(crate) struct ConfigActor {
    user_actor: kameo::actor::ActorRef<UserActor>,
}

#[derive(Debug)]
pub struct ConfigResult {
    pub message: String,
}

impl ConfigActor {
    pub async fn new(user_actor: kameo::actor::ActorRef<UserActor>) -> Self {
        Self { user_actor }
    }
}

impl Message<String> for ConfigActor {
    type Reply = Result<ConfigResult, Error>;

    async fn handle(
        &mut self,
        msg: String,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        debug!("ConfigActor received message: {}", msg);

        let args = msg.trim();

        // Parse the subcommand
        let parts: Vec<&str> = args.splitn(2, ' ').collect();

        if parts.is_empty() || args.is_empty() {
            // No subcommand - show usage.
            let message = format!(
                "Usage: !config default {}",
                "[set your default settings here]".to_string()
            );

            return Ok(ConfigResult { message });
        }

        let subcommand = parts[0];

        match subcommand {
            "default" => {
                if parts.len() < 2 || parts[1].trim().is_empty() {
                    // No value provided, show current default
                    let default_prompt = self
                        .user_actor
                        .ask(GetDefaultPrompt)
                        .await
                        .context("Failed to get default prompt")?;

                    let message = format!(
                        "Usage: !config default {}",
                        default_prompt.unwrap_or("[set your default settings here]".to_string())
                    );

                    Ok(ConfigResult { message })
                } else {
                    // Set new default
                    let new_default = parts[1].trim();

                    // Validate the prompt text using the parser
                    match Generate::from_str(&new_default) {
                        Ok(_) => {
                            // Valid prompt syntax, save it
                            self.user_actor
                                .ask(SetDefaultPrompt(Some(new_default.to_string())))
                                .await
                                .context("Failed to set default prompt")?;

                            info!("User set default prompt to: {}", new_default);
                            Ok(ConfigResult {
                                message: format!(
                                    "Default prompt settings updated: {}",
                                    new_default
                                ),
                            })
                        }
                        Err(e) => {
                            // Invalid syntax
                            bail!("Invalid prompt syntax: {}", e)
                        }
                    }
                }
            }
            _ => {
                bail!("Unknown config subcommand: {}.", subcommand)
            }
        }
    }
}
