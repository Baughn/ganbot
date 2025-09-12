use anyhow::{Context as _, Error, Result, bail};
use kameo::{Actor, prelude::Message};
use tracing::{debug, info};

use crate::{
    messages::imagen::Generate,
    persistence::user::{
        GetAlias, GetAllAliases, GetDefaultPrompt, SetAlias, SetDefaultPrompt, UserActor,
    },
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
            let message =
                format!("Usage: !config default [settings] or !config alias [name] [settings]");

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
            "alias" => {
                if parts.len() < 2 || parts[1].trim().is_empty() {
                    // No alias name provided, show all aliases
                    let aliases = self
                        .user_actor
                        .ask(GetAllAliases)
                        .await
                        .context("Failed to get aliases")?;

                    if aliases.is_empty() {
                        Ok(ConfigResult {
                            message: "No aliases configured. Use !config alias [name] [settings] to add one.".to_string(),
                        })
                    } else {
                        let alias_list: Vec<String> = aliases
                            .iter()
                            .map(|(name, settings)| format!("{}: {}", name, settings))
                            .collect();
                        Ok(ConfigResult {
                            message: format!("Configured aliases:\n{}", alias_list.join("\n")),
                        })
                    }
                } else {
                    // Parse alias name and optional settings
                    let alias_args = parts[1].trim();
                    let alias_parts: Vec<&str> = alias_args.splitn(2, ' ').collect();
                    let alias_name = alias_parts[0];

                    if alias_parts.len() < 2 || alias_parts[1].trim().is_empty() {
                        // No settings provided, show current alias
                        let alias_settings = self
                            .user_actor
                            .ask(GetAlias(alias_name.to_string()))
                            .await
                            .context("Failed to get alias")?;

                        if let Some(settings) = alias_settings {
                            Ok(ConfigResult {
                                message: format!("Alias '{}': {}", alias_name, settings),
                            })
                        } else {
                            Ok(ConfigResult {
                                message: format!(
                                    "Alias '{}' not found. Use !config alias {} [settings] to create it.",
                                    alias_name, alias_name
                                ),
                            })
                        }
                    } else {
                        // Set new alias
                        let new_settings = alias_parts[1].trim();

                        // Validate the prompt text using the parser
                        match Generate::from_str(&new_settings) {
                            Ok(_) => {
                                // Valid prompt syntax, save it
                                self.user_actor
                                    .ask(SetAlias {
                                        name: alias_name.to_string(),
                                        settings: Some(new_settings.to_string()),
                                    })
                                    .await
                                    .context("Failed to set alias")?;

                                info!("User set alias '{}' to: {}", alias_name, new_settings);
                                Ok(ConfigResult {
                                    message: format!(
                                        "Alias '{}' updated: {}",
                                        alias_name, new_settings
                                    ),
                                })
                            }
                            Err(e) => {
                                // Invalid syntax
                                bail!("Invalid prompt syntax for alias: {}", e)
                            }
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
