use anyhow::{Context, Result, anyhow};
use mozjpeg_rs::Encoder;
use rawlib::{DecodeOptions, ImageFormat, ThumbnailData};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fs::{File, create_dir_all, remove_file};
use std::io::{BufWriter, Cursor};
use std::path::Path;

pub const JPEG_QUALITY: u8 = 82;

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

/// Decodes camera RAW data using LibRaw while catching internal sensor corruption callbacks.
fn decode_raw_with_error_handler(input_path: &Path, opts: &DecodeOptions) -> Result<ThumbnailData> {
    if !input_path.exists() {
        return Err(anyhow!("File does not exist: {}", input_path.display()));
    }

    let handle = RawHandle(unsafe { rawlib::ffi::libraw_init(rawlib::ffi::LIBRAW_OPTIONS_NONE) });
    if handle.0.is_null() {
        return Err(anyhow!("Failed to initialize LibRaw processor"));
    }

    let mut data_error: Option<String> = None;
    unsafe {
        libraw_set_dataerror_handler(
            handle.0,
            Some(raw_data_error_callback),
            &mut data_error as *mut _ as *mut c_void,
        );
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = input_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let ret = unsafe { rawlib::ffi::libraw_open_wfile(handle.0, wide.as_ptr()) };
        if ret != rawlib::ffi::LIBRAW_SUCCESS {
            let err_c = unsafe { rawlib::ffi::libraw_strerror(ret) };
            let err_str = unsafe { CStr::from_ptr(err_c) }.to_string_lossy();
            return Err(anyhow!("LibRaw open_file error {ret}: {err_str}"));
        }
    }

    #[cfg(not(windows))]
    {
        let path_str = input_path
            .to_str()
            .ok_or_else(|| anyhow!("Invalid UTF-8 path encoding: {}", input_path.display()))?;
        let c_path = CString::new(path_str).map_err(|e| anyhow!("Path contains null byte: {e}"))?;
        let ret = unsafe { rawlib::ffi::libraw_open_file(handle.0, c_path.as_ptr()) };
        if ret != rawlib::ffi::LIBRAW_SUCCESS {
            let err_c = unsafe { rawlib::ffi::libraw_strerror(ret) };
            let err_str = unsafe { CStr::from_ptr(err_c) }.to_string_lossy();
            return Err(anyhow!("LibRaw open_file error {ret}: {err_str}"));
        }
    }

    if let Some(err) = data_error.take() {
        return Err(anyhow!("LibRaw data corruption during open: {err}"));
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

    let ret = unsafe { rawlib::ffi::libraw_unpack(handle.0) };
    if let Some(err) = data_error.take() {
        return Err(anyhow!(
            "LibRaw sensor data corruption during unpack: {err}"
        ));
    }
    if ret != rawlib::ffi::LIBRAW_SUCCESS {
        let err_c = unsafe { rawlib::ffi::libraw_strerror(ret) };
        let err_str = unsafe { CStr::from_ptr(err_c) }.to_string_lossy();
        return Err(anyhow!("LibRaw unpack error {ret}: {err_str}"));
    }

    let ret = unsafe { rawlib::ffi::libraw_dcraw_process(handle.0) };
    if let Some(err) = data_error.take() {
        return Err(anyhow!(
            "LibRaw sensor data corruption during decode: {err}"
        ));
    }
    if ret != rawlib::ffi::LIBRAW_SUCCESS {
        let err_c = unsafe { rawlib::ffi::libraw_strerror(ret) };
        let err_str = unsafe { CStr::from_ptr(err_c) }.to_string_lossy();
        return Err(anyhow!("LibRaw process error {ret}: {err_str}"));
    }

    let mut errc: c_int = 0;
    let img_ptr = unsafe { rawlib::ffi::libraw_dcraw_make_mem_image(handle.0, &mut errc) };
    if let Some(err) = data_error.take() {
        if !img_ptr.is_null() {
            unsafe { rawlib::ffi::libraw_dcraw_clear_mem(img_ptr) };
        }
        return Err(anyhow!(
            "LibRaw sensor data corruption during image generation: {err}"
        ));
    }
    if img_ptr.is_null() {
        let err_c = unsafe { rawlib::ffi::libraw_strerror(errc) };
        let err_str = unsafe { CStr::from_ptr(err_c) }.to_string_lossy();
        return Err(anyhow!("LibRaw make_mem_image error {errc}: {err_str}"));
    }

    let img = unsafe { &*img_ptr };
    let width = img.width;
    let height = img.height;
    let colors = img.colors;
    let bits = img.bits;
    let data_size = img.data_size as usize;
    let data = unsafe { std::slice::from_raw_parts(img.data.as_ptr(), data_size).to_vec() };

    unsafe {
        rawlib::ffi::libraw_dcraw_clear_mem(img_ptr);
    }

    Ok(ThumbnailData {
        format: ImageFormat::Bitmap,
        width,
        height,
        colors,
        bits,
        data,
    })
}

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

    let image = decode_raw_with_error_handler(input_path, &decode_options)
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
