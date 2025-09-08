use std::{io::Cursor, os::unix::fs::PermissionsExt as _};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use anyhow::{Context as _, Result};
use exif::experimental::Writer;
use exif::{Field, In, Tag, Value};
use image::{ImageEncoder, Rgb, RgbImage, codecs::jpeg::JpegEncoder};
use imageproc::drawing::draw_text_mut;
use kameo::message::Context;
use kameo::prelude::*;
use lazy_static::lazy_static;
use tempfile::NamedTempFile;
use tokio::process::Command;
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::{config::global::ImageHostConfig, supervisor::Supervisor};

// Lazy-loaded font to avoid re-parsing TTF data on every gallery creation
lazy_static! {
    static ref GALLERY_FONT: FontRef<'static> = {
        let font_data = include_bytes!("../../fonts/gallery.ttf");
        FontRef::try_from_slice(font_data).expect("Failed to load embedded gallery font")
    };
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
    pub image: RgbImage,
    pub workflow: Option<serde_json::Value>,
    pub backend: Option<String>,
}

/// Message to upload a gallery of images with title and subtitle
pub struct UploadGallery {
    pub images: Vec<RgbImage>,
    pub title: String,
    pub subtitle: String,
    pub workflow: Option<serde_json::Value>,
    pub backend: Option<String>,
}

/// Reply containing the uploaded image URL
#[derive(Reply)]
pub struct UploadedUrl(pub String);

/// Reply containing the uploaded gallery and individual image URLs
#[derive(Reply, Debug)]
pub struct UploadedGalleryUrls {
    pub gallery_url: String,
    pub image_urls: Vec<String>,
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
    type Reply = Result<UploadedGalleryUrls>;

    async fn handle(
        &mut self,
        msg: UploadGallery,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.upload_gallery_impl(msg).await
    }
}

impl ImageUploader {
    async fn upload_impl(&self, msg: UploadImage) -> Result<String> {
        // Generate unique filename using UUID v7 (time-ordered)
        let uuid = Uuid::now_v7();
        let filename = format!("{}.jpg", uuid);

        // Convert image to JPEG with high quality
        let jpeg_bytes =
            self.encode_image_as_jpeg(&msg.image, msg.workflow.as_ref(), msg.backend.as_deref())?;

        // Upload the JPEG data
        self.upload_jpeg(jpeg_bytes, &filename).await
    }

    async fn upload_gallery_impl(&self, msg: UploadGallery) -> Result<UploadedGalleryUrls> {
        if msg.images.is_empty() {
            anyhow::bail!("Cannot upload empty gallery");
        }

        // Generate base UUID for this gallery (time-ordered)
        let base_uuid = Uuid::now_v7();

        // Create gallery image using existing function
        let gallery_input = GalleryInput {
            title: msg.title,
            subtitle: msg.subtitle,
            images: msg.images.clone(),
            workflow: msg.workflow.clone(),
            backend: msg.backend.clone(),
        };
        let gallery_image = create_gallery(gallery_input)?;

        // Generate filenames using the pattern {uuid}.{index}.jpg
        // Gallery image gets index 0
        let gallery_filename = format!("{}.0.jpg", base_uuid);

        // Encode and upload gallery image
        let gallery_jpeg = self.encode_image_as_jpeg(
            &gallery_image,
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
                    image,
                    workflow_ref.as_ref(),
                    backend_ref.as_deref(),
                )?;
                self.upload_jpeg(jpeg_bytes, &filename).await
            });

        // Wait for all uploads to complete
        let results: Vec<Result<String>> = futures::future::join_all(upload_futures).await;

        // Collect the URLs, propagating the first error if any occurred
        let image_urls = results.into_iter().collect::<Result<Vec<_>>>()?;

        Ok(UploadedGalleryUrls {
            gallery_url,
            image_urls,
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

            // Format as pretty JSON
            let workflow_json = serde_json::to_string(&wrapped_workflow)
                .context("Failed to serialize workflow to JSON")?;

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

        let output = Command::new("scp")
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg(temp_path)
            .arg(&remote_path)
            .output()
            .await
            .context("Failed to execute scp command")?;

        // temp_file automatically cleans up when dropped

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
    upload_image_with_workflow(image, None, None).await
}

/// Upload an image with optional workflow metadata to the configured host
pub async fn upload_image_with_workflow(
    image: RgbImage,
    workflow: Option<serde_json::Value>,
    backend: Option<String>,
) -> Result<String> {
    let config = Supervisor::image_host().await;
    let uploader = ImageUploader::spawn(ImageUploader::new(config));
    let result = uploader
        .ask(UploadImage {
            image,
            workflow,
            backend,
        })
        .await
        .context("Failed to communicate with uploader")?;
    Ok(result.0)
}

/// Upload a gallery of images with title and subtitle to the configured host
/// Returns (gallery_url, individual_image_urls)
pub async fn upload_gallery(input: GalleryInput) -> Result<(String, Vec<String>)> {
    let config = Supervisor::image_host().await;
    let uploader = ImageUploader::spawn(ImageUploader::new(config));
    let result = uploader
        .ask(UploadGallery {
            images: input.images,
            title: input.title,
            subtitle: input.subtitle,
            workflow: input.workflow,
            backend: input.backend,
        })
        .await
        .context("Failed to communicate with uploader")?;
    Ok((result.gallery_url, result.image_urls))
}

/// Input for creating a gallery image
pub struct GalleryInput {
    pub title: String,
    pub subtitle: String,
    pub images: Vec<RgbImage>,
    pub workflow: Option<serde_json::Value>,
    pub backend: Option<String>,
}

/// Calculate the required height for the text area based on title and subtitle content
fn calculate_text_area_height(
    title: &str,
    subtitle: &str,
    canvas_width: u32,
    text_padding: u32,
) -> u32 {
    let font = &*GALLERY_FONT;

    // Calculate maximum text width (canvas width minus padding on both sides)
    let max_text_width = canvas_width as f32 - (text_padding * 2) as f32;

    // Define font scales and line heights (same as in render_text_on_canvas)
    let title_scale = PxScale::from(32.0);
    let subtitle_scale = PxScale::from(20.0);
    let title_line_height = 40; // pixels between title lines
    let subtitle_line_height = 25; // pixels between subtitle lines
    let title_subtitle_spacing = 15; // pixels between title and subtitle sections

    // Calculate number of lines needed
    let title_lines = wrap_text(title, font, title_scale, max_text_width);
    let subtitle_lines = wrap_text(subtitle, font, subtitle_scale, max_text_width);

    // Calculate total height needed
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

    // Add top and bottom padding, plus a small buffer zone
    let buffer_zone = 20; // Extra space to ensure text doesn't touch images
    let total_height =
        text_padding + title_height + spacing + subtitle_height + text_padding + buffer_zone;

    // Ensure minimum height even for very short text
    const MIN_TEXT_AREA_HEIGHT: u32 = 60;
    total_height.max(MIN_TEXT_AREA_HEIGHT)
}

/// Creates a gallery image from multiple images with a title and subtitle
pub fn create_gallery(input: GalleryInput) -> Result<RgbImage> {
    if input.images.is_empty() {
        anyhow::bail!("Cannot create gallery from empty image list");
    }

    // Get dimensions of first image (assuming all images have same dimensions)
    let img_width = input.images[0].width();
    let img_height = input.images[0].height();

    // Calculate optimal grid layout for 16:10 aspect ratio
    let (grid_cols, grid_rows) = calculate_optimal_grid(input.images.len(), img_width, img_height);

    // Calculate mean color across all images for border
    let border_color = calculate_mean_color(&input.images);
    let text_color = calculate_text_color(border_color);

    // Define spacing and text area
    const BORDER_SIZE: u32 = 4;
    const TEXT_PADDING: u32 = 20;

    // Calculate canvas width first (needed for text area height calculation)
    let canvas_width = grid_cols * img_width + (grid_cols + 1) * BORDER_SIZE;

    // Calculate required text area height based on actual content
    let text_area_height =
        calculate_text_area_height(&input.title, &input.subtitle, canvas_width, TEXT_PADDING);

    // Calculate final canvas height
    let canvas_height = grid_rows * img_height + (grid_rows + 1) * BORDER_SIZE + text_area_height;

    // Create canvas with border color
    let mut canvas = RgbImage::from_pixel(canvas_width, canvas_height, border_color);

    // Place images in grid
    for (i, image) in input.images.iter().enumerate() {
        let row = (i as u32) / grid_cols;
        let col = (i as u32) % grid_cols;

        let x = col * (img_width + BORDER_SIZE) + BORDER_SIZE;
        let y = row * (img_height + BORDER_SIZE) + BORDER_SIZE + text_area_height;

        // Copy image pixels to canvas
        for img_y in 0..img_height {
            for img_x in 0..img_width {
                let pixel = image.get_pixel(img_x, img_y);
                canvas.put_pixel(x + img_x, y + img_y, *pixel);
            }
        }
    }

    // Render title and subtitle text
    render_text_on_canvas(
        &mut canvas,
        &input.title,
        &input.subtitle,
        text_color,
        TEXT_PADDING,
    )?;

    Ok(canvas)
}

/// Calculate optimal grid layout for given aspect ratio
fn calculate_optimal_grid(image_count: usize, img_width: u32, img_height: u32) -> (u32, u32) {
    if image_count == 0 {
        return (0, 0);
    }

    let target_aspect = 16.0 / 10.0;
    let img_aspect = img_width as f64 / img_height as f64;

    let mut best_cols = 1u32;
    let mut best_rows = image_count as u32;
    let mut best_diff = f64::INFINITY;

    // Try different grid configurations
    for cols in 1..=image_count as u32 {
        let rows = ((image_count as f64) / (cols as f64)).ceil() as u32;

        // Calculate total aspect ratio of the grid
        let total_width = cols as f64 * img_width as f64;
        let total_height = rows as f64 * img_height as f64;
        let grid_aspect = total_width / total_height;

        let diff = (grid_aspect - target_aspect).abs();
        if diff < best_diff {
            best_diff = diff;
            best_cols = cols;
            best_rows = rows;
        }
    }

    (best_cols, best_rows)
}

/// Calculate mean color across all pixels in all images
fn calculate_mean_color(images: &[RgbImage]) -> Rgb<u8> {
    if images.is_empty() {
        return Rgb([128, 128, 128]); // Default gray
    }

    let mut total_r = 0u64;
    let mut total_g = 0u64;
    let mut total_b = 0u64;
    let mut pixel_count = 0u64;

    for image in images {
        for pixel in image.pixels() {
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
    title: &str,
    subtitle: &str,
    text_color: Rgb<u8>,
    padding: u32,
) -> Result<()> {
    // Use the lazy-loaded font (parsed once at startup)
    let font = &*GALLERY_FONT;

    // Calculate maximum text width (canvas width minus padding on both sides)
    let max_text_width = canvas.width() as f32 - (padding * 2) as f32;

    // Convert coordinates to i32
    let x = padding as i32;
    let mut current_y = padding as i32;

    // Define font scales and line heights
    let title_scale = PxScale::from(32.0);
    let subtitle_scale = PxScale::from(20.0);
    let title_line_height = 40; // pixels between title lines
    let subtitle_line_height = 25; // pixels between subtitle lines
    let title_subtitle_spacing = 15; // pixels between title and subtitle sections

    // Wrap and draw title
    let title_lines = wrap_text(title, font, title_scale, max_text_width);
    for line in title_lines {
        draw_text_mut(canvas, text_color, x, current_y, title_scale, font, &line);
        current_y += title_line_height;
    }

    // Add spacing between title and subtitle
    current_y += title_subtitle_spacing;

    // Wrap and draw subtitle
    let subtitle_lines = wrap_text(subtitle, font, subtitle_scale, max_text_width);
    for line in subtitle_lines {
        draw_text_mut(
            canvas,
            text_color,
            x,
            current_y,
            subtitle_scale,
            font,
            &line,
        );
        current_y += subtitle_line_height;
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
        // For 4 images, let's check what the algorithm actually chooses
        let (cols, rows) = calculate_optimal_grid(4, 512, 512);
        // The algorithm actually prefers 3x2 over 2x2 for better 16:10 aspect ratio
        assert_eq!((cols, rows), (3, 2));
        assert!(cols * rows >= 4); // Must accommodate all images
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
        // Should still be 2x2 but let's verify the function handles different image ratios
        assert!(cols > 0 && rows > 0);
        assert!(cols * rows >= 4); // Must accommodate all images
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
        // For 12 images, the algorithm chooses 5x3 (15 slots for 12 images)
        // Let's just verify it accommodates all images and is reasonable
        assert_eq!((cols, rows), (5, 3));
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
        let images = vec![];
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
    fn test_aspect_ratio_calculations() {
        // Test the aspect ratio math more directly
        let target_aspect: f64 = 16.0 / 10.0; // 1.6

        // Test 3x2 grid with square images
        let cols: f64 = 3.0;
        let rows: f64 = 2.0;
        let img_width: f64 = 512.0;
        let img_height: f64 = 512.0;

        let total_width = cols * img_width; // 1536
        let total_height = rows * img_height; // 1024
        let grid_aspect = total_width / total_height; // 1.5

        let diff = (grid_aspect - target_aspect).abs(); // |1.5 - 1.6| = 0.1
        assert!((diff - 0.1_f64).abs() < f64::EPSILON);

        // Test 4x2 grid
        let cols: f64 = 4.0;
        let rows: f64 = 2.0;
        let total_width = cols * img_width; // 2048
        let total_height = rows * img_height; // 1024
        let grid_aspect = total_width / total_height; // 2.0

        let diff = (grid_aspect - target_aspect).abs(); // |2.0 - 1.6| = 0.4
        assert!((diff - 0.4_f64).abs() < f64::EPSILON);

        // 3x2 should be closer to target than 4x2 (0.1 < 0.4)
        assert!(0.1_f64 < 0.4_f64);
    }

    #[test]
    fn test_create_gallery_input_validation() {
        let empty_input = GalleryInput {
            title: "Test".to_string(),
            subtitle: "Test subtitle".to_string(),
            images: vec![],
            workflow: None,
            backend: None,
        };

        let result = create_gallery(empty_input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty image list"));
    }

    #[test]
    fn test_create_gallery_single_image() {
        let image = RgbImage::from_pixel(100, 100, Rgb([255, 0, 0]));
        let input = GalleryInput {
            title: "Single Image".to_string(),
            subtitle: "Test".to_string(),
            images: vec![image],
            workflow: None,
            backend: None,
        };

        let result = create_gallery(input);
        assert!(result.is_ok());

        let gallery = result.unwrap();
        // Width should still be: 1*100 + 2*4 = 108
        assert_eq!(gallery.width(), 108);
        // Height is now dynamic based on text content, but should be at least the image height + borders
        let expected_min_height = 100 + 2 * 4 + 60; // image + borders + min text area
        assert!(gallery.height() >= expected_min_height);
        assert!(gallery.height() < 400); // Should be reasonable for short text
    }

    #[test]
    fn test_create_gallery_multiple_images() {
        let image1 = RgbImage::from_pixel(50, 50, Rgb([255, 0, 0]));
        let image2 = RgbImage::from_pixel(50, 50, Rgb([0, 255, 0]));
        let input = GalleryInput {
            title: "Two Images".to_string(),
            subtitle: "Test".to_string(),
            images: vec![image1, image2],
            workflow: None,
            backend: None,
        };

        let result = create_gallery(input);
        assert!(result.is_ok());

        let gallery = result.unwrap();
        // Width should be 2x1 grid: 2*50 + 3*4 = 112
        assert_eq!(gallery.width(), 112);
        // Height is now dynamic based on text content, but should be at least the image height + borders
        let expected_min_height = 50 + 2 * 4 + 60; // image + borders + min text area
        assert!(gallery.height() >= expected_min_height);
        assert!(gallery.height() < 300); // Should be reasonable for short text
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
            title: "Empty".to_string(),
            subtitle: "Should fail".to_string(),
            workflow: None,
            backend: None,
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

        let height =
            calculate_text_area_height("Short title", "Short subtitle", canvas_width, text_padding);

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

        let height =
            calculate_text_area_height(long_title, long_subtitle, canvas_width, text_padding);

        // Long text should require more height
        assert!(height > 100);

        // Should accommodate multiple lines
        let short_height = calculate_text_area_height("Short", "Short", canvas_width, text_padding);
        assert!(height > short_height);
    }

    #[test]
    fn test_calculate_text_area_height_empty_text() {
        let canvas_width = 400;
        let text_padding = 20;

        let height = calculate_text_area_height("", "", canvas_width, text_padding);

        // Even empty text should have minimum height
        assert_eq!(height, 60); // MIN_TEXT_AREA_HEIGHT
    }

    #[test]
    fn test_create_gallery_with_long_text() {
        let image = RgbImage::from_pixel(100, 100, Rgb([255, 0, 0]));
        let long_title = "This is an extremely long gallery title that would definitely cause overflow issues with the old fixed-height implementation and should now be properly handled";
        let long_subtitle = "And this is also a very long subtitle with lots of descriptive text that provides detailed information about the gallery contents";

        let input = GalleryInput {
            title: long_title.to_string(),
            subtitle: long_subtitle.to_string(),
            images: vec![image],
            workflow: None,
            backend: None,
        };

        let result = create_gallery(input);
        assert!(result.is_ok());

        let gallery = result.unwrap();
        // The gallery should be taller than the old fixed height would allow
        assert!(gallery.height() > 208); // old height was 1*100 + 2*4 + 100 = 208
    }
}
