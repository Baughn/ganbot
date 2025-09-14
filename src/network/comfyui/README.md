# ComfyUI Network Module

This module provides a fully async Rust client for interacting with ComfyUI's REST API and WebSocket interface. It allows you to submit workflows, monitor execution progress, and retrieve generated images.

## Features

- **Async/await support** using Tokio
- **Type-safe workflow building** via the Graph API
- **Real-time progress monitoring** via WebSocket
- **Automatic image retrieval** with conversion to `RgbImage`
- **Robust error handling** with custom error types
- **Configurable timeouts and retries**
- **Builder pattern** for client configuration

## Quick Start

### Simple Text-to-Image Generation

```rust
use ganbot::network::comfyui::ComfyUIClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = ComfyUIClient::new();
    
    let image = client.text_to_image(
        "a beautiful sunset over mountains",
        Some("blurry, low quality"),
        "model.safetensors",
        512,
        512,
    ).await?;
    
    image.save("generated_image.png")?;
    Ok(())
}
```

### Using the Graph API

```rust
use ganbot::network::comfyui::{ComfyUIClient, Graph, KSamplerParams};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = ComfyUIClient::new();
    let mut graph = Graph::new();
    
    // Build workflow
    let (model, clip, vae) = graph.checkpoint_loader("model.safetensors");
    let positive = graph.clip_text_encode(&clip, "a cat wearing a wizard hat");
    let negative = graph.clip_text_encode(&clip, "blurry");
    let latent = graph.empty_latent_image(512, 512, 1);
    
    let params = KSamplerParams::default();
    let samples = graph.ksampler(&model, &positive, &negative, &latent, params);
    let images = graph.vae_decode(&vae, &samples);
    graph.save_image(&images, "output");
    
    // Execute with progress tracking
    let progress_callback = |progress: f32, node: Option<&str>| {
        println!("Progress: {:.1}%", progress * 100.0);
    };
    
    let results = client.execute_graph(graph, Some(Box::new(progress_callback))).await?;
    
    for (i, img) in results.iter().enumerate() {
        img.save(format!("result_{}.png", i))?;
    }
    
    Ok(())
}
```

### Custom Configuration

```rust
use ganbot::network::comfyui::create_client;
use std::time::Duration;

let config = create_client()
    .server_address("192.168.1.100:8188")
    .execution_timeout(Duration::from_secs(600))
    .retry_attempts(5)
    .build();

let client = ComfyUIClient::with_config(config);
```

## API Reference

### ComfyUIClient

The main client for interacting with ComfyUI.

#### Methods

- `new()` - Create client with default configuration
- `with_config(config)` - Create client with custom configuration
- `execute_workflow(workflow, callback)` - Execute a JSON workflow
- `execute_graph(graph, callback)` - Execute a Graph-built workflow
- `text_to_image(prompt, negative, model, width, height)` - Simple text-to-image generation

### Graph

Type-safe workflow builder for ComfyUI nodes.

#### Common Node Methods

- `checkpoint_loader(model_name)` - Load a model checkpoint
- `clip_text_encode(clip, text)` - Encode text prompts
- `empty_latent_image(width, height, batch_size)` - Create latent space
- `ksampler(model, positive, negative, latent, params)` - Sample in latent space
- `vae_decode(vae, samples)` - Decode latents to images
- `save_image(images, prefix)` - Save images to disk

### ComfyUIError

Error types that can occur during operations:

- `Http` - HTTP request failures
- `WebSocket` - WebSocket connection issues
- `Json` - JSON parsing errors
- `Image` - Image processing errors
- `WorkflowValidation` - Workflow validation failures
- `ExecutionTimeout` - Execution timeouts
- `Connection` - Connection failures
- `Server` - Server-side errors
- `NoImagesFound` - No images in results
- `InvalidWorkflow` - Invalid workflow structure

### Configuration Options

- `server_address` - ComfyUI server address (default: "127.0.0.1:8188")
- `connection_timeout` - HTTP connection timeout (default: 30s)
- `execution_timeout` - Workflow execution timeout (default: 5 minutes)
- `retry_attempts` - Number of retry attempts (default: 3)
- `retry_delay` - Delay between retries (default: 1s)

## Requirements

- ComfyUI server running and accessible
- Rust with Tokio async runtime
- Required model files in ComfyUI's models directory

## Error Handling

The module uses comprehensive error handling with the `ComfyUIError` enum. All async methods return `Result<T, ComfyUIError>`.

```rust
match client.text_to_image("prompt", None, "model.safetensors", 512, 512).await {
    Ok(image) => {
        // Handle success
        image.save("output.png")?;
    }
    Err(ComfyUIError::ExecutionTimeout { timeout }) => {
        eprintln!("Execution timed out after {} seconds", timeout);
    }
    Err(ComfyUIError::Server { status, message }) => {
        eprintln!("Server error {}: {}", status, message);
    }
    Err(e) => {
        eprintln!("Other error: {}", e);
    }
}
```

## Progress Monitoring

You can monitor workflow execution progress using the progress callback:

```rust
let progress_callback = |progress: f32, node: Option<&str>| {
    if let Some(node_name) = node {
        println!("Executing {}: {:.1}%", node_name, progress * 100.0);
    }
};

let results = client.execute_graph(graph, Some(Box::new(progress_callback))).await?;
```

## Integration with Ganbot3

This module is designed to integrate seamlessly with Ganbot3's actor-based architecture:

```rust
use kameo::actor::{Actor, ActorRef};

pub struct ImageGenerationActor {
    comfyui_client: ComfyUIClient,
}

impl Actor for ImageGenerationActor {
    async fn on_start(&mut self, ctx: &mut Context<Self>) {
        // Initialize ComfyUI client
        self.comfyui_client = ComfyUIClient::new();
    }
}
```

For more examples, see the `examples/` directory in the project root.
