# Kameo Actors Crate Reference

Last updated: August 3, 2025

## Overview

The `kameo_actors` crate provides pre-built utility actors for the Kameo actor framework, designed to handle common messaging patterns in concurrent Rust applications. These actors are built on top of the core Kameo framework and provide ready-to-use implementations for publish-subscribe, message routing, and actor pooling patterns.

**Version**: 0.17.2 (matching your project's Kameo version)
**Documentation Coverage**: 89.9%
**License**: MIT or Apache-2.0

## Key Components

The crate provides four main actor types:

1. **PubSub** - Simple broadcast messaging with optional filtering
2. **Broker** - Topic-based message routing with hierarchical patterns
3. **MessageBus** - Type-based message routing
4. **Pool** - Actor pool for concurrent task execution

## Choosing the Right Actor

| Use Case | Actor | Best For |
|----------|-------|----------|
| Simple broadcast to all listeners | `PubSub` | Event notifications, status updates |
| Topic-based routing with patterns | `Broker` | Platform-specific channels (IRC/Discord) |
| Type-based message routing | `MessageBus` | Command dispatching, service bus |
| Load balancing tasks | `Pool` | CPU-intensive operations, work distribution |

## PubSub Actor

The PubSub actor implements a simple publish-subscribe pattern where messages are broadcast to all subscribed actors.

### Basic Usage

```rust
use kameo::prelude::*;
use kameo_actors::{pubsub::{PubSub, Publish, Subscribe}, DeliveryStrategy};

#[derive(Clone)]
struct GameEvent {
    event_type: String,
    data: String,
}

#[derive(Actor)]
struct IrcHandler;

impl Message<GameEvent> for IrcHandler {
    type Reply = ();
    
    async fn handle(&mut self, msg: GameEvent, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        println!("IRC: {}: {}", msg.event_type, msg.data);
    }
}

#[derive(Actor)]
struct DiscordHandler;

impl Message<GameEvent> for DiscordHandler {
    type Reply = ();
    
    async fn handle(&mut self, msg: GameEvent, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        println!("Discord: {}: {}", msg.event_type, msg.data);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create PubSub actor with guaranteed delivery
    let pubsub = PubSub::spawn(PubSub::<GameEvent>::new(DeliveryStrategy::Guaranteed));
    
    // Spawn platform handlers
    let irc_handler = IrcHandler::spawn(IrcHandler);
    let discord_handler = DiscordHandler::spawn(DiscordHandler);
    
    // Subscribe handlers to game events
    pubsub.ask(Subscribe(irc_handler)).await?;
    pubsub.ask(Subscribe(discord_handler)).await?;
    
    // Publish event to all subscribers
    pubsub.ask(Publish(GameEvent {
        event_type: "player_joined".to_string(),
        data: "Alice has joined the game".to_string(),
    })).await?;
    
    Ok(())
}
```

### PubSub with Filtering

```rust
use kameo_actors::pubsub::SubscribeFilter;

// Subscribe with a filter predicate
pubsub.ask(SubscribeFilter(irc_handler, |event: &GameEvent| {
    event.event_type.starts_with("irc_")
})).await?;

pubsub.ask(SubscribeFilter(discord_handler, |event: &GameEvent| {
    event.event_type.starts_with("discord_")
})).await?;

// This will only be received by the IRC handler
pubsub.ask(Publish(GameEvent {
    event_type: "irc_user_joined".to_string(),
    data: "User joined IRC channel".to_string(),
})).await?;
```

### PubSub API Reference

| Message | Purpose | Parameters |
|---------|---------|------------|
| `Subscribe(ActorRef)` | Subscribe actor to all messages | Actor reference |
| `SubscribeFilter(ActorRef, Predicate)` | Subscribe with filter | Actor reference, filter function |
| `Unsubscribe(ActorRef)` | Remove subscription | Actor reference |
| `Publish(Message)` | Send message to all subscribers | Message to broadcast |

## Broker Actor

The Broker actor provides topic-based routing with support for hierarchical topics and pattern matching.

### Basic Usage

```rust
use kameo_actors::broker::{Broker, Publish, Subscribe};

#[derive(Clone)]
struct ChatMessage {
    content: String,
    user: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let broker = Broker::spawn(Broker::new(DeliveryStrategy::Guaranteed));
    
    // Subscribe to specific topics
    let irc_handler = IrcHandler::spawn(IrcHandler);
    broker.tell(Subscribe {
        topic: "platform.irc.message".parse()?,
        recipient: irc_handler.recipient(),
    }).await?;
    
    // Subscribe with wildcards
    let discord_handler = DiscordHandler::spawn(DiscordHandler);
    broker.tell(Subscribe {
        topic: "platform.discord.*".parse()?,
        recipient: discord_handler.recipient(),
    }).await?;
    
    // Publish to specific topic
    broker.tell(Publish {
        topic: "platform.irc.message".to_string(),
        message: ChatMessage {
            content: "Hello from IRC!".to_string(),
            user: "alice".to_string(),
        },
    }).await?;
    
    Ok(())
}
```

### Ganbot3 Topic Convention

For your ganbot project, consider this topic hierarchy:

```rust
// Platform events
"platform.irc.message"
"platform.irc.join"
"platform.irc.part"
"platform.discord.message"
"platform.discord.reaction"

// Commands
"command.help"
"command.roll"
"command.game.start"

// Game events
"game.poker.hand_dealt"
"game.poker.player_fold"
"game.blackjack.hit"

// System events
"system.health_check"
"system.config_reload"
"system.metrics.update"
```

### Advanced Broker Usage

```rust
// Subscribe to all platform events
broker.tell(Subscribe {
    topic: "platform.*".parse()?,
    recipient: platform_logger.recipient(),
}).await?;

// Subscribe to specific game events
broker.tell(Subscribe {
    topic: "game.poker.*".parse()?,
    recipient: poker_handler.recipient(),
}).await?;

// Subscribe to all commands
broker.tell(Subscribe {
    topic: "command.*".parse()?,
    recipient: command_dispatcher.recipient(),
}).await?;
```

## MessageBus Actor

The MessageBus routes messages based on their type rather than topics, useful for service-oriented architectures.

### Basic Usage

```rust
use kameo_actors::message_bus::{MessageBus, Register, Publish};

#[derive(Clone)]
struct ProcessImageCommand {
    image_path: String,
    filters: Vec<String>,
}

#[derive(Actor)]
struct ImageProcessor;

impl Message<ProcessImageCommand> for ImageProcessor {
    type Reply = ();
    
    async fn handle(&mut self, msg: ProcessImageCommand, _ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        println!("Processing image: {} with filters: {:?}", msg.image_path, msg.filters);
        // Image processing logic here
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let message_bus = MessageBus::spawn(MessageBus::new(DeliveryStrategy::Guaranteed));
    
    // Register handlers for specific message types
    let image_processor = ImageProcessor::spawn(ImageProcessor);
    message_bus.tell(Register(image_processor.recipient())).await?;
    
    // Publish message - will be routed to all registered handlers of this type
    message_bus.tell(Publish(ProcessImageCommand {
        image_path: "/tmp/image.png".to_string(),
        filters: vec!["blur".to_string(), "sepia".to_string()],
    })).await?;
    
    Ok(())
}
```

### MessageBus for Command Handling

```rust
#[derive(Clone)]
struct HelpCommand {
    requester: String,
    channel: String,
}

#[derive(Clone)]
struct RollCommand {
    dice: String,
    requester: String,
}

// Register different handlers for different command types
message_bus.tell(Register(help_handler.recipient())).await?;
message_bus.tell(Register(dice_handler.recipient())).await?;

// Commands will be automatically routed to the correct handler
message_bus.tell(Publish(HelpCommand {
    requester: "alice".to_string(),
    channel: "#general".to_string(),
})).await?;
```

## Pool Actor

The Pool actor manages a pool of worker actors for load balancing and concurrent task execution.

### Basic Usage

```rust
use kameo_actors::pool::{Pool, Execute};

#[derive(Actor)]
struct Worker {
    id: usize,
}

#[derive(Clone)]
struct Task {
    data: String,
}

impl Message<Task> for Worker {
    type Reply = String;
    
    async fn handle(&mut self, msg: Task, _ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // Simulate work
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        format!("Worker {} processed: {}", self.id, msg.data)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a pool of 4 workers
    let mut workers = Vec::new();
    for i in 0..4 {
        workers.push(Worker::spawn(Worker { id: i }));
    }
    
    let pool = Pool::spawn(Pool::new(workers));
    
    // Execute tasks - they'll be distributed among workers
    for i in 0..10 {
        let result = pool.ask(Execute(Task {
            data: format!("task_{}", i),
        })).await?;
        println!("Result: {}", result);
    }
    
    Ok(())
}
```

## Delivery Strategies

All utility actors support different delivery strategies:

```rust
use kameo_actors::DeliveryStrategy;

// Guaranteed delivery - waits for acknowledgment
let strategy = DeliveryStrategy::Guaranteed;

// Best effort - fire and forget
let strategy = DeliveryStrategy::BestEffort;

// Use with any utility actor
let pubsub = PubSub::spawn(PubSub::<MyMessage>::new(strategy));
let broker = Broker::spawn(Broker::new(strategy));
let message_bus = MessageBus::spawn(MessageBus::new(strategy));
```

## Integration with Custom Actors

### Actor with Multiple Message Types

```rust
#[derive(Actor)]
struct GameCoordinator {
    active_games: HashMap<String, GameState>,
}

// Handle platform messages from broker
impl Message<ChatMessage> for GameCoordinator {
    type Reply = ();
    
    async fn handle(&mut self, msg: ChatMessage, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if msg.content.starts_with("!start") {
            // Start a new game
            self.start_game(&msg.user).await;
        }
    }
}

// Handle game events from pubsub
impl Message<GameEvent> for GameCoordinator {
    type Reply = ();
    
    async fn handle(&mut self, msg: GameEvent, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // Update game state
        self.process_game_event(msg).await;
    }
}

// Subscribe to both broker and pubsub
async fn setup_game_coordinator(
    broker: &ActorRef<Broker>,
    pubsub: &ActorRef<PubSub<GameEvent>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let coordinator = GameCoordinator::spawn(GameCoordinator {
        active_games: HashMap::new(),
    });
    
    // Subscribe to platform chat messages
    broker.tell(Subscribe {
        topic: "platform.*.message".parse()?,
        recipient: coordinator.recipient(),
    }).await?;
    
    // Subscribe to game events
    pubsub.ask(Subscribe(coordinator)).await?;
    
    Ok(())
}
```

## Performance Characteristics

### Memory Usage

- **PubSub**: O(n) where n is the number of subscribers
- **Broker**: O(n×m) where n is subscribers and m is topic patterns
- **MessageBus**: O(n) where n is registered handlers per message type
- **Pool**: O(n) where n is the number of worker actors

### Message Throughput

- **Best Effort**: Highest throughput, no delivery guarantees
- **Guaranteed**: Lower throughput due to acknowledgment overhead

### Latency Considerations

- Message routing adds minimal overhead (~1-5μs per hop)
- Network distribution adds significant latency for remote actors
- Pool actors add queueing delay under high load

## Error Handling

### Actor Failures

```rust
use kameo::error::ActorStopReason;

// Handle subscription failures
match pubsub.ask(Subscribe(actor_ref)).await {
    Ok(_) => println!("Subscribed successfully"),
    Err(e) => match e.downcast_ref::<ActorStopReason>() {
        Some(ActorStopReason::Panicked(msg)) => {
            eprintln!("PubSub actor panicked: {}", msg);
            // Respawn the PubSub actor
        }
        _ => eprintln!("Subscription failed: {}", e),
    }
}
```

### Message Delivery Failures

```rust
// With Guaranteed delivery, you can handle acknowledgment failures
match broker.tell(Publish { topic, message }).await {
    Ok(_) => println!("Message delivered to all subscribers"),
    Err(e) => eprintln!("Some subscribers failed to acknowledge: {}", e),
}
```

## Best Practices

### 1. Choose the Right Actor

- Use **PubSub** for simple event broadcasting
- Use **Broker** for complex topic routing in multi-platform scenarios
- Use **MessageBus** for type-based service architectures
- Use **Pool** for CPU-intensive tasks that can be parallelized

### 2. Topic Design (for Broker)

```rust
// Good: Hierarchical and specific
"platform.irc.channel.#general.message"
"game.poker.table.1.player.fold"

// Bad: Flat and generic
"message"
"event"
```

### 3. Message Design

```rust
// Good: Structured and self-contained
#[derive(Clone, Debug)]
struct PlayerAction {
    player_id: String,
    action_type: ActionType,
    game_id: String,
    timestamp: chrono::DateTime<chrono::Utc>,
}

// Bad: Primitive types that require context
type PlayerAction = String;
```

### 4. Supervision Integration

```rust
use kameo::actor::supervision::Supervisor;

// Create supervised utility actors
let supervisor = Supervisor::spawn(|_| async {
    let broker = Broker::spawn(Broker::new(DeliveryStrategy::Guaranteed));
    let pubsub = PubSub::spawn(PubSub::<GameEvent>::new(DeliveryStrategy::Guaranteed));
    
    // If either fails, both will be restarted
    (broker, pubsub)
});
```

### 5. Graceful Shutdown

```rust
// Shutdown utility actors before your application actors
async fn shutdown(
    broker: ActorRef<Broker>,
    pubsub: ActorRef<PubSub<GameEvent>>,
    actors: Vec<ActorRef<MyActor>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Stop application actors first
    for actor in actors {
        actor.stop_gracefully().await?;
    }
    
    // Then stop utility actors
    broker.stop_gracefully().await?;
    pubsub.stop_gracefully().await?;
    
    Ok(())
}
```

## Ganbot3 Integration Examples

### Platform Abstraction Layer

```rust
// Create a unified message bus for platform-agnostic communication
pub struct PlatformMessage {
    pub platform: Platform,
    pub channel: String,
    pub user: String,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

pub enum Platform { Irc, Discord }

// Use broker to route platform-specific messages
let broker = Broker::spawn(Broker::new(DeliveryStrategy::Guaranteed));

// Subscribe command processor to all platforms
broker.tell(Subscribe {
    topic: "platform.*.message".parse()?,
    recipient: command_processor.recipient(),
}).await?;

// Platform-specific actors publish to their topics
// IRC actor publishes to "platform.irc.message"
// Discord actor publishes to "platform.discord.message"
```

### Game State Management

```rust
// Use PubSub for game events that multiple systems need to know about
let game_pubsub = PubSub::spawn(PubSub::<GameEvent>::new(DeliveryStrategy::Guaranteed));

// Subscribe relevant systems
game_pubsub.ask(Subscribe(statistics_tracker)).await?; // Track game stats
game_pubsub.ask(Subscribe(achievement_system)).await?; // Process achievements
game_pubsub.ask(Subscribe(platform_broadcaster)).await?; // Broadcast to platforms

// Game actors publish events
game_pubsub.ask(Publish(GameEvent::PlayerWon {
    player: "alice".to_string(),
    game_type: "poker".to_string(),
    amount: 100,
})).await?;
```

### Image Processing Pipeline

```rust
// Use Pool for concurrent image processing
let image_processors = Pool::spawn(Pool::new(
    (0..4).map(|i| ImageProcessor::spawn(ImageProcessor::new(i))).collect()
));

// Use MessageBus to route different image commands
let image_bus = MessageBus::spawn(MessageBus::new(DeliveryStrategy::Guaranteed));
image_bus.tell(Register(image_processors.recipient())).await?;

// Commands can now be processed concurrently
image_bus.tell(Publish(GenerateMemeCommand {
    template: "drake".to_string(),
    text: vec!["Old bot".to_string(), "Ganbot3".to_string()],
})).await?;
```

This comprehensive documentation should provide you with everything needed to effectively use the kameo_actors crate in your ganbot project, with practical examples tailored to your multi-platform bot architecture.
