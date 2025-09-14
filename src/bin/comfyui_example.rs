// Include the ComfyUI modules we need
#[path = "../network/comfyui/api.rs"]
mod api;
#[path = "../network/comfyui/net.rs"]
mod net;

use api::{Graph, KSamplerParams};
use net::ComfyUIClient;
use net::create_client;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing for logs
    tracing_subscriber::fmt().with_env_filter("debug").init();

    println!("ComfyUI Client Example");
    println!("=====================");

    // Example 1: Create a client with default settings
    let client = ComfyUIClient::new();
    println!("✓ Created ComfyUI client with default settings");

    // Example 2: Create a client with custom configuration
    let config = create_client()
        .server_address("localhost:8188")
        .execution_timeout(Duration::from_secs(600))
        .build();
    let _custom_client = ComfyUIClient::with_config(config);
    println!("✓ Created ComfyUI client with custom configuration");

    // Example 3: Simple text-to-image generation
    println!("\n--- Simple Text-to-Image Generation ---");
    println!("Attempting to generate image...");

    match client
        .text_to_image(
            "a beautiful landscape with mountains and a lake",
            Some("blurry, low quality"),
            "model.safetensors", // Replace with actual model name
            512,
            512,
        )
        .await
    {
        Ok(image) => {
            println!(
                "✓ Successfully generated image: {}x{}",
                image.width(),
                image.height()
            );
            // You could save the image here:
            // image.save("generated_image.png")?;
            println!("  (Image would be saved as 'generated_image.png')");
        }
        Err(e) => {
            println!("✗ Failed to generate image: {}", e);
            println!("  This is expected if ComfyUI server is not running");
        }
    }

    // Example 4: Using the Graph API for more complex workflows
    println!("\n--- Complex Workflow with Graph API ---");
    println!("Building workflow using Graph API...");

    let mut graph = Graph::new();

    // Load model components
    let (model, clip, vae) = graph.checkpoint_loader("model.safetensors");
    println!("✓ Added checkpoint loader");

    // Encode text prompts
    let positive = graph.clip_text_encode(&clip, "a cat wearing a wizard hat");
    let negative = graph.clip_text_encode(&clip, "blurry, low quality");
    println!("✓ Added text encoding nodes");

    // Create latent space
    let latent = graph.empty_latent_image(768, 768, 1);
    println!("✓ Added latent image node");

    // Configure sampling parameters
    let sampler_params = KSamplerParams {
        seed: 42,
        steps: 20,
        cfg: 7.5,
        sampler: "euler".to_string(),
        scheduler: "normal".to_string(),
        denoise: 1.0,
    };
    println!("✓ Configured sampler parameters");

    // Sample in latent space
    let samples = graph.ksampler(&model, &positive, &negative, &latent, sampler_params);
    println!("✓ Added KSampler node");

    // Decode to image
    let images = graph.vae_decode(&vae, &samples);
    println!("✓ Added VAE decode node");

    // Save image
    graph.save_images(&images, "ganbot_example");
    println!("✓ Added save image node");

    println!("✓ Workflow built successfully with {} nodes", 7);

    // Execute the workflow with progress callback
    println!("\nAttempting to execute workflow...");
    let progress_callback = |progress: f32, node: Option<&str>| {
        if let Some(node_name) = node {
            if node_name == "completed" {
                println!("✓ Execution completed!");
            } else {
                println!(
                    "  Progress: {:.1}% - Executing: {}",
                    progress * 100.0,
                    node_name
                );
            }
        } else {
            println!("  Progress: {:.1}%", progress * 100.0);
        }
    };

    match client
        .execute_graph(graph, Some(Box::new(progress_callback)))
        .await
    {
        Ok(images) => {
            println!("✓ Successfully generated {} image(s)", images.len());
            for (i, img) in images.iter().enumerate() {
                println!("  Image {}: {}x{}", i, img.width(), img.height());
                // Save each image
                // img.save(format!("complex_workflow_{}.png", i))?;
                println!("    (Would be saved as 'complex_workflow_{}.png')", i);
            }
        }
        Err(e) => {
            println!("✗ Failed to execute workflow: {}", e);
            println!("  This is expected if ComfyUI server is not running");
        }
    }

    println!("\n--- Example Summary ---");
    println!("This example demonstrates:");
    println!("• Creating ComfyUI clients with default and custom configuration");
    println!("• Simple text-to-image generation with built-in workflow");
    println!("• Complex workflow building using the type-safe Graph API");
    println!("• Progress monitoring during workflow execution");
    println!("• Error handling for various failure scenarios");
    println!("\nTo use with a real ComfyUI server:");
    println!("1. Start ComfyUI server: python main.py --port 8188");
    println!("2. Replace 'model.safetensors' with an actual model filename");
    println!("3. Run: cargo run --bin comfyui_example");

    Ok(())
}
