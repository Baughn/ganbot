# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## General Notes

- If I start a query with "Can you... ?", this is a genuine question. You should investigate, then answer; denial is a valid response.
- Some queries will always be underspecified. To save effort, please ask clarifying questions up front. Think of yourself as a partner, not a robot.

## Project Overview

Ganbot3 is an IRC bot built in Rust using the Kameo actor framework for fault-tolerant message passing and concurrent operations. The bot integrates AI capabilities through OpenRouter API and image generation via ComfyUI, with Redis for persistence and state management.

## Architecture

### Actor-Based Design with Kameo

The codebase follows an actor model pattern using Kameo:

- Each major component (IRC client, AI services, command handlers) is implemented as a separate actor
- Actors communicate via strongly-typed messages using Kameo's message passing
- The supervisor implements automatic restart with exponential backoff for fault recovery
- Long-lived actors (IRC, OpenRouter, UserManager) are managed by the supervisor
- Command actors are spawned on-demand for each invocation

### Key Architectural Principles

- **Direct Actor Communication**: Actors communicate through direct references rather than a message bus (current implementation)
- **Platform Focus**: Currently IRC-only, with architecture supporting future platform additions
- **Command Modularity**: Commands are implemented as separate action actors spawned per invocation
- **Fault Tolerance**: Supervisor provides automatic restart with backoff, connection recovery, and graceful degradation

## Development Commands

DO NOT use git commands.

```bash
# Confirm repository status
jj status
jj log
jj diff

# Commit changes
jj commit -m "[Message in standard commit format]"

# Build the project
cargo build

# Run the bot
cargo run

# Run with debug logging
RUST_LOG=debug cargo run

# Run with module-specific logging
RUST_LOG=ganbot3=trace,irc=warn,kameo=info cargo run

# Check for compilation errors
cargo check

# Generate documentation
cargo doc

# Format code (when rustfmt.toml is added)
cargo fmt

# Run clippy linter (when configured)
cargo clippy -- -D warnings
```

## Module Organization

The actual codebase structure:

```text
src/
├── main.rs          # Entry point and application initialization
├── supervisor.rs    # Root supervision tree and actor lifecycle management
├── network/         # Network clients and external service integrations
│   ├── irc.rs       # IRC client actor with command handling
│   ├── openrouter.rs # OpenRouter AI API integration
│   └── comfyui/     # ComfyUI image generation integration
│       ├── api.rs   # ComfyUI API types and models
│       └── net.rs   # ComfyUI network client implementation
├── actions/         # Command implementations (game logic, AI interactions)
│   ├── ask.rs       # AI question/answer command
│   ├── combine.rs   # Word combination game with image generation
│   └── prompt.rs    # Direct AI prompting command
├── persistence/     # Redis-backed persistence layer
│   ├── user.rs      # User management and state tracking
│   └── images.rs    # Image storage and URL management
├── messages/        # Message types for actor communication
│   ├── chat.rs      # Chat message types and structured responses
│   └── imagen.rs    # Image generation message types
└── config/          # Configuration management
    └── global.rs    # Global configuration structures
```

## Key Dependencies and Usage

### Redis

Redis is used for all persistence through a reconnecting `ConnectionManager`. Current key patterns:

- `user:[userid]` -- JSON string containing User struct with configuration settings etc.
- `combine:combinations` -- Hash containing cached combination results for the combine game. Fields are `[word1]:[word2]` with JSON values containing CombineResult.
- `combine:basis` -- Hash tracking base elements for the combine game. Fields are `[word]` with values referencing the combination that created them.
- `image:files` -- Sorted set of all JPEGs uploaded to the web server. The score is the Unix timestamp at which it was created.

The supervisor maintains a persistent Redis connection that's shared across actors.

### Kameo Actor Framework

- Use `#[derive(Actor)]` macro for actor structs
- Use `kameo::prelude::*` for common imports
- Implement `kameo::message::Message` for actor messages, with `Reply` trait for typed responses
- Use `kameo::actor::ActorRef` for actor references
- The supervisor implements custom supervision logic with exponential backoff

### IRC Integration

- The `irc` crate provides async IRC connectivity
- IRC actor handles connection, command parsing, and response routing
- Commands are parsed with prefix detection (!, ., ~)
- Supports both channel and private messages
- Implements rate limiting and message batching for responses

### Error Handling

- Use `anyhow::Result` for general error handling
- Context is added with `.context()` for error tracing
- Actor failures trigger supervisor restart logic
- Network errors implement retry with backoff

### Logging

- Use `tracing` macros (`info!`, `debug!`, `error!`, etc.) throughout
- Add spans for async operations: `#[tracing::instrument]`
- Structure logs with key-value pairs for better observability

## Implementation Guidelines

### Actor Implementation Pattern

```rust
use kameo::prelude::*;
use kameo::{Actor, Reply};

// Use the #[derive(Actor)] macro for automatic implementation
#[derive(Actor)]
pub struct MyActor {
    // actor state
}

// Reply messages must derive Reply
#[derive(Reply)]
pub struct MyReply {
    // message fields
}

impl Actor for MyActor {
    async fn on_start(&mut self, ctx: &mut Context<Self>) -> Result<(), BoxError> {
        // initialization
        Ok(())
    }
}

impl Handler<MyMessage> for MyActor {
    type Reply = MyReplyType;
    
    async fn handle(&mut self, msg: MyMessage, ctx: &mut Context<Self>) -> Self::Reply {
        // message handling
    }
}
```

### Current Architecture Notes

Currently:

- IRC commands are handled directly in the IRC actor
- Actions are spawned as temporary actors per command invocation
- The supervisor manages long-lived actors (IRC, OpenRouter, UserManager)
- Communication happens through direct actor references rather than a message bus

### Supervision and Fault Tolerance

The supervisor (`src/supervisor.rs`) implements:

- Automatic restart with exponential backoff for failed actors
- Configuration hot-reloading support
- Graceful shutdown handling
- Health monitoring and restart tracking

## Testing Strategy

When adding tests:

- Unit test individual actors in isolation
- Integration test cross-actor communication
- Mock external services (IRC, Discord APIs)
- Test error recovery and supervision strategies

## Configuration

Configuration uses a layered TOML approach:

- `config.toml` - Base configuration
- `config-local.toml` - Local overrides (git-ignored)
- Environment variables can override any setting
- Configuration includes:
  - IRC server settings (server, port, channels, nick)
  - OpenRouter API configuration
  - ComfyUI server endpoints
  - Image hosting service URLs
  - Redis connection parameters

## Common Tasks

### Adding a New Command

1. Create a new actor in `src/actions/` (e.g., `mycommand.rs`)
2. Define the command's message types in the same file or `src/messages/`
3. Add the command handling logic in the IRC actor (`src/network/irc.rs`)
4. If persistent state is needed, add Redis keys and update the persistence layer
5. Register the actor in the supervisor if it needs lifecycle management

### Integrating a New Platform

1. Create a platform actor in `src/network/` (e.g., `discord.rs`)
2. Implement the actor with connection management and event handling
3. Add platform-specific configuration in `src/config/global.rs`
4. Register the platform in the supervisor (`src/supervisor.rs`)
5. Handle platform-specific messages and convert to common chat format

### Adding Game Logic

1. Create a game actor in `src/actions/` (following the pattern of `combine.rs`)
2. Define game state and message types within the actor module
3. Add Redis persistence for game state if needed
4. Integrate with OpenRouter for AI-powered game logic if applicable
5. Add image generation support via ComfyUI if the game needs visuals

## Performance Considerations

- Redis uses a reconnecting `ConnectionManager` for resilience
- IRC implements message batching and rate limiting
- Command actors are short-lived to avoid memory accumulation
- Image generation is async with progress tracking
- OpenRouter requests use structured output for efficiency

## External Service Integrations

### OpenRouter AI API

- Handles all LLM interactions (questions, prompts, game logic)
- Supports structured JSON output via JSON schema
- Configurable model selection and parameters
- Automatic retry on transient failures

### ComfyUI Image Generation

- WebSocket-based workflow execution
- Progress tracking during generation
- Automatic image upload to configured hosting service
- Support for custom workflows and parameters
