use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, SyncSender};
use std::time::UNIX_EPOCH;

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, ImageFormat, ImageReader};

use crate::listing::{Kind, Preview, preview_cancellable};
use crate::read_pool::Cancellation;

const MAX_BYTES: u64 = 128 * 1024 * 1024;
const MAX_ENTRIES: usize = 512;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_EDGE: u32 = 720;

pub struct PreviewDiskCache {
    directory: PathBuf,
    writes: SyncSender<Thumbnail>,
}

struct Thumbnail {
    key: blake3::Hash,
    size: [usize; 2],
    rgba: Vec<u8>,
}

impl PreviewDiskCache {
    pub fn new(cache_root: PathBuf) -> Self {
        // Bump the directory version when the key or thumbnail representation changes.
        let directory = cache_root.join("previews-v1");
        let (writes, pending) = mpsc::sync_channel::<Thumbnail>(2);
        let target = directory.clone();
        std::thread::spawn(move || {
            while let Ok(thumbnail) = pending.recv() {
                save(&target, thumbnail);
            }
        });
        Self { directory, writes }
    }

    pub fn preview(
        &self,
        source: &Path,
        pixels: u32,
        cancel: &Cancellation,
        background: bool,
    ) -> Preview {
        let (key, preview) = self.get_or_decode(source, pixels, cancel, || {
            preview_cancellable(source, Kind::Image, pixels, cancel)
        });
        if let (Some(key), Preview::Image { size, rgba }) = (key, &preview) {
            let thumbnail = Thumbnail {
                key,
                size: *size,
                rgba: rgba.clone(),
            };
            if background {
                save(&self.directory, thumbnail);
            } else {
                let _ = self.writes.try_send(thumbnail);
            }
        }
        preview
    }

    fn get_or_decode(
        &self,
        source: &Path,
        pixels: u32,
        cancel: &Cancellation,
        decode: impl FnOnce() -> Preview,
    ) -> (Option<blake3::Hash>, Preview) {
        if cancel.cancelled() {
            return (None, Preview::Unavailable);
        }
        let key = (pixels > 0 && pixels <= MAX_EDGE)
            .then(|| source_key(source, pixels).ok())
            .flatten();
        if let Some(key) = key
            && let Ok(preview) = load(&cache_path(&self.directory, key), pixels)
            && source_key(source, pixels).ok() == Some(key)
            && !cancel.cancelled()
        {
            return (None, preview);
        }
        if cancel.cancelled() {
            return (None, Preview::Unavailable);
        }
        let preview = decode();
        let valid =
            key.filter(|key| !cancel.cancelled() && source_key(source, pixels).ok() == Some(*key));
        (valid, preview)
    }
}

fn save(directory: &Path, thumbnail: Thumbnail) {
    if store(directory, thumbnail).is_ok() {
        trim(directory, MAX_BYTES, MAX_ENTRIES);
    }
}

fn source_key(path: &Path, pixels: u32) -> io::Result<blake3::Hash> {
    if !path.is_absolute() {
        return Err(io::Error::other("Preview cache requires an absolute path"));
    }
    let metadata = path.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("Preview cache requires a regular file"));
    }
    let mut key = blake3::Hasher::new();
    key.update(path.as_os_str().as_encoded_bytes());
    key.update(&[0]);
    key.update(&pixels.to_le_bytes());
    key.update(&metadata.len().to_le_bytes());
    let modified = metadata.modified()?;
    let nanos = match modified.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    };
    key.update(&nanos.to_le_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        key.update(&metadata.dev().to_le_bytes());
        key.update(&metadata.ino().to_le_bytes());
        key.update(&metadata.ctime().to_le_bytes());
        key.update(&metadata.ctime_nsec().to_le_bytes());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        key.update(&metadata.creation_time().to_le_bytes());
    }
    Ok(key.finalize())
}

fn cache_path(directory: &Path, key: blake3::Hash) -> PathBuf {
    directory.join(format!("{}.png", key.to_hex()))
}

fn load(path: &Path, pixels: u32) -> io::Result<Preview> {
    let metadata = path.symlink_metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err(io::Error::other("Cached thumbnail is too large"));
    }
    let file = File::open(path)?;
    let mut reader = ImageReader::with_format(BufReader::new(file), ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(pixels);
    limits.max_image_height = Some(pixels);
    limits.max_alloc = Some(16 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(io::Error::other)?.into_rgba8();
    // Touch the cache entry after a successful read so active thumbnails survive pruning.
    let _ = filetime::set_file_mtime(path, filetime::FileTime::now());
    Ok(Preview::Image {
        size: [image.width() as usize, image.height() as usize],
        rgba: image.into_raw(),
    })
}

fn store(directory: &Path, thumbnail: Thumbnail) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    PngEncoder::new_with_quality(
        file.as_file_mut(),
        CompressionType::Fast,
        FilterType::Adaptive,
    )
    .write_image(
        &thumbnail.rgba,
        thumbnail.size[0] as u32,
        thumbnail.size[1] as u32,
        ExtendedColorType::Rgba8,
    )
    .map_err(io::Error::other)?;
    if file.as_file().metadata()?.len() > MAX_FILE_BYTES {
        return Err(io::Error::other("Encoded thumbnail is too large"));
    }
    file.persist(cache_path(directory, thumbnail.key))?;
    Ok(())
}

fn trim(directory: &Path, max_bytes: u64, max_entries: usize) {
    let Ok(files) = fs::read_dir(directory) else {
        return;
    };
    let mut bytes = 0_u64;
    let mut entries = Vec::new();
    for entry in files.flatten() {
        let name = entry.file_name();
        let Some(hash) = name.to_str().and_then(|name| name.strip_suffix(".png")) else {
            continue;
        };
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        if let Ok(metadata) = entry.metadata()
            && metadata.is_file()
        {
            bytes = bytes.saturating_add(metadata.len());
            entries.push((
                metadata.modified().unwrap_or(UNIX_EPOCH),
                metadata.len(),
                entry.path(),
            ));
        }
    }
    if bytes <= max_bytes && entries.len() <= max_entries {
        return;
    }
    entries.sort_unstable();
    let mut count = entries.len();
    for (_, len, path) in entries {
        if fs::remove_file(path).is_ok() {
            bytes -= len;
            count -= 1;
        }
        if bytes <= max_bytes && count <= max_entries {
            break;
        }
    }
}

#[cfg(test)]
mod tests;
