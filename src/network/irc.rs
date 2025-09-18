use anyhow::{Context, Result, bail};
use irc::client::prelude::{Client, Command, Config, Response};
use kameo::message::StreamMessage;
use kameo::prelude::*;
use std::collections::HashMap;
use tokio::spawn;
use tokio::sync::OnceCell;
use tokio::time::{Duration, Instant};
use tracing::{error, info, instrument, trace, warn};

use crate::actions;
use crate::config::global::IrcConfig;
use crate::help;
use crate::persistence::user::{self, UserActor, UserManager};

mod sasl;
use self::sasl::SaslManager;

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
    /// Rate limiter to prevent reordering (100ms between messages, no burst)
    token_bucket_fast: TokenBucket,
    /// Cache for user identification status
    /// Maps nickname -> (is_identified, cache_time)
    identification_cache: HashMap<String, (bool, Instant)>,
    /// Pending WHOIS requests to avoid duplicates
    /// Maps nickname -> timestamp when request was sent
    pending_whois: HashMap<String, Instant>,
    sasl: SaslManager,
    joined_channels: bool,
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
struct CheckUserIdentified {
    nickname: String,
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
        let sasl = SaslManager::new(&args);
        let mut actor = IrcActor {
            name: args.server.clone(),
            config: args,
            client: OnceCell::new(),
            message_buffer: HashMap::new(),
            user_manager: UserManager::get().context("while getting UserManager")?,
            token_bucket: TokenBucket::new(4.0, 2.0), // 4 tokens max, 2 tokens/sec
            token_bucket_fast: TokenBucket::new(1.0, 10.0),
            identification_cache: HashMap::new(),
            pending_whois: HashMap::new(),
            sasl,
            joined_channels: false,
        };
        actor.connect(&actor_ref).await?;
        Ok(actor)
    }

    async fn on_stop(
        &mut self,
        _actor_ref: WeakActorRef<Self>,
        reason: ActorStopReason,
    ) -> std::result::Result<(), Self::Error> {
        self.send_quit(format!("Disconnecting ({reason:?})"));
        Ok(())
    }
}

impl IrcActor {
    #[instrument(skip_all, fields(server = %self.name))]
    async fn connect(&mut self, actor_ref: &ActorRef<IrcActor>) -> Result<()> {
        info!("Connecting to IRC server: {}", self.config.server);
        if self.client.get().is_some() {
            bail!("Already connected to IRC server: {}", self.config.server);
        }

        self.sasl.configure(&self.config);
        self.joined_channels = false;

        let irc_config = Config {
            server: Some(self.config.server.clone()),
            port: Some(self.config.port),
            use_tls: Some(self.config.tls),
            nickname: Some(self.config.nick.clone()),
            channels: Vec::new(),
            ..Default::default()
        };

        let mut client = tokio::time::timeout(Duration::from_secs(20), async {
            Client::from_config(irc_config)
                .await
                .context("while creating IRC client")
        })
        .await??;

        let stream = client.stream()?;
        actor_ref.attach_stream(stream, "start", "end");
        self.client
            .set(client)
            .context("while setting IRC client")?;
        let client = self.client.get().expect("client initialized");

        if self.sasl.is_enabled() {
            self.sasl
                .begin(client)
                .context("while initiating SASL negotiation")?;
            self.send_registration(client)
                .context("while sending registration commands")?;
        } else {
            client
                .identify()
                .context("while identifying to IRC server")?;

            if let Some(password) = &self.config.nickserv_password {
                tracing::debug!("Authenticating with NickServ");
                client
                    .send_privmsg("nickserv", format!("IDENTIFY {}", password))
                    .context("while sending NickServ IDENTIFY command")?;
            }
        }
        tracing::info!("Connected to IRC server: {}", self.config.server);

        Ok(())
    }

    fn send_registration(&self, client: &Client) -> Result<()> {
        client
            .send(Command::NICK(self.config.nick.clone()))
            .context("while sending NICK command")?;

        client
            .send(Command::USER(
                self.config.nick.clone(),
                "0".to_string(),
                self.config.nick.clone(),
            ))
            .context("while sending USER command")?;

        Ok(())
    }

    fn join_configured_channels(&mut self) -> Result<()> {
        if self.joined_channels {
            return Ok(());
        }

        let client = self.client.get().context("IRC client not connected")?;
        for channel in &self.config.channels {
            trace!(channel = %channel, "Joining configured IRC channel");
            client
                .send_join(channel)
                .context("while sending JOIN command")?;
        }

        self.joined_channels = true;
        info!(channels = ?self.config.channels, "Joined configured IRC channels");
        Ok(())
    }

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
        let mut empty_lines = 0;
        for line in message.lines() {
            if line.is_empty() && empty_lines < 1 {
                // Send empty lines as a single space to preserve line breaks
                result.push(" ".to_string());
                empty_lines += 1;
                continue;
            } else {
                empty_lines = 0;
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

    fn send_quit(&self, message: impl AsRef<str>) {
        if let Some(client) = self.client.get() {
            if let Err(error) = client.send_quit(message.as_ref()) {
                warn!(?error, "Failed to send IRC QUIT message");
            }
        }
    }

    /// Sends a PRIVMSG, handling newlines and IRC message length limits
    /// Rate limited to 250ms between messages with a burst capacity of 4
    async fn send_privmsg(&mut self, target: &str, message: &str) -> Result<()> {
        let client = self.client.get().context("IRC client not connected")?;

        let formatted_messages = Self::format_privmsg(target, message);

        for msg in formatted_messages {
            // Only skip truly empty strings; allow single-space lines to preserve blanks
            if msg.is_empty() {
                continue;
            }

            // Consume a token before sending each message
            self.token_bucket_fast.consume_token().await;
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
                    help::BackendInfo::ComfyUI {
                        checkpoint,
                        sampler,
                        steps,
                        resolution,
                        cfg,
                        scheduler,
                    } => {
                        output.push("  Backend: Stable Diffusion".to_string());
                        output.push(format!("  Checkpoint: {}", checkpoint));
                        output.push(format!(
                            "  Settings: {}x{}, {} steps, CFG {:.1}",
                            resolution.0, resolution.1, steps, cfg
                        ));
                        output.push(format!("  Sampler: {} ({})", sampler, scheduler));
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

    /// Check if a user is identified with NickServ
    /// Returns cached result if available and not expired (5 minutes TTL)
    /// Otherwise sends WHOIS and returns None to indicate pending status
    fn check_user_identified(&mut self, nickname: &str) -> Result<Option<bool>> {
        let now = Instant::now();

        // Check cache first
        if let Some((is_identified, cache_time)) = self.identification_cache.get(nickname)
            && now.duration_since(*cache_time) < Duration::from_secs(300)
        {
            // 5 minutes TTL
            trace!(
                "Using cached identification status for {}: {}",
                nickname, is_identified
            );
            return Ok(Some(*is_identified));
        }

        // Check if we already have a pending WHOIS request (avoid spam)
        if let Some(request_time) = self.pending_whois.get(nickname)
            && now.duration_since(*request_time) < Duration::from_secs(3)
        {
            // Still waiting for previous WHOIS response
            return Ok(None);
        }

        // Send WHOIS command and mark as pending
        let client = self.client.get().context("IRC client not connected")?;
        self.pending_whois.insert(nickname.to_string(), now);

        trace!("Sending WHOIS for {}", nickname);
        client.send(Command::WHOIS(
            Some(nickname.to_string()),
            nickname.to_string(),
        ))?;

        // Return None to indicate we need to wait for the response
        Ok(None)
    }

    /// Invalidate identification cache for a user
    fn invalidate_user_cache(&mut self, nickname: &str, reason: &str) {
        if self.identification_cache.remove(nickname).is_some() {
            trace!(
                "Invalidated identification cache for {} ({})",
                nickname, reason
            );
        }
        self.pending_whois.remove(nickname);
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

impl Message<CheckUserIdentified> for IrcActor {
    type Reply = Result<Option<bool>>;

    #[instrument(skip_all, fields(server = %self.name, nick = %msg.nickname))]
    async fn handle(
        &mut self,
        msg: CheckUserIdentified,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.check_user_identified(&msg.nickname)
    }
}

impl ReplyActor {
    /// Check if user is identified with NickServ with retry logic
    /// Returns true if identified, false if not identified
    async fn check_user_identified_with_retry(
        irc_actor: &ActorRef<IrcActor>,
        nickname: String,
    ) -> Result<bool> {
        let mut is_identified = false;
        let mut attempts = 0;
        let max_attempts = 5;

        while attempts < max_attempts {
            match irc_actor
                .ask(CheckUserIdentified {
                    nickname: nickname.clone(),
                })
                .await
            {
                Ok(Some(identified)) => {
                    is_identified = identified;
                    break;
                }
                Ok(None) => {
                    // WHOIS is pending, wait and retry
                    attempts += 1;
                    if attempts < max_attempts {
                        let delay_ms = 150 * (1 << (attempts - 1)); // 150, 300, 600, 1200ms
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                }
                Err(e) => {
                    error!("Failed to check user identification: {e:#}");
                    break;
                }
            }
        }

        Ok(is_identified)
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
                        // Format the result nicely with image URL and correction message
                        let mut response = format!(
                            "{}\n**{}**\n{}",
                            result.image_url, result.result, result.reasoning,
                        );

                        // Add correction message if present
                        if let Some(correction_msg) = result.correction_message {
                            response = format!("{}\n({})", response, correction_msg);
                        }

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
                        // Return text and optional image URL with correction message
                        let base_response = match result.image_url {
                            Some(image_url) => {
                                format!("{}: {} {}", msg.privmsg.user, result.text, image_url)
                            }
                            None => format!("{}\n(No image)", result.text),
                        };

                        // Add correction message if present
                        let final_response = if let Some(correction_msg) = result.correction_message
                        {
                            format!("{}\n({})", base_response, correction_msg)
                        } else {
                            base_response
                        };

                        Some(final_response)
                    }
                    Err(e) => Some(format!("Error: {e:#}")),
                }
            }
            "dream" => {
                // Spawn Dream actor to handle this command
                let dream_actor = actions::dream::DreamActor::spawn_link(
                    &ctx.actor_ref(),
                    actions::dream::DreamActor::new(msg.user.clone()).await,
                )
                .await;
                let dream_result = dream_actor.ask(args.to_string()).await;
                match dream_result {
                    Ok(result) => {
                        // Format similar to !prompt command
                        let base_response = match result.image_url {
                            Some(image_url) => {
                                format!("{}: {} {}", msg.privmsg.user, result.text, image_url)
                            }
                            None => format!("{}\n(No image)", result.text),
                        };
                        // Add correction message if present
                        let final_response = if let Some(correction_msg) = result.correction_message
                        {
                            format!("{}\n({})", base_response, correction_msg)
                        } else {
                            base_response
                        };
                        Some(final_response)
                    }
                    Err(e) => Some(format!("Error: {e:#}")),
                }
            }
            "config" => {
                // Check if user is identified before allowing config command
                let is_identified = match Self::check_user_identified_with_retry(
                    &msg.irc_actor,
                    msg.privmsg.user.clone(),
                )
                .await
                {
                    Ok(identified) => identified,
                    Err(e) => {
                        error!("Failed to check user identification: {e:#}");
                        false
                    }
                };

                if !is_identified {
                    Some("You must be identified with NickServ to use this command.".to_string())
                } else {
                    // Spawn ConfigActor to handle this command
                    let config_actor = actions::config::ConfigActor::spawn_link(
                        &ctx.actor_ref(),
                        actions::config::ConfigActor::new(msg.user.clone()).await,
                    )
                    .await;
                    let config_result = config_actor.ask(args.to_string()).await;
                    match config_result {
                        Ok(result) => Some(result.message),
                        Err(e) => Some(format!("Error: {e:#}")),
                    }
                }
            }
            "select" => {
                // Check if user is identified before allowing select command
                let is_identified = match Self::check_user_identified_with_retry(
                    &msg.irc_actor,
                    msg.privmsg.user.clone(),
                )
                .await
                {
                    Ok(identified) => identified,
                    Err(e) => {
                        error!("Failed to check user identification: {e:#}");
                        false
                    }
                };

                if !is_identified {
                    Some("You must be identified with NickServ to use this command.".to_string())
                } else {
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
                        // Return text and optional image URL with correction message
                        let base_response = match result.image_url {
                            Some(image_url) => {
                                format!("{}: {} {}", msg.privmsg.user, result.text, image_url)
                            }
                            None => format!("{}\n(No image)", result.text),
                        };

                        // Add correction message if present
                        let final_response = if let Some(correction_msg) = result.correction_message
                        {
                            format!("{}\n({})", base_response, correction_msg)
                        } else {
                            base_response
                        };

                        Some(final_response)
                    }
                    Err(e) => Some(format!("Error: {e:#}")),
                }
            }
            "delete" => {
                // Check if user is identified before allowing delete command
                let is_identified = match Self::check_user_identified_with_retry(
                    &msg.irc_actor,
                    msg.privmsg.user.clone(),
                )
                .await
                {
                    Ok(identified) => identified,
                    Err(e) => {
                        error!("Failed to check user identification: {e:#}");
                        false
                    }
                };

                if !is_identified {
                    Some("You must be identified with NickServ to use this command.".to_string())
                } else {
                    // Spawn DeleteActor to handle this command
                    let delete_actor = actions::delete::DeleteActor::spawn_link(
                        &ctx.actor_ref(),
                        actions::delete::DeleteActor::new(msg.user.clone()).await,
                    )
                    .await;
                    let delete_result = delete_actor.ask(args.to_string()).await;
                    match delete_result {
                        Ok(result) => Some(result.message),
                        Err(e) => Some(format!("Error: {e:#}")),
                    }
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
                self.send_quit("Stream error, reconnecting");
                info!("Stopping IRC actor due to stream error, supervisor will restart");
                let _ = ctx.actor_ref().stop_gracefully().await;
            }
            StreamMessage::Finished(marker) => {
                info!("IRC stream finished: {}", marker);
                self.send_quit("Stream finished, reconnecting");
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

    if let Some(client) = actor.client.get() {
        actor
            .sasl
            .handle_message(&message, client)
            .context("while processing SASL negotiation")?;
    }

    match message.command {
        Command::Response(Response::RPL_ENDOFMOTD, _)
        | Command::Response(Response::ERR_NOMOTD, _) => {
            if actor.sasl.allows_channel_join() {
                actor
                    .join_configured_channels()
                    .context("while joining configured IRC channels")?;
            } else {
                let reason = actor
                    .sasl
                    .failure_reason()
                    .unwrap_or("SASL authentication not confirmed");
                error!(state = ?actor.sasl.state(), reason, "Dying due to SASL state");
                let _ = ctx.actor_ref().stop_gracefully().await;
            }
        }
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
        Command::QUIT(ref reason) => {
            // User quit - invalidate their identification cache
            let user = message.source_nickname().unwrap_or("unknown").to_string();
            let quit_reason = format!("quit: {:?}", reason);
            actor.invalidate_user_cache(&user, &quit_reason);
        }
        Command::NICK(ref new_nick) => {
            // User changed nick - invalidate old nick cache
            let old_nick = message.source_nickname().unwrap_or("unknown").to_string();
            let nick_reason = format!("nick change to {}", new_nick);
            actor.invalidate_user_cache(&old_nick, &nick_reason);
        }
        Command::KICK(ref _channel, ref kicked_nick, ref _reason) => {
            // User was kicked - invalidate their identification cache
            actor.invalidate_user_cache(kicked_nick, "kicked");
        }
        Command::PART(ref _channel, ref reason) => {
            // User left channel - invalidate their identification cache
            let user = message.source_nickname().unwrap_or("unknown").to_string();
            let part_reason = format!("part: {:?}", reason);
            actor.invalidate_user_cache(&user, &part_reason);
        }
        Command::Response(response, args) => {
            // Handle numeric responses, particularly WHOIS responses
            match response {
                irc::client::prelude::Response::RPL_ENDOFWHOIS => {
                    // End of WHOIS - if we haven't seen identification, user is not identified
                    if let Some(nickname) = args.get(1)
                        && !actor.identification_cache.contains_key(nickname)
                    {
                        actor
                            .identification_cache
                            .insert(nickname.clone(), (false, Instant::now()));
                        trace!("WHOIS completed for {} - not identified", nickname);
                    }
                }
                irc::client::prelude::Response::RPL_LOGGEDIN => {
                    // User is logged in (RPL_LOGGEDIN)
                    if let Some(nickname) = args.first() {
                        actor
                            .identification_cache
                            .insert(nickname.clone(), (true, Instant::now()));
                        trace!("User {} is identified (RPL_LOGGEDIN)", nickname);
                    }
                }
                _ => {
                    trace!("Other WHOIS response: {:?} {:?}", response, args);
                }
            }
        }
        Command::Raw(command, args) => {
            // Handle raw numeric codes that might not be in the Response enum
            if let Ok(numeric_code) = command.parse::<u16>() {
                match numeric_code {
                    330 => {
                        // RPL_WHOISACCOUNT - user is identified
                        if let Some(nickname) = args.get(1) {
                            actor
                                .identification_cache
                                .insert(nickname.clone(), (true, Instant::now()));
                            trace!("User {} is identified (330 response)", nickname);
                        }
                    }
                    307 => {
                        // RPL_WHOISREGNICK - user is registered
                        if let Some(nickname) = args.get(1) {
                            actor
                                .identification_cache
                                .insert(nickname.clone(), (true, Instant::now()));
                            trace!("User {} is identified (307 response)", nickname);
                        }
                    }
                    _ => {
                        trace!("Unhandled raw numeric {}: {:?}", numeric_code, args);
                    }
                }
            } else {
                trace!("Unhandled raw command: {} {:?}", command, args);
            }
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
                if let Err(e) = self
                    .process_privmsg(privmsg.clone(), ctx.actor_ref().clone())
                    .await
                {
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
        assert_eq!(result, vec!["Line 1", " ", "", "Line 4"]);
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
