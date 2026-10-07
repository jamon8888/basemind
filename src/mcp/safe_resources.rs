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

fn safe_dir(root: &Path) -> PathBuf {
    root.join("safe")
}

/// Everything under `safe/` the agent may read, in a stable order. Symlinks are never listed or
/// followed: a mirror holds regular files, and a link is how a path would point out of it.
pub(crate) fn list(root: &Path) -> ListResourcesResult {
    let base = safe_dir(root);
    let mut found: Vec<(String, u64)> = Vec::new();
    walk(&base, &base, 0, &mut found);
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

fn walk(base: &Path, dir: &Path, depth: usize, found: &mut Vec<(String, u64)>) {
    if depth > MAX_DEPTH || found.len() >= MAX_LISTED {
        return;
    }
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
            walk(base, &path, depth + 1, found);
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
    let base = fs::canonicalize(&base).map_err(|_| not_found(uri))?;
    let target = fs::canonicalize(base.join(&relative)).map_err(|_| not_found(uri))?;
    // The resolved path, not the requested one: this is what catches a symlink that points out.
    if !target.starts_with(&base) {
        return Err(outside_safe());
    }
    let metadata = fs::metadata(&target).map_err(|_| not_found(uri))?;
    if !metadata.is_file() {
        return Err(ErrorData::invalid_params("not a file", Some(json!({ "uri": uri }))));
    }
    if metadata.len() > MAX_RESOURCE_BYTES {
        return Err(too_large(uri));
    }
    let file = fs::File::open(&target).map_err(|_| not_found(uri))?;
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
}
