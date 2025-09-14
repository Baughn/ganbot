# Kameo Actor Framework Guide (v0.17.2)

Last updated: August 3, 2025

## Table of Contents

1. [Overview](#overview)
2. [Core Concepts](#core-concepts)
3. [Creating and Spawning Actors](#creating-and-spawning-actors)
4. [Message Passing and Handling](#message-passing-and-handling)
5. [Actor Lifecycle](#actor-lifecycle)
6. [Supervision Strategies and Fault Tolerance](#supervision-strategies-and-fault-tolerance)
7. [Request-Reply Patterns](#request-reply-patterns)
8. [Actor Pools and Load Balancing](#actor-pools-and-load-balancing)
9. [PubSub/EventBus Functionality](#pubsubeventbus-functionality)
10. [Error Handling](#error-handling)
11. [Testing Actors](#testing-actors)
12. [Performance Considerations](#performance-considerations)
13. [Integration Patterns](#integration-patterns)
14. [Best Practices](#best-practices)

## Overview

Kameo is a fault-tolerant async actor framework for Rust, built on top of Tokio. It provides lightweight actors that run in their own asynchronous tasks, offering type-safe message passing, fault tolerance through supervision strategies, and the ability to scale from local applications to distributed systems.

### Key Features

- **Lightweight actors** running in Tokio async tasks
- **Type-safe message passing** with compile-time guarantees
- **Fault tolerance** with automatic error recovery
- **Bounded and unbounded** message channels with backpressure management
- **Panic safety** - individual actor failures don't crash the entire system
- **Local and distributed** actor communication
- **Compatible** with existing Rust async ecosystems

### Installation

Add Kameo to your `Cargo.toml`:

```toml
[dependencies]
kameo = "0.17.2"
tokio = { version = "1.0", features = ["full"] }
```

Requires Rust 1.79 or later.

## Core Concepts

### Actors

Actors are the fundamental unit of computation in Kameo. Each actor:

- Encapsulates state and behavior
- Processes messages sequentially (no shared mutable state)
- Runs in its own async task
- Can create other actors
- Has a unique address (ActorRef)

### Messages

Messages are the way actors communicate. They are:

- Rust structs that represent requests or notifications
- Type-safe and checked at compile time
- Processed asynchronously
- Can optionally return replies

### ActorRef

An `ActorRef` is a reference to an actor that allows you to:

- Send messages to the actor
- Check if the actor is still alive
- Link actors for supervision

### Context

The `Context` provides actors with:

- Access to their own `ActorRef`
- Ability to spawn child actors
- Control over message processing
- Integration with the actor system

### Mailbox

Each actor has a mailbox that:

- Queues incoming messages
- Can be bounded (with backpressure) or unbounded
- Processes messages in FIFO order
- Provides flow control mechanisms

## Creating and Spawning Actors

### Basic Actor Definition

```rust
use kameo::actor::{Actor, ActorRef};
use kameo::message::{Context, Message};

#[derive(Actor)]
pub struct Counter {
    count: i64,
}

// Define a message
pub struct Increment {
    amount: i64,
}

// Implement message handling
impl Message<Increment> for Counter {
    type Reply = i64;

    async fn handle(
        &mut self,
        msg: Increment,
        _ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        self.count += msg.amount;
        self.count
    }
}
```

### Spawning Actors

```rust
use kameo::spawn;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Spawn with default bounded mailbox (capacity 64)
    let counter_ref = spawn(Counter { count: 0 });
    
    // Send a message and get reply
    let result = counter_ref.ask(Increment { amount: 5 }).await?;
    println!("Counter value: {}", result); // Counter value: 5
    
    Ok(())
}
```

### Advanced Spawning Options

```rust
use kameo::mailbox::{unbounded, bounded};
use kameo::actor::spawn_with;

// Spawn with unbounded mailbox
let actor_ref = spawn_with(
    Counter { count: 0 },
    unbounded()
);

// Spawn with custom bounded mailbox
let actor_ref = spawn_with(
    Counter { count: 0 },
    bounded(128) // capacity of 128 messages
);

// Spawn in dedicated thread (for CPU-intensive work)
let actor_ref = Counter::spawn_in_thread(Counter { count: 0 });
```

## Message Passing and Handling

### Tell vs Ask Pattern

```rust
// Tell: Fire-and-forget (no reply expected)
actor_ref.tell(Increment { amount: 1 }).await?;

// Ask: Request-reply pattern (wait for response)
let response = actor_ref.ask(Increment { amount: 1 }).await?;
```

### Message Without Reply

```rust
pub struct Reset;

impl Message<Reset> for Counter {
    type Reply = (); // No meaningful reply

    async fn handle(
        &mut self,
        _msg: Reset,
        _ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        self.count = 0;
    }
}

// Usage
counter_ref.tell(Reset).await?;
```

### Complex Message Handling

```rust
pub struct GetStats;

#[derive(Debug, Clone)]
pub struct CounterStats {
    pub current_value: i64,
    pub message_count: u64,
}

impl Message<GetStats> for Counter {
    type Reply = CounterStats;

    async fn handle(
        &mut self,
        _msg: GetStats,
        ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        CounterStats {
            current_value: self.count,
            message_count: ctx.message_count(),
        }
    }
}
```

## Actor Lifecycle

### Lifecycle Methods

```rust
use kameo::actor::{Actor, ActorRef};
use std::convert::Infallible;

impl Actor for Counter {
    type Args = Self;
    type Error = Infallible;

    // Called when actor starts
    async fn on_start(
        args: Self::Args,
        actor_ref: ActorRef<Self>
    ) -> Result<Self, Self::Error> {
        println!("Counter actor started with initial value: {}", args.count);
        Ok(args)
    }

    // Called when actor is about to stop
    async fn on_stop(
        self,
        _actor_ref: ActorRef<Self>,
        _reason: kameo::actor::StopReason
    ) {
        println!("Counter actor stopping with final value: {}", self.count);
    }

    // Called when an actor panics
    async fn on_panic(
        &mut self,
        _error: Box<dyn std::any::Any + Send>,
        _ctx: &mut Context<Self>
    ) -> bool {
        println!("Counter actor panicked, resetting count");
        self.count = 0;
        true // Return true to continue, false to stop
    }
}
```

### Actor Termination

```rust
// Graceful shutdown
actor_ref.stop().await;

// Immediate termination
actor_ref.kill().await;

// Check if actor is alive
if actor_ref.is_alive() {
    println!("Actor is still running");
}
```

## Supervision Strategies and Fault Tolerance

### Actor Linking

```rust
use kameo::actor::spawn_link;

// Parent actor
#[derive(Actor)]
pub struct Supervisor {
    children: Vec<ActorRef<WorkerActor>>,
}

impl Supervisor {
    async fn spawn_workers(&mut self, ctx: &mut Context<Self>) {
        for i in 0..5 {
            // Link child to parent for supervision
            let child = spawn_link(
                WorkerActor::new(i),
                ctx.actor_ref().clone()
            );
            self.children.push(child);
        }
    }
}

// Handle child actor failures
pub struct ChildStopped {
    pub actor_ref: ActorRef<WorkerActor>,
    pub reason: StopReason,
}

impl Message<ChildStopped> for Supervisor {
    type Reply = ();

    async fn handle(
        &mut self,
        msg: ChildStopped,
        ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        println!("Child actor stopped: {:?}", msg.reason);
        
        // Restart the child if it crashed
        if matches!(msg.reason, StopReason::Panic(_)) {
            let new_child = spawn_link(
                WorkerActor::new(0),
                ctx.actor_ref().clone()
            );
            
            // Replace the old reference
            if let Some(pos) = self.children.iter()
                .position(|child| child.id() == msg.actor_ref.id()) {
                self.children[pos] = new_child;
            }
        }
    }
}
```

### Fault Tolerance Patterns

```rust
#[derive(Actor)]
pub struct ResilientWorker {
    retry_count: u32,
    max_retries: u32,
}

pub struct DoWork {
    pub task: String,
}

impl Message<DoWork> for ResilientWorker {
    type Reply = Result<String, String>;

    async fn handle(
        &mut self,
        msg: DoWork,
        _ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        // Simulate work that might fail
        if self.should_fail() && self.retry_count < self.max_retries {
            self.retry_count += 1;
            return Err(format!("Task failed, retry {}/{}", 
                              self.retry_count, self.max_retries));
        }
        
        self.retry_count = 0;
        Ok(format!("Completed task: {}", msg.task))
    }
}

impl Actor for ResilientWorker {
    // ... other implementations

    async fn on_panic(
        &mut self,
        _error: Box<dyn std::any::Any + Send>,
        _ctx: &mut Context<Self>
    ) -> bool {
        if self.retry_count < self.max_retries {
            self.retry_count += 1;
            true // Continue running
        } else {
            false // Stop the actor
        }
    }
}
```

## Request-Reply Patterns

### Synchronous-style Request-Reply

```rust
use kameo::request::MessageSend;

// Simple request-reply
let response = actor_ref.ask(GetStats).await?;
println!("Stats: {:?}", response);

// With timeout
use tokio::time::{timeout, Duration};

let response = timeout(
    Duration::from_secs(5),
    actor_ref.ask(GetStats)
).await??;
```

### Asynchronous Request-Reply

```rust
// Send request and get a future
let future = actor_ref.ask(GetStats);

// Do other work...
tokio::task::yield_now().await;

// Wait for response
let response = future.await?;
```

### Batch Requests

```rust
use futures::future::join_all;

async fn get_multiple_stats(
    actors: Vec<ActorRef<Counter>>
) -> Result<Vec<CounterStats>, Box<dyn std::error::Error>> {
    let futures: Vec<_> = actors.iter()
        .map(|actor| actor.ask(GetStats))
        .collect();
    
    let results = join_all(futures).await;
    
    // Handle individual errors
    results.into_iter().collect()
}
```

## Actor Pools and Load Balancing

### Simple Actor Pool

```rust
use kameo::actor::ActorPool;

#[derive(Actor)]
pub struct WorkerActor {
    id: usize,
}

pub struct ProcessTask {
    pub data: String,
}

impl Message<ProcessTask> for WorkerActor {
    type Reply = String;

    async fn handle(
        &mut self,
        msg: ProcessTask,
        _ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        // Simulate work
        tokio::time::sleep(Duration::from_millis(100)).await;
        format!("Worker {} processed: {}", self.id, msg.data)
    }
}

// Create a pool
async fn create_worker_pool() -> ActorPool<WorkerActor> {
    let mut pool = ActorPool::new();
    
    for i in 0..5 {
        let worker = spawn(WorkerActor { id: i });
        pool.add_actor(worker);
    }
    
    pool
}

// Use the pool
let pool = create_worker_pool().await;
let result = pool.ask(ProcessTask { 
    data: "important task".to_string() 
}).await?;
```

### Load Balancing Strategies

```rust
// Round-robin distribution
#[derive(Actor)]
pub struct LoadBalancer {
    workers: Vec<ActorRef<WorkerActor>>,
    current_index: usize,
}

impl LoadBalancer {
    fn next_worker(&mut self) -> &ActorRef<WorkerActor> {
        let worker = &self.workers[self.current_index];
        self.current_index = (self.current_index + 1) % self.workers.len();
        worker
    }
}

impl Message<ProcessTask> for LoadBalancer {
    type Reply = String;

    async fn handle(
        &mut self,
        msg: ProcessTask,
        _ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        let worker = self.next_worker();
        worker.ask(msg).await.unwrap_or_else(|_| 
            "Worker failed".to_string()
        )
    }
}
```

## PubSub/EventBus Functionality

### Event-Driven Architecture

```rust
use std::collections::HashMap;

#[derive(Actor)]
pub struct EventBus {
    subscribers: HashMap<String, Vec<ActorRef<dyn EventHandler>>>,
}

pub struct Subscribe {
    pub topic: String,
    pub subscriber: ActorRef<dyn EventHandler>,
}

pub struct Publish {
    pub topic: String,
    pub event: Event,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub id: String,
    pub data: serde_json::Value,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl Message<Subscribe> for EventBus {
    type Reply = ();

    async fn handle(
        &mut self,
        msg: Subscribe,
        _ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        self.subscribers
            .entry(msg.topic)
            .or_insert_with(Vec::new)
            .push(msg.subscriber);
    }
}

impl Message<Publish> for EventBus {
    type Reply = ();

    async fn handle(
        &mut self,
        msg: Publish,
        _ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        if let Some(subscribers) = self.subscribers.get(&msg.topic) {
            for subscriber in subscribers {
                let _ = subscriber.tell(msg.event.clone()).await;
            }
        }
    }
}
```

### Topic-Based Messaging for Ganbot3

```rust
// Bot-specific event system
#[derive(Clone, Debug)]
pub enum BotEvent {
    MessageReceived { platform: String, content: String, user: String },
    CommandExecuted { command: String, result: String },
    UserJoined { platform: String, user: String },
    UserLeft { platform: String, user: String },
}

#[derive(Actor)]
pub struct BotEventBus {
    platform_handlers: HashMap<String, ActorRef<PlatformHandler>>,
    command_handlers: HashMap<String, ActorRef<CommandHandler>>,
}

impl Message<BotEvent> for BotEventBus {
    type Reply = ();

    async fn handle(
        &mut self,
        event: BotEvent,
        _ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        match event {
            BotEvent::MessageReceived { platform, content, user } => {
                // Route to appropriate platform handler
                if let Some(handler) = self.platform_handlers.get(&platform) {
                    let _ = handler.tell(ProcessMessage { content, user }).await;
                }
            },
            BotEvent::CommandExecuted { command, result } => {
                // Log command execution
                println!("Command '{}' executed: {}", command, result);
            },
            _ => {}
        }
    }
}
```

## Error Handling

### Custom Error Types

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CounterError {
    #[error("Counter overflow: attempted to increment beyond maximum")]
    Overflow,
    #[error("Counter underflow: attempted to decrement below minimum")]
    Underflow,
    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
}

impl Actor for Counter {
    type Args = Self;
    type Error = CounterError;

    async fn on_start(
        args: Self::Args,
        _actor_ref: ActorRef<Self>
    ) -> Result<Self, Self::Error> {
        if args.count < 0 {
            return Err(CounterError::InvalidOperation(
                "Cannot start with negative count".to_string()
            ));
        }
        Ok(args)
    }
}
```

### Error Propagation

```rust
pub struct SafeIncrement {
    amount: i64,
}

impl Message<SafeIncrement> for Counter {
    type Reply = Result<i64, CounterError>;

    async fn handle(
        &mut self,
        msg: SafeIncrement,
        _ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        if self.count.checked_add(msg.amount).is_none() {
            return Err(CounterError::Overflow);
        }
        
        self.count += msg.amount;
        Ok(self.count)
    }
}

// Usage with error handling
match counter_ref.ask(SafeIncrement { amount: 1 }).await? {
    Ok(new_count) => println!("New count: {}", new_count),
    Err(CounterError::Overflow) => println!("Counter would overflow!"),
    Err(e) => println!("Other error: {}", e),
}
```

### Panic Recovery

```rust
impl Actor for ResilientCounter {
    // ... other implementations

    async fn on_panic(
        &mut self,
        error: Box<dyn std::any::Any + Send>,
        _ctx: &mut Context<Self>
    ) -> bool {
        // Log the panic
        if let Some(msg) = error.downcast_ref::<&str>() {
            eprintln!("Actor panicked with message: {}", msg);
        } else {
            eprintln!("Actor panicked with unknown error");
        }
        
        // Reset to safe state
        self.count = 0;
        
        // Continue running
        true
    }
}
```

## Testing Actors

### Unit Testing Individual Actors

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio_test;

    #[tokio::test]
    async fn test_counter_increment() {
        let counter = spawn(Counter { count: 0 });
        
        let result = counter.ask(Increment { amount: 5 }).await.unwrap();
        assert_eq!(result, 5);
        
        let result = counter.ask(Increment { amount: 3 }).await.unwrap();
        assert_eq!(result, 8);
    }

    #[tokio::test]
    async fn test_counter_reset() {
        let counter = spawn(Counter { count: 10 });
        
        counter.tell(Reset).await.unwrap();
        
        let result = counter.ask(GetStats).await.unwrap();
        assert_eq!(result.current_value, 0);
    }
}
```

### Integration Testing

```rust
#[tokio::test]
async fn test_event_bus_integration() {
    let event_bus = spawn(EventBus::new());
    let subscriber = spawn(TestSubscriber::new());
    
    // Subscribe to events
    event_bus.tell(Subscribe {
        topic: "test".to_string(),
        subscriber: subscriber.clone().into(),
    }).await.unwrap();
    
    // Publish an event
    let event = Event {
        id: "test-1".to_string(),
        data: serde_json::json!({"message": "hello"}),
        timestamp: chrono::Utc::now(),
    };
    
    event_bus.tell(Publish {
        topic: "test".to_string(),
        event: event.clone(),
    }).await.unwrap();
    
    // Verify subscriber received the event
    tokio::time::sleep(Duration::from_millis(100)).await;
    let received_events = subscriber.ask(GetReceivedEvents).await.unwrap();
    assert_eq!(received_events.len(), 1);
    assert_eq!(received_events[0].id, event.id);
}
```

### Mock Actors for Testing

```rust
#[derive(Actor)]
pub struct MockActor {
    responses: VecDeque<String>,
}

impl MockActor {
    pub fn with_responses(responses: Vec<String>) -> Self {
        Self {
            responses: responses.into(),
        }
    }
}

impl Message<ProcessTask> for MockActor {
    type Reply = String;

    async fn handle(
        &mut self,
        _msg: ProcessTask,
        _ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        self.responses.pop_front()
            .unwrap_or_else(|| "No response configured".to_string())
    }
}
```

## Performance Considerations

### Mailbox Sizing

```rust
// For high-throughput actors, use larger bounded mailboxes
let high_throughput_actor = spawn_with(
    ProcessorActor::new(),
    bounded(1000) // Can handle bursts
);

// For memory-sensitive scenarios, use smaller mailboxes
let memory_constrained_actor = spawn_with(
    LightweightActor::new(),
    bounded(16) // Provides backpressure quickly
);

// For fire-and-forget scenarios, unbounded can be appropriate
let fire_and_forget_actor = spawn_with(
    LoggingActor::new(),
    unbounded() // No backpressure, but risk of memory growth
);
```

### Actor Placement and CPU Affinity

```rust
// CPU-intensive actors should run in dedicated threads
let cpu_intensive = CounterActor::spawn_in_thread(CounterActor::new());

// I/O bound actors can share the Tokio runtime
let io_bound = spawn(NetworkActor::new());
```

### Batch Processing

```rust
pub struct BatchProcessor {
    buffer: Vec<ProcessTask>,
    batch_size: usize,
    flush_interval: Duration,
    last_flush: Instant,
}

impl Message<ProcessTask> for BatchProcessor {
    type Reply = ();

    async fn handle(
        &mut self,
        msg: ProcessTask,
        ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        self.buffer.push(msg);
        
        if self.buffer.len() >= self.batch_size 
            || self.last_flush.elapsed() > self.flush_interval {
            self.flush_batch().await;
            self.last_flush = Instant::now();
        }
    }
}

impl BatchProcessor {
    async fn flush_batch(&mut self) {
        if !self.buffer.is_empty() {
            let batch = std::mem::take(&mut self.buffer);
            // Process entire batch at once
            self.process_batch(batch).await;
        }
    }
}
```

### Memory Management

```rust
// Implement Drop for cleanup
impl Drop for ResourceActor {
    fn drop(&mut self) {
        // Clean up resources
        if let Some(connection) = self.connection.take() {
            // Close connection gracefully
            let _ = connection.close();
        }
    }
}

// Use weak references to avoid circular dependencies
use std::rc::Weak;

#[derive(Actor)]
pub struct ChildActor {
    parent: Option<Weak<ActorRef<ParentActor>>>,
}
```

## Integration Patterns

### Multi-Platform Bot Architecture

```rust
// Platform abstraction
#[async_trait::async_trait]
pub trait Platform: Send + Sync {
    async fn send_message(&self, channel: &str, content: &str) -> Result<(), PlatformError>;
    async fn get_user_info(&self, user_id: &str) -> Result<UserInfo, PlatformError>;
}

// Platform-specific actors
#[derive(Actor)]
pub struct DiscordPlatform {
    client: Arc<discord::Client>,
    event_bus: ActorRef<BotEventBus>,
}

#[derive(Actor)]
pub struct IrcPlatform {
    client: Arc<irc::Client>,
    event_bus: ActorRef<BotEventBus>,
}

// Unified bot controller
#[derive(Actor)]
pub struct BotController {
    platforms: HashMap<String, Box<dyn Platform>>,
    command_registry: ActorRef<CommandRegistry>,
    event_bus: ActorRef<BotEventBus>,
}

impl Message<SendMessage> for BotController {
    type Reply = Result<(), BotError>;

    async fn handle(
        &mut self,
        msg: SendMessage,
        _ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        if let Some(platform) = self.platforms.get(&msg.platform) {
            platform.send_message(&msg.channel, &msg.content).await?;
            
            // Notify event bus
            self.event_bus.tell(BotEvent::MessageSent {
                platform: msg.platform,
                channel: msg.channel,
                content: msg.content,
            }).await?;
            
            Ok(())
        } else {
            Err(BotError::PlatformNotFound(msg.platform))
        }
    }
}
```

### Stream Processing Integration

```rust
use kameo::message::StreamMessage;

#[derive(Actor)]
pub struct StreamProcessor {
    processed_count: usize,
}

// Custom marker types (can be any type)
#[derive(Debug, Clone)]
pub struct StreamStarted {
    pub stream_id: String,
    pub timestamp: std::time::Instant,
}

#[derive(Debug, Clone)]
pub struct StreamEnded {
    pub stream_id: String,
    pub total_processed: usize,
}

// Attach stream to actor using Context
impl Message<ConnectStream> for StreamProcessor {
    type Reply = Result<(), ProcessorError>;

    async fn handle(
        &mut self,
        msg: ConnectStream,
        ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        let stream = msg.stream;
        
        // Markers can be any type - strings, structs, enums, etc.
        let start_marker = StreamStarted {
            stream_id: "processor-1".to_string(),
            timestamp: std::time::Instant::now(),
        };
        let end_marker = StreamEnded {
            stream_id: "processor-1".to_string(),
            total_processed: 0, // Will be updated when stream ends
        };
        
        // Attach stream with typed markers
        ctx.actor_ref().attach_stream(stream, start_marker, end_marker);
        
        Ok(())
    }
}

// Handle stream messages with custom marker types
impl Message<StreamMessage<Result<String, ProcessorError>, StreamStarted, StreamEnded>>
    for StreamProcessor
{
    type Reply = ();

    async fn handle(
        &mut self,
        msg: StreamMessage<Result<String, ProcessorError>, StreamStarted, StreamEnded>,
        ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        match msg {
            StreamMessage::Started(marker) => {
                tracing::info!("Stream {} started at {:?}", 
                    marker.stream_id, marker.timestamp);
            }
            StreamMessage::Next(Ok(item)) => {
                self.processed_count += 1;
                tracing::info!("Processed item {}: {}", self.processed_count, item);
            }
            StreamMessage::Next(Err(e)) => {
                tracing::error!("Stream error: {}", e);
            }
            StreamMessage::Ended(marker) => {
                tracing::info!("Stream {} ended, processed {} items", 
                    marker.stream_id, self.processed_count);
            }
        }
    }
}

// Real-world example based on IRC client (using string markers for simplicity)
impl Message<StreamMessage<Result<irc::proto::Message, irc::error::Error>, &'static str, &'static str>>
    for IrcActor
{
    type Reply = ();

    async fn handle(
        &mut self,
        msg: StreamMessage<Result<irc::proto::Message, irc::error::Error>, &str, &str>,
        ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        match msg {
            StreamMessage::Started(marker) => {
                tracing::info!("IRC stream started: {}", marker);
            }
            StreamMessage::Next(Ok(irc_message)) => {
                // Process IRC message
                self.handle_irc_message(irc_message).await;
            }
            StreamMessage::Next(Err(e)) => {
                tracing::error!("IRC stream error: {}", e);
            }
            StreamMessage::Ended(marker) => {
                tracing::info!("IRC stream ended: {}", marker);
            }
        }
    }
}

// Example with different marker types
#[derive(Debug)]
enum ConnectionState { Starting, Ending }

// You can use enums, numbers, or any type as markers
ctx.actor_ref().attach_stream(stream, ConnectionState::Starting, ConnectionState::Ending);
ctx.actor_ref().attach_stream(stream, 42u32, 100u32);
ctx.actor_ref().attach_stream(stream, true, false);
```

### Key Points for Stream Integration:

1. **Use `attach_stream`** via `ctx.actor_ref().attach_stream(stream, start_marker, end_marker)`
2. **Import StreamMessage** from `kameo::message::StreamMessage`
3. **Markers are flexible** - can be any type (`&str`, custom structs, enums, primitives, etc.)
4. **Handle all StreamMessage variants**:
   - `Started(start_marker)` - Stream began with your start marker
   - `Next(item)` - New item from stream (can be Result for error handling)  
   - `Ended(end_marker)` - Stream completed with your end marker
5. **Error handling** - Stream items can be `Result` types for graceful error handling

## Best Practices

### 1. Actor Design Principles

```rust
// ✅ Good: Single responsibility
#[derive(Actor)]
pub struct UserManager {
    users: HashMap<String, User>,
}

// ❌ Bad: Multiple responsibilities
#[derive(Actor)]
pub struct EverythingManager {
    users: HashMap<String, User>,
    messages: Vec<Message>,
    connections: Vec<Connection>,
    configuration: Config,
}
```

### 2. Message Design

```rust
// ✅ Good: Specific, typed messages
pub struct CreateUser {
    pub username: String,
    pub email: String,
}

pub struct UpdateUserEmail {
    pub user_id: String,
    pub new_email: String,
}

// ❌ Bad: Generic, stringly-typed messages
pub struct GenericMessage {
    pub action: String,
    pub data: HashMap<String, String>,
}
```

### 3. Error Handling Strategy

```rust
// ✅ Good: Structured error handling
#[derive(Error, Debug)]
pub enum UserError {
    #[error("User not found: {id}")]
    NotFound { id: String },
    #[error("Invalid email format: {email}")]
    InvalidEmail { email: String },
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
}

// Graceful degradation
impl Message<GetUser> for UserManager {
    type Reply = Result<User, UserError>;

    async fn handle(
        &mut self,
        msg: GetUser,
        _ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        self.users.get(&msg.id)
            .cloned()
            .ok_or_else(|| UserError::NotFound { id: msg.id })
    }
}
```

### 4. State Management

```rust
// ✅ Good: Immutable state transitions
impl Message<UpdateUser> for UserManager {
    type Reply = Result<User, UserError>;

    async fn handle(
        &mut self,
        msg: UpdateUser,
        _ctx: &mut Context<Self, Self::Reply>
    ) -> Self::Reply {
        let user = self.users.get(&msg.id)
            .ok_or_else(|| UserError::NotFound { id: msg.id.clone() })?;
        
        let updated_user = User {
            id: user.id.clone(),
            username: msg.username.unwrap_or_else(|| user.username.clone()),
            email: msg.email.unwrap_or_else(|| user.email.clone()),
            updated_at: chrono::Utc::now(),
            ..user.clone()
        };
        
        self.users.insert(msg.id, updated_user.clone());
        Ok(updated_user)
    }
}
```

### 5. Supervision Hierarchies

```rust
// Create clear parent-child relationships
#[derive(Actor)]
pub struct ApplicationSupervisor {
    database_manager: Option<ActorRef<DatabaseManager>>,
    web_server: Option<ActorRef<WebServer>>,
    background_workers: Vec<ActorRef<BackgroundWorker>>,
}

impl ApplicationSupervisor {
    async fn start_subsystems(&mut self, ctx: &mut Context<Self>) {
        // Start critical systems first
        self.database_manager = Some(spawn_link(
            DatabaseManager::new(),
            ctx.actor_ref().clone()
        ));
        
        // Then dependent systems
        self.web_server = Some(spawn_link(
            WebServer::new(),
            ctx.actor_ref().clone()
        ));
        
        // Finally, workers
        for i in 0..5 {
            let worker = spawn_link(
                BackgroundWorker::new(i),
                ctx.actor_ref().clone()
            );
            self.background_workers.push(worker);
        }
    }
}
```

### 6. Resource Cleanup

```rust
impl Actor for ResourceOwner {
    // Implement proper cleanup
    async fn on_stop(
        mut self,
        _actor_ref: ActorRef<Self>,
        _reason: StopReason
    ) {
        // Close database connections
        if let Some(conn) = self.db_connection.take() {
            let _ = conn.close().await;
        }
        
        // Cancel background tasks
        for handle in self.background_tasks.drain(..) {
            handle.abort();
        }
        
        // Notify dependent actors
        for dependent in &self.dependents {
            let _ = dependent.tell(OwnerStopping).await;
        }
    }
}
```

### 7. Testing Strategy

```rust
// Test actors in isolation
#[tokio::test]
async fn test_user_manager() {
    let manager = spawn(UserManager::new());
    
    // Test successful case
    let user = manager.ask(CreateUser {
        username: "test_user".to_string(),
        email: "test@example.com".to_string(),
    }).await.unwrap().unwrap();
    
    assert_eq!(user.username, "test_user");
    
    // Test error case
    let result = manager.ask(CreateUser {
        username: "".to_string(), // Invalid username
        email: "invalid-email".to_string(),
    }).await.unwrap();
    
    assert!(result.is_err());
}
```

This comprehensive guide should provide you with everything needed to effectively use Kameo v0.17.2 in your ganbot project, from basic concepts to advanced patterns for building fault-tolerant, scalable multi-platform bot systems.
