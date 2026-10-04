use anyhow::{Context, Result, anyhow};
use mozjpeg_rs::Encoder;
use rawlib::{DecodeOptions, extract_image_with_options};
use std::fs::{File, create_dir_all, remove_file};
use std::io::{BufWriter, Cursor};
use std::path::Path;

pub const JPEG_QUALITY: u8 = 82;

/// Renders and creates a high-efficiency JPEG from a camera RAW image file, matching suisai.
pub fn extract_thumbnail<P: AsRef<Path>, Q: AsRef<Path>>(input: P, output: Q) -> Result<()> {
    test_extract_thumbnail(input, Some(output.as_ref()), false)
}

/// Tests raw decoding and JPEG thumbnail encoding with suisai's exact pipeline parameters.
pub fn test_extract_thumbnail<P: AsRef<Path>>(
    input: P,
    output: Option<&Path>,
    half_size: bool,
) -> Result<()> {
    let input_path = input.as_ref();

    let decode_options = DecodeOptions {
        half_size,
        demosaic_quality: 2,
        output_bps: 8,
        no_auto_bright: true,
        output_color: 1,
        linear_gamma: false,
        use_camera_wb: true,
    };

    let image = extract_image_with_options(input_path, &decode_options)
        .map_err(|e| anyhow!("Failed to decode RAW from {}: {e}", input_path.display()))?;

    if image.width == 0 || image.height == 0 {
        return Err(anyhow!(
            "Decoded RAW has zero dimensions: {}x{}",
            image.width,
            image.height
        ));
    }

    let encoder = Encoder::fastest().quality(JPEG_QUALITY);

    if let Some(output_path) = output {
        if let Some(parent) = output_path.parent() {
            create_dir_all(parent).with_context(|| {
                format!("Failed to create thumbnail directory {}", parent.display())
            })?;
        }

        let file = File::create(output_path).with_context(|| {
            format!(
                "Failed to create JPEG output file {}",
                output_path.display()
            )
        })?;
        let writer = BufWriter::new(file);

        if let Err(e) = encoder.encode_rgb_to_writer(
            &image.data,
            image.width as u32,
            image.height as u32,
            writer,
        ) {
            let _ = remove_file(output_path);
            return Err(anyhow!(
                "Failed to encode JPEG thumbnail for {}: {e}",
                output_path.display()
            ));
        }
    } else {
        let mut sink = Cursor::new(Vec::new());
        if let Err(e) = encoder.encode_rgb_to_writer(
            &image.data,
            image.width as u32,
            image.height as u32,
            &mut sink,
        ) {
            return Err(anyhow!("Failed to encode JPEG thumbnail in memory: {e}"));
        }
    }

    Ok(())
}
