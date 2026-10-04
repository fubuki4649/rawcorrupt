use anyhow::{Result, anyhow};
use mozjpeg_rs::Encoder;
use rawlib::DecodeOptions;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fs::{File, create_dir_all, remove_file};
use std::io::BufWriter;
use std::path::Path;

pub const JPEG_QUALITY: u8 = 82;

const LIBRAW_FILE_UNSUPPORTED: c_int = -2;
const LIBRAW_NO_THUMBNAIL: c_int = -5;
const LIBRAW_UNSUPPORTED_THUMBNAIL: c_int = -6;
const LIBRAW_DATA_ERROR: c_int = -100008;
const LIBRAW_IO_ERROR: c_int = -100009;

unsafe extern "C" {
    fn libraw_set_dataerror_handler(
        data: *mut rawlib::ffi::libraw_data_t,
        func: Option<unsafe extern "C" fn(data: *mut c_void, file: *const c_char, offset: i64)>,
        datap: *mut c_void,
    );
}

/// RAII guard for `*mut rawlib::ffi::libraw_data_t` ensuring cleanup on early return or error.
struct RawHandle(*mut rawlib::ffi::libraw_data_t);

impl Drop for RawHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { rawlib::ffi::libraw_close(self.0) };
        }
    }
}

/// LibRaw data error callback that intercepts sensor data corruption reports (offset >= 0)
/// and unexpected end of file (offset < 0). This prevents LibRaw from silently logging
/// "data corrupted at <offset>" to stderr while returning LIBRAW_SUCCESS.
unsafe extern "C" fn raw_data_error_callback(data: *mut c_void, file: *const c_char, offset: i64) {
    if !data.is_null() {
        let err_slot = unsafe { &mut *(data as *mut Option<String>) };
        let filename = if file.is_null() {
            "unknown".to_string()
        } else {
            unsafe { CStr::from_ptr(file).to_string_lossy().into_owned() }
        };
        let msg = if offset < 0 {
            format!("{filename}: unexpected end of file")
        } else {
            format!("{filename}: sensor data corrupted at offset {offset}")
        };
        if err_slot.is_none() {
            *err_slot = Some(msg);
        }
    }
}

use crate::ingest::failure::FailureDetail;

fn libraw_err_str(code: c_int) -> std::borrow::Cow<'static, str> {
    unsafe {
        let err_c = rawlib::ffi::libraw_strerror(code);
        if err_c.is_null() {
            "unknown".into()
        } else {
            CStr::from_ptr(err_c).to_string_lossy()
        }
    }
}

fn map_libraw_open_error(ret: c_int) -> FailureDetail {
    let err_str = libraw_err_str(ret);
    match ret {
        LIBRAW_IO_ERROR => {
            FailureDetail::file_read(format!("LibRaw read I/O error {ret}: {err_str}"))
        }
        LIBRAW_FILE_UNSUPPORTED => {
            FailureDetail::metadata(format!("Unsupported camera RAW format: {err_str}"))
        }
        _ => FailureDetail::raw_sensor_data(format!("LibRaw open_file error {ret}: {err_str}")),
    }
}

/// Decodes camera RAW data using LibRaw while catching internal sensor corruption callbacks.
fn decode_raw_with_error_handler(
    input_path: &Path,
    opts: &DecodeOptions,
) -> Result<ProcessedImage, FailureDetail> {
    if !input_path.exists() {
        return Err(FailureDetail::file_read(format!(
            "File does not exist: {}",
            input_path.display()
        )));
    }

    let handle = RawHandle(unsafe { rawlib::ffi::libraw_init(rawlib::ffi::LIBRAW_OPTIONS_NONE) });
    if handle.0.is_null() {
        return Err(FailureDetail::raw_sensor_data(
            "Failed to initialize LibRaw processor",
        ));
    }

    let mut data_error: Option<String> = None;
    unsafe {
        libraw_set_dataerror_handler(
            handle.0,
            Some(raw_data_error_callback),
            &mut data_error as *mut _ as *mut c_void,
        );
    }

    use std::os::unix::ffi::OsStrExt;
    let c_path = CString::new(input_path.as_os_str().as_bytes())
        .map_err(|e| FailureDetail::file_read(format!("Path contains null byte: {e}")))?;
    let ret = unsafe { rawlib::ffi::libraw_open_file(handle.0, c_path.as_ptr()) };

    if ret != rawlib::ffi::LIBRAW_SUCCESS {
        return Err(map_libraw_open_error(ret));
    }

    if let Some(err) = data_error.take() {
        return Err(FailureDetail::raw_sensor_data(format!(
            "LibRaw data corruption during open: {err}"
        )));
    }

    // Verify embedded thumbnail if present in camera RAW file
    let thumb_ret = unsafe { rawlib::ffi::libraw_unpack_thumb(handle.0) };
    if let Some(err) = data_error.take() {
        return Err(FailureDetail::embedded_thumbnail(format!(
            "Corrupted embedded thumbnail in RAW file: {err}"
        )));
    }
    if thumb_ret != rawlib::ffi::LIBRAW_SUCCESS
        && thumb_ret != LIBRAW_NO_THUMBNAIL
        && thumb_ret != LIBRAW_UNSUPPORTED_THUMBNAIL
        && thumb_ret == LIBRAW_DATA_ERROR
    {
        return Err(FailureDetail::embedded_thumbnail(format!(
            "Corrupted embedded thumbnail stream: {}",
            libraw_err_str(thumb_ret)
        )));
    }

    unsafe {
        rawlib::ffi::libraw_set_half_size(handle.0, opts.half_size as c_int);
        rawlib::ffi::libraw_set_demosaic(handle.0, opts.demosaic_quality);
        rawlib::ffi::libraw_set_output_bps(handle.0, opts.output_bps);
        rawlib::ffi::libraw_set_no_auto_bright(handle.0, opts.no_auto_bright as c_int);
        rawlib::ffi::libraw_set_output_color(handle.0, opts.output_color);
        rawlib::ffi::libraw_set_use_camera_wb(handle.0, opts.use_camera_wb as c_int);
        if opts.linear_gamma {
            rawlib::ffi::libraw_set_gamma(handle.0, 0, 1.0);
            rawlib::ffi::libraw_set_gamma(handle.0, 1, 1.0);
        }
    }

    let check_step =
        |ret: c_int, err: &mut Option<String>, step: &str| -> Result<(), FailureDetail> {
            if let Some(e) = err.take() {
                Err(FailureDetail::raw_sensor_data(format!(
                    "LibRaw sensor data corruption during {step}: {e}"
                )))
            } else if ret != rawlib::ffi::LIBRAW_SUCCESS {
                Err(FailureDetail::raw_sensor_data(format!(
                    "LibRaw {step} error {ret}: {}",
                    libraw_err_str(ret)
                )))
            } else {
                Ok(())
            }
        };

    check_step(
        unsafe { rawlib::ffi::libraw_unpack(handle.0) },
        &mut data_error,
        "unpack",
    )?;
    check_step(
        unsafe { rawlib::ffi::libraw_dcraw_process(handle.0) },
        &mut data_error,
        "decode",
    )?;

    let mut errc: c_int = 0;
    let img_ptr = unsafe { rawlib::ffi::libraw_dcraw_make_mem_image(handle.0, &mut errc) };
    if let Some(err) = data_error.take() {
        if !img_ptr.is_null() {
            unsafe { rawlib::ffi::libraw_dcraw_clear_mem(img_ptr) };
        }
        return Err(FailureDetail::raw_sensor_data(format!(
            "LibRaw sensor data corruption during image generation: {err}"
        )));
    }
    if img_ptr.is_null() {
        return Err(FailureDetail::raw_sensor_data(format!(
            "LibRaw make_mem_image error {errc}: {}",
            libraw_err_str(errc)
        )));
    }

    Ok(ProcessedImage(img_ptr))
}

/// Zero-copy RAII wrapper around LibRaw's decoded bitmap buffer.
pub struct ProcessedImage(*mut rawlib::ffi::libraw_processed_image_t);

impl Drop for ProcessedImage {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { rawlib::ffi::libraw_dcraw_clear_mem(self.0) };
        }
    }
}

impl ProcessedImage {
    pub fn width(&self) -> u32 {
        unsafe { (*self.0).width as u32 }
    }

    pub fn height(&self) -> u32 {
        unsafe { (*self.0).height as u32 }
    }

    pub fn data(&self) -> &[u8] {
        unsafe {
            let img = &*self.0;
            std::slice::from_raw_parts(img.data.as_ptr(), img.data_size as usize)
        }
    }
}

/// Renders and creates a high-efficiency JPEG from a camera RAW image file, matching suisai.
pub fn extract_thumbnail<P: AsRef<Path>, Q: AsRef<Path>>(input: P, output: Q) -> Result<()> {
    test_extract_thumbnail(input, Some(output.as_ref()), false).map_err(|e| anyhow!("{e}"))
}

/// Tests raw decoding and JPEG thumbnail encoding with suisai's exact pipeline parameters.
pub fn test_extract_thumbnail<P: AsRef<Path>>(
    input: P,
    output: Option<&Path>,
    half_size: bool,
) -> Result<(), FailureDetail> {
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

    let image = decode_raw_with_error_handler(input_path, &decode_options)?;

    let width = image.width();
    let height = image.height();

    if width == 0 || height == 0 {
        return Err(FailureDetail::raw_sensor_data(format!(
            "Decoded RAW has zero dimensions: {width}x{height}"
        )));
    }

    let encoder = Encoder::fastest().quality(JPEG_QUALITY);

    if let Some(out) = output {
        if let Some(parent) = out.parent() {
            create_dir_all(parent).map_err(|e| {
                FailureDetail::jpeg_encode(format!(
                    "Failed to create thumbnail directory {}: {e}",
                    parent.display()
                ))
            })?;
        }
        let file = File::create(out).map_err(|e| {
            FailureDetail::jpeg_encode(format!(
                "Failed to create JPEG output file {}: {e}",
                out.display()
            ))
        })?;
        if let Err(e) =
            encoder.encode_rgb_to_writer(image.data(), width, height, BufWriter::new(file))
        {
            let _ = remove_file(out);
            return Err(FailureDetail::jpeg_encode(format!(
                "Failed to encode JPEG thumbnail for {}: {e}",
                out.display()
            )));
        }
    } else {
        encoder
            .encode_rgb_to_writer(image.data(), width, height, &mut std::io::sink())
            .map_err(|e| {
                FailureDetail::jpeg_encode(format!(
                    "Failed to encode JPEG thumbnail in memory: {e}"
                ))
            })?;
    }

    Ok(())
}
