use std::{
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom, Write},
    os::unix::fs::PermissionsExt as _,
    path::{Component, Path},
};

use serde::Deserialize;
use sha2::{Digest as _, Sha256};

pub(crate) const MAGIC: &[u8; 8] = b"CWDELTA1";
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const WINDOW_LOG_MAX: u32 = 31;

#[derive(Deserialize)]
pub(crate) struct Manifest {
    pub(crate) from: String,
    pub(crate) to: String,
    entries: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Entry {
    Dir {
        path: String,
        mode: u32,
    },
    File {
        path: String,
        mode: u32,
        sha256: String,
        #[serde(flatten)]
        source: Source,
    },
    Symlink {
        path: String,
        target: String,
    },
}

#[derive(Deserialize)]
#[serde(tag = "source", rename_all = "lowercase")]
enum Source {
    Copy,
    Patch {
        source_sha256: String,
        offset: u64,
        length: u64,
    },
    Data {
        offset: u64,
        length: u64,
    },
}

fn safe_relative(path: &str) -> Result<&Path, String> {
    let relative = Path::new(path);
    if path.is_empty()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(format!("The update names an unsafe path: {path:?}"));
    }
    Ok(relative)
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut reader = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

struct HashingWriter {
    file: File,
    hasher: Sha256,
}

impl Write for HashingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.file.write(bytes)?;
        self.hasher.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

pub(crate) fn read_manifest(delta: &mut File) -> Result<(Manifest, u64), String> {
    let mut magic = [0_u8; 8];
    delta
        .read_exact(&mut magic)
        .map_err(|_| "The update patch is truncated".to_string())?;
    if &magic != MAGIC {
        return Err("The file is not a Capwise update patch".to_string());
    }
    let mut length = [0_u8; 8];
    delta
        .read_exact(&mut length)
        .map_err(|_| "The update patch is truncated".to_string())?;
    let length = u64::from_le_bytes(length);
    if length > MAX_MANIFEST_BYTES {
        return Err("The update patch manifest is too large".to_string());
    }
    let mut manifest = vec![0_u8; length as usize];
    delta
        .read_exact(&mut manifest)
        .map_err(|_| "The update patch is truncated".to_string())?;
    let manifest = serde_json::from_slice(&manifest)
        .map_err(|error| format!("Invalid update patch manifest: {error}"))?;
    Ok((manifest, 16 + length))
}

/// Rebuilds the new app bundle at `output` from the installed `source` bundle
/// and a signed patch. Every file is checked against the hash the release
/// recorded, so the result is byte-for-byte the signed, notarized bundle or
/// the call fails and the caller falls back to the full download.
pub(crate) fn apply(
    delta_path: &Path,
    source: &Path,
    output: &Path,
    from: &str,
    to: &str,
) -> Result<(), String> {
    let mut delta = File::open(delta_path).map_err(|error| error.to_string())?;
    let (manifest, payload) = read_manifest(&mut delta)?;
    if manifest.from != from || manifest.to != to {
        return Err(format!(
            "The update patch is for {} → {}, not {from} → {to}",
            manifest.from, manifest.to
        ));
    }
    std::fs::create_dir(output).map_err(|error| format!("{}: {error}", output.display()))?;

    for entry in &manifest.entries {
        match entry {
            Entry::Dir { path, mode } => {
                let directory = output.join(safe_relative(path)?);
                std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(*mode))
                    .map_err(|error| error.to_string())?;
            }
            Entry::File {
                path,
                mode,
                sha256,
                source: origin,
            } => {
                let relative = safe_relative(path)?;
                let target = output.join(relative);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                let written = match origin {
                    Source::Copy => {
                        std::fs::copy(source.join(relative), &target)
                            .map_err(|error| format!("{path}: {error}"))?;
                        sha256_file(&target)?
                    }
                    Source::Patch {
                        source_sha256,
                        offset,
                        length,
                    } => {
                        let original = std::fs::read(source.join(relative))
                            .map_err(|error| format!("{path}: {error}"))?;
                        if hex(&Sha256::digest(&original)) != *source_sha256 {
                            return Err(format!(
                                "The installed {path} is not the file this patch expects"
                            ));
                        }
                        decode(
                            &mut delta,
                            payload + offset,
                            *length,
                            Some(&original),
                            &target,
                        )?
                    }
                    Source::Data { offset, length } => {
                        decode(&mut delta, payload + offset, *length, None, &target)?
                    }
                };
                if written != *sha256 {
                    return Err(format!("The rebuilt {path} does not match the release"));
                }
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(*mode))
                    .map_err(|error| error.to_string())?;
            }
            Entry::Symlink { path, target } => {
                let link = output.join(safe_relative(path)?);
                if let Some(parent) = link.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                std::os::unix::fs::symlink(target, &link)
                    .map_err(|error| format!("{path}: {error}"))?;
            }
        }
    }
    Ok(())
}

fn decode(
    delta: &mut File,
    offset: u64,
    length: u64,
    prefix: Option<&[u8]>,
    target: &Path,
) -> Result<String, String> {
    delta
        .seek(SeekFrom::Start(offset))
        .map_err(|error| error.to_string())?;
    let compressed = BufReader::new((&mut *delta).take(length));
    let mut decoder = match prefix {
        Some(prefix) => zstd::stream::read::Decoder::with_ref_prefix(compressed, prefix),
        None => zstd::stream::read::Decoder::with_buffer(compressed),
    }
    .map_err(|error| error.to_string())?;
    decoder
        .window_log_max(WINDOW_LOG_MAX)
        .map_err(|error| error.to_string())?;
    let mut writer = HashingWriter {
        file: File::create(target).map_err(|error| error.to_string())?,
        hasher: Sha256::new(),
    };
    std::io::copy(&mut decoder, &mut writer)
        .map_err(|error| format!("Could not apply the update patch: {error}"))?;
    writer.file.sync_all().map_err(|error| error.to_string())?;
    Ok(hex(&writer.hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "capwise-delta-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn compress(bytes: &[u8], prefix: Option<&[u8]>) -> Vec<u8> {
        let mut encoder = match prefix {
            Some(prefix) => zstd::stream::write::Encoder::with_ref_prefix(Vec::new(), 3, prefix),
            None => zstd::stream::write::Encoder::new(Vec::new(), 3),
        }
        .unwrap();
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    fn write_delta(path: &Path, manifest: serde_json::Value, payload: &[u8]) {
        let manifest = serde_json::to_vec(&manifest).unwrap();
        let mut file = File::create(path).unwrap();
        file.write_all(MAGIC).unwrap();
        file.write_all(&(manifest.len() as u64).to_le_bytes())
            .unwrap();
        file.write_all(&manifest).unwrap();
        file.write_all(payload).unwrap();
    }

    struct Fixture {
        root: std::path::PathBuf,
        old: std::path::PathBuf,
        delta: std::path::PathBuf,
        binary_new: Vec<u8>,
    }

    fn fixture(name: &str, installed_binary: &[u8]) -> Fixture {
        let root = scratch(name);
        let old = root.join("Old.app");
        std::fs::create_dir_all(old.join("Contents/MacOS")).unwrap();
        std::fs::create_dir_all(old.join("Contents/Resources")).unwrap();
        let binary_old = b"binary version one, with plenty of shared bytes".repeat(64);
        let mut binary_new = binary_old.clone();
        binary_new.extend_from_slice(b" and a new feature");
        std::fs::write(old.join("Contents/MacOS/App"), installed_binary).unwrap();
        std::fs::write(old.join("Contents/Resources/font.ttf"), b"font").unwrap();
        std::fs::write(old.join("Contents/Resources/stale.txt"), b"gone").unwrap();

        let patch = compress(&binary_new, Some(&binary_old));
        let plist = b"<plist>2</plist>";
        let data = compress(plist, None);
        let mut payload = patch.clone();
        payload.extend_from_slice(&data);
        let delta = root.join("update.delta");
        write_delta(
            &delta,
            serde_json::json!({
                "from": "1.0.0",
                "to": "1.1.0",
                "entries": [
                    {"kind": "dir", "path": "Contents", "mode": 0o755},
                    {"kind": "dir", "path": "Contents/MacOS", "mode": 0o755},
                    {"kind": "dir", "path": "Contents/Resources", "mode": 0o755},
                    {"kind": "file", "path": "Contents/MacOS/App", "mode": 0o755,
                     "sha256": sha(&binary_new), "source": "patch",
                     "source_sha256": sha(&binary_old), "offset": 0, "length": patch.len()},
                    {"kind": "file", "path": "Contents/Info.plist", "mode": 0o644,
                     "sha256": sha(plist), "source": "data",
                     "offset": patch.len(), "length": data.len()},
                    {"kind": "file", "path": "Contents/Resources/font.ttf", "mode": 0o644,
                     "sha256": sha(b"font"), "source": "copy"},
                    {"kind": "symlink", "path": "Contents/Current", "target": "Resources"},
                ],
            }),
            &payload,
        );
        Fixture {
            root,
            old,
            delta,
            binary_new,
        }
    }

    #[test]
    fn a_patch_rebuilds_exactly_the_new_bundle() {
        let binary_old = b"binary version one, with plenty of shared bytes".repeat(64);
        let fixture = fixture("apply", &binary_old);
        let new = fixture.root.join("New.app");
        apply(&fixture.delta, &fixture.old, &new, "1.0.0", "1.1.0").unwrap();

        assert_eq!(
            std::fs::read(new.join("Contents/MacOS/App")).unwrap(),
            fixture.binary_new
        );
        assert_eq!(
            std::fs::read(new.join("Contents/Info.plist")).unwrap(),
            b"<plist>2</plist>"
        );
        assert_eq!(
            std::fs::read(new.join("Contents/Current/font.ttf")).unwrap(),
            b"font"
        );
        assert!(!new.join("Contents/Resources/stale.txt").exists());
        let mode = std::fs::metadata(new.join("Contents/MacOS/App"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
        std::fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn a_modified_install_is_refused_before_patching() {
        let fixture = fixture("modified", b"someone edited this binary");
        let new = fixture.root.join("New.app");
        let error = apply(&fixture.delta, &fixture.old, &new, "1.0.0", "1.1.0").unwrap_err();
        assert!(error.contains("not the file this patch expects"), "{error}");
        std::fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn a_patch_for_another_version_is_refused() {
        let binary_old = b"binary version one, with plenty of shared bytes".repeat(64);
        let fixture = fixture("versions", &binary_old);
        let new = fixture.root.join("New.app");
        let error = apply(&fixture.delta, &fixture.old, &new, "0.9.0", "1.1.0").unwrap_err();
        assert!(error.contains("not 0.9.0 → 1.1.0"), "{error}");
        assert!(!new.exists());
        std::fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn paths_cannot_escape_the_bundle() {
        for path in ["../evil", "/etc/evil", "Contents/../../evil", ""] {
            assert!(safe_relative(path).is_err(), "{path}");
        }
        assert!(safe_relative("Contents/MacOS/App").is_ok());
    }
}
