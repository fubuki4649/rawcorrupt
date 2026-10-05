use anyhow::{Result, anyhow};
use mozjpeg_rs::{Encoder, Preset};
use rawler::imgop::develop::{Intermediate, RawDevelop};
use std::fs::{File, create_dir_all, remove_file};
use std::io::BufWriter;
use std::path::Path;

use crate::ingest::failure::{DecoderDiagnostics, FailureDetail};

pub const JPEG_QUALITY: u8 = 82;

/// Returns the rawler crate version string.
pub fn rawler_version_string() -> &'static str {
    "0.8.0"
}

/// Renders and creates a high-efficiency JPEG from a camera RAW image file, matching suisai.
pub fn extract_thumbnail<P: AsRef<Path>, Q: AsRef<Path>>(input: P, output: Q) -> Result<()> {
    test_extract_thumbnail(input, Some(output.as_ref()))
        .map(|_| ())
        .map_err(|e| anyhow!("{e}"))
}

/// Tests raw decoding and JPEG thumbnail encoding with suisai's exact rawler pipeline.
pub fn test_extract_thumbnail<P: AsRef<Path>>(
    input: P,
    output: Option<&Path>,
) -> Result<DecoderDiagnostics, FailureDetail> {
    let input_path = input.as_ref();

    if !input_path.exists() {
        return Err(FailureDetail::file_read(format!(
            "File does not exist: {}",
            input_path.display()
        )));
    }

    let raw_image = rawler::decode_file(input_path).map_err(|e| {
        FailureDetail::raw_sensor_data(format!(
            "Failed to decode RAW from {}: {e}",
            input_path.display()
        ))
    })?;

    let develop = RawDevelop::default();
    let intermediate = develop.develop_intermediate(&raw_image).map_err(|e| {
        FailureDetail::raw_sensor_data(format!(
            "Failed to develop RAW from {}: {e}",
            input_path.display()
        ))
    })?;

    let (rgb_data, width, height, color_mode) = match intermediate {
        Intermediate::ThreeColor(pixels) => {
            let mut buf = Vec::with_capacity(pixels.data.len() * 3);
            for [r, g, b] in pixels.data {
                buf.push((r.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                buf.push((g.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                buf.push((b.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
            (buf, pixels.width as u32, pixels.height as u32, "ThreeColor")
        }
        Intermediate::Monochrome(pixels) => {
            let mut buf = Vec::with_capacity(pixels.data.len() * 3);
            for v in pixels.data {
                let val = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                buf.extend_from_slice(&[val, val, val]);
            }
            (buf, pixels.width as u32, pixels.height as u32, "Monochrome")
        }
        Intermediate::FourColor(pixels) => {
            let mut buf = Vec::with_capacity(pixels.data.len() * 3);
            for [r, g, b, _] in pixels.data {
                buf.push((r.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                buf.push((g.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                buf.push((b.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
            (buf, pixels.width as u32, pixels.height as u32, "FourColor")
        }
    };

    let raw_dim = (raw_image.width as u32, raw_image.height as u32);
    let diag = DecoderDiagnostics::new(raw_dim, (width, height), color_mode);

    if width == 0 || height == 0 {
        return Err(FailureDetail::raw_sensor_data(format!(
            "Decoded RAW has zero dimensions: {width}x{height}"
        ))
        .with_diagnostics(diag));
    }

    let encoder = Encoder::new(Preset::ProgressiveSmallest).quality(JPEG_QUALITY);

    if let Some(out) = output {
        if let Some(parent) = out.parent() {
            create_dir_all(parent).map_err(|e| {
                FailureDetail::jpeg_encode(format!(
                    "Failed to create thumbnail directory {}: {e}",
                    parent.display()
                ))
                .with_diagnostics(diag.clone())
            })?;
        }
        let file = File::create(out).map_err(|e| {
            FailureDetail::jpeg_encode(format!(
                "Failed to create JPEG output file {}: {e}",
                out.display()
            ))
            .with_diagnostics(diag.clone())
        })?;
        if let Err(e) = encoder.encode_rgb_to_writer(&rgb_data, width, height, BufWriter::new(file))
        {
            let _ = remove_file(out);
            return Err(FailureDetail::jpeg_encode(format!(
                "Failed to encode JPEG thumbnail for {}: {e}",
                out.display()
            ))
            .with_diagnostics(diag));
        }
    } else {
        encoder
            .encode_rgb_to_writer(&rgb_data, width, height, &mut std::io::sink())
            .map_err(|e| {
                FailureDetail::jpeg_encode(format!(
                    "Failed to encode JPEG thumbnail in memory: {e}"
                ))
                .with_diagnostics(diag.clone())
            })?;
    }

    Ok(diag)
}
