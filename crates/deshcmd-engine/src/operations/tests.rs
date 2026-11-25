use super::*;
use std::time::{Duration, Instant};

#[test]
fn cancellation_during_rename_existing_keeps_both_original_paths() {
    let fixture = tempfile::tempdir().unwrap();
    let destination = fixture.path().join("destination");
    fs::create_dir(&destination).unwrap();
    let source = fixture.path().join("example.txt");
    let target = destination.join("example.txt");
    fs::write(&source, b"incoming").unwrap();
    fs::write(&target, b"existing").unwrap();
    let control = Arc::new(Control::default());
    let report = |progress: Progress| {
        if progress.current == source.to_string_lossy() {
            control.cancel();
        }
    };
    let mut worker = Worker {
        control: control.clone(),
        progress: Progress::default(),
        report: &report,
        conflict: &|_| control.answer(Collision::RenameExisting, false),
        last_report: Instant::now() - Duration::from_secs(1),
        warnings: Vec::new(),
    };
    let error = worker
        .transfer(
            vec![source.clone()],
            &destination,
            true,
            TransferOptions::default(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("Cancelled · Moved 0 of 1"));
    assert!(control.is_cancelled());
    assert_eq!(fs::read(&source).unwrap(), b"incoming");
    assert_eq!(fs::read(&target).unwrap(), b"existing");
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
}

#[test]
fn rename_existing_restores_the_previous_item_if_publication_fails() {
    let fixture = tempfile::tempdir().unwrap();
    let target = fixture.path().join("existing");
    let backup = fixture.path().join("backup");
    fs::write(&target, b"existing").unwrap();
    let error =
        publish_renaming_existing(&fixture.path().join("missing"), &target, &backup).unwrap_err();
    assert!(error.to_string().contains("Restored the previous item"));
    assert_eq!(fs::read(&target).unwrap(), b"existing");
    assert!(!backup.exists());
}
