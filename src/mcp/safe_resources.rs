//! `resources/list` and `resources/read` for the `safe/` mirror.
//!
//! A hacienda Safe workspace keeps a redacted copy of every original under `<root>/safe/`, and
//! confines the agent's shell to that folder. The agent still has to read the mirror, and
//! `resources/read` is the standard way to do it: without it the agent got `-32601`, fell back to
//! the shell, and tried the original.
//!
//! This surface serves `safe/**` and nothing else. Every refusal below is a path that could reach
//! outside it (`..`, an absolute path, an encoded separator, a symlink that points out), and none
//! of them reads the target first, so an error can never carry the content it refused.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use rmcp::ErrorData;
use rmcp::model::{ListResourcesResult, ReadResourceResult, Resource, ResourceContents};
use serde_json::json;

/// Every resource URI starts with this; the rest is the path under `safe/`.
const URI_PREFIX: &str = "basemind://safe/";
/// Same ceiling as the inline text of `redact_text`.
const MAX_RESOURCE_BYTES: u64 = 1 << 20;
/// A mirror this large is not a folder an agent browses; stop listing rather than stall.
const MAX_LISTED: usize = 5_000;
const MAX_DEPTH: usize = 32;
/// Directories visited per listing. The file cap alone does not bound a tree of many empty folders.
const MAX_DIRS: usize = 2_000;

fn safe_dir(root: &Path) -> PathBuf {
    root.join("safe")
}

/// Everything under `safe/` the agent may read, in a stable order. Symlinks are never listed or
/// followed: a mirror holds regular files, and a link is how a path would point out of it.
pub(crate) fn list(root: &Path) -> ListResourcesResult {
    let base = safe_dir(root);
    let mut found: Vec<(String, u64)> = Vec::new();
    // A `safe` that is a link is not the mirror: it may point at the originals.
    if is_plain_directory(&base) {
        let mut visited = 0;
        walk(&base, &base, 0, &mut visited, &mut found);
    }
    found.sort();
    let resources = found
        .into_iter()
        .map(|(relative, size)| {
            let uri = format!("{URI_PREFIX}{}", encode_path(&relative));
            Resource::new(uri, relative.clone())
                .with_mime_type(mime_for(&relative))
                .with_size(size)
        })
        .collect();
    ListResourcesResult::with_all_items(resources)
}

fn walk(base: &Path, dir: &Path, depth: usize, visited: &mut usize, found: &mut Vec<(String, u64)>) {
    if depth > MAX_DEPTH || found.len() >= MAX_LISTED || *visited >= MAX_DIRS {
        return;
    }
    *visited += 1;
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if found.len() >= MAX_LISTED {
            return;
        }
        // `DirEntry::file_type` does not follow symlinks, so a link is neither file nor dir here.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            walk(base, &path, depth + 1, visited, found);
        } else if kind.is_file() {
            let Ok(relative) = path.strip_prefix(base) else {
                continue;
            };
            let Some(relative) = relative_to_uri_path(relative) else {
                continue;
            };
            let size = entry.metadata().map_or(0, |metadata| metadata.len());
            found.push((relative, size));
        }
    }
}

/// True for a real directory, false for a missing path, a file, or a symlink (even one to a directory).
fn is_plain_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// The identity of the file at `target`, taken WITHOUT following a link, or a refusal.
///
/// `target` was resolved a moment ago, but the agent may write under `safe/_drafts` and swap a file
/// for a link to an original while a read is in flight. `fs::metadata` would follow such a link and
/// record the original's identity as if it were the validated file's, so the identity is read with
/// `symlink_metadata`: a link at this point is refused, and a later swap cannot match it.
fn validated_file_identity(target: &Path, uri: &str) -> Result<fs::Metadata, ErrorData> {
    let metadata = fs::symlink_metadata(target).map_err(|_| not_found(uri))?;
    if metadata.file_type().is_symlink() {
        return Err(outside_safe());
    }
    if !metadata.is_file() {
        return Err(ErrorData::invalid_params("not a file", Some(json!({ "uri": uri }))));
    }
    Ok(metadata)
}

/// Open `path` and confirm it is the very file that was validated.
///
/// On Unix the final component is opened with `O_NOFOLLOW`, so a link swapped in after validation
/// fails to open instead of being followed, and with `O_NONBLOCK`, so a FIFO swapped in cannot hang
/// the thread. The opened descriptor's device and inode are then compared with the validated file's,
/// which also catches a directory on the way that was swapped for a link. Windows keeps the static
/// checks only: std exposes no stable file identity there.
fn open_checked(path: &Path, validated: &fs::Metadata) -> std::io::Result<fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let opened = file.metadata()?;
        if opened.dev() != validated.dev() || opened.ino() != validated.ino() {
            return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let _ = validated;
        fs::File::open(path)
    }
}

/// `a/b.md` with forward slashes, or `None` for a name that is not valid UTF-8.
fn relative_to_uri_path(relative: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in relative.components() {
        parts.push(component.as_os_str().to_str()?.to_owned());
    }
    Some(parts.join("/"))
}

fn mime_for(relative: &str) -> &'static str {
    if relative.ends_with(".md") {
        "text/markdown"
    } else {
        "text/plain"
    }
}

/// Read one mirror file as text.
pub(crate) fn read(root: &Path, uri: &str) -> Result<ReadResourceResult, ErrorData> {
    let relative = relative_from_uri(uri)?;
    let base = safe_dir(root);
    // Refuse a linked `safe` before canonicalizing it: the link target would become the permitted base.
    match fs::symlink_metadata(&base) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(outside_safe()),
        Err(_) => return Err(not_found(uri)),
    }
    let base = fs::canonicalize(&base).map_err(|_| not_found(uri))?;
    let target = fs::canonicalize(base.join(&relative)).map_err(|_| not_found(uri))?;
    // The resolved path, not the requested one: this is what catches a symlink that points out.
    if !target.starts_with(&base) {
        return Err(outside_safe());
    }
    let metadata = validated_file_identity(&target, uri)?;
    if metadata.len() > MAX_RESOURCE_BYTES {
        return Err(too_large(uri));
    }
    // Only a missing file is "not found": a refused follow (ELOOP) or an identity mismatch is the
    // path having been re-pointed, and says so.
    let file = open_checked(&target, &metadata).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            not_found(uri)
        } else {
            outside_safe()
        }
    })?;
    // Bounded again at read time: the file may have grown since the metadata call.
    let mut bytes = Vec::new();
    file.take(MAX_RESOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| not_found(uri))?;
    if bytes.len() as u64 > MAX_RESOURCE_BYTES {
        return Err(too_large(uri));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| ErrorData::invalid_params("not valid UTF-8 text", Some(json!({ "uri": uri }))))?;
    let contents = ResourceContents::text(text, uri).with_mime_type(mime_for(&relative));
    Ok(ReadResourceResult::new(vec![contents]))
}

/// The decoded path under `safe/` a URI names, or a refusal. Pure: touches no file.
fn relative_from_uri(uri: &str) -> Result<String, ErrorData> {
    let Some(encoded) = uri.strip_prefix(URI_PREFIX) else {
        return Err(ErrorData::invalid_params(
            "only basemind://safe/ resources are served",
            Some(json!({ "uri": uri })),
        ));
    };
    let decoded = percent_decode(encoded).ok_or_else(outside_safe)?;
    if decoded.is_empty() || decoded.contains('\0') || decoded.contains('\\') {
        return Err(outside_safe());
    }
    for segment in decoded.split('/') {
        // An empty segment is `//` (an absolute path in disguise); `.` and `..` walk upward.
        // On Windows a segment with `:` can name another drive.
        if segment.is_empty() || segment == "." || segment == ".." || (cfg!(windows) && segment.contains(':')) {
            return Err(outside_safe());
        }
    }
    Ok(decoded)
}

fn outside_safe() -> ErrorData {
    ErrorData::invalid_params("path is outside safe/", None)
}

fn not_found(uri: &str) -> ErrorData {
    ErrorData::resource_not_found("resource not found", Some(json!({ "uri": uri })))
}

fn too_large(uri: &str) -> ErrorData {
    ErrorData::invalid_params(
        format!("resource is larger than {MAX_RESOURCE_BYTES} bytes"),
        Some(json!({ "uri": uri })),
    )
}

/// Strict RFC 3986 percent-decoding to UTF-8: a malformed escape or invalid UTF-8 is `None`.
fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_value(*bytes.get(index + 1)?)?;
            let low = hex_value(*bytes.get(index + 2)?)?;
            out.push(high << 4 | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Percent-encode each path segment, keeping `/` as the separator.
fn encode_path(relative: &str) -> String {
    let mut out = String::with_capacity(relative.len() + 8);
    for byte in relative.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_handles_utf8_and_refuses_malformed_input() {
        assert_eq!(
            percent_decode("with%20space%20%C3%A9.md").as_deref(),
            Some("with space é.md")
        );
        assert_eq!(percent_decode("a%2fb").as_deref(), Some("a/b"));
        assert_eq!(percent_decode("bad%zz"), None);
        assert_eq!(percent_decode("cut%2"), None);
        assert_eq!(percent_decode("%ff%fe"), None, "not UTF-8");
    }

    #[test]
    fn encode_then_decode_round_trips_awkward_names() {
        for name in [
            "note.md",
            "sub/deep.md",
            "with space é.md",
            "100%.md",
            "a#b?c.md",
            "ünï/cødé.md",
        ] {
            assert_eq!(percent_decode(&encode_path(name)).as_deref(), Some(name), "{name}");
        }
    }

    #[test]
    fn the_uri_must_stay_under_safe() {
        for uri in [
            "basemind://safe/../a",
            "basemind://safe/a/../../b",
            "basemind://safe/%2e%2e/b",
            "basemind://safe/..%2fb",
            "basemind://safe//etc/passwd",
            "basemind://safe/a\\b",
            "basemind://safe/a%00b",
            "basemind://safe/",
            "basemind://safe",
            "basemind://elsewhere/b",
            "file:///etc/passwd",
        ] {
            assert!(relative_from_uri(uri).is_err(), "{uri} must be refused");
        }
        assert_eq!(relative_from_uri("basemind://safe/a/b.md").unwrap(), "a/b.md");
    }

    #[test]
    fn listing_stops_after_a_bounded_number_of_directories() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("safe");
        // Many directories, almost no files: the file cap alone never trips.
        for index in 0..(MAX_DIRS + 500) {
            fs::create_dir_all(base.join(format!("d{index}"))).unwrap();
        }
        fs::write(base.join("d0/only.md"), "x").unwrap();
        let mut found = Vec::new();
        let mut visited = 0;
        walk(&base, &base, 0, &mut visited, &mut found);
        assert!(
            visited <= MAX_DIRS,
            "visited {visited} directories, budget is {MAX_DIRS}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_safe_directory_that_is_a_link_is_not_served() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("original.txt"), "ORIGINAL-SECRET").unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("safe")).unwrap();
        assert!(
            list(dir.path()).resources.is_empty(),
            "a linked safe/ must list nothing"
        );
        assert!(
            read(dir.path(), "basemind://safe/original.txt").is_err(),
            "a linked safe/ must read nothing"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_file_swapped_for_a_link_after_validation_is_not_opened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inside.md");
        let secret = dir.path().join("outside.txt");
        fs::write(&path, "inside").unwrap();
        fs::write(&secret, "ORIGINAL-SECRET").unwrap();
        let validated = validated_file_identity(&path, "u").unwrap();
        // Between the check and the open, another process replaces the file with a link out.
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&secret, &path).unwrap();
        assert!(
            open_checked(&path, &validated).is_err(),
            "the swapped file must not be opened"
        );
        // Untouched file: the check passes.
        let intact = dir.path().join("intact.md");
        fs::write(&intact, "ok").unwrap();
        assert!(open_checked(&intact, &validated_file_identity(&intact, "u").unwrap()).is_ok());
    }

    /// The sequence from the review: the file is swapped for a link BEFORE its identity is taken, so
    /// an identity read through the link would be the original's and the comparison would pass.
    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_the_file_is_refused_even_if_its_identity_were_read_through_it() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("swapped.md");
        let secret = dir.path().join("outside.txt");
        fs::write(&secret, "ORIGINAL-SECRET").unwrap();
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        // The identity is not taken through a link ...
        assert!(
            validated_file_identity(&link, "u").is_err(),
            "a link must not yield an identity"
        );
        // ... and even the mistake of following it does not let the open through: the open itself
        // refuses to follow a link.
        let followed = fs::metadata(&link).unwrap();
        assert!(
            open_checked(&link, &followed).is_err(),
            "O_NOFOLLOW must refuse to open a link"
        );
    }

    #[cfg(unix)]
    #[test]
    fn something_that_is_not_a_regular_file_has_no_identity() {
        let dir = tempfile::tempdir().unwrap();
        assert!(validated_file_identity(dir.path(), "u").is_err(), "a directory");
        let fifo = dir.path().join("pipe");
        let c_path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        assert!(validated_file_identity(&fifo, "u").is_err(), "a FIFO");
    }

    /// `O_NOFOLLOW` only covers the last path component. A directory on the way swapped for a link
    /// still redirects the open, and only the inode comparison catches it.
    #[cfg(unix)]
    #[test]
    fn a_directory_on_the_way_swapped_for_a_link_is_caught_by_the_identity_check() {
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("a");
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(&inside).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(inside.join("f.md"), "inside").unwrap();
        fs::write(elsewhere.join("f.md"), "ORIGINAL-SECRET").unwrap();
        let path = inside.join("f.md");
        let validated = validated_file_identity(&path, "u").unwrap();
        // The directory is replaced by a link to another one holding a file of the same name.
        fs::remove_dir_all(&inside).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &inside).unwrap();
        let error = open_checked(&path, &validated).expect_err("the redirected open must be refused");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }
}
