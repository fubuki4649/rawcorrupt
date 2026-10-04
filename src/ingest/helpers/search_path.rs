use infer::MatcherType::Image;
use infer::get_from_path;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::SendError;

pub const COMMON_RAW_EXTS: &[&str] = &[
    "arw", "cr2", "cr3", "nef", "dng", "raf", "rw2", "orf", "pef", "srw", "srf", "erf", "mrw",
    "3fr", "kdc", "mos", "mef", "nrw", "rwl", "raw",
];

/// Recursively traverses a directory, and sends `(absolute_path, relative_path)` pairs for all
/// applicable assets inside it and its children to a channel, matching suisai's search_path_for_assets.
pub fn search_path_for_assets(
    src: &Path,
    sender: &Sender<(PathBuf, PathBuf)>,
) -> Result<(), SendError<(PathBuf, PathBuf)>> {
    search_recursive(src, src, sender)
}

fn search_recursive(
    root: &Path,
    current: &Path,
    sender: &Sender<(PathBuf, PathBuf)>,
) -> Result<(), SendError<(PathBuf, PathBuf)>> {
    if current.is_file() {
        if is_candidate_asset(current) {
            let rel = current.strip_prefix(root).unwrap_or(current).to_path_buf();
            sender.blocking_send((current.to_path_buf(), rel))?;
        }
    } else if current.is_dir()
        && let Ok(read_dir) = current.read_dir()
    {
        for child in read_dir.flatten() {
            search_recursive(root, child.path().as_path(), sender)?;
        }
    }

    Ok(())
}

/// Checks if a file is an image using infer's type detection, matching suisai's check.
pub fn is_image_by_infer(path: &Path) -> bool {
    matches!(
        get_from_path(path).ok().flatten().map(|t| t.matcher_type()),
        Some(Image)
    )
}

/// Checks if a file is a candidate for camera raw testing (by extension or infer).
fn is_candidate_asset(path: &Path) -> bool {
    if let Some(name) = path.file_name().and_then(|n| n.to_str())
        && name.starts_with('.')
    {
        return false;
    }

    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_lowercase();
        if COMMON_RAW_EXTS.contains(&ext_lower.as_str()) {
            return true;
        }
    }

    is_image_by_infer(path)
}
