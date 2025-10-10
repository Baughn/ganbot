//! Image compression and resizing utilities using WebP format

use anyhow::{Context, Result};
use image::imageops::FilterType;
use webp::Encoder;

/// Resize mode for image compression
#[derive(Debug, Clone, Copy)]
pub enum ResizeMode {
    /// Scale by a multiplier (e.g., 0.5 for 50% size, 1.0 for original)
    Scale(f32),
    /// Resize to target resolution in pixels (width for square images)
    TargetResolution(u32),
}

/// Compress and optionally resize an image to WebP format
///
/// # Arguments
/// * `image_bytes` - Original image bytes (JPEG or other supported format)
/// * `resize_mode` - How to resize the image (scale factor or target resolution)
/// * `quality` - WebP quality (0-100, where 100 is highest quality)
///
/// # Returns
/// Compressed WebP bytes
pub fn compress_image(image_bytes: &[u8], resize_mode: ResizeMode, quality: u8) -> Result<Vec<u8>> {
    // Decode the image (automatically detect format)
    let img = image::load_from_memory(image_bytes).context("Failed to decode image")?;

    // Convert to RGB8 for consistent encoding
    let rgb_img = img.to_rgb8();

    // Calculate scale factor based on resize mode
    let scale = match resize_mode {
        ResizeMode::Scale(s) => s,
        ResizeMode::TargetResolution(target_width) => {
            let original_width = rgb_img.width() as f32;
            target_width as f32 / original_width
        }
    };

    // Resize if scale is not 1.0
    let final_img = if (scale - 1.0).abs() > 0.001 {
        // Not equal to 1.0, need to resize
        let new_width = (rgb_img.width() as f32 * scale).round() as u32;
        let new_height = (rgb_img.height() as f32 * scale).round() as u32;

        if new_width == 0 || new_height == 0 {
            anyhow::bail!("Scale factor {} results in zero dimensions", scale);
        }

        image::imageops::resize(&rgb_img, new_width, new_height, FilterType::Lanczos3)
    } else {
        // Scale is 1.0, use original image
        rgb_img
    };

    // Encode as WebP with lossy compression
    let encoder = Encoder::from_rgb(final_img.as_raw(), final_img.width(), final_img.height());

    // Encode with specified quality (0-100)
    let webp_data = encoder.encode(quality as f32);

    Ok(webp_data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, Rgb, RgbImage, codecs::jpeg::JpegEncoder};
    use std::io::Cursor;

    fn create_test_jpeg(width: u32, height: u32, quality: u8) -> Vec<u8> {
        let img = RgbImage::from_pixel(width, height, Rgb([128, 128, 128]));
        let mut output = Vec::new();
        let mut cursor = Cursor::new(&mut output);
        let encoder = JpegEncoder::new_with_quality(&mut cursor, quality);
        encoder
            .write_image(img.as_raw(), width, height, image::ExtendedColorType::Rgb8)
            .unwrap();
        output
    }

    #[test]
    fn test_compress_image_no_scale() {
        // Use a real photo for realistic compression testing
        let original = std::fs::read("testdata/vacation.jpg")
            .expect("Failed to read test image (run from repository root)");

        // Get original dimensions
        let original_img = image::load_from_memory_with_format(&original, image::ImageFormat::Jpeg)
            .expect("Failed to decode original test image");
        let original_width = original_img.width();
        let original_height = original_img.height();

        let compressed = compress_image(&original, ResizeMode::Scale(1.0), 75).unwrap();

        // Real photos should compress smaller at lower quality
        assert!(
            compressed.len() < original.len(),
            "Compressed size ({} bytes) should be less than original ({} bytes)",
            compressed.len(),
            original.len()
        );

        // Verify it's a valid WebP with correct dimensions
        let decoded = image::load_from_memory_with_format(&compressed, image::ImageFormat::WebP);
        assert!(decoded.is_ok());
        let img = decoded.unwrap();
        assert_eq!(img.width(), original_width);
        assert_eq!(img.height(), original_height);
    }

    #[test]
    fn test_compress_image_with_scale() {
        // Use a real photo for realistic compression testing
        let original = std::fs::read("testdata/vacation.jpg")
            .expect("Failed to read test image (run from repository root)");

        // Get original dimensions
        let original_img = image::load_from_memory_with_format(&original, image::ImageFormat::Jpeg)
            .expect("Failed to decode original test image");
        let original_width = original_img.width();
        let original_height = original_img.height();

        let compressed = compress_image(&original, ResizeMode::Scale(0.5), 75).unwrap();

        // Verify it's much smaller (both scaled and lower quality)
        assert!(
            compressed.len() < original.len(),
            "Compressed size ({} bytes) should be less than original ({} bytes)",
            compressed.len(),
            original.len()
        );

        // Verify dimensions are halved
        let decoded = image::load_from_memory_with_format(&compressed, image::ImageFormat::WebP);
        assert!(decoded.is_ok());
        let img = decoded.unwrap();
        assert_eq!(img.width(), (original_width as f32 * 0.5).round() as u32);
        assert_eq!(img.height(), (original_height as f32 * 0.5).round() as u32);
    }

    #[test]
    fn test_compress_image_with_target_resolution() {
        let original = std::fs::read("testdata/vacation.jpg")
            .expect("Failed to read test image (run from repository root)");

        let compressed = compress_image(&original, ResizeMode::TargetResolution(512), 75).unwrap();

        // Verify dimensions match target
        let decoded = image::load_from_memory_with_format(&compressed, image::ImageFormat::WebP);
        assert!(decoded.is_ok());
        let img = decoded.unwrap();
        assert_eq!(img.width(), 512);
    }

    #[test]
    fn test_compress_image_zero_scale() {
        let original = create_test_jpeg(100, 100, 95);
        let result = compress_image(&original, ResizeMode::Scale(0.0), 75);
        assert!(result.is_err());
    }

    #[test]
    fn test_compress_image_invalid_input() {
        let invalid_data = vec![0u8; 100];
        let result = compress_image(&invalid_data, ResizeMode::Scale(1.0), 75);
        assert!(result.is_err());
    }
}
