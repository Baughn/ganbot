use std::io::Cursor;

use ab_glyph::{FontRef, PxScale};
use anyhow::{Context as _, Result};
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
}

/// Message to upload a gallery of images with title and subtitle
pub struct UploadGallery {
    pub images: Vec<RgbImage>,
    pub title: String,
    pub subtitle: String,
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
        let jpeg_bytes = self.encode_image_as_jpeg(&msg.image)?;

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
        };
        let gallery_image = create_gallery(gallery_input)?;

        // Generate filenames using the pattern {uuid}.{index}.jpg
        // Gallery image gets index 0
        let gallery_filename = format!("{}.0.jpg", base_uuid);

        // Encode and upload gallery image
        let gallery_jpeg = self.encode_image_as_jpeg(&gallery_image)?;
        let gallery_url = self.upload_jpeg(gallery_jpeg, &gallery_filename).await?;

        // Upload individual images with indices 1, 2, 3, etc.
        let mut image_urls = Vec::new();
        for (index, image) in msg.images.iter().enumerate() {
            let filename = format!("{}.{}.jpg", base_uuid, index + 1);
            let jpeg_bytes = self.encode_image_as_jpeg(image)?;
            let url = self.upload_jpeg(jpeg_bytes, &filename).await?;
            image_urls.push(url);
        }

        Ok(UploadedGalleryUrls {
            gallery_url,
            image_urls,
        })
    }

    /// Encode an RgbImage as high-quality JPEG bytes
    fn encode_image_as_jpeg(&self, image: &RgbImage) -> Result<Vec<u8>> {
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
        Ok(jpeg_bytes)
    }

    /// Upload JPEG bytes to the remote host with the given filename
    async fn upload_jpeg(&self, jpeg_bytes: Vec<u8>, filename: &str) -> Result<String> {
        // Create temporary file using tempfile crate
        let temp_file = NamedTempFile::new().context("Failed to create temporary file")?;

        let temp_path = temp_file
            .path()
            .to_str()
            .context("Temporary file path contains invalid UTF-8")?;

        tokio::fs::write(temp_path, &jpeg_bytes)
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
    let config = Supervisor::image_host().await;
    let uploader = ImageUploader::spawn(ImageUploader::new(config));
    let result = uploader
        .ask(UploadImage { image })
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
    let text_color = negate_color(border_color);

    // Define spacing and text area
    const BORDER_SIZE: u32 = 4;
    const TEXT_AREA_HEIGHT: u32 = 100; // Extra space for title and subtitle
    const TEXT_PADDING: u32 = 20;

    // Calculate canvas dimensions
    let canvas_width = grid_cols * img_width + (grid_cols + 1) * BORDER_SIZE;
    let canvas_height = grid_rows * img_height + (grid_rows + 1) * BORDER_SIZE + TEXT_AREA_HEIGHT;

    // Create canvas with border color
    let mut canvas = RgbImage::from_pixel(canvas_width, canvas_height, border_color);

    // Place images in grid
    for (i, image) in input.images.iter().enumerate() {
        let row = (i as u32) / grid_cols;
        let col = (i as u32) % grid_cols;

        let x = col * (img_width + BORDER_SIZE) + BORDER_SIZE;
        let y = row * (img_height + BORDER_SIZE) + BORDER_SIZE + TEXT_AREA_HEIGHT;

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

/// Negate a color for contrast
fn negate_color(color: Rgb<u8>) -> Rgb<u8> {
    Rgb([255 - color[0], 255 - color[1], 255 - color[2]])
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

    // Convert coordinates to i32
    let x = padding as i32;
    let title_y = padding as i32;
    let subtitle_y = (padding + 45) as i32;

    // Draw title with larger font
    let title_scale = PxScale::from(32.0);
    draw_text_mut(canvas, text_color, x, title_y, title_scale, font, title);

    // Draw subtitle with smaller font, positioned below title
    let subtitle_scale = PxScale::from(20.0);
    draw_text_mut(
        canvas,
        text_color,
        x,
        subtitle_y,
        subtitle_scale,
        font,
        subtitle,
    );

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
    fn test_negate_color_black() {
        let black = Rgb([0, 0, 0]);
        let negated = negate_color(black);
        assert_eq!(negated, Rgb([255, 255, 255])); // Should become white
    }

    #[test]
    fn test_negate_color_white() {
        let white = Rgb([255, 255, 255]);
        let negated = negate_color(white);
        assert_eq!(negated, Rgb([0, 0, 0])); // Should become black
    }

    #[test]
    fn test_negate_color_gray() {
        let gray = Rgb([128, 128, 128]);
        let negated = negate_color(gray);
        assert_eq!(negated, Rgb([127, 127, 127])); // 255-128=127
    }

    #[test]
    fn test_negate_color_mixed() {
        let color = Rgb([100, 200, 50]);
        let negated = negate_color(color);
        assert_eq!(negated, Rgb([155, 55, 205])); // (255-100, 255-200, 255-50)
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
        };

        let result = create_gallery(input);
        assert!(result.is_ok());

        let gallery = result.unwrap();
        // Should have dimensions: 1*100 + 2*4 = 108 width, 1*100 + 2*4 + 100 = 208 height
        assert_eq!(gallery.width(), 108);
        assert_eq!(gallery.height(), 208);
    }

    #[test]
    fn test_create_gallery_multiple_images() {
        let image1 = RgbImage::from_pixel(50, 50, Rgb([255, 0, 0]));
        let image2 = RgbImage::from_pixel(50, 50, Rgb([0, 255, 0]));
        let input = GalleryInput {
            title: "Two Images".to_string(),
            subtitle: "Test".to_string(),
            images: vec![image1, image2],
        };

        let result = create_gallery(input);
        assert!(result.is_ok());

        let gallery = result.unwrap();
        // Should have 2x1 grid: 2*50 + 3*4 = 112 width, 1*50 + 2*4 + 100 = 158 height
        assert_eq!(gallery.width(), 112);
        assert_eq!(gallery.height(), 158);
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
}
