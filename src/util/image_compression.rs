//! JPEG compression and resizing utilities

use anyhow::{Context, Result};
use image::{ImageEncoder, codecs::jpeg::JpegEncoder, imageops::FilterType};
use std::io::Cursor;

/// Compress and optionally resize a JPEG image
///
/// # Arguments
/// * `jpeg_bytes` - Original JPEG bytes
/// * `scale` - Scale factor (1.0 = original size, 0.5 = 50% reduction, etc.)
/// * `quality` - JPEG quality (1-100, where 100 is highest quality)
///
/// # Returns
/// Compressed JPEG bytes
pub fn compress_jpeg(jpeg_bytes: &[u8], scale: f32, quality: u8) -> Result<Vec<u8>> {
    // Decode the JPEG
    let img = image::load_from_memory_with_format(jpeg_bytes, image::ImageFormat::Jpeg)
        .context("Failed to decode JPEG image")?;

    // Convert to RGB8 for consistent encoding
    let rgb_img = img.to_rgb8();

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

    // Encode as JPEG with specified quality
    let mut output = Vec::new();
    {
        let mut cursor = Cursor::new(&mut output);
        let encoder = JpegEncoder::new_with_quality(&mut cursor, quality);
        encoder
            .write_image(
                final_img.as_raw(),
                final_img.width(),
                final_img.height(),
                image::ExtendedColorType::Rgb8,
            )
            .context("Failed to encode compressed JPEG")?;
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

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
    fn test_compress_jpeg_no_scale() {
        // Use a real photo for realistic compression testing
        let original = std::fs::read("testdata/vacation.jpg")
            .expect("Failed to read test image (run from repository root)");

        // Get original dimensions
        let original_img = image::load_from_memory_with_format(&original, image::ImageFormat::Jpeg)
            .expect("Failed to decode original test image");
        let original_width = original_img.width();
        let original_height = original_img.height();

        let compressed = compress_jpeg(&original, 1.0, 75).unwrap();

        // Real photos should compress smaller at lower quality
        assert!(
            compressed.len() < original.len(),
            "Compressed size ({} bytes) should be less than original ({} bytes)",
            compressed.len(),
            original.len()
        );

        // Verify it's a valid JPEG with correct dimensions
        let decoded = image::load_from_memory_with_format(&compressed, image::ImageFormat::Jpeg);
        assert!(decoded.is_ok());
        let img = decoded.unwrap();
        assert_eq!(img.width(), original_width);
        assert_eq!(img.height(), original_height);
    }

    #[test]
    fn test_compress_jpeg_with_scale() {
        // Use a real photo for realistic compression testing
        let original = std::fs::read("testdata/vacation.jpg")
            .expect("Failed to read test image (run from repository root)");

        // Get original dimensions
        let original_img = image::load_from_memory_with_format(&original, image::ImageFormat::Jpeg)
            .expect("Failed to decode original test image");
        let original_width = original_img.width();
        let original_height = original_img.height();

        let compressed = compress_jpeg(&original, 0.5, 75).unwrap();

        // Verify it's much smaller (both scaled and lower quality)
        assert!(
            compressed.len() < original.len(),
            "Compressed size ({} bytes) should be less than original ({} bytes)",
            compressed.len(),
            original.len()
        );

        // Verify dimensions are halved
        let decoded = image::load_from_memory_with_format(&compressed, image::ImageFormat::Jpeg);
        assert!(decoded.is_ok());
        let img = decoded.unwrap();
        assert_eq!(img.width(), (original_width as f32 * 0.5).round() as u32);
        assert_eq!(img.height(), (original_height as f32 * 0.5).round() as u32);
    }

    #[test]
    fn test_compress_jpeg_zero_scale() {
        let original = create_test_jpeg(100, 100, 95);
        let result = compress_jpeg(&original, 0.0, 75);
        assert!(result.is_err());
    }

    #[test]
    fn test_compress_jpeg_invalid_input() {
        let invalid_data = vec![0u8; 100];
        let result = compress_jpeg(&invalid_data, 1.0, 75);
        assert!(result.is_err());
    }
}
