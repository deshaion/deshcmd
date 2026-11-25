"""Create the signed update metadata from the five standalone release archives."""

import hashlib
import json
import re
import subprocess
import sys
import tarfile
import zipfile
from pathlib import Path

ARCHIVES = (
    "deshcmd-linux-x86_64.tar.gz",
    "deshcmd-linux-aarch64.tar.gz",
    "deshcmd-windows-x86_64.zip",
    "deshcmd-macos-aarch64.tar.gz",
    "deshcmd-macos-x86_64.tar.gz",
)
ED25519_SPKI_PREFIX = bytes.fromhex("302a300506032b6570032100")
MAX_SIZE = 256 * 1024 * 1024


def executable_hash(path):
    if path.suffix == ".zip":
        with zipfile.ZipFile(path) as archive:
            entries = archive.infolist()
            if len(entries) != 1 or entries[0].filename != "deshcmd.exe":
                raise ValueError(f"Expected only deshcmd.exe in {path}")
            if not 0 < entries[0].file_size <= MAX_SIZE:
                raise ValueError(f"Invalid executable size in {path}")
            with archive.open(entries[0]) as binary:
                return hashlib.file_digest(binary, "sha256").hexdigest()
    with tarfile.open(path, "r:gz") as archive:
        entries = archive.getmembers()
        if len(entries) != 1 or entries[0].name != "deshcmd" or not entries[0].isfile():
            raise ValueError(f"Expected only deshcmd in {path}")
        if not 0 < entries[0].size <= MAX_SIZE:
            raise ValueError(f"Invalid executable size in {path}")
        with archive.extractfile(entries[0]) as binary:
            return hashlib.file_digest(binary, "sha256").hexdigest()


def describe_asset(path):
    size = path.stat().st_size
    if not 0 < size <= MAX_SIZE:
        raise ValueError(f"Invalid archive size in {path}")
    with path.open("rb") as file:
        digest = hashlib.file_digest(file, "sha256").hexdigest()
    return {
        "name": path.name,
        "size": size,
        "sha256": digest,
        "executable_sha256": executable_hash(path),
    }


def main(dist, version, tag, signing_key, public_key):
    if not re.fullmatch(r"(0|[1-9][0-9]*)(\.(0|[1-9][0-9]*)){2}", version):
        raise ValueError("The workspace version must be a stable MAJOR.MINOR.PATCH version")
    if tag not in (version, f"v{version}"):
        raise ValueError(f"Release tag {tag} must match workspace version {version}")
    if not re.fullmatch(r"[a-fA-F0-9]{64}", public_key):
        raise ValueError("The update public key must be a 32-byte Ed25519 key in hex")
    derived = subprocess.check_output(
        ["openssl", "pkey", "-in", signing_key, "-pubout", "-outform", "DER"]
    )
    if derived != ED25519_SPKI_PREFIX + bytes.fromhex(public_key):
        raise ValueError("The signing key does not match the update public key")

    dist = Path(dist)
    manifest = {
        "schema": 1,
        "version": version,
        "tag": tag,
        "assets": [describe_asset(dist / name) for name in ARCHIVES],
    }
    manifest_file = dist / "latest.json"
    manifest_file.write_bytes((json.dumps(manifest, indent=2) + "\n").encode("utf-8"))
    subprocess.run(
        ["openssl", "pkeyutl", "-sign", "-rawin", "-inkey", signing_key,
         "-in", manifest_file, "-out", dist / "latest.json.sig"],
        check=True,
    )


if __name__ == "__main__":
    if len(sys.argv) != 6:
        sys.exit(f"Usage: {sys.argv[0]} DIST VERSION TAG SIGNING_KEY_PEM PUBLIC_KEY_HEX")
    main(*sys.argv[1:])
