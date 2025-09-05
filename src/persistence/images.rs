use std::io::Cursor;

use anyhow::{Context as _, Result};
use image::{ImageEncoder, RgbImage, codecs::jpeg::JpegEncoder};
use kameo::message::Context;
use kameo::prelude::*;
use tokio::process::Command;
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::{config::global::ImageHostConfig, supervisor::Supervisor};

/// Actor responsible for uploading images to a remote host
#[derive(Actor)]
pub struct ImageUploader {
    config: ImageHostConfig,
}

impl ImageUploader {
    pub fn new(config: ImageHostConfig) -> Self {
        Self { config }
    }
}

/// Message to upload an image
pub struct UploadImage {
    pub image: RgbImage,
}

/// Reply containing the uploaded image URL
#[derive(Reply)]
pub struct UploadedUrl(pub String);

impl Message<UploadImage> for ImageUploader {
    type Reply = Result<UploadedUrl>;

    async fn handle(
        &mut self,
        msg: UploadImage,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.upload_impl(msg).await.map(UploadedUrl)
    }
}

impl ImageUploader {
    async fn upload_impl(&self, msg: UploadImage) -> Result<String> {
        // Generate unique filename using UUID v7 (time-ordered)
        let uuid = Uuid::now_v7();
        let filename = format!("{}.jpg", uuid);

        // Convert image to JPEG with high quality
        let mut jpeg_bytes = Vec::new();
        {
            let mut cursor = Cursor::new(&mut jpeg_bytes);
            let encoder = JpegEncoder::new_with_quality(&mut cursor, 95);
            encoder
                .write_image(
                    msg.image.as_raw(),
                    msg.image.width(),
                    msg.image.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .context("Failed to encode image as JPEG")?;
        }

        // Create temp file
        let temp_path = format!("/tmp/{}", filename);
        tokio::fs::write(&temp_path, &jpeg_bytes)
            .await
            .context("Failed to write temporary file")?;

        // Upload via SCP
        let remote_path = format!(
            "{}:{}/{}",
            self.config.ssh_hostname, self.config.ssh_directory, filename
        );

        debug!("Uploading image to {}", remote_path);

        let output = Command::new("scp")
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg(&temp_path)
            .arg(&remote_path)
            .output()
            .await
            .context("Failed to execute scp command")?;

        // Clean up temp file
        let _ = tokio::fs::remove_file(&temp_path).await;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("SCP upload failed: {}", stderr);
            anyhow::bail!("Failed to upload image: {}", stderr);
        }

        // Construct public URL
        let url = format!(
            "{}/{}",
            self.config.base_url.trim_end_matches('/'),
            filename
        );
        info!("Successfully uploaded image to {}", url);

        Ok(url)
    }
}

/// Upload an image to the configured host
pub async fn upload_image(image: RgbImage) -> Result<String> {
    let config = Supervisor::image_host().await;
    let uploader = ImageUploader::spawn(ImageUploader::new(config));
    let result = uploader
        .ask(UploadImage { image })
        .await
        .context("Failed to communicate with uploader")?;
    Ok(result.0)
}
