use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use irc::client::prelude::{Capability, Client};
use irc::proto::{CapSubCommand, Command, Message, NegotiationVersion, Response};
use tracing::{debug, info, trace, warn};

use crate::config::global::IrcConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaslState {
    Disabled,
    WaitingForLs,
    WaitingForAck,
    WaitingForChallenge,
    WaitingForResult,
    Authenticated,
    Failed,
    Unsupported,
}

#[derive(Debug, Clone)]
struct SaslCredentials {
    username: String,
    password: String,
}

#[derive(Debug)]
pub struct SaslManager {
    credentials: Option<SaslCredentials>,
    state: SaslState,
    cap_end_sent: bool,
    cap_req_sent: bool,
    supports_sasl: bool,
    error_reason: Option<String>,
}

impl SaslManager {
    pub fn new(config: &IrcConfig) -> Self {
        let mut manager = Self {
            credentials: SaslCredentials::from_config(config),
            state: SaslState::Disabled,
            cap_end_sent: false,
            cap_req_sent: false,
            supports_sasl: false,
            error_reason: None,
        };
        manager.reset_state();
        manager
    }

    pub fn configure(&mut self, config: &IrcConfig) {
        self.credentials = SaslCredentials::from_config(config);
        self.reset_state();
    }

    pub fn is_enabled(&self) -> bool {
        self.credentials.is_some()
    }

    pub fn state(&self) -> SaslState {
        self.state
    }

    pub fn failure_reason(&self) -> Option<&str> {
        self.error_reason.as_deref()
    }

    pub fn allows_channel_join(&self) -> bool {
        matches!(self.state, SaslState::Disabled | SaslState::Authenticated)
    }

    pub fn begin(&mut self, client: &Client) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }
        debug!("Starting SASL capability negotiation");
        client
            .send_cap_ls(NegotiationVersion::V302)
            .context("while requesting CAP LS")?;
        self.state = SaslState::WaitingForLs;
        Ok(())
    }

    pub fn handle_message(&mut self, message: &Message, client: &Client) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }

        match &message.command {
            Command::CAP(target, CapSubCommand::LS, parameter, capabilities) => {
                let cap_list = capabilities
                    .as_deref()
                    .or(target.as_ref().and(parameter.as_deref()));

                if let Some(list) = cap_list {
                    trace!(caps = %list, "Server capability list");
                    if list
                        .split_whitespace()
                        .any(|cap| cap.eq_ignore_ascii_case("sasl"))
                    {
                        self.supports_sasl = true;
                        if !self.cap_req_sent {
                            debug!("SASL capability advertised; requesting it");
                            client
                                .send_cap_req(&[Capability::Sasl])
                                .context("while requesting SASL capability")?;
                            self.cap_req_sent = true;
                            self.state = SaslState::WaitingForAck;
                        }
                    }
                }

                let has_more = matches!(parameter.as_deref(), Some("*"));
                if target.is_some() && !has_more && !self.supports_sasl {
                    self.mark_unsupported(client)?;
                }
            }
            Command::CAP(target, CapSubCommand::ACK, parameter, capabilities) => {
                if !self.cap_req_sent {
                    return Ok(());
                }

                let cap_list = capabilities.as_deref().or(parameter.as_deref());

                if cap_list
                    .map(|caps| {
                        caps.split_whitespace()
                            .any(|cap| cap.eq_ignore_ascii_case("sasl"))
                    })
                    .unwrap_or(false)
                {
                    debug!("SASL capability acknowledged; starting authentication");
                    client
                        .send_sasl_plain()
                        .context("while requesting SASL PLAIN mechanism")?;
                    self.state = SaslState::WaitingForChallenge;
                } else if target.is_some() && cap_list.is_some() {
                    trace!(caps = ?cap_list, "SASL not present in ACK list");
                }
            }
            Command::CAP(target, CapSubCommand::NAK, parameter, capabilities) => {
                let cap_list = capabilities.as_deref().or(parameter.as_deref());

                if cap_list
                    .map(|caps| {
                        caps.split_whitespace()
                            .any(|cap| cap.eq_ignore_ascii_case("sasl"))
                    })
                    .unwrap_or(false)
                {
                    self.fail(client, "Server rejected SASL capability request")?;
                } else if target.is_some() && cap_list.is_some() {
                    trace!(caps = ?cap_list, "SASL not present in NAK list");
                }
            }
            Command::AUTHENTICATE(payload) => {
                if payload == "+" && matches!(self.state, SaslState::WaitingForChallenge) {
                    if let Some(creds) = &self.credentials {
                        let encoded = creds.encode_plain();
                        debug!("Sending SASL PLAIN payload");
                        client
                            .send_sasl(encoded)
                            .context("while sending SASL payload")?;
                        self.state = SaslState::WaitingForResult;
                    } else {
                        self.fail(client, "Missing SASL credentials")?;
                    }
                } else if payload == "*" {
                    self.fail(client, "Server aborted SASL authentication")?;
                }
            }
            Command::Response(Response::RPL_SASLSUCCESS, _) => {
                self.on_success(client)?;
            }
            Command::Response(Response::ERR_SASLALREADY, _) => {
                // Already authenticated is effectively success.
                self.on_success(client)?;
            }
            Command::Response(Response::ERR_SASLFAIL, _) => {
                self.fail(client, "SASL authentication failed")?;
            }
            Command::Response(Response::ERR_SASLTOOLONG, _) => {
                self.fail(client, "SASL payload was too long")?;
            }
            Command::Response(Response::ERR_SASLABORT, _) => {
                self.fail(client, "SASL authentication aborted")?;
            }
            Command::Response(Response::RPL_SASLMECHS, arguments) => {
                debug!("Server SASL mechanisms available: {:?}", arguments);
            }
            _ => {}
        }

        Ok(())
    }

    fn on_success(&mut self, client: &Client) -> Result<()> {
        if matches!(self.state, SaslState::Authenticated) {
            return Ok(());
        }

        info!("SASL authentication completed successfully");
        self.state = SaslState::Authenticated;
        self.error_reason = None;
        self.send_cap_end(client)?;
        Ok(())
    }

    fn fail(&mut self, client: &Client, reason: &str) -> Result<()> {
        warn!("{}", reason);
        self.state = SaslState::Failed;
        self.error_reason = Some(reason.to_string());
        self.send_cap_end(client)?;
        Ok(())
    }

    fn mark_unsupported(&mut self, client: &Client) -> Result<()> {
        if matches!(self.state, SaslState::Unsupported) {
            return Ok(());
        }
        warn!("Server does not advertise SASL capability");
        self.state = SaslState::Unsupported;
        self.error_reason = Some("Server does not advertise SASL capability".to_string());
        self.send_cap_end(client)?;
        Ok(())
    }

    fn send_cap_end(&mut self, client: &Client) -> Result<()> {
        if self.cap_end_sent {
            return Ok(());
        }

        debug!("Sending CAP END");
        client
            .send(Command::CAP(None, CapSubCommand::END, None, None))
            .context("while sending CAP END")?;
        self.cap_end_sent = true;
        Ok(())
    }

    fn reset_state(&mut self) {
        self.cap_end_sent = false;
        self.cap_req_sent = false;
        self.supports_sasl = false;
        self.error_reason = None;
        self.state = if self.credentials.is_some() {
            SaslState::WaitingForLs
        } else {
            SaslState::Disabled
        };
    }
}

impl SaslCredentials {
    fn from_config(config: &IrcConfig) -> Option<Self> {
        let password = config.sasl_password.clone()?;
        if password.is_empty() {
            return None;
        }

        let username = config
            .sasl_username
            .clone()
            .unwrap_or_else(|| config.nick.clone());

        Some(Self { username, password })
    }

    fn encode_plain(&self) -> String {
        let payload = format!("\u{0}{}\u{0}{}", self.username, self.password);
        BASE64_STANDARD.encode(payload.as_bytes())
    }
}
