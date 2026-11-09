//! Input normalization at the xberg extraction boundary.

use std::path::Path;

use super::ExtractError;
use image::ImageDecoder;
use xberg::ExtractInput;

/// Compose supported raster images over white before OCR so transparent pixels stay visible.
pub(super) fn prepare_extraction_input(path: &Path, mime_type: Option<&str>) -> Result<ExtractInput, ExtractError> {
    if !matches!(
        mime_type,
        Some("image/png" | "image/webp" | "image/gif" | "image/tiff" | "image/bmp" | "image/x-portable-pixmap")
    ) {
        return Ok(uri_input(path, mime_type));
    }

    let bytes =
        std::fs::read(path).map_err(|error| ExtractError::Document(format!("read PNG {}: {error}", path.display())))?;
    if mime_type == Some("image/png") && !png_may_have_alpha(&bytes) {
        return Ok(uri_input(path, mime_type));
    }
    let decoder = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|error| ExtractError::Document(format!("identify image {}: {error}", path.display())))?
        .into_decoder()
        .map_err(|error| ExtractError::Document(format!("read image metadata {}: {error}", path.display())))?;
    if !decoder.color_type().has_alpha() {
        return Ok(uri_input(path, mime_type));
    }
    let (width, height) = decoder.dimensions();
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| ExtractError::Document(format!("image {} dimensions overflow", path.display())))?;
    let rgba_bytes = pixels
        .checked_mul(4)
        .ok_or_else(|| ExtractError::Document(format!("image {} allocation size overflow", path.display())))?;
    let max_content_size =
        u64::try_from(xberg::extractors::security::SecurityLimits::default().max_content_size).unwrap_or(u64::MAX);
    if rgba_bytes > max_content_size {
        return Err(ExtractError::Document(format!(
            "image {} is too large to normalize safely ({rgba_bytes} decoded bytes; max {max_content_size})",
            path.display()
        )));
    }
    let rgba = image::load_from_memory(&bytes)
        .map_err(|error| ExtractError::Document(format!("decode image {}: {error}", path.display())))?
        .to_rgba8();
    if rgba.pixels().all(|pixel| pixel[3] == u8::MAX) {
        return Ok(uri_input(path, mime_type));
    }

    let mut rgb = image::RgbImage::new(rgba.width(), rgba.height());
    for (x, y, pixel) in rgba.enumerate_pixels() {
        let alpha = u32::from(pixel[3]);
        let dst = rgb.get_pixel_mut(x, y);
        for channel in 0..3 {
            dst[channel] = ((u32::from(pixel[channel]) * alpha + 255 * (255 - alpha)) / 255) as u8;
        }
    }
    let mut normalized = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(rgb)
        .write_to(&mut normalized, image::ImageFormat::Png)
        .map_err(|error| ExtractError::Document(format!("encode normalized image {}: {error}", path.display())))?;
    Ok(ExtractInput::from_bytes(
        normalized.into_inner(),
        "image/png",
        path.file_name().map(|name| name.to_string_lossy().into_owned()),
    ))
}

fn uri_input(path: &Path, mime_type: Option<&str>) -> ExtractInput {
    let mut input = ExtractInput::from_uri(path.to_string_lossy().into_owned());
    input.mime_type = mime_type.map(str::to_string);
    input
}

fn png_may_have_alpha(bytes: &[u8]) -> bool {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 29 || &bytes[..8] != SIGNATURE {
        return true;
    }
    if matches!(bytes[25], 4 | 6) {
        return true;
    }
    let mut offset = 8usize;
    while offset.saturating_add(12) <= bytes.len() {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let chunk_end = match offset.checked_add(12).and_then(|value| value.checked_add(length)) {
            Some(value) if value <= bytes.len() => value,
            _ => return true,
        };
        if &bytes[offset + 4..offset + 8] == b"tRNS" {
            return true;
        }
        if &bytes[offset + 4..offset + 8] == b"IEND" {
            break;
        }
        offset = chunk_end;
    }
    false
}
