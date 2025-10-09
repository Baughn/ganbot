use std::{io::Cursor, os::unix::fs::PermissionsExt as _, sync::Arc};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use anyhow::{Context as _, Result};
use exif::experimental::Writer;
use exif::{Field, In, Tag, Value};
use image::{ImageEncoder, Rgb, RgbImage, codecs::jpeg::JpegEncoder};
use imageproc::drawing::draw_text_mut;
use kameo::{Actor, actor::ActorRef, message::Context, prelude::*, registry::ACTOR_REGISTRY};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use tokio::process::Command;
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::{
    config::global::ImageHostConfig, messages::imagen::Generate, persistence::user::UserId,
    supervisor::Supervisor,
};

use redis::aio::ConnectionManager;

// Lazy-loaded font to avoid re-parsing TTF data on every gallery creation
lazy_static! {
    static ref GALLERY_FONT: FontRef<'static> = {
        let font_data = include_bytes!("../../fonts/gallery.ttf");
        FontRef::try_from_slice(font_data).expect("Failed to load embedded gallery font")
    };
}

const GALLERY_METADATA_KEY: &str = "image:galleries";
const GALLERY_MESSAGE_INDEX_KEY: &str = "image:gallery:messages";

/// Layout information for a rendered gallery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GalleryLayout {
    pub columns: u32,
    pub rows: u32,
    pub row_counts: Vec<u32>,
}

impl GalleryLayout {
    pub fn new(columns: u32, rows: u32, image_count: usize) -> Self {
        let mut row_counts = Vec::with_capacity(rows as usize);
        let mut remaining = image_count as u32;
        for _ in 0..rows {
            if remaining == 0 {
                row_counts.push(0);
                continue;
            }
            let count = remaining.min(columns);
            row_counts.push(count);
            remaining = remaining.saturating_sub(count);
        }
        Self {
            columns,
            rows,
            row_counts,
        }
    }
}

/// Result of constructing a gallery image.
#[derive(Debug)]
pub struct GalleryRender {
    pub image: RgbImage,
    pub layout: GalleryLayout,
}

/// Represents an image generation request for tracking in Redis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageGenerationRequest {
    /// The original generation request
    pub prompt: Generate,
    /// When the image was generated
    pub timestamp: String,
    /// Which backend was used (StableDiffusion, NanoBanana, etc.)
    pub backend: String,
    /// Optional ComfyUI workflow JSON
    pub workflow: Option<serde_json::Value>,
}

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
    pub image: Arc<RgbImage>,
    pub workflow: Option<serde_json::Value>,
    pub backend: Option<String>,
    pub generation_request: Option<Generate>,
}

/// Message to create and upload a gallery.
pub struct UploadGallery {
    pub images: Vec<GalleryImageInput>,
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub workflow: Option<serde_json::Value>,
    pub backend: Option<String>,
    pub generation_request: Option<Generate>,
}

/// Reply containing the uploaded image URL
#[derive(Reply)]
pub struct UploadedUrl(pub String);

/// Reply containing the uploaded gallery information
#[derive(Reply, Debug, Clone)]
pub struct UploadedGallery {
    pub id: String,
    pub gallery_url: String,
    pub image_urls: Vec<String>,
    pub layout: GalleryLayout,
}

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

impl Message<UploadGallery> for ImageUploader {
    type Reply = Result<UploadedGallery>;

    async fn handle(
        &mut self,
        msg: UploadGallery,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.upload_gallery_impl(msg).await
    }
}

/// Truncates EXIF data by replacing large strings and limiting total size
fn truncate_exif_data(workflow: &serde_json::Value) -> Result<String> {
    // Helper function to recursively walk JSON and replace large strings
    fn truncate_large_strings(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(s) => {
                if s.len() > 10 * 1024 {
                    // 10 KiB
                    let size_kb = s.len() / 1024;
                    *s = format!("[{} KiB of data elided]", size_kb);
                }
            }
            serde_json::Value::Object(map) => {
                for (_, v) in map.iter_mut() {
                    truncate_large_strings(v);
                }
            }
            serde_json::Value::Array(arr) => {
                for v in arr.iter_mut() {
                    truncate_large_strings(v);
                }
            }
            _ => {} // Numbers, booleans, null are fine as-is
        }
    }

    // Clone the workflow so we don't modify the original
    let mut truncated_workflow = workflow.clone();
    truncate_large_strings(&mut truncated_workflow);

    // Serialize to pretty JSON string
    let json_string = serde_json::to_string_pretty(&truncated_workflow)
        .context("Failed to serialize truncated workflow to JSON")?;

    // If the final JSON is still larger than 63 KiB, truncate it
    if json_string.len() > 63 * 1024 {
        let suffix = "...\n[JSON truncated to 63 KiB limit]";
        let target_size = 63 * 1024;
        let mut truncated = json_string;
        truncated.truncate(target_size - suffix.len()); // Leave exact room for suffix
        truncated.push_str(suffix);
        Ok(truncated)
    } else {
        Ok(json_string)
    }
}

impl ImageUploader {
    async fn upload_impl(&self, msg: UploadImage) -> Result<String> {
        // Generate unique filename using UUID v7 (time-ordered)
        let uuid = Uuid::now_v7();
        let filename = format!("{}.jpg", uuid);

        // Store generation data if provided
        if let Some(ref generation_request) = msg.generation_request
            && let Some(ref backend) = msg.backend
            && let Err(e) = Self::store_generation_data(
                &uuid.to_string(),
                generation_request,
                backend,
                msg.workflow.as_ref(),
            )
            .await
        {
            error!("Failed to store generation data: {:#}", e);
            // Continue with upload even if generation data storage fails
        }

        // Convert image to JPEG with high quality
        let jpeg_bytes = self.encode_image_as_jpeg(
            msg.image.as_ref(),
            msg.workflow.as_ref(),
            msg.backend.as_deref(),
        )?;

        // Upload the JPEG data
        self.upload_jpeg(jpeg_bytes, &filename).await
    }

    async fn upload_gallery_impl(&self, msg: UploadGallery) -> Result<UploadedGallery> {
        if msg.images.is_empty() {
            anyhow::bail!("Cannot upload empty gallery");
        }

        // Generate base UUID for this gallery (time-ordered)
        let base_uuid = Uuid::now_v7();

        // Store generation data if requested
        if let Some(ref generation_request) = msg.generation_request
            && let Some(ref backend) = msg.backend
            && let Err(e) = Self::store_generation_data(
                &base_uuid.to_string(),
                generation_request,
                backend,
                msg.workflow.as_ref(),
            )
            .await
        {
            error!("Failed to store generation data for gallery image: {:#}", e);
            // Continue with upload even if generation data storage fails
        }

        // Create gallery image using existing function
        let gallery_input = GalleryInput {
            title: msg.title.clone(),
            subtitle: msg.subtitle.clone(),
            images: msg.images.clone(),
            workflow: msg.workflow.clone(),
            backend: msg.backend.clone(),
            generation_request: None, // Not used for gallery image creation
        };
        let gallery_render = create_gallery(gallery_input)?;
        let layout = gallery_render.layout.clone();

        // Generate filenames using the pattern {uuid}.{index}.jpg
        // Gallery image gets index 0
        let gallery_filename = format!("{}.0.jpg", base_uuid);

        // Encode and upload gallery image
        let gallery_jpeg = self.encode_image_as_jpeg(
            &gallery_render.image,
            msg.workflow.as_ref(),
            msg.backend.as_deref(),
        )?;
        let gallery_url = self.upload_jpeg(gallery_jpeg, &gallery_filename).await?;

        // Upload individual images with indices 1, 2, 3, etc.
        // Concurrently encode and upload individual images
        let workflow_ref = &msg.workflow;
        let backend_ref = &msg.backend;
        let upload_futures = msg
            .images
            .iter()
            .enumerate()
            .map(|(index, image)| async move {
                let filename = format!("{}.{}.jpg", base_uuid, index + 1);
                let jpeg_bytes = self.encode_image_as_jpeg(
                    image.image.as_ref(),
                    workflow_ref.as_ref(),
                    backend_ref.as_deref(),
                )?;
                self.upload_jpeg(jpeg_bytes, &filename).await
            });

        // Wait for all uploads to complete
        let results: Vec<Result<String>> = futures::future::join_all(upload_futures).await;

        // Collect the URLs, propagating the first error if any occurred
        let image_urls = results.into_iter().collect::<Result<Vec<_>>>()?;

        Ok(UploadedGallery {
            id: base_uuid.to_string(),
            gallery_url,
            image_urls,
            layout,
        })
    }

    /// Encode an RgbImage as high-quality JPEG bytes with optional workflow EXIF metadata
    fn encode_image_as_jpeg(
        &self,
        image: &RgbImage,
        workflow: Option<&serde_json::Value>,
        backend: Option<&str>,
    ) -> Result<Vec<u8>> {
        let mut jpeg_bytes = Vec::new();
        {
            let mut cursor = Cursor::new(&mut jpeg_bytes);
            let encoder = JpegEncoder::new_with_quality(&mut cursor, 95);
            encoder
                .write_image(
                    image.as_raw(),
                    image.width(),
                    image.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .context("Failed to encode image as JPEG")?;
        }

        // Add EXIF metadata if workflow is provided
        if let (Some(workflow), Some(backend_name)) = (workflow, backend) {
            // Create the wrapped workflow JSON
            let wrapped_workflow = serde_json::json!({
                "backend": backend_name,
                "workflow": workflow
            });

            // Truncate EXIF data to prevent oversized metadata
            let workflow_json = truncate_exif_data(&wrapped_workflow)?;

            // Create EXIF writer and add workflow as UserComment
            let mut writer = Writer::new();
            let field = Field {
                tag: Tag::UserComment,
                ifd_num: In::PRIMARY,
                value: Value::Undefined(workflow_json.into_bytes(), 0),
            };
            writer.push_field(&field);

            // Write EXIF metadata to a buffer
            let mut exif_buffer = Cursor::new(Vec::new());
            writer
                .write(&mut exif_buffer, false) // false for big-endian
                .context("Failed to write EXIF metadata")?;

            // Get the EXIF data
            let exif_data = exif_buffer.into_inner();

            // Insert EXIF data into JPEG
            // For JPEG, we need to manually construct the EXIF APP1 segment
            // JPEG format: SOI (0xFFD8) + APP1 (0xFFE1) + length + "Exif\0\0" + EXIF data + rest of JPEG
            if jpeg_bytes.len() >= 2 && jpeg_bytes[0] == 0xFF && jpeg_bytes[1] == 0xD8 {
                let mut new_jpeg = Vec::new();

                // Add SOI marker
                new_jpeg.extend_from_slice(&[0xFF, 0xD8]);

                // Add APP1 segment with EXIF data
                let mut exif_segment_data = b"Exif\0\0".to_vec();
                exif_segment_data.extend_from_slice(&exif_data);
                let segment_length = (exif_segment_data.len() + 2) as u16; // +2 for length field itself

                new_jpeg.extend_from_slice(&[0xFF, 0xE1]); // APP1 marker
                new_jpeg.extend_from_slice(&segment_length.to_be_bytes()); // Length
                new_jpeg.extend_from_slice(&exif_segment_data);

                // Add the rest of the original JPEG (skip SOI marker)
                new_jpeg.extend_from_slice(&jpeg_bytes[2..]);

                jpeg_bytes = new_jpeg;
            }
        }

        Ok(jpeg_bytes)
    }

    /// Upload JPEG bytes to the remote host with the given filename
    async fn upload_jpeg(&self, jpeg_bytes: Vec<u8>, filename: &str) -> Result<String> {
        // Register the file in Redis for tracking and cleanup
        let mut conn = Supervisor::redis().await;
        let timestamp = chrono::Utc::now().timestamp() as f64;
        let _: () = redis::cmd("ZADD")
            .arg("image:files")
            .arg(timestamp)
            .arg(filename)
            .query_async(&mut conn)
            .await
            .context("Failed to register image in Redis")?;

        // Create temporary file using tempfile crate
        let temp_file = NamedTempFile::new().context("Failed to create temporary file")?;

        let temp_path = temp_file
            .path()
            .to_str()
            .context("Temporary file path contains invalid UTF-8")?;

        tokio::fs::write(temp_path, &jpeg_bytes)
            .await
            .context("Failed to write temporary file")?;
        tokio::fs::set_permissions(temp_path, std::fs::Permissions::from_mode(0o644))
            .await
            .context("Failed to set permissions on temporary file")?;

        // Upload via SCP
        let remote_path = format!(
            "{}:{}/{}",
            self.config.ssh_hostname, self.config.ssh_directory, filename
        );

        debug!("Uploading image to {}", remote_path);

        // Execute SCP and ensure Redis registration is cleaned up on error
        let scp_result = Command::new("scp")
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg(temp_path)
            .arg(&remote_path)
            .output()
            .await;

        let cleanup_redis = || async {
            let mut cleanup_conn = Supervisor::redis().await;
            let _: () = redis::cmd("ZREM")
                .arg("image:files")
                .arg(filename)
                .query_async(&mut cleanup_conn)
                .await
                .context("Failed to cleanup Redis after SCP error")?;
            Ok(())
        };

        let output = match scp_result {
            Ok(o) => o,
            Err(e) => return cleanup_redis().await.and_then(|_| Err(e.into())),
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(stderr = %stderr, filename = %filename, "SCP upload failed");
            // Cleanup Redis registration since upload failed
            return cleanup_redis()
                .await
                .and_then(|_| anyhow::bail!("Failed to upload image via SCP: {}", stderr));
        }

        // Construct public URL
        let url = format!(
            "{}/{}",
            self.config.base_url.trim_end_matches('/'),
            filename
        );
        info!(url = %url, filename = %filename, "Successfully uploaded image");

        Ok(url)
    }

    /// Store image generation data in Redis for later retrieval
    async fn store_generation_data(
        uuid: &str,
        generation_request: &Generate,
        backend: &str,
        workflow: Option<&serde_json::Value>,
    ) -> Result<()> {
        let generation_data = ImageGenerationRequest {
            prompt: generation_request.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            backend: backend.to_string(),
            workflow: workflow.cloned(),
        };

        let mut conn = Supervisor::redis().await;
        let generation_json = serde_json::to_string(&generation_data)
            .context("Failed to serialize generation data")?;

        let _: () = redis::cmd("HSET")
            .arg("image:generations")
            .arg(uuid)
            .arg(generation_json)
            .query_async(&mut conn)
            .await
            .context("Failed to store generation data in Redis")?;

        debug!("Stored generation data for image UUID: {}", uuid);
        Ok(())
    }
}

// /// Retrieve image generation data from Redis by UUID. Currently unused.
// pub async fn get_image_generation(uuid: &str) -> Result<Option<ImageGenerationRequest>> {
//     let mut conn = Supervisor::redis().await;

//     let result: Option<String> = redis::cmd("HGET")
//         .arg("image:generations")
//         .arg(uuid)
//         .query_async(&mut conn)
//         .await
//         .context("Failed to retrieve generation data from Redis")?;

//     match result {
//         Some(json_data) => {
//             let generation_data = serde_json::from_str(&json_data)
//                 .context("Failed to deserialize generation data")?;
//             Ok(Some(generation_data))
//         }
//         None => Ok(None),
//     }
// }

/// Result of a delete operation
#[derive(Debug)]
pub struct DeleteResult {
    pub message: String,
}

/// Delete an image by UUID, verifying user ownership
pub async fn delete_image(uuid: &str, username: &str) -> Result<DeleteResult> {
    info!(uuid = %uuid, user = %username, "Attempting to delete image");

    let mut conn = Supervisor::redis().await;

    // Verify ownership by checking if this image is in the user's history
    let user_images_key = format!("user:images:{}", username);
    let user_images: Vec<String> = redis::cmd("ZRANGE")
        .arg(&user_images_key)
        .arg(0)
        .arg(-1)
        .query_async(&mut conn)
        .await
        .context("Failed to get user images from Redis")?;

    let mut user_owns_image = false;
    for image_json in &user_images {
        if let Ok(image_data) = serde_json::from_str::<serde_json::Value>(image_json)
            && let Some(url) = image_data.get("url").and_then(|v| v.as_str())
            && url.contains(uuid)
        {
            user_owns_image = true;
            break;
        }
    }

    if !user_owns_image {
        anyhow::bail!("You can only delete images you generated.");
    }

    info!(user = %username, uuid = %uuid, "User authorized to delete image");

    // Find all files associated with this UUID (including gallery images) via ZSCAN MATCH
    let pattern = format!("{}*", uuid);
    let mut cursor: u64 = 0;
    let mut matching_files: Vec<String> = Vec::new();
    loop {
        let (next_cursor, entries): (u64, Vec<(String, f64)>) = redis::cmd("ZSCAN")
            .arg("image:files")
            .arg(cursor)
            .arg("MATCH")
            .arg(&pattern)
            .arg("COUNT")
            .arg(1000)
            .query_async(&mut conn)
            .await
            .context("Failed to scan Redis image files")?;

        for (member, _score) in entries {
            matching_files.push(member);
        }

        if next_cursor == 0 {
            break;
        }
        cursor = next_cursor;
    }

    if matching_files.is_empty() {
        info!("No files found for UUID {}", uuid);
        // Still remove from generations hash in case of orphaned data
        let _: () = redis::cmd("HDEL")
            .arg("image:generations")
            .arg(uuid)
            .query_async(&mut conn)
            .await
            .context("Failed to remove generation data from Redis")?;

        return Ok(DeleteResult {
            message: "Image metadata cleaned up (files were already missing).".to_string(),
        });
    }

    info!(uuid = %uuid, count = matching_files.len(), files = ?matching_files, "Found files to delete");

    // Delete files from remote server
    let config = Supervisor::image_host().await;
    let mut deletion_errors = Vec::new();
    let mut deleted_files = Vec::new();

    for filename in &matching_files {
        let remote_path = format!(
            "{}:{}/{}",
            config.ssh_hostname, config.ssh_directory, filename
        );

        debug!("Deleting remote file: {}", remote_path);

        let output = Command::new("ssh")
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg(&config.ssh_hostname)
            .arg("rm")
            .arg("-f")
            .arg(format!("{}/{}", config.ssh_directory, filename))
            .output()
            .await
            .context("Failed to execute ssh rm command")?;

        if output.status.success() {
            info!(filename = %filename, "Successfully deleted remote file");
            deleted_files.push(filename.clone());
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let error_msg = format!("Failed to delete {}: {}", filename, stderr);
            error!("{:#}", error_msg);
            deletion_errors.push(error_msg);
        }
    }

    // Remove files from Redis tracking (only successfully deleted ones)
    for filename in &deleted_files {
        let _: () = redis::cmd("ZREM")
            .arg("image:files")
            .arg(filename)
            .query_async(&mut conn)
            .await
            .context("Failed to remove file from Redis tracking")?;
    }

    // Remove generation data from Redis
    let _: () = redis::cmd("HDEL")
        .arg("image:generations")
        .arg(uuid)
        .query_async(&mut conn)
        .await
        .context("Failed to remove generation data from Redis")?;

    // Remove from user's image history
    // Find and remove entries that reference this UUID
    for image_json in &user_images {
        if let Ok(image_data) = serde_json::from_str::<serde_json::Value>(image_json)
            && let Some(url) = image_data.get("url").and_then(|v| v.as_str())
            && url.contains(uuid)
        {
            let _: () = redis::cmd("ZREM")
                .arg(&user_images_key)
                .arg(image_json)
                .query_async(&mut conn)
                .await
                .context("Failed to remove image from user history")?;
            debug!("Removed image from user history: {}", url);
        }
    }

    // Prepare result message
    let message = if deletion_errors.is_empty() {
        if deleted_files.len() == 1 {
            "Successfully deleted image.".to_string()
        } else {
            format!(
                "Successfully deleted {} images (including gallery images).",
                deleted_files.len()
            )
        }
    } else {
        format!(
            "Partially completed: deleted {} files, but {} failures: {}",
            deleted_files.len(),
            deletion_errors.len(),
            deletion_errors.join("; ")
        )
    };

    info!(uuid = %uuid, result = %message, "Delete operation completed");

    Ok(DeleteResult { message })
}

/// Upload an image to the configured host
pub async fn upload_image(image: impl Into<Arc<RgbImage>>) -> Result<String> {
    upload_image_with_workflow(image, None, None).await
}

/// Upload an image with optional workflow metadata to the configured host
pub async fn upload_image_with_workflow(
    image: impl Into<Arc<RgbImage>>,
    workflow: Option<serde_json::Value>,
    backend: Option<String>,
) -> Result<String> {
    upload_image_with_generation(image, workflow, backend, None).await
}

/// Upload an image with complete generation request data to the configured host
pub async fn upload_image_with_generation(
    image: impl Into<Arc<RgbImage>>,
    workflow: Option<serde_json::Value>,
    backend: Option<String>,
    generation_request: Option<Generate>,
) -> Result<String> {
    let image = image.into();
    let config = Supervisor::image_host().await;
    let uploader = ImageUploader::spawn(ImageUploader::new(config));
    let result = uploader
        .ask(UploadImage {
            image,
            workflow,
            backend,
            generation_request,
        })
        .await
        .context("Failed to communicate with uploader")?;
    Ok(result.0)
}

/// Upload a gallery of images with title and subtitle to the configured host
/// Returns (gallery_url, individual_image_urls)
pub async fn upload_gallery(input: GalleryInput) -> Result<UploadedGallery> {
    let config = Supervisor::image_host().await;
    let uploader = ImageUploader::spawn(ImageUploader::new(config));
    let result = uploader
        .ask(UploadGallery {
            images: input.images,
            title: input.title,
            subtitle: input.subtitle,
            workflow: input.workflow,
            backend: input.backend,
            generation_request: input.generation_request,
        })
        .await
        .context("Failed to communicate with uploader")?;
    Ok(result)
}

/// Input for creating a gallery image
#[derive(Clone)]
pub struct GalleryImageInput {
    pub image: Arc<RgbImage>,
    pub title: Option<String>,
}

/// Input for creating a gallery image
pub struct GalleryInput {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub images: Vec<GalleryImageInput>,
    pub workflow: Option<serde_json::Value>,
    pub backend: Option<String>,
    pub generation_request: Option<Generate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GalleryMetadata {
    pub id: String,
    pub owner_id: UserId,
    pub gallery_url: Option<String>,
    pub image_urls: Vec<String>,
    pub prompts: Vec<String>,
    pub display_prompts: Vec<String>,
    pub layout: GalleryLayout,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct RegisterGallery {
    pub metadata: GalleryMetadata,
}

#[derive(Debug, Clone)]
pub struct AssociateMessage {
    pub gallery_id: String,
    pub channel_id: u64,
    pub message_id: u64,
}

#[derive(Debug, Clone)]
pub struct GetGallery(pub String);

#[derive(Debug, Clone)]
pub struct GetGalleryByMessage {
    pub channel_id: u64,
    pub message_id: u64,
}

pub struct GalleryRegistry {
    redis: ConnectionManager,
}

impl GalleryRegistry {
    pub fn get() -> Result<ActorRef<Self>> {
        ACTOR_REGISTRY
            .lock()
            .unwrap()
            .get::<Self, str>("gallery_registry")
            .context("while fetching GalleryRegistry")?
            .context("GalleryRegistry not registered")
    }

    fn message_field(channel_id: u64, message_id: u64) -> String {
        format!("{}:{}", channel_id, message_id)
    }
}

impl Actor for GalleryRegistry {
    type Args = ConnectionManager;
    type Error = anyhow::Error;

    async fn on_start(args: Self::Args, actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        ACTOR_REGISTRY
            .lock()
            .unwrap()
            .insert("gallery_registry", actor_ref);
        Ok(Self { redis: args })
    }
}

impl Message<RegisterGallery> for GalleryRegistry {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        msg: RegisterGallery,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let payload =
            serde_json::to_string(&msg.metadata).context("while serializing gallery metadata")?;
        let mut conn = self.redis.clone();
        redis::cmd("HSET")
            .arg(GALLERY_METADATA_KEY)
            .arg(&msg.metadata.id)
            .arg(payload)
            .query_async::<()>(&mut conn)
            .await
            .context("while storing gallery metadata")?;
        Ok(())
    }
}

impl Message<AssociateMessage> for GalleryRegistry {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        msg: AssociateMessage,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let field = Self::message_field(msg.channel_id, msg.message_id);
        let mut conn = self.redis.clone();
        redis::cmd("HSET")
            .arg(GALLERY_MESSAGE_INDEX_KEY)
            .arg(field)
            .arg(msg.gallery_id)
            .query_async::<()>(&mut conn)
            .await
            .context("while storing gallery message association")?;
        Ok(())
    }
}

impl Message<GetGallery> for GalleryRegistry {
    type Reply = Result<Option<GalleryMetadata>>;

    async fn handle(
        &mut self,
        msg: GetGallery,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let mut conn = self.redis.clone();
        let data: Option<String> = redis::cmd("HGET")
            .arg(GALLERY_METADATA_KEY)
            .arg(&msg.0)
            .query_async(&mut conn)
            .await
            .context("while loading gallery metadata")?;

        let gallery = if let Some(json) = data {
            Some(serde_json::from_str(&json).context("while deserializing gallery metadata")?)
        } else {
            None
        };

        Ok(gallery)
    }
}

impl Message<GetGalleryByMessage> for GalleryRegistry {
    type Reply = Result<Option<GalleryMetadata>>;

    async fn handle(
        &mut self,
        msg: GetGalleryByMessage,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let field = Self::message_field(msg.channel_id, msg.message_id);
        let mut conn = self.redis.clone();
        let gallery_id: Option<String> = redis::cmd("HGET")
            .arg(GALLERY_MESSAGE_INDEX_KEY)
            .arg(&field)
            .query_async(&mut conn)
            .await
            .context("while fetching gallery id for message")?;

        if let Some(id) = gallery_id {
            self.handle(GetGallery(id), ctx).await
        } else {
            Ok(None)
        }
    }
}

/// Calculate the required height for the text area based on title and subtitle content
fn calculate_text_area_height(
    title: Option<&str>,
    subtitle: Option<&str>,
    canvas_width: u32,
    text_padding: u32,
) -> u32 {
    let font = &*GALLERY_FONT;

    let title = title.and_then(|t| {
        let trimmed = t.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });
    let subtitle = subtitle.and_then(|t| {
        let trimmed = t.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });

    if title.is_none() && subtitle.is_none() {
        return 0;
    }

    // Calculate maximum text width (canvas width minus padding on both sides)
    let max_text_width = canvas_width as f32 - (text_padding * 2) as f32;

    // Define font scales and line heights (same as in render_text_on_canvas)
    let title_scale = PxScale::from(32.0);
    let subtitle_scale = PxScale::from(20.0);
    let title_line_height = 36; // pixels between title lines
    let subtitle_line_height = 22; // pixels between subtitle lines
    let title_subtitle_spacing = 6; // pixels between title and subtitle sections

    let title_lines = title
        .map(|value| wrap_text(value, font, title_scale, max_text_width))
        .unwrap_or_default();
    let subtitle_lines = subtitle
        .map(|value| wrap_text(value, font, subtitle_scale, max_text_width))
        .unwrap_or_default();

    let title_height = if title_lines.is_empty() {
        0
    } else {
        title_lines.len() as u32 * title_line_height
    };

    let subtitle_height = if subtitle_lines.is_empty() {
        0
    } else {
        subtitle_lines.len() as u32 * subtitle_line_height
    };

    let spacing = if !title_lines.is_empty() && !subtitle_lines.is_empty() {
        title_subtitle_spacing
    } else {
        0
    };

    let buffer_zone = 20; // Extra space to ensure text doesn't touch images
    let total_height =
        text_padding + title_height + spacing + subtitle_height + text_padding + buffer_zone;

    const MIN_TEXT_AREA_HEIGHT: u32 = 60;
    total_height.max(MIN_TEXT_AREA_HEIGHT)
}

/// Calculate the required height for caption text
fn calculate_caption_text_height(prompt: &str, max_text_width: f32, text_padding: u32) -> u32 {
    let font = &*GALLERY_FONT;

    // Use smaller font for individual prompts
    let prompt_scale = PxScale::from(16.0);
    let prompt_line_height = 20; // pixels between prompt lines

    // Calculate number of lines needed
    let prompt_lines = wrap_text(prompt, font, prompt_scale, max_text_width);

    // Calculate total height needed
    let prompt_height = if prompt_lines.is_empty() {
        0
    } else {
        prompt_lines.len() as u32 * prompt_line_height
    };

    // Add top and bottom padding, plus a small buffer zone
    let buffer_zone = 10; // Less buffer for individual prompts
    let total_height = text_padding + prompt_height + text_padding + buffer_zone;

    // Ensure minimum height even for very short text
    const MIN_PROMPT_TEXT_HEIGHT: u32 = 40;
    total_height.max(MIN_PROMPT_TEXT_HEIGHT)
}

/// Creates a gallery image from multiple images with optional metadata
pub fn create_gallery(input: GalleryInput) -> Result<GalleryRender> {
    if input.images.is_empty() {
        anyhow::bail!("Cannot create gallery from empty image list");
    }

    let image_refs: Vec<&RgbImage> = input
        .images
        .iter()
        .map(|entry| entry.image.as_ref())
        .collect();
    let border_color = calculate_mean_color(&image_refs);
    let text_color = calculate_text_color(border_color);

    let prepared_images = prepare_gallery_images(&input.images, border_color, text_color)?;

    let img_width = prepared_images[0].width();
    let img_height = prepared_images[0].height();
    let (grid_cols, grid_rows) =
        calculate_optimal_grid(prepared_images.len(), img_width, img_height);

    const BORDER_SIZE: u32 = 4;
    const TEXT_PADDING: u32 = 20;

    let canvas_width = grid_cols * img_width + (grid_cols + 1) * BORDER_SIZE;

    let title = input.title.as_deref();
    let subtitle = input.subtitle.as_deref();
    let text_area_height = calculate_text_area_height(title, subtitle, canvas_width, TEXT_PADDING);

    let canvas_height = grid_rows * img_height + (grid_rows + 1) * BORDER_SIZE + text_area_height;
    let mut canvas = RgbImage::from_pixel(canvas_width, canvas_height, border_color);

    for (i, image) in prepared_images.iter().enumerate() {
        let row = (i as u32) / grid_cols;
        let col = (i as u32) % grid_cols;

        let x = col * (img_width + BORDER_SIZE) + BORDER_SIZE;
        let y = row * (img_height + BORDER_SIZE) + BORDER_SIZE + text_area_height;

        blit_image(&mut canvas, image, x, y);
    }

    if text_area_height > 0 {
        render_text_on_canvas(&mut canvas, title, subtitle, text_color, TEXT_PADDING)?;
    }

    Ok(GalleryRender {
        image: canvas,
        layout: GalleryLayout::new(grid_cols, grid_rows, prepared_images.len()),
    })
}

fn prepare_gallery_images(
    entries: &[GalleryImageInput],
    border_color: Rgb<u8>,
    text_color: Rgb<u8>,
) -> Result<Vec<RgbImage>> {
    let first_image = &entries[0].image;
    let base_width = first_image.width();
    let base_height = first_image.height();

    const TEXT_PADDING: u32 = 10;
    let max_text_width = base_width as f32 - (TEXT_PADDING * 2) as f32;

    let has_titles = entries.iter().any(|entry| {
        entry
            .title
            .as_deref()
            .map(|title| !title.trim().is_empty())
            .unwrap_or(false)
    });

    let caption_height = if has_titles {
        entries
            .iter()
            .filter_map(|entry| entry.title.as_deref())
            .map(|title| calculate_caption_text_height(title, max_text_width, TEXT_PADDING))
            .max()
            .unwrap_or(40)
    } else {
        0
    };

    let mut prepared = Vec::with_capacity(entries.len());

    for entry in entries {
        if entry.image.width() != base_width || entry.image.height() != base_height {
            anyhow::bail!("All images in a gallery must have the same dimensions");
        }

        if caption_height == 0 {
            prepared.push(entry.image.as_ref().clone());
            continue;
        }

        let mut canvas =
            RgbImage::from_pixel(base_width, base_height + caption_height, border_color);

        if let Some(title) = entry
            .title
            .as_deref()
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
        {
            render_caption_text(
                &mut canvas,
                title,
                text_color,
                TEXT_PADDING,
                TEXT_PADDING,
                max_text_width,
            )?;
        }

        blit_image(&mut canvas, entry.image.as_ref(), 0, caption_height);
        prepared.push(canvas);
    }

    Ok(prepared)
}

fn blit_image(destination: &mut RgbImage, source: &RgbImage, dest_x: u32, dest_y: u32) {
    for y in 0..source.height() {
        for x in 0..source.width() {
            let pixel = source.get_pixel(x, y);
            destination.put_pixel(dest_x + x, dest_y + y, *pixel);
        }
    }
}

/// Calculate optimal grid layout for given aspect ratio
fn calculate_optimal_grid(image_count: usize, img_width: u32, img_height: u32) -> (u32, u32) {
    if image_count == 0 {
        return (0, 0);
    }

    let target_aspect = 1.7;
    let mut have_perfect_grid = false; // Set if we can make one with no black slots.
    let mut best_cols = 1;
    let mut best_rows = image_count as u32;
    let mut best_diff = f64::INFINITY;
    for cols in 1..=image_count as u32 {
        let rows = ((image_count as f64) / (cols as f64)).ceil() as u32;
        let is_perfect = rows * cols == image_count as u32 && cols > 1;
        let grid_aspect = (cols as f64 * img_width as f64) / (rows as f64 * img_height as f64);
        let diff = (grid_aspect - target_aspect).abs();
        let best_yet = match (is_perfect, have_perfect_grid) {
            (true, false) => true,
            (false, true) => false,
            _ => diff < best_diff,
        };
        if best_yet {
            have_perfect_grid = is_perfect;
            best_cols = cols;
            best_rows = rows;
            best_diff = diff;
        }
    }

    (best_cols, best_rows)
}

/// Calculate mean color across all pixels in all images
fn calculate_mean_color<T>(images: &[T]) -> Rgb<u8>
where
    T: std::borrow::Borrow<RgbImage>,
{
    if images.is_empty() {
        return Rgb([128, 128, 128]); // Default gray
    }

    let mut total_r = 0u64;
    let mut total_g = 0u64;
    let mut total_b = 0u64;
    let mut pixel_count = 0u64;

    for image in images {
        for pixel in image.borrow().pixels() {
            total_r += pixel[0] as u64;
            total_g += pixel[1] as u64;
            total_b += pixel[2] as u64;
            pixel_count += 1;
        }
    }

    if pixel_count == 0 {
        return Rgb([128, 128, 128]);
    }

    Rgb([
        (total_r / pixel_count) as u8,
        (total_g / pixel_count) as u8,
        (total_b / pixel_count) as u8,
    ])
}

/// Calculate optimal text color (black or white) for maximum contrast against background
fn calculate_text_color(background: Rgb<u8>) -> Rgb<u8> {
    // Calculate perceived brightness using the luminance formula
    // This accounts for how the human eye perceives different colors
    let r = background[0] as f32;
    let g = background[1] as f32;
    let b = background[2] as f32;

    let luminance = 0.299 * r + 0.587 * g + 0.114 * b;

    // Use white text for dark backgrounds, black text for light backgrounds
    if luminance < 128.0 {
        Rgb([255, 255, 255]) // White text
    } else {
        Rgb([0, 0, 0]) // Black text
    }
}

/// Measure the width of text in pixels using the given font and scale
fn measure_text_width(text: &str, font: &FontRef, scale: PxScale) -> f32 {
    let scaled_font = font.as_scaled(scale);
    text.chars()
        .map(|c| scaled_font.h_advance(scaled_font.glyph_id(c)))
        .sum()
}

/// Split text into lines that fit within the given width, wrapping at word boundaries
fn wrap_text(text: &str, font: &FontRef, scale: PxScale, max_width: f32) -> Vec<String> {
    if text.is_empty() {
        return vec![];
    }

    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();

    let space_width = measure_text_width(" ", font, scale);

    for word in words {
        let word_width = measure_text_width(word, font, scale);

        // If this is the first word in the line
        if current_line.is_empty() {
            // If even a single word is too wide, we have to use it anyway
            current_line.push_str(word);
        } else {
            // Calculate width if we add this word (including space)
            let current_width = measure_text_width(&current_line, font, scale);
            let total_width = current_width + space_width + word_width;

            if total_width <= max_width {
                // Word fits, add it to current line
                current_line.push(' ');
                current_line.push_str(word);
            } else {
                // Word doesn't fit, start new line
                lines.push(current_line);
                current_line = word.to_string();
            }
        }
    }

    // Don't forget the last line
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    lines
}

/// Render title and subtitle text on canvas
fn render_text_on_canvas(
    canvas: &mut RgbImage,
    title: Option<&str>,
    subtitle: Option<&str>,
    text_color: Rgb<u8>,
    padding: u32,
) -> Result<()> {
    let font = &*GALLERY_FONT;

    let title = title.and_then(|t| {
        let trimmed = t.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });
    let subtitle = subtitle.and_then(|t| {
        let trimmed = t.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });

    if title.is_none() && subtitle.is_none() {
        return Ok(());
    }

    let max_text_width = canvas.width() as f32 - (padding * 2) as f32;
    let x = padding as i32;
    let mut current_y = padding as i32;

    let title_scale = PxScale::from(32.0);
    let subtitle_scale = PxScale::from(20.0);
    let title_line_height = 40;
    let subtitle_line_height = 25;
    let title_subtitle_spacing = 15;

    let title_lines = title
        .map(|value| wrap_text(value, font, title_scale, max_text_width))
        .unwrap_or_default();
    for line in &title_lines {
        draw_text_mut(canvas, text_color, x, current_y, title_scale, font, line);
        current_y += title_line_height;
    }

    let subtitle_lines = subtitle
        .map(|value| wrap_text(value, font, subtitle_scale, max_text_width))
        .unwrap_or_default();
    if !title_lines.is_empty() && !subtitle_lines.is_empty() {
        current_y += title_subtitle_spacing;
    }

    for line in &subtitle_lines {
        draw_text_mut(canvas, text_color, x, current_y, subtitle_scale, font, line);
        current_y += subtitle_line_height;
    }

    Ok(())
}

/// Render caption text at specified position
fn render_caption_text(
    canvas: &mut RgbImage,
    prompt: &str,
    text_color: Rgb<u8>,
    x: u32,
    y: u32,
    max_text_width: f32,
) -> Result<()> {
    // Use the lazy-loaded font
    let font = &*GALLERY_FONT;

    // Define font scale and line height for individual prompts
    let prompt_scale = PxScale::from(16.0);
    let prompt_line_height = 20; // pixels between prompt lines

    // Wrap text to fit within the specified width
    let prompt_lines = wrap_text(prompt, font, prompt_scale, max_text_width);

    // Render each line
    let mut current_y = y as i32;
    for line in prompt_lines {
        draw_text_mut(
            canvas,
            text_color,
            x as i32,
            current_y,
            prompt_scale,
            font,
            &line,
        );
        current_y += prompt_line_height;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn test_calculate_optimal_grid_single_image() {
        let (cols, rows) = calculate_optimal_grid(1, 512, 512);
        assert_eq!((cols, rows), (1, 1));
    }

    #[test]
    fn test_calculate_optimal_grid_two_images() {
        // For 2 images with square dimensions, should prefer 2x1 for better aspect ratio
        let (cols, rows) = calculate_optimal_grid(2, 512, 512);
        assert_eq!((cols, rows), (2, 1));
    }

    #[test]
    fn test_calculate_optimal_grid_four_images() {
        // For 4 images, we'll always get 2x2
        let (cols, rows) = calculate_optimal_grid(4, 512, 512);
        assert_eq!((cols, rows), (2, 2));
    }

    #[test]
    fn test_calculate_optimal_grid_six_images() {
        // For 6 images, should prefer 3x2 over 2x3 for better 16:10 aspect ratio
        let (cols, rows) = calculate_optimal_grid(6, 512, 512);
        assert_eq!((cols, rows), (3, 2));
    }

    #[test]
    fn test_calculate_optimal_grid_with_wide_images() {
        // With wide images (2:1 ratio), different grid layout might be preferred
        let (cols, rows) = calculate_optimal_grid(4, 1024, 512);
        assert!(cols == 2 && rows == 2);
        // For 1536:1024 (The NoobAI wide ratio), we still want a wide layout.
        let (cols, rows) = calculate_optimal_grid(6, 1536, 1024);
        assert_eq!((cols, rows), (3, 2));
    }

    #[test]
    fn test_calculate_optimal_grid_empty() {
        let (cols, rows) = calculate_optimal_grid(0, 512, 512);
        assert_eq!((cols, rows), (0, 0));
    }

    #[test]
    fn test_calculate_optimal_grid_large_count() {
        let (cols, rows) = calculate_optimal_grid(12, 512, 512);
        assert!(cols > 0 && rows > 0);
        assert!(cols * rows >= 12); // Must accommodate all images
        // For 12 images, the algorithm chooses 4x3
        assert_eq!((cols, rows), (4, 3));
    }

    #[test]
    fn test_calculate_mean_color_single_image() {
        // Create a simple 2x2 red image
        let mut image = RgbImage::new(2, 2);
        let red = Rgb([255, 0, 0]);
        for x in 0..2 {
            for y in 0..2 {
                image.put_pixel(x, y, red);
            }
        }

        let images = vec![image];
        let mean_color = calculate_mean_color(&images);
        assert_eq!(mean_color, Rgb([255, 0, 0]));
    }

    #[test]
    fn test_calculate_mean_color_multiple_images() {
        // Create two 1x1 images: one red, one blue
        let mut red_image = RgbImage::new(1, 1);
        red_image.put_pixel(0, 0, Rgb([255, 0, 0]));

        let mut blue_image = RgbImage::new(1, 1);
        blue_image.put_pixel(0, 0, Rgb([0, 0, 255]));

        let images = vec![red_image, blue_image];
        let mean_color = calculate_mean_color(&images);
        // Should be average: (255+0)/2, (0+0)/2, (0+255)/2 = (127, 0, 127)
        assert_eq!(mean_color, Rgb([127, 0, 127]));
    }

    #[test]
    fn test_calculate_mean_color_mixed_colors() {
        // Create a 2x1 image with different colors
        let mut image = RgbImage::new(2, 1);
        image.put_pixel(0, 0, Rgb([100, 150, 200])); // Pixel 1
        image.put_pixel(1, 0, Rgb([200, 50, 100])); // Pixel 2

        let images = vec![image];
        let mean_color = calculate_mean_color(&images);
        // Mean should be: ((100+200)/2, (150+50)/2, (200+100)/2) = (150, 100, 150)
        assert_eq!(mean_color, Rgb([150, 100, 150]));
    }

    #[test]
    fn test_calculate_mean_color_empty() {
        let images: Vec<RgbImage> = vec![];
        let mean_color = calculate_mean_color(&images);
        assert_eq!(mean_color, Rgb([128, 128, 128])); // Default gray
    }

    #[test]
    fn test_calculate_text_color_black_background() {
        let black = Rgb([0, 0, 0]);
        let text_color = calculate_text_color(black);
        assert_eq!(text_color, Rgb([255, 255, 255])); // Should be white text on black
    }

    #[test]
    fn test_calculate_text_color_white_background() {
        let white = Rgb([255, 255, 255]);
        let text_color = calculate_text_color(white);
        assert_eq!(text_color, Rgb([0, 0, 0])); // Should be black text on white
    }

    #[test]
    fn test_calculate_text_color_dark_gray() {
        let dark_gray = Rgb([64, 64, 64]); // Dark gray, luminance = 64
        let text_color = calculate_text_color(dark_gray);
        assert_eq!(text_color, Rgb([255, 255, 255])); // Should be white text on dark
    }

    #[test]
    fn test_calculate_text_color_light_gray() {
        let light_gray = Rgb([192, 192, 192]); // Light gray, luminance = 192
        let text_color = calculate_text_color(light_gray);
        assert_eq!(text_color, Rgb([0, 0, 0])); // Should be black text on light
    }

    #[test]
    fn test_calculate_text_color_colored_backgrounds() {
        // Test with various colored backgrounds
        let dark_red = Rgb([100, 0, 0]); // Luminance ≈ 30, should be white text
        let text_color = calculate_text_color(dark_red);
        assert_eq!(text_color, Rgb([255, 255, 255]));

        let light_yellow = Rgb([200, 200, 100]); // Luminance ≈ 180, should be black text
        let text_color = calculate_text_color(light_yellow);
        assert_eq!(text_color, Rgb([0, 0, 0]));

        let dark_blue = Rgb([0, 0, 100]); // Luminance ≈ 11, should be white text
        let text_color = calculate_text_color(dark_blue);
        assert_eq!(text_color, Rgb([255, 255, 255]));
    }

    #[test]
    fn test_create_gallery_input_validation() {
        let empty_input = GalleryInput {
            title: Some("Test".to_string()),
            subtitle: Some("Test subtitle".to_string()),
            images: vec![],
            workflow: None,
            backend: None,
            generation_request: None,
        };

        let result = create_gallery(empty_input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty image list"));
    }

    #[test]
    fn test_create_gallery_single_image() {
        let image = RgbImage::from_pixel(100, 100, Rgb([255, 0, 0]));
        let input = GalleryInput {
            title: Some("Single Image".to_string()),
            subtitle: Some("Test".to_string()),
            images: vec![GalleryImageInput {
                image: Arc::new(image),
                title: None,
            }],
            workflow: None,
            backend: None,
            generation_request: None,
        };

        let result = create_gallery(input);
        assert!(result.is_ok());

        let gallery = result.unwrap();
        assert_eq!(gallery.layout.columns, 1);
        assert_eq!(gallery.layout.rows, 1);
        assert_eq!(gallery.layout.row_counts, vec![1]);

        let image = gallery.image;
        // Width should still be: 1*100 + 2*4 = 108
        assert_eq!(image.width(), 108);
        // Height is now dynamic based on text content, but should be at least the image height + borders
        let expected_min_height = 100 + 2 * 4 + 60; // image + borders + min text area
        assert!(image.height() >= expected_min_height);
        assert!(image.height() < 400); // Should be reasonable for short text
    }

    #[test]
    fn test_create_gallery_with_per_image_titles() {
        let base_image = RgbImage::from_pixel(64, 64, Rgb([0, 0, 255]));
        let inputs = vec![
            GalleryImageInput {
                image: Arc::new(base_image.clone()),
                title: Some("First".to_string()),
            },
            GalleryImageInput {
                image: Arc::new(base_image),
                title: Some("Second".to_string()),
            },
        ];

        let gallery = create_gallery(GalleryInput {
            title: None,
            subtitle: None,
            images: inputs,
            workflow: None,
            backend: None,
            generation_request: None,
        })
        .expect("expected gallery to render");

        assert_eq!(gallery.layout.columns, 2);
        assert_eq!(gallery.layout.rows, 1);
        assert_eq!(gallery.layout.row_counts, vec![2]);

        let image = gallery.image;
        // With per-image titles the gallery should be taller than just the images and borders.
        let base_height_with_borders = 64 + 2 * 4;
        assert!(image.height() > base_height_with_borders);
        // No gallery title/subtitle means width should still match the calculated layout.
        assert!(image.width() >= 64 + 2 * 4);
    }

    #[test]
    fn test_create_gallery_multiple_images() {
        let image1 = RgbImage::from_pixel(50, 50, Rgb([255, 0, 0]));
        let image2 = RgbImage::from_pixel(50, 50, Rgb([0, 255, 0]));
        let input = GalleryInput {
            title: Some("Two Images".to_string()),
            subtitle: Some("Test".to_string()),
            images: vec![
                GalleryImageInput {
                    image: Arc::new(image1),
                    title: None,
                },
                GalleryImageInput {
                    image: Arc::new(image2),
                    title: None,
                },
            ],
            workflow: None,
            backend: None,
            generation_request: None,
        };

        let result = create_gallery(input);
        assert!(result.is_ok());

        let gallery = result.unwrap();
        assert_eq!(gallery.layout.columns, 2);
        assert_eq!(gallery.layout.rows, 1);
        assert_eq!(gallery.layout.row_counts, vec![2]);

        let image = gallery.image;
        // Width should be 2x1 grid: 2*50 + 3*4 = 112
        assert_eq!(image.width(), 112);
        // Height is now dynamic based on text content, but should be at least the image height + borders
        let expected_min_height = 50 + 2 * 4 + 60; // image + borders + min text area
        assert!(image.height() >= expected_min_height);
        assert!(image.height() < 300); // Should be reasonable for short text
    }

    #[test]
    fn test_upload_gallery_empty_images() {
        use super::*;
        let config = ImageHostConfig {
            ssh_hostname: "test.example.com".to_string(),
            ssh_directory: "/test".to_string(),
            base_url: "https://test.example.com".to_string(),
        };
        let uploader = ImageUploader::new(config);

        let empty_gallery = UploadGallery {
            images: vec![],
            title: Some("Empty".to_string()),
            subtitle: Some("Should fail".to_string()),
            workflow: None,
            backend: None,
            generation_request: None,
        };

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(uploader.upload_gallery_impl(empty_gallery));

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Cannot upload empty gallery")
        );
    }

    #[test]
    fn test_measure_text_width() {
        let font = &*GALLERY_FONT;
        let scale = PxScale::from(20.0);

        let width = measure_text_width("Hello", font, scale);
        assert!(width > 0.0);

        // Longer text should be wider
        let longer_width = measure_text_width("Hello World", font, scale);
        assert!(longer_width > width);

        // Empty text should have zero width
        let empty_width = measure_text_width("", font, scale);
        assert_eq!(empty_width, 0.0);
    }

    #[test]
    fn test_wrap_text_empty() {
        let font = &*GALLERY_FONT;
        let scale = PxScale::from(20.0);
        let max_width = 100.0;

        let lines = wrap_text("", font, scale, max_width);
        assert!(lines.is_empty());

        let lines = wrap_text("   ", font, scale, max_width);
        assert!(lines.is_empty());
    }

    #[test]
    fn test_wrap_text_single_word() {
        let font = &*GALLERY_FONT;
        let scale = PxScale::from(20.0);
        let max_width = 1000.0; // Large width

        let lines = wrap_text("Hello", font, scale, max_width);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "Hello");
    }

    #[test]
    fn test_wrap_text_multiple_words_fit() {
        let font = &*GALLERY_FONT;
        let scale = PxScale::from(20.0);
        let max_width = 1000.0; // Large width

        let lines = wrap_text("Hello World Test", font, scale, max_width);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "Hello World Test");
    }

    #[test]
    fn test_wrap_text_wrapping_required() {
        let font = &*GALLERY_FONT;
        let scale = PxScale::from(20.0);
        let max_width = 50.0; // Very narrow width to force wrapping

        let lines = wrap_text(
            "This is a very long line that should wrap",
            font,
            scale,
            max_width,
        );
        assert!(
            lines.len() > 1,
            "Text should be wrapped into multiple lines"
        );

        // Verify that each line is not empty
        for line in &lines {
            assert!(!line.is_empty(), "No line should be empty");
        }

        // Verify that when joined, we get back the original words (may have different spacing)
        let joined = lines.join(" ");
        let original_words: Vec<&str> = "This is a very long line that should wrap"
            .split_whitespace()
            .collect();
        let joined_words: Vec<&str> = joined.split_whitespace().collect();
        assert_eq!(original_words, joined_words);
    }

    #[test]
    fn test_wrap_text_single_long_word() {
        let font = &*GALLERY_FONT;
        let scale = PxScale::from(20.0);
        let max_width = 10.0; // Very narrow, even single word won't fit

        let lines = wrap_text("supercalifragilisticexpialidocious", font, scale, max_width);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "supercalifragilisticexpialidocious");
    }

    #[test]
    fn test_calculate_text_area_height_short_text() {
        let canvas_width = 400;
        let text_padding = 20;

        let height = calculate_text_area_height(
            Some("Short title"),
            Some("Short subtitle"),
            canvas_width,
            text_padding,
        );

        // Should be at least the minimum height
        assert!(height >= 60);
        // Should be reasonable for short text
        assert!(height < 200);
    }

    #[test]
    fn test_calculate_text_area_height_long_text() {
        let canvas_width = 400;
        let text_padding = 20;

        let long_title = "This is a very long title that should definitely wrap to multiple lines when rendered in a gallery with limited width";
        let long_subtitle = "This is also a very long subtitle that contains lots of detailed information and should also wrap to multiple lines";

        let height = calculate_text_area_height(
            Some(long_title),
            Some(long_subtitle),
            canvas_width,
            text_padding,
        );

        // Long text should require more height
        assert!(height > 100);

        // Should accommodate multiple lines
        let short_height =
            calculate_text_area_height(Some("Short"), Some("Short"), canvas_width, text_padding);
        assert!(height > short_height);
    }

    #[test]
    fn test_calculate_text_area_height_empty_text() {
        let canvas_width = 400;
        let text_padding = 20;

        let height = calculate_text_area_height(None, None, canvas_width, text_padding);

        // Even empty text should have minimum height
        assert_eq!(height, 0);
    }

    #[test]
    fn test_truncate_exif_data_small_json() {
        let small_workflow = serde_json::json!({
            "backend": "test",
            "workflow": {
                "nodes": ["small", "data"]
            }
        });

        let result = truncate_exif_data(&small_workflow).unwrap();
        assert!(result.len() < 63 * 1024);
        assert!(result.contains("test"));
        assert!(result.contains("small"));
    }

    #[test]
    fn test_truncate_exif_data_large_strings() {
        // Create a string larger than 10 KiB
        let large_string = "x".repeat(15 * 1024); // 15 KiB
        let workflow = serde_json::json!({
            "backend": "test",
            "workflow": {
                "large_field": large_string,
                "small_field": "normal text"
            }
        });

        let result = truncate_exif_data(&workflow).unwrap();
        assert!(result.len() < 63 * 1024);
        assert!(result.contains("15 KiB of data elided"));
        assert!(result.contains("normal text"));
        assert!(!result.contains("xxxxxxx")); // Large string should be replaced
    }

    #[test]
    fn test_truncate_exif_data_oversized_json() {
        // Create a very large workflow that exceeds 63 KiB even after string truncation
        let mut large_workflow = serde_json::json!({
            "backend": "test",
            "workflow": {}
        });

        // Add many fields to make it large
        if let Some(workflow_obj) = large_workflow
            .get_mut("workflow")
            .and_then(|v| v.as_object_mut())
        {
            for i in 0..5000 {
                workflow_obj.insert(
                    format!("field_{}", i),
                    serde_json::Value::String(format!(
                        "some moderately long value for field {}",
                        i
                    )),
                );
            }
        }

        let result = truncate_exif_data(&large_workflow).unwrap();
        assert_eq!(result.len(), 63 * 1024);
        assert!(result.ends_with("[JSON truncated to 63 KiB limit]"));
    }

    #[test]
    fn test_create_gallery_with_long_text() {
        let image = RgbImage::from_pixel(100, 100, Rgb([255, 0, 0]));
        let long_title = "This is an extremely long gallery title that would definitely cause overflow issues with the old fixed-height implementation and should now be properly handled";
        let long_subtitle = "And this is also a very long subtitle with lots of descriptive text that provides detailed information about the gallery contents";

        let input = GalleryInput {
            title: Some(long_title.to_string()),
            subtitle: Some(long_subtitle.to_string()),
            images: vec![GalleryImageInput {
                image: Arc::new(image),
                title: None,
            }],
            workflow: None,
            backend: None,
            generation_request: None,
        };

        let result = create_gallery(input);
        assert!(result.is_ok());

        let gallery = result.unwrap();
        assert_eq!(gallery.layout.columns, 1);
        let image = gallery.image;
        // The gallery should be taller than the old fixed height would allow
        assert!(image.height() > 208); // old height was 1*100 + 2*4 + 100 = 208
    }
}
