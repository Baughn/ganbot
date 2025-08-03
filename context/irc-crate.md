# Rust IRC Crate Documentation (v1.1.0)

Last Updated: August 3, 2025

## Overview

The `irc` crate is a thread-safe, async-friendly IRC client library for Rust that provides comprehensive support for building IRC clients and bots. It's fully compliant with RFC 2812, IRCv3.1, and IRCv3.2 specifications, making it suitable for modern IRC development.

### Key Features

- **Async/Await Support**: Built on tokio with futures-based streaming
- **Thread-Safe**: Safe for concurrent use across multiple tasks
- **Configuration Flexibility**: Supports TOML, JSON, and YAML config files
- **Protocol Compliance**: RFC 2812, IRCv3.1, and IRCv3.2 compliant
- **TLS Support**: Built-in SSL/TLS encryption support
- **Comprehensive**: Handles authentication, channel management, and message routing

## Core Architecture

The crate is organized around several key modules:

### Main Modules

- `client`: High-level client API for IRC connections
- `proto`: Low-level IRC protocol implementation
- `client::prelude`: Contains the most commonly used types and traits

### Key Types

- `Client`: The main IRC client struct
- `Config`: Configuration structure for client setup
- `Message`: Represents IRC protocol messages
- `Command`: IRC command types
- `ClientStream`: Async stream of incoming messages

## Installation and Setup

Add to your `Cargo.toml`:

```toml
[dependencies]
irc = "1.1.0"
tokio = { version = "1.0", features = ["rt", "rt-multi-thread", "macros", "net", "time"] }
futures = "0.3"
anyhow = "1.0"  # For error handling
```

## Configuration

### Programmatic Configuration

```rust
use irc::client::prelude::*;

let config = Config {
    nickname: Some("mybot".to_owned()),
    server: Some("irc.libera.chat".to_owned()),
    port: Some(6697),                    // TLS port
    use_tls: Some(true),
    channels: vec!["#mychannel".to_owned()],
    username: Some("mybot".to_owned()),
    realname: Some("My IRC Bot".to_owned()),
    ..Config::default()
};
```

### TOML Configuration File

Create a `config.toml` file:

```toml
nickname = "mybot"
nick_password = "mypassword"  # Optional NickServ password
alt_nicks = ["mybot_", "mybot__"]
username = "mybot"
realname = "My IRC Bot"
server = "irc.libera.chat"
port = 6697
use_tls = true
encoding = "UTF-8"
channels = ["#mychannel", "#anotherchannel"]
channel_keys = { "#privatechannel" = "secretkey" }

[options]
ping_time = 180
ping_timeout = 20
```

### Loading Configuration

```rust
// From TOML file
let config = Config::load("config.toml")?;

// From JSON file  
let config = Config::load("config.json")?;

// From YAML file
let config = Config::load("config.yaml")?;
```

## Basic Client Usage

### Simple Connection and Message Handling

```rust
use irc::client::prelude::*;
use futures::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config {
        nickname: Some("mybot".to_owned()),
        server: Some("irc.libera.chat".to_owned()),
        channels: vec!["#test".to_owned()],
        ..Config::default()
    };

    // Create client and connect
    let mut client = Client::from_config(config).await?;
    client.identify()?;

    // Get message stream
    let mut stream = client.stream()?;
    
    // Handle incoming messages
    while let Some(message) = stream.next().await.transpose()? {
        println!("{}", message);
        
        // Handle specific message types
        match message.command {
            Command::PRIVMSG(target, text) => {
                println!("Message from {}: {}", message.source_nickname().unwrap_or("unknown"), text);
            },
            Command::JOIN(channel, _, _) => {
                println!("{} joined {}", message.source_nickname().unwrap_or("someone"), channel);
            },
            _ => {}
        }
    }

    Ok(())
}
```

## Message Handling Patterns

### Processing Different Message Types

```rust
use irc::client::prelude::*;
use futures::prelude::*;

async fn handle_messages(mut stream: ClientStream) -> Result<(), Box<dyn std::error::Error>> {
    while let Some(message) = stream.next().await.transpose()? {
        match &message.command {
            Command::PRIVMSG(target, text) => {
                let sender = message.source_nickname().unwrap_or("unknown");
                println!("[{}] <{}> {}", target, sender, text);
                
                // Respond to mentions
                if text.contains("mybot") {
                    // Handle bot mention
                }
            },
            
            Command::JOIN(channel, _, _) => {
                let nick = message.source_nickname().unwrap_or("someone");
                println!("{} joined {}", nick, channel);
            },
            
            Command::PART(channel, reason) => {
                let nick = message.source_nickname().unwrap_or("someone");
                let reason = reason.as_deref().unwrap_or("No reason");
                println!("{} left {} ({})", nick, channel, reason);
            },
            
            Command::QUIT(reason) => {
                let nick = message.source_nickname().unwrap_or("someone");
                let reason = reason.as_deref().unwrap_or("No reason");
                println!("{} quit ({})", nick, reason);
            },
            
            Command::KICK(channel, nick, reason) => {
                let kicker = message.source_nickname().unwrap_or("server");
                let reason = reason.as_deref().unwrap_or("No reason");
                println!("{} was kicked from {} by {} ({})", nick, channel, kicker, reason);
            },
            
            Command::Response(response, args) => {
                // Handle numeric responses (RPL_*, ERR_*)
                match response {
                    Response::RPL_WELCOME => {
                        println!("Successfully connected to server");
                    },
                    Response::ERR_NICKNAMEINUSE => {
                        println!("Nickname already in use");
                    },
                    _ => {
                        println!("Server response: {:?} {:?}", response, args);
                    }
                }
            },
            
            _ => {
                // Handle other commands
                println!("Other command: {:?}", message.command);
            }
        }
    }
    Ok(())
}
```

## Sending Messages and Commands

### Basic Message Sending

```rust
use irc::client::prelude::*;

async fn send_messages(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
    // Send a message to a channel
    client.send_privmsg("#mychannel", "Hello, channel!")?;
    
    // Send a private message to a user
    client.send_privmsg("username", "Hello, user!")?;
    
    // Send an action (/me command)
    client.send_action("#mychannel", "waves at everyone")?;
    
    // Send a notice
    client.send_notice("#mychannel", "This is a notice")?;
    
    Ok(())
}
```

### Advanced Commands

```rust
use irc::client::prelude::*;

async fn advanced_commands(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
    // Join channels
    client.send_join("#newchannel")?;
    client.send_join_with_keys(&["#privatechannel"], &["secretkey"])?;
    
    // Leave channels
    client.send_part("#oldchannel")?;
    client.send_part_with_reason("#channel", "Goodbye!")?;
    
    // Change nickname
    client.send_nick("newnick")?;
    
    // Set topic
    client.send_topic("#mychannel", "New channel topic")?;
    
    // Kick user
    client.send_kick("#mychannel", "baduser", "Reason for kick")?;
    
    // Set user mode
    client.send_mode("mynick", "+i")?;  // Set invisible mode
    
    // Set channel mode
    client.send_mode("#mychannel", "+m")?;  // Set moderated mode
    
    // Send raw IRC command
    client.send(Command::WHO(Some("#mychannel".to_string()), None))?;
    
    Ok(())
}
```

## Authentication and NickServ

### Automatic NickServ Authentication

```rust
use irc::client::prelude::*;
use futures::prelude::*;

async fn handle_auth(client: &Client, mut stream: ClientStream) -> Result<(), Box<dyn std::error::Error>> {
    while let Some(message) = stream.next().await.transpose()? {
        match &message.command {
            Command::Response(Response::RPL_WELCOME, _) => {
                // Connected successfully, identify with NickServ
                client.send_privmsg("NickServ", "IDENTIFY mypassword")?;
            },
            
            Command::NOTICE(target, text) => {
                if message.source_nickname() == Some("NickServ") {
                    if text.contains("You are now identified") {
                        println!("Successfully authenticated with NickServ");
                        // Join channels after authentication
                        client.send_join("#mychannel")?;
                    }
                }
            },
            
            _ => {}
        }
    }
    Ok(())
}
```

### Using Config for NickServ

```toml
# config.toml
nickname = "mybot"
nick_password = "mypassword"  # Automatically sends IDENTIFY command
server = "irc.libera.chat"
channels = ["#mychannel"]
```

## Error Handling

### Comprehensive Error Handling

```rust
use irc::client::prelude::*;
use futures::prelude::*;
use std::time::Duration;

#[tokio::main]
async fn main() {
    if let Err(e) = run_bot().await {
        eprintln!("Bot error: {}", e);
        std::process::exit(1);
    }
}

async fn run_bot() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load("config.toml")?;
    
    loop {
        match connect_and_run(&config).await {
            Ok(_) => {
                println!("Connection ended gracefully");
                break;
            },
            Err(e) => {
                eprintln!("Connection error: {}", e);
                println!("Reconnecting in 30 seconds...");
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        }
    }
    
    Ok(())
}

async fn connect_and_run(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = Client::from_config(config.clone()).await?;
    client.identify()?;
    
    let mut stream = client.stream()?;
    
    while let Some(message_result) = stream.next().await {
        match message_result {
            Ok(message) => {
                if let Err(e) = handle_message(&client, message).await {
                    eprintln!("Error handling message: {}", e);
                }
            },
            Err(e) => {
                eprintln!("Stream error: {}", e);
                return Err(e.into());
            }
        }
    }
    
    Ok(())
}

async fn handle_message(client: &Client, message: Message) -> Result<(), Box<dyn std::error::Error>> {
    match &message.command {
        Command::PRIVMSG(target, text) => {
            // Safe message sending with error handling
            if let Err(e) = client.send_privmsg(target, "Response message") {
                eprintln!("Failed to send message: {}", e);
            }
        },
        _ => {}
    }
    Ok(())
}
```

## Integration with Kameo Actors

### IRC Actor Implementation

```rust
use irc::client::prelude::*;
use kameo::actor::{Actor, ActorRef};
use kameo::message::{Message as KameoMessage, Context};
use futures::prelude::*;
use tokio::sync::mpsc;

pub struct IrcActor {
    client: Option<Client>,
    config: Config,
    broker: Option<ActorRef<MessageBroker>>,
}

#[derive(KameoMessage)]
pub struct ConnectToIrc {
    pub config: Config,
}

#[derive(KameoMessage)]
pub struct SendIrcMessage {
    pub target: String,
    pub message: String,
}

#[derive(KameoMessage)]
pub struct IrcMessageReceived {
    pub source: Option<String>,
    pub target: String,
    pub message: String,
}

impl Actor for IrcActor {
    async fn on_start(&mut self, ctx: &mut Context<Self>) -> Result<(), Box<dyn std::error::Error>> {
        // Connect to IRC on startup
        self.connect_to_irc().await?;
        
        // Start message handling task
        if let Some(client) = &self.client {
            let stream = client.stream()?;
            let actor_ref = ctx.actor_ref().clone();
            
            tokio::spawn(async move {
                Self::handle_irc_stream(stream, actor_ref).await;
            });
        }
        
        Ok(())
    }
}

impl IrcActor {
    pub fn new(config: Config) -> Self {
        Self {
            client: None,
            config,
            broker: None,
        }
    }
    
    async fn connect_to_irc(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut client = Client::from_config(self.config.clone()).await?;
        client.identify()?;
        self.client = Some(client);
        Ok(())
    }
    
    async fn handle_irc_stream(mut stream: ClientStream, actor_ref: ActorRef<Self>) {
        while let Some(message_result) = stream.next().await {
            match message_result {
                Ok(message) => {
                    if let Command::PRIVMSG(target, text) = &message.command {
                        let irc_msg = IrcMessageReceived {
                            source: message.source_nickname().map(|s| s.to_string()),
                            target: target.clone(),
                            message: text.clone(),
                        };
                        
                        // Send to actor for processing
                        let _ = actor_ref.tell(irc_msg).await;
                    }
                },
                Err(e) => {
                    eprintln!("IRC stream error: {}", e);
                    break;
                }
            }
        }
    }
}

impl kameo::message::Handler<SendIrcMessage> for IrcActor {
    async fn handle(&mut self, msg: SendIrcMessage, _ctx: &mut Context<Self>) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(client) = &self.client {
            client.send_privmsg(&msg.target, &msg.message)?;
        }
        Ok(())
    }
}

impl kameo::message::Handler<IrcMessageReceived> for IrcActor {
    async fn handle(&mut self, msg: IrcMessageReceived, _ctx: &mut Context<Self>) -> Result<(), Box<dyn std::error::Error>> {
        // Forward to message broker or handle directly
        if let Some(broker) = &self.broker {
            // Publish to message bus
            broker.tell(PublishMessage {
                topic: format!("platform.irc.message"),
                payload: serde_json::to_value(&msg)?,
            }).await?;
        }
        Ok(())
    }
}
```

### Actor-Based Bot Example

```rust
use irc::client::prelude::*;
use kameo::actor::{Actor, ActorRef, spawn};
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load("config.toml")?;
    
    // Spawn IRC actor
    let irc_actor = spawn(IrcActor::new(config));
    
    // Keep the main task alive
    tokio::signal::ctrl_c().await?;
    println!("Shutting down...");
    
    Ok(())
}
```

## Advanced Features

### Channel State Tracking

```rust
use irc::client::prelude::*;
use std::collections::{HashMap, HashSet};

pub struct ChannelState {
    users: HashSet<String>,
    topic: Option<String>,
    modes: String,
}

pub struct IrcBot {
    client: Client,
    channels: HashMap<String, ChannelState>,
}

impl IrcBot {
    async fn handle_message(&mut self, message: Message) {
        match &message.command {
            Command::Response(Response::RPL_NAMREPLY, args) => {
                // Handle user list response
                if args.len() >= 4 {
                    let channel = &args[2];
                    let users: Vec<&str> = args[3].split_whitespace().collect();
                    
                    let state = self.channels.entry(channel.clone()).or_insert(ChannelState {
                        users: HashSet::new(),
                        topic: None,
                        modes: String::new(),
                    });
                    
                    for user in users {
                        // Remove mode prefixes (@, +, etc.)
                        let clean_nick = user.trim_start_matches(['@', '+', '%', '&', '~']);
                        state.users.insert(clean_nick.to_string());
                    }
                }
            },
            
            Command::JOIN(channel, _, _) => {
                if let Some(nick) = message.source_nickname() {
                    if let Some(state) = self.channels.get_mut(channel) {
                        state.users.insert(nick.to_string());
                    }
                }
            },
            
            Command::PART(channel, _) => {
                if let Some(nick) = message.source_nickname() {
                    if let Some(state) = self.channels.get_mut(channel) {
                        state.users.remove(nick);
                    }
                }
            },
            
            _ => {}
        }
    }
    
    pub fn get_channel_users(&self, channel: &str) -> Option<&HashSet<String>> {
        self.channels.get(channel).map(|state| &state.users)
    }
}
```

### Rate Limiting and Flood Protection

```rust
use std::time::{Duration, Instant};
use std::collections::VecDeque;

pub struct RateLimiter {
    messages: VecDeque<Instant>,
    max_messages: usize,
    time_window: Duration,
}

impl RateLimiter {
    pub fn new(max_messages: usize, time_window: Duration) -> Self {
        Self {
            messages: VecDeque::new(),
            max_messages,
            time_window,
        }
    }
    
    pub fn can_send(&mut self) -> bool {
        let now = Instant::now();
        
        // Remove old messages outside the time window
        while let Some(&front) = self.messages.front() {
            if now.duration_since(front) > self.time_window {
                self.messages.pop_front();
            } else {
                break;
            }
        }
        
        if self.messages.len() < self.max_messages {
            self.messages.push_back(now);
            true
        } else {
            false
        }
    }
    
    pub async fn wait_if_needed(&mut self) {
        if !self.can_send() {
            if let Some(&front) = self.messages.front() {
                let wait_time = self.time_window - Instant::now().duration_since(front);
                tokio::time::sleep(wait_time).await;
            }
        }
    }
}

// Usage in IRC actor
impl IrcActor {
    async fn send_with_rate_limit(&mut self, target: &str, message: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.rate_limiter.wait_if_needed().await;
        
        if let Some(client) = &self.client {
            client.send_privmsg(target, message)?;
        }
        
        Ok(())
    }
}
```

## Best Practices

### 1. Connection Management

```rust
// Always implement reconnection logic
async fn run_with_reconnect(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let mut backoff = 1;
    const MAX_BACKOFF: u64 = 300; // 5 minutes
    
    loop {
        match connect_and_run(&config).await {
            Ok(_) => break,
            Err(e) => {
                eprintln!("Connection failed: {}", e);
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = std::cmp::min(backoff * 2, MAX_BACKOFF);
            }
        }
    }
    
    Ok(())
}
```

### 2. Message Processing

```rust
// Process messages asynchronously to avoid blocking
async fn process_message_async(client: Client, message: Message) {
    tokio::spawn(async move {
        match handle_message(&client, message).await {
            Ok(_) => {},
            Err(e) => eprintln!("Message handling error: {}", e),
        }
    });
}
```

### 3. Graceful Shutdown

```rust
use tokio::signal;

async fn run_bot() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load("config.toml")?;
    let mut client = Client::from_config(config).await?;
    client.identify()?;
    
    let mut stream = client.stream()?;
    
    loop {
        tokio::select! {
            message = stream.next() => {
                match message {
                    Some(Ok(msg)) => handle_message(&client, msg).await?,
                    Some(Err(e)) => return Err(e.into()),
                    None => break,
                }
            },
            _ = signal::ctrl_c() => {
                println!("Received Ctrl+C, shutting down gracefully...");
                client.send_quit("Bot shutting down")?;
                break;
            }
        }
    }
    
    Ok(())
}
```

### 4. Command Parsing

```rust
fn parse_command(text: &str) -> Option<(String, Vec<String>)> {
    if !text.starts_with('!') {
        return None;
    }
    
    let parts: Vec<&str> = text[1..].split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }
    
    let command = parts[0].to_lowercase();
    let args = parts[1..].iter().map(|s| s.to_string()).collect();
    
    Some((command, args))
}

async fn handle_command(client: &Client, channel: &str, command: String, args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    match command.as_str() {
        "ping" => {
            client.send_privmsg(channel, "Pong!")?;
        },
        "echo" => {
            let message = args.join(" ");
            client.send_privmsg(channel, &message)?;
        },
        "help" => {
            client.send_privmsg(channel, "Available commands: !ping, !echo, !help")?;
        },
        _ => {
            client.send_privmsg(channel, "Unknown command. Type !help for available commands.")?;
        }
    }
    Ok(())
}
```

## Common Pitfalls and Solutions

### 1. Message Encoding Issues

```rust
// Always specify UTF-8 encoding in config
let config = Config {
    encoding: Some("UTF-8".to_string()),
    // ... other config
    ..Config::default()
};
```

### 2. Nickname Conflicts

```rust
// Handle nickname conflicts gracefully
match &message.command {
    Command::Response(Response::ERR_NICKNAMEINUSE, _) => {
        // Try alternative nickname
        client.send_nick("mybot_")?;
    },
    _ => {}
}
```

### 3. Channel Key Management

```rust
// Store channel keys securely
let config = Config {
    channels: vec!["#public".to_string(), "#private".to_string()],
    channel_keys: Some({
        let mut keys = HashMap::new();
        keys.insert("#private".to_string(), "secretkey".to_string());
        keys
    }),
    ..Config::default()
};
```

## Troubleshooting

### Debug Logging

Enable debug logging to diagnose connection issues:

```rust
// Add to Cargo.toml
[dependencies]
tracing = "0.1"
tracing-subscriber = "0.3"

// In main.rs
use tracing::{info, debug, error};
use tracing_subscriber;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    
    info!("Starting IRC bot");
    // ... rest of code
}
```

### Connection Testing

```rust
async fn test_connection(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing connection to {}:{}", 
             config.server.as_deref().unwrap_or("unknown"),
             config.port.unwrap_or(6667));
    
    let client = Client::from_config(config.clone()).await?;
    println!("Connection successful!");
    
    Ok(())
}
```

## References

- [IRC RFC 2812](https://tools.ietf.org/html/rfc2812)
- [IRCv3 Specifications](https://ircv3.net/specs/)
- [irc crate documentation](https://docs.rs/irc/)
- [irc crate GitHub repository](https://github.com/aatxe/irc)

This documentation covers the essential aspects of using the `irc` crate v1.1.0 in the context of the ganbot3 project. The examples show how to integrate IRC functionality with Kameo actors for building a robust, fault-tolerant IRC bot.
