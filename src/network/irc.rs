use anyhow::{Context, Result, bail};
use irc::client::prelude::{Client, Command, Config};
use kameo::message::StreamMessage;
use kameo::prelude::*;
use std::collections::HashMap;
use tokio::spawn;
use tokio::sync::OnceCell;
use tokio::time::{Duration, Instant};
use tracing::{error, info, instrument, trace};

use crate::actions;
use crate::config::global::IrcConfig;
use crate::help;
use crate::persistence::user::{self, UserActor, UserManager};

pub struct IrcActor {
    name: String,
    config: IrcConfig,
    client: OnceCell<Client>,
    /// Buffer for joining split messages
    /// Keyed by (user, channel)
    message_buffer: HashMap<(String, Option<String>), BufferedMessage>,
    user_manager: ActorRef<UserManager>,
    /// Rate limiter for outgoing messages (250ms between messages, burst of 4)
    token_bucket: TokenBucket,
}

#[derive(Debug, Clone)]
struct PrivMsg {
    channel: Option<String>,
    user: String,
    message: String,
}

impl PrivMsg {
    fn get_reply_target(&self, reply_privately: bool) -> &str {
        if reply_privately {
            &self.user
        } else {
            self.channel.as_deref().unwrap_or(&self.user)
        }
    }
}

struct BufferedMessage {
    content: String,
    last_updated: Instant,
}

// Internal commands.
struct Connect;
struct ProcessBufferedMessages;
#[derive(Clone)]
struct ProcessCommand {
    irc_actor: ActorRef<IrcActor>,
    user: ActorRef<UserActor>,
    privmsg: PrivMsg,
}
struct SendReply {
    /// Implies the target to send the reply to. (User or channel)
    privmsg: PrivMsg,
    /// The actual message to send.
    message: String,
    /// Whether to reply privately to the user (via direct message) instead of the channel.
    reply_privately: bool,
}

// Internal actor for handling command processing and replies.
#[derive(Actor)]
struct ReplyActor;

/// Simple token bucket rate limiter
struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    refill_rate: f64, // tokens per second
    max_tokens: f64,
}

impl TokenBucket {
    fn new(max_tokens: f64, refill_rate: f64) -> Self {
        Self {
            tokens: max_tokens, // Start with full bucket
            last_refill: Instant::now(),
            refill_rate,
            max_tokens,
        }
    }

    async fn consume_token(&mut self) {
        loop {
            let now = Instant::now();
            let elapsed = now.duration_since(self.last_refill);

            // Refill tokens based on elapsed time
            let tokens_to_add = elapsed.as_secs_f64() * self.refill_rate;
            self.tokens = (self.tokens + tokens_to_add).min(self.max_tokens);
            self.last_refill = now;

            // If we have a token, consume it and return
            if self.tokens >= 1.0 {
                self.tokens -= 1.0;
                return;
            }

            // Calculate how long to wait for the next token
            let tokens_needed = 1.0 - self.tokens;
            let wait_time = Duration::from_secs_f64(tokens_needed / self.refill_rate);

            tokio::time::sleep(wait_time).await;
        }
    }
}

impl Actor for IrcActor {
    type Args = IrcConfig;
    type Error = anyhow::Error;

    #[instrument(skip_all, fields(server = %args.server))]
    async fn on_start(
        args: Self::Args,
        actor_ref: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        tracing::info!("Starting IRC actor for server: {}", args.server);
        actor_ref.tell(Connect).send().await?;
        Ok(IrcActor {
            name: args.server.clone(),
            config: args,
            client: OnceCell::new(),
            message_buffer: HashMap::new(),
            user_manager: UserManager::get().context("while getting UserManager")?,
            token_bucket: TokenBucket::new(4.0, 2.0), // 4 tokens max, 2 tokens/sec
        })
    }
}

impl IrcActor {
    /// Formats a message for IRC PRIVMSG, handling newlines and length limits
    /// Returns a vector of messages that can be sent individually
    fn format_privmsg(target: &str, message: &str) -> Vec<String> {
        const MAX_IRC_MESSAGE_LEN: usize = 512;

        // Calculate overhead: ":nick!user@host PRIVMSG target :\r\n"
        // We use a conservative estimate of 100 chars for the prefix
        let prefix_overhead = 100;
        let privmsg_overhead = format!(" PRIVMSG {} :", target).len();
        let total_overhead = prefix_overhead + privmsg_overhead + 2; // +2 for \r\n
        let max_content_len = MAX_IRC_MESSAGE_LEN.saturating_sub(total_overhead);

        let mut result = Vec::new();

        // Split on newlines first
        for line in message.lines() {
            if line.is_empty() {
                // Send empty lines as a single space to preserve line breaks
                result.push(" ".to_string());
                continue;
            }

            // Handle long lines by breaking on word boundaries
            if line.len() <= max_content_len {
                result.push(line.to_string());
            } else {
                // Break long lines on word boundaries
                let words: Vec<&str> = line.split_whitespace().collect();
                let mut current_line = String::new();

                for word in words {
                    // If a single word is longer than max length, we need to hard break it
                    if word.len() > max_content_len {
                        // Finish current line if it has content
                        if !current_line.is_empty() {
                            result.push(current_line.trim().to_string());
                            current_line.clear();
                        }

                        // Split the long word, respecting Unicode boundaries
                        let mut remaining = word;
                        while remaining.len() > max_content_len {
                            // Find a safe cut point that respects character boundaries
                            let mut cut_point = max_content_len;
                            while cut_point > 0 && !remaining.is_char_boundary(cut_point) {
                                cut_point -= 1;
                            }
                            // If we couldn't find a boundary, take at least one character
                            if cut_point == 0 {
                                if let Some((i, _)) = remaining.char_indices().nth(1) {
                                    cut_point = i;
                                } else {
                                    // Single character remaining
                                    cut_point = remaining.len();
                                }
                            }
                            result.push(remaining[..cut_point].to_string());
                            remaining = &remaining[cut_point..];
                        }
                        if !remaining.is_empty() {
                            current_line = remaining.to_string();
                        }
                        continue;
                    }

                    // Check if adding this word would exceed the limit
                    let test_line = if current_line.is_empty() {
                        word.to_string()
                    } else {
                        format!("{} {}", current_line, word)
                    };

                    if test_line.len() <= max_content_len {
                        current_line = test_line;
                    } else {
                        // Current line is full, start a new one
                        if !current_line.is_empty() {
                            result.push(current_line);
                        }
                        current_line = word.to_string();
                    }
                }

                // Don't forget the last line
                if !current_line.is_empty() {
                    result.push(current_line);
                }
            }
        }

        // If the original message was empty or only whitespace, send at least one message
        if result.is_empty() {
            result.push(" ".to_string());
        }

        result
    }

    /// Sends a PRIVMSG, handling newlines and IRC message length limits
    /// Rate limited to 250ms between messages with a burst capacity of 4
    async fn send_privmsg(&mut self, target: &str, message: &str) -> Result<()> {
        let client = self.client.get().context("IRC client not connected")?;

        let formatted_messages = Self::format_privmsg(target, message);

        for msg in formatted_messages {
            // Skip empty lines (represented as single space)
            if msg.trim().is_empty() {
                continue;
            }

            // Consume a token before sending each message
            self.token_bucket.consume_token().await;

            client
                .send_privmsg(target, msg)
                .context("while sending PRIVMSG")?;
        }

        Ok(())
    }

    /// Formats models help information for IRC display
    fn format_models_for_irc(models_help: &help::ModelsHelp) -> String {
        let mut output = Vec::new();

        // Header
        output.push("=== Available Models ===".to_string());
        output.push(format!("Default: {}", models_help.default));

        // Aliases section
        if !models_help.aliases.is_empty() {
            output.push("".to_string()); // Empty line
            output.push("== Aliases ==".to_string());
            for (alias, target) in &models_help.aliases {
                output.push(format!("  {} -> {}", alias, target));
            }
        }

        // Models section
        if !models_help.models.is_empty() {
            output.push("".to_string()); // Empty line
            output.push("== Models ==".to_string());
            for model in &models_help.models {
                output.push(format!("• {}", model.name));

                if let Some(ref desc) = model.description {
                    output.push(format!("  Description: {}", desc));
                }

                match &model.backend_info {
                    help::BackendInfo::NanoBanana => {
                        output.push(
                            "Backend:  NanoBanana (Gemini 2.5-flash-image-preview)".to_string(),
                        );
                    }
                    help::BackendInfo::StableDiffusion {
                        checkpoint,
                        sampler,
                        steps,
                        resolution,
                        cfg,
                        scheduler,
                        vae,
                    } => {
                        output.push("  Backend: Stable Diffusion".to_string());
                        output.push(format!("  Checkpoint: {}", checkpoint));
                        output.push(format!(
                            "  Settings: {}x{}, {} steps, CFG {:.1}",
                            resolution.0, resolution.1, steps, cfg
                        ));
                        output.push(format!("  Sampler: {} ({})", sampler, scheduler));
                        if let Some(vae_name) = vae {
                            output.push(format!("  VAE: {}", vae_name));
                        }
                    }
                }
                output.push("".to_string()); // Empty line after each model
            }
        }

        output.join("\n")
    }

    async fn process_privmsg(
        &mut self,
        privmsg: PrivMsg,
        actor_ref: ActorRef<IrcActor>,
    ) -> Result<()> {
        // First off, is this a command?
        if let Some(stripped_message) = privmsg.message.strip_prefix(&self.config.command_prefix) {
            // Handle command logic here
            info!(
                "Processing command (user {}): {}",
                privmsg.user, privmsg.message
            );
            let (command, args) = stripped_message
                .split_once(' ')
                .unwrap_or((stripped_message, ""));
            let user = self
                .user_manager
                .ask(user::GetUser(
                    user::UserId::Irc(privmsg.user.clone()),
                    privmsg.user.clone(),
                ))
                .await
                .context("while fetching user")?;

            // We've got a command, guys! Delegate to the ReplyActor to avoid blocking the IRC actor.
            ReplyActor::spawn_link(&actor_ref, ReplyActor)
                .await
                .tell(ProcessCommand {
                    irc_actor: actor_ref,
                    user,
                    privmsg: PrivMsg {
                        message: stripped_message.to_string(),
                        ..privmsg
                    },
                })
                .send()
                .await
                .context("while sending ProcessCommand")?;
        }

        Ok(())
    }
}

impl Message<SendReply> for IrcActor {
    type Reply = ();

    #[instrument(skip_all, fields(server = %self.name))]
    async fn handle(
        &mut self,
        msg: SendReply,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let target = msg.privmsg.get_reply_target(msg.reply_privately);
        if let Err(e) = self.send_privmsg(target, &msg.message).await {
            error!("Error sending reply to {}: {:#}", target, e);
        }
    }
}

impl Message<ProcessCommand> for ReplyActor {
    type Reply = ();

    async fn handle(
        &mut self,
        msg: ProcessCommand,
        ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        info!(
            "Processing command from user {}: {}",
            msg.privmsg.user, msg.privmsg.message
        );

        // Process the command directly
        let command = msg
            .privmsg
            .message
            .split_ascii_whitespace()
            .next()
            .unwrap_or(&msg.privmsg.message);
        let args = msg
            .privmsg
            .message
            .strip_prefix(command)
            .unwrap_or("")
            .trim();

        let mut reply_privately = false;

        let reply = match command {
            "ping" => Some("Pong!".to_string()),
            "ask" => {
                // Spawn AskActor to handle this command
                let ask_actor = actions::ask::AskActor::spawn_link(
                    &ctx.actor_ref(),
                    actions::ask::AskActor::new().await,
                )
                .await;
                let ask_result = ask_actor.ask(args.to_string()).await;
                match ask_result {
                    Ok(result) => Some(result.response),
                    Err(e) => Some(format!("Error: {e:#}")),
                }
            }
            "combine" => {
                // Spawn Combine actor to handle this command
                let combine_actor = actions::combine::CombineActor::spawn_link(
                    &ctx.actor_ref(),
                    actions::combine::CombineActor::new().await,
                )
                .await;
                let combine_result = combine_actor.ask(args.to_string()).await;
                match combine_result {
                    Ok(result) => {
                        // Format the result nicely with image URL
                        let response = format!(
                            "{}\n**{}**\n{}",
                            result.image_url, result.result, result.reasoning,
                        );
                        Some(response)
                    }
                    Err(e) => Some(format!("Error: {e:#}")),
                }
            }
            "prompt" => {
                // Spawn Prompt actor to handle image generation
                let prompt_actor = actions::prompt::PromptActor::spawn_link(
                    &ctx.actor_ref(),
                    actions::prompt::PromptActor::new(msg.user.clone()).await,
                )
                .await;
                let prompt_result = prompt_actor.ask(args.to_string()).await;
                match prompt_result {
                    Ok(result) => {
                        // Return text and optional image URL
                        match result.image_url {
                            Some(image_url) => Some(format!(
                                "{}: {} {}",
                                msg.privmsg.user, result.text, image_url
                            )),
                            None => Some(format!("{}\n(No image)", result.text)),
                        }
                    }
                    Err(e) => Some(format!("Error: {e:#}")),
                }
            }
            "select" => {
                // Spawn SelectActor to handle this command
                let select_actor = actions::select::SelectActor::spawn_link(
                    &ctx.actor_ref(),
                    actions::select::SelectActor::new(msg.user.clone()).await,
                )
                .await;
                let select_result = select_actor.ask(args.to_string()).await;
                match select_result {
                    Ok(result) => Some(result.message),
                    Err(e) => Some(format!("Error: {e:#}")),
                }
            }
            "edit" => {
                // Spawn EditActor to handle this command
                let edit_actor = actions::edit::EditActor::spawn_link(
                    &ctx.actor_ref(),
                    actions::edit::EditActor::new(msg.user.clone()).await,
                )
                .await;
                let edit_result = edit_actor.ask(args.to_string()).await;
                match edit_result {
                    Ok(result) => {
                        // Return text and optional image URL
                        match result.image_url {
                            Some(image_url) => Some(format!(
                                "{}: {} {}",
                                msg.privmsg.user, result.text, image_url
                            )),
                            None => Some(format!("{}\n(No image)", result.text)),
                        }
                    }
                    Err(e) => Some(format!("Error: {e:#}")),
                }
            }
            "models" => {
                // Fetch and format models configuration
                match help::get_models_help().await {
                    Ok(models_help) => {
                        let formatted = IrcActor::format_models_for_irc(&models_help);
                        reply_privately = true;
                        Some(formatted)
                    }
                    Err(e) => Some(format!("Error fetching models: {e:#}")),
                }
            }
            x => {
                info!("Unknown command: {}", x);
                None
            }
        };

        // Send the reply back to the user if we have one
        if let Some(reply_message) = reply {
            let _ = msg
                .irc_actor
                .tell(SendReply {
                    privmsg: msg.privmsg,
                    message: reply_message,
                    reply_privately,
                })
                .send()
                .await;
        }
    }
}

impl Message<Connect> for IrcActor {
    type Reply = Result<()>;

    #[instrument(skip_all, fields(server = %self.name))]
    async fn handle(
        &mut self,
        msg: Connect,
        ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        info!("Connecting to IRC server: {}", self.config.server);
        if self.client.get().is_some() {
            bail!("Already connected to IRC server: {}", self.config.server);
        }

        // Create IRC client configuration
        let irc_config = Config {
            server: Some(self.config.server.clone()),
            port: Some(self.config.port),
            use_tls: Some(self.config.tls),
            nickname: Some(self.config.nick.clone()),
            channels: self.config.channels.clone(),
            ..Default::default()
        };

        // Try to connect
        let mut client = tokio::time::timeout(Duration::from_secs(20), async {
            Client::from_config(irc_config)
                .await
                .context("while creating IRC client")
        })
        .await??;

        // Identify to the server
        client
            .identify()
            .context("while identifying to IRC server")?;

        let stream = client.stream()?;
        ctx.actor_ref().attach_stream(stream, "start", "end");

        // Identify with NickServ if password is provided
        if let Some(password) = &self.config.nickserv_password {
            tracing::debug!("Authenticating with NickServ");
            client
                .send_privmsg("nickserv", format!("IDENTIFY {}", password))
                .context("while sending NickServ IDENTIFY command")?;
        }

        // Store the client in the actor
        self.client
            .set(client)
            .context("while setting IRC client")?;
        tracing::info!("Connected to IRC server: {}", self.config.server);

        Ok(())
    }
}

impl
    Message<
        StreamMessage<Result<irc::proto::Message, irc::error::Error>, &'static str, &'static str>,
    > for IrcActor
{
    type Reply = ();

    #[instrument(skip_all, fields(server = %self.name))]
    async fn handle(
        &mut self,
        msg: StreamMessage<Result<irc::proto::Message, irc::error::Error>, &str, &str>,
        ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        trace!("Received IRC message: {:?}", msg);

        match msg {
            StreamMessage::Started(marker) => {
                info!("IRC stream started: {}", marker);
            }
            StreamMessage::Next(Ok(message)) => {
                if let Err(e) = handle_irc_message(message, self, ctx).await {
                    error!("Error processing IRC message: {e:#}");
                }
            }
            StreamMessage::Next(Err(e)) => {
                error!("Error in IRC stream: {e:#}");
                info!("Stopping IRC actor due to stream error, supervisor will restart");
                let _ = ctx.actor_ref().stop_gracefully().await;
            }
            StreamMessage::Finished(marker) => {
                info!("IRC stream finished: {}", marker);
                info!("Stopping IRC actor for restart");
                let _ = ctx.actor_ref().stop_gracefully().await;
            }
        }
    }
}

async fn handle_irc_message(
    message: irc::proto::Message,
    actor: &mut IrcActor,
    ctx: &mut kameo::prelude::Context<IrcActor, ()>,
) -> Result<()> {
    trace!("Handling IRC message: {:?}", message);

    match message.command {
        Command::PRIVMSG(ref target, ref text) => {
            let channel = if target.starts_with('#') {
                Some(target.clone())
            } else {
                None
            };

            let user = message.source_nickname().unwrap_or("unknown").to_string();
            let key = (user.clone(), channel.clone());

            // Check if we already have a buffered message from this user/channel
            if let Some(buffered) = actor.message_buffer.get_mut(&key) {
                // Append to existing message and update timestamp
                buffered.content.push(' ');
                buffered.content.push_str(text);
                buffered.last_updated = Instant::now();
                trace!("Appending to buffered message from {:?}", key);
            } else {
                // Start a new buffered message
                actor.message_buffer.insert(
                    key.clone(),
                    BufferedMessage {
                        content: text.clone(),
                        last_updated: Instant::now(),
                    },
                );
                trace!("Starting new buffered message from {:?}", key);
            }

            // Either way, process the buffered messages in 500ms or so.
            let actor_ref = ctx.actor_ref().clone();
            spawn(async move {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let _ = actor_ref.tell(ProcessBufferedMessages).try_send();
            });
        }
        _ => {
            trace!("Unhandled IRC command: {:?}", message.command);
        }
    }

    Ok(())
}

impl Message<ProcessBufferedMessages> for IrcActor {
    type Reply = ();

    #[instrument(skip_all, fields(server = %self.name))]
    async fn handle(
        &mut self,
        _msg: ProcessBufferedMessages,
        ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        trace!("Processing buffered messages");

        let now = Instant::now();
        let mut to_flush = Vec::new();

        // Check and flush messages that have been idle for 500ms
        for (key, buffered) in &self.message_buffer {
            if now.duration_since(buffered.last_updated) >= Duration::from_millis(500) {
                to_flush.push(key.clone());
            }
        }

        // Process old messages
        for key in to_flush {
            if let Some(buffered) = self.message_buffer.remove(&key) {
                let (user, channel) = key;
                let privmsg = PrivMsg {
                    channel,
                    user,
                    message: buffered.content,
                };
                trace!("Flushing buffered message: {:?}", privmsg);
                if let Err(e) = self.process_privmsg(privmsg.clone(), ctx.actor_ref()).await {
                    // Log full error chain with Debug format for complete details
                    error!("Error processing buffered message: {:#}", e);

                    // And attempt to notify the user.
                    let reply_target = privmsg.get_reply_target(false);
                    let error_message = format!("{}: {e:#}", privmsg.user);
                    let _ = self.send_privmsg(reply_target, &error_message).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_privmsg_simple_message() {
        let result = IrcActor::format_privmsg("#test", "Hello world");
        assert_eq!(result, vec!["Hello world"]);
    }

    #[test]
    fn test_format_privmsg_empty_message() {
        let result = IrcActor::format_privmsg("#test", "");
        assert_eq!(result, vec![" "]);
    }

    #[test]
    fn test_format_privmsg_whitespace_only() {
        let result = IrcActor::format_privmsg("#test", "   ");
        assert_eq!(result, vec!["   "]);
    }

    #[test]
    fn test_format_privmsg_newlines() {
        let result = IrcActor::format_privmsg("#test", "Line 1\nLine 2\nLine 3");
        assert_eq!(result, vec!["Line 1", "Line 2", "Line 3"]);
    }

    #[test]
    fn test_format_privmsg_empty_lines() {
        let result = IrcActor::format_privmsg("#test", "Line 1\n\nLine 3");
        assert_eq!(result, vec!["Line 1", " ", "Line 3"]);
    }

    #[test]
    fn test_format_privmsg_multiple_empty_lines() {
        let result = IrcActor::format_privmsg("#test", "Line 1\n\n\nLine 4");
        assert_eq!(result, vec!["Line 1", " ", " ", "Line 4"]);
    }

    #[test]
    fn test_format_privmsg_long_line_word_breaking() {
        // Create a long message that needs breaking
        let long_message = "word1 word2 word3 ".repeat(30); // Much longer than IRC limit
        let result = IrcActor::format_privmsg("#test", &long_message);

        // Should be broken into multiple messages
        assert!(result.len() > 1);

        // Each message should be within reasonable length
        for msg in &result {
            assert!(msg.len() <= 400); // Conservative estimate of max content length
        }

        // All messages should contain actual words (not just spaces)
        for msg in &result {
            assert!(msg.trim().len() > 0);
        }
    }

    #[test]
    fn test_format_privmsg_very_long_single_word() {
        // A single word longer than the IRC limit
        let long_word = "a".repeat(500);
        let result = IrcActor::format_privmsg("#test", &long_word);

        // Should be broken into multiple messages
        assert!(result.len() > 1);

        // When concatenated, should recreate the original word
        let reconstructed = result.join("");
        assert_eq!(reconstructed, long_word);
    }

    #[test]
    fn test_format_privmsg_mixed_newlines_and_long_lines() {
        let mixed_content = format!(
            "Short line\n{}\nAnother short line\n{}",
            "long ".repeat(50),
            "also_long ".repeat(40)
        );

        let result = IrcActor::format_privmsg("#test", &mixed_content);

        // Should have multiple messages
        assert!(result.len() > 3);

        // First message should be the short line
        assert_eq!(result[0], "Short line");

        // Should contain "Another short line" somewhere
        assert!(result.iter().any(|msg| msg == "Another short line"));
    }

    #[test]
    fn test_format_privmsg_preserves_leading_trailing_spaces() {
        let result = IrcActor::format_privmsg("#test", "  spaced content  ");
        assert_eq!(result, vec!["  spaced content  "]);
    }

    #[test]
    fn test_format_privmsg_different_target_lengths() {
        // Test with different target lengths to ensure overhead calculation works
        let message = "test message";

        let short_target_result = IrcActor::format_privmsg("#t", message);
        let long_target_result = IrcActor::format_privmsg("#very-long-channel-name", message);

        // Both should work and produce the same result for a short message
        assert_eq!(short_target_result, vec![message]);
        assert_eq!(long_target_result, vec![message]);
    }

    #[test]
    fn test_format_privmsg_boundary_case() {
        // Create a message that's exactly at the boundary
        const MAX_IRC_MESSAGE_LEN: usize = 512;
        let target = "#test";
        let prefix_overhead = 100;
        let privmsg_overhead = format!(" PRIVMSG {} :", target).len();
        let total_overhead = prefix_overhead + privmsg_overhead + 2;
        let max_content_len = MAX_IRC_MESSAGE_LEN - total_overhead;

        let boundary_message = "a".repeat(max_content_len);
        let result = IrcActor::format_privmsg(target, &boundary_message);

        // Should fit in exactly one message
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], boundary_message);

        // One character more should split into two
        let over_boundary_message = "a".repeat(max_content_len + 1);
        let over_result = IrcActor::format_privmsg(target, &over_boundary_message);
        assert!(over_result.len() > 1);
    }

    #[test]
    fn test_format_privmsg_unicode_handling() {
        // Test with Unicode characters that may take multiple bytes
        let unicode_message = "Hello 世界 🌍 café";
        let result = IrcActor::format_privmsg("#test", unicode_message);
        assert_eq!(result, vec![unicode_message]);

        // Test with long Unicode message
        let long_unicode = "🌍".repeat(200);
        let unicode_result = IrcActor::format_privmsg("#test", &long_unicode);

        // Should be broken appropriately
        assert!(unicode_result.len() >= 1);

        // When joined, should preserve the original
        let reconstructed = unicode_result.join("");
        assert_eq!(reconstructed, long_unicode);
    }
}
