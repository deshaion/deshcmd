use super::*;
use std::time::{Duration, Instant};

fn image() -> Preview {
    Preview::Image {
        size: [2, 1],
        rgba: vec![1, 2, 3, 4, 251, 252, 253, 255],
    }
}

#[test]
fn reuses_exact_thumbnail_across_instances_and_invalidates_on_source_change() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let source = root.join("source.jpg");
    fs::write(&source, "original").unwrap();
    let cancel = Cancellation::default();

    let first = PreviewDiskCache::new(root.clone());
    let key = source_key(&source, 720).unwrap();
    let destination = cache_path(&first.directory, key);
    let (write_key, preview) = first.get_or_decode(&source, 720, &cancel, image);
    let Preview::Image { size, rgba } = preview else {
        panic!("expected image")
    };
    first
        .writes
        .send(Thumbnail {
            key: write_key.unwrap(),
            size,
            rgba,
        })
        .unwrap();
    drop(first);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !destination.is_file() {
        assert!(Instant::now() < deadline, "thumbnail write timed out");
        std::thread::sleep(Duration::from_millis(1));
    }

    let reopened = PreviewDiskCache::new(root);
    let (write_key, hit) =
        reopened.get_or_decode(&source, 720, &cancel, || panic!("decoded source on hit"));
    assert!(write_key.is_none());
    assert!(matches!((hit, image()),
        (Preview::Image { size, rgba }, Preview::Image { size: expected, rgba: pixels })
            if size == expected && rgba == pixels));

    fs::write(&source, "replaced").unwrap();
    filetime::set_file_mtime(&source, filetime::FileTime::from_unix_time(1, 0)).unwrap();
    assert_ne!(key, source_key(&source, 720).unwrap());
    assert!(matches!(
        reopened
            .get_or_decode(&source, 720, &cancel, || Preview::Unavailable)
            .1,
        Preview::Unavailable
    ));
}

#[test]
fn corrupt_cache_entry_falls_back_and_cancelled_requests_skip_decode() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let source = root.join("source.png");
    fs::write(&source, []).unwrap();
    let cache = PreviewDiskCache::new(root);
    fs::create_dir_all(&cache.directory).unwrap();
    let key = source_key(&source, 720).unwrap();
    fs::write(cache_path(&cache.directory, key), "bad thumbnail").unwrap();

    let cancel = Cancellation::default();
    assert!(matches!(
        cache.get_or_decode(&source, 720, &cancel, image).1,
        Preview::Image { .. }
    ));
    cancel.cancel();
    assert!(matches!(
        cache
            .get_or_decode(&source, 720, &cancel, || panic!(
                "decoded after cancellation"
            ))
            .1,
        Preview::Unavailable
    ));
}

#[test]
fn trimming_enforces_byte_and_entry_budgets_and_keeps_recent_thumbnails() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let old = cache_path(root, blake3::hash(b"old"));
    let recent = cache_path(root, blake3::hash(b"recent"));
    let newest = cache_path(root, blake3::hash(b"newest"));
    for (index, path) in [&old, &recent, &newest].into_iter().enumerate() {
        fs::write(path, [0; 10]).unwrap();
        filetime::set_file_mtime(
            path,
            filetime::FileTime::from_unix_time(index as i64 + 1, 0),
        )
        .unwrap();
    }
    let other = root.join("unrelated.png");
    fs::write(&other, [0; 100]).unwrap();

    trim(root, 25, 3);
    assert!(!old.exists());
    assert!(recent.is_file());
    assert!(newest.is_file());
    trim(root, 25, 1);
    assert!(!recent.exists());
    assert!(newest.is_file());
    assert!(other.is_file());
}

#[test]
fn cache_key_changes_after_same_size_rewrite_with_restored_mtime() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().canonicalize().unwrap().join("source.png");
    fs::write(&path, b"first").unwrap();
    let time = filetime::FileTime::from_unix_time(1_000_000_000, 0);
    filetime::set_file_mtime(&path, time).unwrap();
    let Ok(before) = source_key(&path, 720) else {
        // Some volumes cannot provide a reliable change stamp.
        return;
    };

    std::thread::sleep(Duration::from_millis(20));
    fs::write(&path, b"other").unwrap();
    filetime::set_file_mtime(&path, time).unwrap();
    assert_ne!(source_key(&path, 720).unwrap(), before);
}

#[test]
fn background_preview_returns_before_the_cache_write() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let source = root.join("source.png");
    image::RgbaImage::from_pixel(2, 1, image::Rgba([10, 20, 30, 255]))
        .save(&source)
        .unwrap();
    let cache = PreviewDiskCache::new(root);
    let key = source_key(&source, 720).unwrap();
    let target = cache_path(&cache.directory, key);
    let guard = cache_lock(&cache.directory).unwrap();
    let (send, result) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(cache.preview(&source, 720, &Cancellation::default()));
    });
    let preview = result.recv_timeout(Duration::from_secs(5));
    assert!(!target.exists());
    drop(guard);
    assert!(matches!(preview, Ok(Preview::Image { .. })));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !target.is_file() {
        assert!(
            Instant::now() < deadline,
            "deferred thumbnail was not written"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn startup_reclaims_staging_files_after_live_writer_releases_lock() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let directory = root.join("previews-v1");
    let guard = cache_lock(&directory).unwrap();
    let abandoned = directory.join(format!("{STAGING_PREFIX}abandoned"));
    let active = directory.join(format!("{STAGING_PREFIX}active"));
    let unrelated = directory.join("keep-me");
    for path in [&abandoned, &active, &unrelated] {
        fs::write(path, b"file").unwrap();
    }
    let cache = PreviewDiskCache::new(root);
    assert!(active.is_file());
    fs::remove_file(&active).unwrap();
    drop(guard);
    let deadline = Instant::now() + Duration::from_secs(5);
    while abandoned.exists() {
        assert!(
            Instant::now() < deadline,
            "abandoned staging file was not removed"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(unrelated.is_file());
    drop(cache);
}
