//! The pinned input files: the manifest that lists them, reading them back
//! checked, and fetching them.
//!
//! `manifest.txt` lists each file under `data/` with its SHA-256 and the URL
//! it comes from. The files themselves are not in the public repository;
//! `cargo run -p unicode_gen -- fetch` downloads them with `curl` and checks
//! each against its hash. The README lists them too.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use crate::sha256::sha256;

/// The manifest, from this crate's manifest directory.
const MANIFEST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/manifest.txt");

/// What a missing input file's error adds: where the inputs come from.
const FETCH_HINT: &str = "the generator's input files are not in the public repository; \
     run `cargo run -p unicode_gen -- fetch` to download and check them, \
     or see support/unicode_gen/README.md";

/// One line of the manifest.
pub(crate) struct Entry {
    /// The file's path under `data/`, such as `18.0.0/LineBreak.txt`.
    pub(crate) path: String,
    /// The file's SHA-256 in lowercase hex.
    pub(crate) hash: String,
    /// Where the file is downloaded from.
    pub(crate) url: String,
}

/// Returns every entry of the manifest.
pub(crate) fn manifest() -> Result<Vec<Entry>, String> {
    let text = std::fs::read_to_string(MANIFEST)
        .map_err(|error| format!("reading {MANIFEST}: {error}"))?;
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (Some(path), Some(hash), Some(url), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(format!("manifest line {line:?} is not `path sha256 url`"));
        };
        let name = path.rsplit('/').next().unwrap_or(path);
        if !url.ends_with(&format!("/{name}")) {
            return Err(format!("{path}: {url} names another file"));
        }
        entries.push(Entry {
            path: path.to_owned(),
            hash: hash.to_owned(),
            url: url.to_owned(),
        });
    }
    Ok(entries)
}

/// Reads every file the manifest lists under the folder `folder` of `data`,
/// checking each against its SHA-256, by name.
///
/// Returns an error where a listed file is missing or changed, or where the
/// folder holds a file the manifest does not list.
pub(crate) fn read_pinned(data: &Path, folder: &str) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let dir = data.join(folder);
    let mut files = BTreeMap::new();
    for entry in manifest()? {
        let Some(name) = entry
            .path
            .strip_prefix(folder)
            .and_then(|rest| rest.strip_prefix('/'))
        else {
            continue;
        };
        let path = dir.join(name);
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("cannot read {} ({error}): {FETCH_HINT}", path.display()))?;
        if sha256(&bytes) != entry.hash {
            return Err(format!(
                "{} is not the file the manifest pins: {FETCH_HINT}",
                entry.path
            ));
        }
        files.insert(name.to_owned(), bytes);
    }
    if files.is_empty() {
        return Err(format!("the manifest lists no files in {folder}"));
    }
    let listing =
        std::fs::read_dir(&dir).map_err(|error| format!("listing {}: {error}", dir.display()))?;
    for entry in listing {
        let name = entry
            .map_err(|error| format!("listing {}: {error}", dir.display()))?
            .file_name()
            .to_string_lossy()
            .into_owned();
        if !files.contains_key(&name) {
            return Err(format!("{folder}/{name} is not in the manifest"));
        }
    }
    Ok(files)
}

/// Downloads every file the manifest lists into `data`, with `curl`, and
/// checks each against its SHA-256.
///
/// Skips a file already present with the right hash. Removes a download
/// whose hash differs and returns an error naming it.
pub(crate) fn fetch(data: &Path) -> Result<(), String> {
    let entries = manifest()?;
    let mut fetched = 0;
    for entry in &entries {
        let path = data.join(&entry.path);
        if std::fs::read(&path).is_ok_and(|bytes| sha256(&bytes) == entry.hash) {
            continue;
        }
        println!("fetching {}", entry.path);
        let status = Command::new("curl")
            .args(["--fail", "--silent", "--show-error", "--location"])
            .arg("--create-dirs")
            .arg("--output")
            .arg(&path)
            .arg(&entry.url)
            .status()
            .map_err(|error| format!("running curl: {error}; is curl installed?"))?;
        if !status.success() {
            return Err(format!("curl failed on {} ({status})", entry.url));
        }
        let bytes =
            std::fs::read(&path).map_err(|error| format!("reading {}: {error}", path.display()))?;
        if sha256(&bytes) != entry.hash {
            let _ = std::fs::remove_file(&path);
            return Err(format!(
                "{} downloaded from {} does not have the pinned SHA-256",
                entry.path, entry.url
            ));
        }
        fetched += 1;
    }
    println!(
        "{fetched} fetched, {} already present",
        entries.len() - fetched
    );
    Ok(())
}
