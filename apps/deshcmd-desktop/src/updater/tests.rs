use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::json;

use super::*;

fn test_key() -> Ed25519KeyPair {
    let seed = [22; 32];
    Ed25519KeyPair::from_seed_unchecked(&seed).unwrap()
}

fn manifest(tag: &str, name: &str, executable: &[u8]) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": 1,
        "version": "999.0.0",
        "tag": tag,
        "assets": [{
            "name": name,
            "size": 10,
            "sha256": "unused in these fixtures",
            "executable_sha256": hex::encode(digest::digest(&digest::SHA256, executable)),
        }]
    }))
    .unwrap()
}

#[test]
fn authenticates_the_exact_manifest_and_rejects_replays() {
    let key = test_key();
    let public_key = key.public_key().as_ref().try_into().unwrap();
    let name = "deshcmd-linux-x86_64.tar.gz";
    let bytes = manifest("v999.0.0", name, b"test");
    let sig = key.sign(&bytes);
    let current = Version::new(1, 0, 0);

    let update = authenticate(&bytes, sig.as_ref(), public_key, &current, name)
        .unwrap()
        .unwrap();
    assert_eq!(update.version, "999.0.0");
    assert!(authenticate(&bytes, sig.as_ref(), public_key, &current, "missing").is_err());
    for version in [Version::new(999, 0, 0), Version::new(1000, 0, 0)] {
        assert!(
            authenticate(&bytes, sig.as_ref(), public_key, &version, name)
                .unwrap()
                .is_none()
        );
    }

    let mut tampered = bytes;
    tampered.push(b' ');
    assert!(authenticate(&tampered, sig.as_ref(), public_key, &current, name).is_err());
    let bad_tag = manifest("../999.0.0", name, b"test");
    let sig = key.sign(&bad_tag);
    assert!(authenticate(&bad_tag, sig.as_ref(), public_key, &current, name).is_err());
}

#[cfg(unix)]
#[test]
fn extracts_only_the_single_expected_executable() {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Seek;

    for (names, valid) in [
        (&["deshcmd"][..], true),
        (&["other"][..], false),
        (&["deshcmd", "other"][..], false),
    ] {
        let mut archive = tempfile::tempfile().unwrap();
        let mut tar = tar::Builder::new(GzEncoder::new(&mut archive, Compression::fast()));
        let payload = b"test executable";
        for name in names {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o755);
            header.set_size(payload.len() as u64);
            tar.append_data(&mut header, name, &payload[..]).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        archive.rewind().unwrap();

        let mut output = tempfile::tempfile().unwrap();
        assert_eq!(extract(archive, &mut output).is_ok(), valid);
        if valid {
            output.rewind().unwrap();
            let mut actual = Vec::new();
            output.read_to_end(&mut actual).unwrap();
            assert_eq!(actual, payload);
        }
    }
}

#[cfg(unix)]
#[test]
fn replacement_preserves_the_backup_and_restores_it_on_launch_error() {
    use std::os::unix::fs::PermissionsExt;

    let key = test_key();
    let public_key = key.public_key().as_ref().try_into().unwrap();
    let name = platform_asset().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("deshcmd");
    let backup = dir.path().join("deshcmd.previous");
    let stage = dir.path().join(".deshcmd-update-test");
    fs::create_dir(&stage).unwrap();
    let helper = stage.join(HELPER);
    let replacement = stage.join("replacement");
    let old = b"old executable fixture";
    let older = b"existing rollback executable fixture";
    let installed = b"#!/bin/sh\n: > \"$0.started\"\n";
    fs::write(&helper, old).unwrap();
    fs::write(&target, old).unwrap();
    fs::write(&backup, older).unwrap();

    let write_signed_stage = |payload: &[u8]| {
        fs::write(&replacement, payload).unwrap();
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o700)).unwrap();
        let manifest = manifest("v999.0.0", name, payload);
        fs::write(stage.join(MANIFEST), &manifest).unwrap();
        fs::write(stage.join(SIGNATURE), key.sign(&manifest).as_ref()).unwrap();
    };

    write_signed_stage(b"invalid executable");
    assert!(
        replace_and_restart(&target, &stage, &helper, public_key)
            .unwrap_err()
            .to_string()
            .contains("Restored the previous version")
    );
    assert_eq!(fs::read(&target).unwrap(), old);
    assert_eq!(fs::read(&backup).unwrap(), older);

    write_signed_stage(installed);
    fs::write(&replacement, b"tampered").unwrap();
    assert!(replace_and_restart(&target, &stage, &helper, public_key).is_err());
    assert_eq!(fs::read(&target).unwrap(), old);

    write_signed_stage(installed);
    replace_and_restart(&target, &stage, &helper, public_key).unwrap();
    assert_eq!(fs::read(&target).unwrap(), installed);
    assert_eq!(fs::read(&backup).unwrap(), old);
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !dir.path().join("deshcmd.started").is_file() {
        assert!(
            std::time::Instant::now() < deadline,
            "Updated executable did not start"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(windows)]
#[test]
fn a_busy_executable_does_not_replace_the_existing_rollback_copy() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    let key = test_key();
    let public_key = key.public_key().as_ref().try_into().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let stage = dir.path().join("stage");
    fs::create_dir(&stage).unwrap();
    let target = dir.path().join("deshcmd.exe");
    let backup = dir.path().join("deshcmd.exe.previous");
    let helper = stage.join(HELPER);
    fs::write(&target, b"installed").unwrap();
    fs::write(&helper, b"installed").unwrap();
    fs::write(&backup, b"previous").unwrap();
    fs::write(stage.join("replacement"), b"update").unwrap();
    let manifest = manifest("v999.0.0", platform_asset().unwrap(), b"update");
    fs::write(stage.join(MANIFEST), &manifest).unwrap();
    fs::write(stage.join(SIGNATURE), key.sign(&manifest).as_ref()).unwrap();

    let busy = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&target)
        .unwrap();
    assert!(replace_and_restart(&target, &stage, &helper, public_key).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"installed");
    assert_eq!(fs::read(&backup).unwrap(), b"previous");
    drop(busy);
}
