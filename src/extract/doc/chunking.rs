//! Chunk construction helpers shared by the document extraction pipeline.

use std::borrow::Cow;
use std::fmt::Write;

use super::DocChunk;

pub(crate) fn dense_retrieval_text<'a>(
    content: &'a str,
    heading_context: Option<&xberg::types::HeadingContext>,
) -> Cow<'a, str> {
    match heading_context {
        Some(context) if !context.headings.is_empty() => {
            Cow::Owned(xberg::chunking::render_heading_breadcrumb(content, context))
        }
        _ => Cow::Borrowed(content),
    }
}

pub(super) fn heading_path_of(heading_context: Option<&xberg::types::HeadingContext>) -> String {
    let Some(context) = heading_context.filter(|c| !c.headings.is_empty()) else {
        return String::new();
    };
    let mut out = String::new();
    for (i, heading) in context.headings.iter().enumerate() {
        if i > 0 {
            out.push_str(" > ");
        }
        for _ in 0..heading.level {
            out.push('#');
        }
        let _ = write!(out, " {}", heading.text);
    }
    out
}

pub(super) fn prepare_doc_chunk(
    content: String,
    byte_start: usize,
    byte_end: usize,
    heading_context: Option<&xberg::types::HeadingContext>,
    embed_requested: bool,
) -> (DocChunk, Option<String>) {
    let heading_path = heading_path_of(heading_context);
    let dense_input = embed_requested.then(|| dense_retrieval_text(&content, heading_context).into_owned());
    let chunk = DocChunk {
        byte_start: u32::try_from(byte_start).unwrap_or(u32::MAX),
        byte_end: u32::try_from(byte_end).unwrap_or(u32::MAX),
        text: content,
        embedding: Vec::new(),
        heading_path,
        cites: String::new(),
    };
    (chunk, dense_input)
}
