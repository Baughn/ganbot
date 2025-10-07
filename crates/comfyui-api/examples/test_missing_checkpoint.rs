use comfyui_api::{ComfyUIClient, Graph, KSamplerParams};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing to see what happens
    tracing_subscriber::fmt().with_env_filter("debug").init();

    println!("Testing ComfyUI error with nonexistent checkpoint...\n");

    let client = ComfyUIClient::new();
    let mut graph = Graph::new();

    // Use a checkpoint that definitely doesn't exist
    let (model, clip, vae) = graph.checkpoint_loader("NONEXISTENT_FILE.safetensors");

    let positive = graph.clip_text_encode(&clip, "a test image");
    let negative = graph.clip_text_encode(&clip, "bad quality");
    let latent = graph.empty_latent_image(512, 512, 1);

    let sampler_params = KSamplerParams {
        seed: 42,
        steps: 20,
        cfg: 7.5,
        sampler: "euler".to_string(),
        scheduler: "normal".to_string(),
        denoise: 1.0,
    };

    let samples = graph.ksampler(&model, &positive, &negative, &latent, sampler_params);
    let images = graph.vae_decode(&vae, &samples);
    graph.save_images(&images, "test");

    println!("Executing workflow with nonexistent checkpoint...\n");

    match client.execute_graph(graph, None).await {
        Ok(_) => println!("Unexpectedly succeeded!"),
        Err(e) => {
            println!("Got error (this is expected):");
            println!("Display format: {}", e);
            println!("\nDebug format: {:?}", e);
            println!("\nAlternate format: {:#}", e);

            // Check if it's anyhow::Error with a chain
            println!("\nError chain:");
            let mut current_error = &e as &dyn std::error::Error;
            let mut depth = 0;
            loop {
                println!("  [{}] {}", depth, current_error);
                match current_error.source() {
                    Some(source) => {
                        current_error = source;
                        depth += 1;
                    }
                    None => break,
                }
            }
        }
    }

    Ok(())
}
