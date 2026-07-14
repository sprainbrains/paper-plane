use gtk::gdk;
use gtk::glib;
use image::DynamicImage;
use image::ImageDecoder;
use image::ImageError;
use image::ImageReader;
use image::ImageResult;
use image::Limits;
use image::error::LimitError;
use image::error::LimitErrorKind;
use thiserror::Error;

const MAX_IMAGE_DIMENSION: u32 = 32 * 1024;
const MAX_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_DECODE_ALLOC: u64 = MAX_IMAGE_PIXELS * 4;

#[derive(Error, Debug)]
pub(crate) enum DecodeError {
    #[error("I/O error: {0:?}")]
    IoError(std::io::Error),
    #[error("Image decoding error: {0:?}")]
    ImageError(image::error::ImageError),
    #[error("Decoding for this image format is not currently implemented")]
    Unimplemented,
}

pub(crate) fn decode_image_from_path(path: &str) -> Result<gdk::MemoryTexture, DecodeError> {
    decode_image_from_path_at_size(path, None)
}

pub(crate) fn decode_image_from_path_at_size(
    path: &str,
    maximum_size: Option<u32>,
) -> Result<gdk::MemoryTexture, DecodeError> {
    use image::DynamicImage::*;

    let mut reader = ImageReader::open(path).map_err(DecodeError::IoError)?;
    reader.limits(decode_limits());

    let mut decoder = reader.into_decoder().map_err(DecodeError::ImageError)?;
    let (width, height) = decoder.dimensions();
    let remaining_limits =
        validate_image_resources(width, height, decoder.total_bytes(), maximum_size)
            .map_err(DecodeError::ImageError)?;
    decoder
        .set_limits(remaining_limits)
        .map_err(DecodeError::ImageError)?;

    let mut dynamic_image = DynamicImage::from_decoder(decoder).map_err(DecodeError::ImageError)?;
    if let Some(maximum_size) = maximum_size
        && (dynamic_image.width() > maximum_size || dynamic_image.height() > maximum_size)
    {
        dynamic_image = dynamic_image.thumbnail(maximum_size, maximum_size);
    }
    let (memory_format, layout, data) = match dynamic_image {
        ImageLuma8(_) => {
            let buffer = dynamic_image.to_rgb8();
            (
                gdk::MemoryFormat::R8g8b8,
                buffer.sample_layout(),
                buffer.into_raw(),
            )
        }
        ImageLumaA8(_) => {
            let buffer = dynamic_image.to_rgba8();
            (
                gdk::MemoryFormat::R8g8b8a8,
                buffer.sample_layout(),
                buffer.into_raw(),
            )
        }
        ImageRgb8(buffer) => (
            gdk::MemoryFormat::R8g8b8,
            buffer.sample_layout(),
            buffer.into_raw(),
        ),
        ImageRgba8(buffer) => (
            gdk::MemoryFormat::R8g8b8a8,
            buffer.sample_layout(),
            buffer.into_raw(),
        ),
        _ => return Err(DecodeError::Unimplemented),
    };

    let bytes = glib::Bytes::from_owned(data);
    let texture = gdk::MemoryTexture::new(
        layout.width as i32,
        layout.height as i32,
        memory_format,
        &bytes,
        layout.height_stride,
    );

    Ok(texture)
}

fn decode_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    limits
}

fn validate_image_resources(
    width: u32,
    height: u32,
    decoded_bytes: u64,
    maximum_size: Option<u32>,
) -> ImageResult<Limits> {
    let mut limits = decode_limits();
    limits.check_dimensions(width, height)?;

    if u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
        return Err(ImageError::Limits(LimitError::from_kind(
            LimitErrorKind::InsufficientMemory,
        )));
    }

    // Resizing and color conversion can temporarily coexist with the decoded source.
    // Reserve four bytes per output pixel so peak memory remains within the budget.
    let output_width = maximum_size.map_or(width, |size| width.min(size));
    let output_height = maximum_size.map_or(height, |size| height.min(size));
    let output_bytes = u64::from(output_width) * u64::from(output_height) * 4;
    if decoded_bytes.saturating_add(output_bytes) > MAX_DECODE_ALLOC {
        return Err(ImageError::Limits(LimitError::from_kind(
            LimitErrorKind::InsufficientMemory,
        )));
    }

    limits.reserve(decoded_bytes)?;
    Ok(limits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit_error_kind(result: ImageResult<Limits>) -> LimitErrorKind {
        match result.unwrap_err() {
            ImageError::Limits(error) => error.kind(),
            error => panic!("expected a resource limit error, got {error:?}"),
        }
    }

    #[test]
    fn accepts_images_at_resource_limits() {
        assert!(
            validate_image_resources(
                MAX_IMAGE_DIMENSION,
                MAX_IMAGE_PIXELS as u32 / MAX_IMAGE_DIMENSION,
                MAX_DECODE_ALLOC / 2,
                Some(1),
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_oversized_dimensions_before_decode() {
        assert_eq!(
            limit_error_kind(validate_image_resources(
                MAX_IMAGE_DIMENSION + 1,
                1,
                4,
                None,
            )),
            LimitErrorKind::DimensionError
        );
    }

    #[test]
    fn rejects_excessive_pixel_count_before_decode() {
        assert_eq!(
            limit_error_kind(validate_image_resources(
                MAX_IMAGE_DIMENSION,
                MAX_IMAGE_PIXELS as u32 / MAX_IMAGE_DIMENSION + 1,
                4,
                None,
            )),
            LimitErrorKind::InsufficientMemory
        );
    }

    #[test]
    fn rejects_excessive_decoded_allocation_before_decode() {
        assert_eq!(
            limit_error_kind(validate_image_resources(1, 1, MAX_DECODE_ALLOC + 1, None,)),
            LimitErrorKind::InsufficientMemory
        );
    }

    #[test]
    fn accounts_for_conversion_peak_memory() {
        assert_eq!(
            limit_error_kind(validate_image_resources(
                MAX_IMAGE_DIMENSION,
                MAX_IMAGE_PIXELS as u32 / MAX_IMAGE_DIMENSION,
                MAX_DECODE_ALLOC / 2,
                None,
            )),
            LimitErrorKind::InsufficientMemory
        );
    }
}
