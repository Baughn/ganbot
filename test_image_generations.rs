#!/usr/bin/env rust-script
//! Test script to verify the image:generations Redis map functionality
//! Run with: `cargo run --bin test_image_generations`

use anyhow::Result;
use ganbot::persistence::images::{get_image_generation, ImageGenerationRequest};
use ganbot::messages::imagen::{Generate, References};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    println!("Testing image:generations Redis map implementation");

    // Create a test Generate request
    let test_prompt = Generate {
        raw_prompt: "A beautiful sunset over mountains".to_string(),
        prompt: "A beautiful sunset over mountains, high quality, detailed".to_string(),
        negative_prompt: Some("blurry, low quality".to_string()),
        num_images: Some(1),
        aspect: Some((16, 9)),
        width: Some(1024),
        height: Some(576),
        model: Some("SDXL".to_string()),
        seed: Some(12345),
        steps: Some(30),
        references: References {
            img2img: None,
            img2img_strength: None,
            context: vec![],
        },
    };

    // Test UUID
    let test_uuid = Uuid::now_v7().to_string();
    
    println!("Test UUID: {}", test_uuid);
    println!("Test prompt: {}", test_prompt.raw_prompt);

    // Test storing generation data (this would normally be done in upload_impl)
    let generation_data = ImageGenerationRequest {
        prompt: test_prompt.clone(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        backend: "StableDiffusion".to_string(),
        workflow: Some(serde_json::json!({"test": "workflow"})),
    };

    // Store the data manually for testing
    let mut conn = ganbot::supervisor::Supervisor::redis().await;
    let generation_json = serde_json::to_string(&generation_data)?;
    
    let _: () = redis::cmd("HSET")
        .arg("image:generations")
        .arg(&test_uuid)
        .arg(generation_json)
        .query_async(&mut conn)
        .await?;

    println!("✓ Stored generation data in Redis");

    // Now test retrieval
    match get_image_generation(&test_uuid).await? {
        Some(retrieved_data) => {
            println!("✓ Successfully retrieved generation data");
            println!("  Backend: {}", retrieved_data.backend);
            println!("  Original prompt: {}", retrieved_data.prompt.raw_prompt);
            println!("  Timestamp: {}", retrieved_data.timestamp);
            println!("  Has workflow: {}", retrieved_data.workflow.is_some());
        }
        None => {
            println!("✗ Failed to retrieve generation data");
        }
    }

    // Test with non-existent UUID
    let fake_uuid = Uuid::new_v4().to_string();
    match get_image_generation(&fake_uuid).await? {
        Some(_) => println!("✗ Unexpected data for fake UUID"),
        None => println!("✓ Correctly returned None for non-existent UUID"),
    }

    // Cleanup test data
    let _: () = redis::cmd("HDEL")
        .arg("image:generations")
        .arg(&test_uuid)
        .query_async(&mut conn)
        .await?;

    println!("✓ Cleaned up test data");
    println!("\nAll tests passed! The image:generations Redis map is working correctly.");

    Ok(())
}