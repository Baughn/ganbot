# OpenRouter API Documentation

*Last updated: August 14, 2025*

## Overview

OpenRouter is a unified API service that provides access to hundreds of AI language models through a single endpoint. It normalizes the schema across different model providers (OpenAI, Anthropic, Meta, Google, etc.) and provides automatic fallbacks, cost optimization, and comprehensive analytics.

This document covers the OpenRouter API, the `openrouter_api` Rust crate (v0.1.6), and integration patterns for Rust applications.

## Table of Contents

- [What is OpenRouter?](#what-is-openrouter)
- [API Architecture](#api-architecture)
- [Authentication](#authentication)
- [Core Endpoints](#core-endpoints)
- [Request/Response Formats](#requestresponse-formats)
- [Rust Integration with openrouter_api Crate](#rust-integration-with-openrouter_api-crate)
- [Rate Limiting and Best Practices](#rate-limiting-and-best-practices)
- [Advanced Features](#advanced-features)
- [Error Handling](#error-handling)
- [Common Usage Patterns](#common-usage-patterns)
- [Configuration Examples](#configuration-examples)

## What is OpenRouter?

OpenRouter serves as a unified gateway to AI models, offering several key benefits:

- **Unified API**: Access 400+ models through a single, OpenAI-compatible interface
- **Automatic Fallbacks**: If one provider fails, requests automatically route to backup providers
- **Cost Optimization**: Intelligent routing to the most cost-effective providers
- **Aggregated Billing**: Consolidate usage across multiple model providers
- **Advanced Analytics**: Comprehensive usage tracking and cost analysis
- **Multimodal Support**: Images, PDFs, and audio processing capabilities

## API Architecture

### Base URL
```
https://openrouter.ai/api/v1
```

### OpenAI Compatibility
OpenRouter implements the OpenAI API specification, making it a drop-in replacement for OpenAI's endpoints:
- `/chat/completions` - Primary chat/completion endpoint
- `/completions` - Text completion endpoint (legacy)
- `/models` - List available models
- `/generation` - Retrieve detailed generation statistics

### Model Selection
Models are specified using provider prefixes:
```
openai/gpt-4o
anthropic/claude-3-5-sonnet
meta-llama/llama-3.1-8b-instruct
google/gemini-pro
```

### Model Variants
Special model variants provide specific behaviors:
- `:free` - Free models with rate limits
- `:online` - Web search enabled models
- `:nitro` - Optimized for speed/throughput
- `:floor` - Optimized for lowest cost

## Authentication

### API Key Authentication
```bash
Authorization: Bearer $OPENROUTER_API_KEY
```

### Optional Headers
```bash
HTTP-Referer: https://yoursite.com  # Your site URL
X-Title: YourAppName                # Your app name
```

### Environment Variables
```bash
export OPENROUTER_API_KEY="sk-or-v1-..."
```

## Core Endpoints

### Chat Completions
**POST** `/api/v1/chat/completions`

Primary endpoint for conversational AI interactions.

```json
{
  "model": "anthropic/claude-3-5-sonnet",
  "messages": [
    {
      "role": "user",
      "content": "Hello, how are you?"
    }
  ],
  "temperature": 0.7,
  "max_tokens": 1000,
  "stream": false
}
```

### Models List
**GET** `/api/v1/models`

Returns available models with metadata, pricing, and capabilities.

### Generation Details
**GET** `/api/v1/generation/{generation_id}`

Retrieve detailed statistics for a completed generation, including exact token counts and costs.

## Request/Response Formats

### Standard Chat Request
```json
{
  "model": "openai/gpt-4o",
  "messages": [
    {
      "role": "system",
      "content": "You are a helpful assistant."
    },
    {
      "role": "user", 
      "content": "Write a haiku about programming."
    }
  ],
  "temperature": 0.8,
  "max_tokens": 100,
  "top_p": 1.0,
  "frequency_penalty": 0.0,
  "presence_penalty": 0.0,
  "stream": false
}
```

### Response Format
```json
{
  "id": "gen-123abc",
  "object": "chat.completion",
  "created": 1692901234,
  "model": "openai/gpt-4o",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "Code flows like water,\nBugs surface in quiet moments—\nDebug with patience."
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 25,
    "completion_tokens": 17,
    "total_tokens": 42
  }
}
```

### Streaming Response
When `stream: true`, responses are sent as Server-Sent Events:

```
data: {"id":"gen-123","object":"chat.completion.chunk","created":1692901234,"model":"openai/gpt-4o","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}

data: {"id":"gen-123","object":"chat.completion.chunk","created":1692901234,"model":"openai/gpt-4o","choices":[{"index":0,"delta":{"content":"Code"},"finish_reason":null}]}

data: [DONE]
```

## Rust Integration with openrouter_api Crate

### Dependencies
Add to `Cargo.toml`:
```toml
[dependencies]
openrouter_api = { version = "0.1.6", features = ["tracing"] }
tokio = { version = "1.0", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
```

### Basic Client Setup
```rust
use openrouter_api::{OpenRouterClient, types::*};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize client with API key
    let client = OpenRouterClient::new()
        .api_key("sk-or-v1-your-api-key")
        .site_url("https://yoursite.com")
        .app_name("YourApp")
        .build()?;

    // Create a chat completion request
    let request = ChatCompletionRequest {
        model: "anthropic/claude-3-5-sonnet".to_string(),
        messages: vec![
            Message {
                role: "user".to_string(),
                content: "Hello, how are you?".to_string(),
            }
        ],
        temperature: Some(0.7),
        max_tokens: Some(1000),
        ..Default::default()
    };

    // Send request
    let response = client.chat_completions(&request).await?;
    
    if let Some(choice) = response.choices.first() {
        println!("Response: {}", choice.message.content);
    }

    Ok(())
}
```

### Streaming Chat Completion
```rust
use futures::StreamExt;

async fn streaming_chat() -> Result<(), Box<dyn std::error::Error>> {
    let client = OpenRouterClient::new()
        .api_key(std::env::var("OPENROUTER_API_KEY")?)
        .build()?;

    let request = ChatCompletionRequest {
        model: "openai/gpt-4o".to_string(),
        messages: vec![
            Message {
                role: "user".to_string(),
                content: "Write a story about a robot.".to_string(),
            }
        ],
        stream: Some(true),
        ..Default::default()
    };

    let mut stream = client.chat_completions_stream(&request).await?;

    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(response) => {
                if let Some(choice) = response.choices.first() {
                    if let Some(content) = &choice.delta.content {
                        print!("{}", content);
                    }
                }
            }
            Err(e) => eprintln!("Stream error: {}", e),
        }
    }

    Ok(())
}
```

### Model Information
```rust
async fn list_models() -> Result<(), Box<dyn std::error::Error>> {
    let client = OpenRouterClient::new()
        .api_key(std::env::var("OPENROUTER_API_KEY")?)
        .build()?;

    let models = client.models().await?;

    for model in models.data {
        println!(
            "Model: {} - Context: {} tokens - Price: ${}/1M tokens",
            model.id,
            model.context_length.unwrap_or(0),
            model.pricing.prompt
        );
    }

    Ok(())
}
```

### Error Handling Pattern
```rust
use openrouter_api::error::OpenRouterError;

async fn robust_chat() -> Result<String, OpenRouterError> {
    let client = OpenRouterClient::new()
        .api_key(std::env::var("OPENROUTER_API_KEY")?)
        .build()?;

    let request = ChatCompletionRequest {
        model: "anthropic/claude-3-5-sonnet".to_string(),
        messages: vec![
            Message {
                role: "user".to_string(),
                content: "Hello!".to_string(),
            }
        ],
        ..Default::default()
    };

    match client.chat_completions(&request).await {
        Ok(response) => {
            if let Some(choice) = response.choices.first() {
                Ok(choice.message.content.clone())
            } else {
                Err(OpenRouterError::ApiError("No response choices".to_string()))
            }
        }
        Err(OpenRouterError::RateLimitExceeded) => {
            // Implement exponential backoff
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            // Retry logic here
            Err(OpenRouterError::RateLimitExceeded)
        }
        Err(e) => Err(e),
    }
}
```

## Rate Limiting and Best Practices

### Rate Limits
- **Free models (`:free`)**: 20 requests/minute
- **Free tier (< $10 credits)**: 50 free requests/day
- **Paid tier (≥ $10 credits)**: 1000 free requests/day
- **Paid models**: Based on your credit balance and provider limits

### Best Practices

1. **Implement Exponential Backoff**
```rust
use tokio::time::{sleep, Duration};

async fn retry_with_backoff<F, T, E>(
    mut operation: F,
    max_retries: u32,
) -> Result<T, E>
where
    F: FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, E>>>>,
{
    let mut delay = Duration::from_millis(1000);
    
    for attempt in 0..max_retries {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) if attempt == max_retries - 1 => return Err(e),
            Err(_) => {
                sleep(delay).await;
                delay *= 2; // Exponential backoff
            }
        }
    }
    
    unreachable!()
}
```

2. **Use Appropriate Model Variants**
```rust
// For development/testing
let model = "openai/gpt-4o:free";

// For production with cost optimization
let model = "openai/gpt-4o:floor";

// For low-latency applications
let model = "openai/gpt-4o:nitro";

// For web-enhanced responses
let model = "openai/gpt-4o:online";
```

3. **Token Management**
```rust
let request = ChatCompletionRequest {
    model: "anthropic/claude-3-5-sonnet".to_string(),
    messages: messages,
    max_tokens: Some(500), // Limit response length
    temperature: Some(0.7),
    top_p: Some(0.9),
    ..Default::default()
};
```

## Advanced Features

### Function Calling
```rust
use serde_json::json;

let request = ChatCompletionRequest {
    model: "openai/gpt-4o".to_string(),
    messages: vec![
        Message {
            role: "user".to_string(),
            content: "What's the weather like in San Francisco?".to_string(),
        }
    ],
    tools: Some(vec![
        Tool {
            type_: "function".to_string(),
            function: FunctionDefinition {
                name: "get_weather".to_string(),
                description: Some("Get weather information for a city".to_string()),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "city": {
                            "type": "string",
                            "description": "The city name"
                        }
                    },
                    "required": ["city"]
                }),
            },
        }
    ]),
    tool_choice: Some("auto".to_string()),
    ..Default::default()
};
```

### Multimodal Input (Images)
```rust
let request = ChatCompletionRequest {
    model: "openai/gpt-4o".to_string(),
    messages: vec![
        Message {
            role: "user".to_string(),
            content: vec![
                ContentPart::Text {
                    text: "What's in this image?".to_string(),
                },
                ContentPart::Image {
                    image_url: ImageUrl {
                        url: "https://example.com/image.jpg".to_string(),
                        detail: Some("high".to_string()),
                    },
                },
            ],
        }
    ],
    ..Default::default()
};
```

### Provider Preferences
```rust
let request = ChatCompletionRequest {
    model: "anthropic/claude-3-5-sonnet".to_string(),
    messages: messages,
    provider: Some(Provider {
        order: vec!["Anthropic".to_string(), "AWS".to_string()],
        allow_fallbacks: Some(true),
        require_parameters: Some(false),
    }),
    ..Default::default()
};
```

## Error Handling

### Common Error Types
```rust
use openrouter_api::error::OpenRouterError;

match client.chat_completions(&request).await {
    Ok(response) => { /* Handle success */ }
    Err(OpenRouterError::AuthenticationError) => {
        // Invalid API key
        eprintln!("Authentication failed. Check your API key.");
    }
    Err(OpenRouterError::RateLimitExceeded) => {
        // Rate limit hit
        eprintln!("Rate limit exceeded. Implementing backoff...");
    }
    Err(OpenRouterError::ModelNotFound) => {
        // Invalid model specified
        eprintln!("Model not available. Check model name.");
    }
    Err(OpenRouterError::InsufficientCredits) => {
        // Not enough credits
        eprintln!("Insufficient credits. Please add funds.");
    }
    Err(OpenRouterError::NetworkError(e)) => {
        // Network issues
        eprintln!("Network error: {}", e);
    }
    Err(e) => {
        eprintln!("Unexpected error: {}", e);
    }
}
```

## Common Usage Patterns

### Configuration Management
```rust
use serde::Deserialize;

#[derive(Deserialize)]
struct OpenRouterConfig {
    api_key: String,
    default_model: String,
    temperature: f32,
    max_tokens: u32,
    site_url: Option<String>,
    app_name: Option<String>,
}

impl OpenRouterConfig {
    fn from_env() -> Result<Self, config::ConfigError> {
        let mut settings = config::Config::default();
        settings.merge(config::Environment::with_prefix("OPENROUTER"))?;
        settings.try_into()
    }
    
    fn to_client(&self) -> Result<OpenRouterClient, openrouter_api::error::OpenRouterError> {
        let mut builder = OpenRouterClient::new()
            .api_key(&self.api_key);
            
        if let Some(site_url) = &self.site_url {
            builder = builder.site_url(site_url);
        }
        
        if let Some(app_name) = &self.app_name {
            builder = builder.app_name(app_name);
        }
        
        builder.build()
    }
}
```

### Actor-Based Integration (Kameo)
```rust
use kameo::prelude::*;
use openrouter_api::OpenRouterClient;

pub struct OpenRouterActor {
    client: OpenRouterClient,
    default_model: String,
}

#[derive(Message)]
#[message(result = "Result<String, String>")]
pub struct ChatRequest {
    pub messages: Vec<String>,
    pub model: Option<String>,
}

impl Actor for OpenRouterActor {
    type Mailbox = UnboundedMailbox<Self>;

    async fn on_start(&mut self, _ctx: &mut Context<Self>) -> Result<(), BoxError> {
        tracing::info!("OpenRouter actor started");
        Ok(())
    }
}

impl Handler<ChatRequest> for OpenRouterActor {
    async fn handle(
        &mut self,
        msg: ChatRequest,
        _ctx: &mut Context<Self>,
    ) -> Result<String, String> {
        let model = msg.model.unwrap_or_else(|| self.default_model.clone());
        
        let messages = msg.messages
            .into_iter()
            .enumerate()
            .map(|(i, content)| Message {
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                content,
            })
            .collect();

        let request = ChatCompletionRequest {
            model,
            messages,
            ..Default::default()
        };

        match self.client.chat_completions(&request).await {
            Ok(response) => {
                if let Some(choice) = response.choices.first() {
                    Ok(choice.message.content.clone())
                } else {
                    Err("No response from model".to_string())
                }
            }
            Err(e) => Err(format!("OpenRouter error: {}", e)),
        }
    }
}
```

### Async Stream Processing
```rust
use futures::{Stream, StreamExt};
use tokio::sync::mpsc;

async fn process_chat_stream(
    client: &OpenRouterClient,
    request: ChatCompletionRequest,
) -> Result<mpsc::Receiver<String>, openrouter_api::error::OpenRouterError> {
    let (tx, rx) = mpsc::channel(100);
    
    let mut stream = client.chat_completions_stream(&request).await?;
    
    tokio::spawn(async move {
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(response) => {
                    if let Some(choice) = response.choices.first() {
                        if let Some(content) = &choice.delta.content {
                            if tx.send(content.clone()).await.is_err() {
                                break; // Receiver dropped
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Stream error: {}", e);
                    break;
                }
            }
        }
    });
    
    Ok(rx)
}
```

## Configuration Examples

### Environment Variables
```bash
# .env file
OPENROUTER_API_KEY=sk-or-v1-your-api-key-here
OPENROUTER_DEFAULT_MODEL=anthropic/claude-3-5-sonnet
OPENROUTER_TEMPERATURE=0.7
OPENROUTER_MAX_TOKENS=1000
OPENROUTER_SITE_URL=https://yoursite.com
OPENROUTER_APP_NAME=YourApp
```

### TOML Configuration
```toml
# config/openrouter.toml
[openrouter]
api_key = "sk-or-v1-your-api-key-here"
default_model = "anthropic/claude-3-5-sonnet"
temperature = 0.7
max_tokens = 1000
site_url = "https://yoursite.com"
app_name = "Ganbot3"

[openrouter.models]
chat = "anthropic/claude-3-5-sonnet"
fast = "openai/gpt-4o:nitro"
cheap = "meta-llama/llama-3.1-8b-instruct:free"
web = "openai/gpt-4o:online"
```

### Integration with Ganbot3 Architecture
```rust
// src/network/openrouter.rs - Enhanced implementation
use openrouter_api::{OpenRouterClient, types::*};
use kameo::prelude::*;
use crate::config::OpenRouterConfig;

pub struct OpenRouterService {
    client: OpenRouterClient,
    config: OpenRouterConfig,
}

impl OpenRouterService {
    pub async fn new(config: OpenRouterConfig) -> Result<Self, openrouter_api::error::OpenRouterError> {
        let client = OpenRouterClient::new()
            .api_key(&config.api_key)
            .site_url(&config.site_url.unwrap_or_else(|| "https://ganbot3.local".to_string()))
            .app_name("Ganbot3")
            .build()?;

        Ok(Self { client, config })
    }

    pub async fn chat_completion(
        &self,
        messages: Vec<String>,
        model: Option<String>,
    ) -> Result<String, openrouter_api::error::OpenRouterError> {
        let model = model.unwrap_or_else(|| self.config.default_model.clone());
        
        let request_messages = messages
            .into_iter()
            .enumerate()
            .map(|(i, content)| Message {
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                content,
            })
            .collect();

        let request = ChatCompletionRequest {
            model,
            messages: request_messages,
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            ..Default::default()
        };

        let response = self.client.chat_completions(&request).await?;
        
        response
            .choices
            .first()
            .map(|choice| choice.message.content.clone())
            .ok_or_else(|| openrouter_api::error::OpenRouterError::ApiError("No response".to_string()))
    }
}
```

## Security Considerations

### API Key Management
- Store API keys in environment variables or secure configuration files
- Never commit API keys to version control
- Use different API keys for development and production
- Implement key rotation policies

### Request Validation
```rust
fn validate_request(request: &ChatCompletionRequest) -> Result<(), &'static str> {
    if request.messages.is_empty() {
        return Err("Messages cannot be empty");
    }
    
    if let Some(max_tokens) = request.max_tokens {
        if max_tokens > 4000 {
            return Err("Max tokens exceeds reasonable limit");
        }
    }
    
    if let Some(temperature) = request.temperature {
        if temperature < 0.0 || temperature > 2.0 {
            return Err("Temperature must be between 0.0 and 2.0");
        }
    }
    
    Ok(())
}
```

## References

- **OpenRouter Official Documentation**: https://openrouter.ai/docs
- **OpenRouter API Reference**: https://openrouter.ai/docs/api-reference
- **openrouter_api Crate**: https://crates.io/crates/openrouter_api
- **OpenRouter Models**: https://openrouter.ai/docs/overview/models
- **Rate Limits**: https://openrouter.ai/docs/api-reference/limits

---

This documentation provides comprehensive coverage of the OpenRouter API and its integration with Rust applications using the `openrouter_api` crate. For the most up-to-date information, always refer to the official OpenRouter documentation.