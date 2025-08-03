# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Ganbot3 is a multi-platform bot (Discord & IRC) built in Rust using the Kameo actor framework for fault-tolerant message passing and concurrent operations. The project emphasizes extensibility through a pubsub message bus with topics for long-term modularity.

## Architecture

### Actor-Based Design with Kameo

The codebase follows an actor model pattern using Kameo (v0.17.2):

- Each major component (IRC client, Discord client, command handlers) should be implemented as a separate actor
- Actors communicate via strongly-typed messages using Kameo's message passing
- Use supervision strategies for fault recovery (actors should restart on failure)
- The pubsub broker acts as the central message bus between platform-specific actors

### Key Architectural Principles

- **Message Bus Pattern**: All cross-platform communication goes through the Kameo-based broker
- **Platform Abstraction**: IRC and Discord implementations should be isolated in separate actors
- **Command Modularity**: Commands and game logic should be platform-agnostic actors that subscribe to relevant topics
- **Fault Tolerance**: Use Kameo's supervision trees to ensure bot resilience

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

When implementing features, follow this structure:

```text
src/
├── network/.        # Network wrappers/clients of various kinds
│   ├── irc.rs       # IRC client actor using the `irc` crate
│   ├── discord.rs   # Discord client actor
│   └── openapi.rs   # OpenAPI client
├── messages/        # Message types for actor communication
├── games/           # Game logic modules
├── image/           # Image generation/editing (Kontext integration)
└── config/          # Configuration structs and loading
```

## Key Dependencies and Usage

### Kameo Actor Framework

- Use `kameo::Actor` trait for all actors
- Implement `kameo::message::Message` for actor messages
- Use `kameo::actor::ActorRef` for actor references
- Leverage `kameo::supervision` for fault tolerance

### IRC Integration

- The `irc` crate (v1.1.0) provides async IRC connectivity
- Configure via `irc::client::data::Config`
- Use `irc::client::Client` for connection management
- Handle IRC events through the actor's message handler

### Error Handling

- Use `thiserror` for all custom error types
- Create domain-specific error enums in each module
- Propagate errors through the actor system appropriately

### Logging

- Use `tracing` macros (`info!`, `debug!`, `error!`, etc.) throughout
- Add spans for async operations: `#[tracing::instrument]`
- Structure logs with key-value pairs for better observability

## Implementation Guidelines

### Actor Implementation Pattern

```rust
use kameo::actor::{Actor, ActorRef};
use kameo::message::{Message, Context};

pub struct MyActor {
    // actor state
}

#[derive(Message)]
pub struct MyMessage {
    // message fields
}

impl Actor for MyActor {
    async fn on_start(&mut self, ctx: &mut Context<Self>) {
        // initialization
    }
}

impl Handler<MyMessage> for MyActor {
    async fn handle(&mut self, msg: MyMessage, ctx: &mut Context<Self>) {
        // message handling
    }
}
```

### Pubsub Topics Convention

- Platform events: `platform.{irc|discord}.{event_type}`
- Commands: `command.{command_name}`
- Games: `game.{game_name}.{event}`
- System: `system.{health|metrics|config}`

## Testing Strategy

When adding tests:

- Unit test individual actors in isolation
- Integration test cross-actor communication
- Mock external services (IRC, Discord APIs)
- Test error recovery and supervision strategies

## Configuration

Implement TOML-based configuration:

- `config/default.toml` for defaults
- Environment variable overrides via `GANBOT_` prefix
- Separate configs for IRC servers, Discord tokens, and feature flags

## Common Tasks

### Adding a New Command

1. Create a new actor in `src/actors/commands/`
2. Define the command's message types in `src/messages/`
3. Subscribe the actor to relevant pubsub topics
4. Add command parsing logic to route messages

### Integrating a New Platform

1. Create a platform actor in `src/actors/`
2. Implement message translation to/from the common format
3. Connect to the broker for message routing
4. Add platform-specific configuration

### Adding Game Logic

1. Create a game actor in `src/games/`
2. Define game state and message types
3. Subscribe to player input topics
4. Publish game events back to the broker

## Performance Considerations

- Use bounded channels for backpressure management
- Implement rate limiting in platform actors
- Consider actor pooling for CPU-intensive tasks (image processing)
- Monitor actor mailbox sizes for bottleneck detection
