//! NER windowing for `redact_text` (#71): GLiNER2's candle backend reads only
//! the encoder's first `max_position_embeddings` tokens — and the schema
//! prompt already eats a large share of that budget — so the tail of any text
//! longer than a few hundred characters was silently dropped while the tool
//! answered `ner_ran: true` as if every character had been read. Names,
//! companies and addresses past the cut were never detected.
//!
//! The planner in this module lets every character reach the encoder:
//!
//! - windows are cut at word boundaries, `WINDOW_CHARS` at most, each with
//!   `OVERLAP_CHARS` carried into the next window, so a mention cut at a
//!   boundary is found whole (again) in the next window;
//! - the token budget — the encoder's real limit (its config's
//!   `max_position_embeddings`) minus the schema prompt counted in the
//!   model's own `tokenizer.json`, minus a margin — shrinks a window before
//!   the character cap does, which is what token-dense text needs (numbers,
//!   e-mail addresses, identifiers tokenize into far more words than prose);
//! - when no window fits the budget at some position, planning stops there
//!   and the caller reports `ner_truncated`; `require_ner` then fails the
//!   call;
//! - mentions overlapping across neighbouring windows merge: same label to
//!   their union, rival labels keep the more confident one (the longer span
//!   on a tie) — over-masking, never a leak.
//!
//! Byte offsets into and out of every function are UTF-8 byte offsets into
//! the caller's own text (the #70 contract). The splitter, schema framing and
//! token counting mirror `xberg-gliner` 1.3.9's candle path exactly; an
//! upstream change to them must be re-mirrored here.

use std::collections::HashMap;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use regex::Regex;
use xberg::types::entity::{Entity, EntityCategory};

/// Mirror of the app-side windowing gate (hacienda-cowork #74): that caller
/// proves coverage only when `ner_window_chars` is at most the window size it
/// windows with itself, so a larger window here would make long documents
/// unmirrored, not more thoroughly read.
pub(crate) const WINDOW_CHARS: usize = 400;
/// Characters the next window carries back, so a mention cut at the boundary
/// is re-read whole.
pub(crate) const OVERLAP_CHARS: usize = 100;
/// Margin between the planned sequence and the encoder's exact limit. Counts
/// are measured from the same artifacts the backend loads, so this only
/// guards replica drift (a lowercase form that changes byte length, added
/// tokens) — it is not an estimate of the prompt.
const TOKENS_MARGIN: usize = 32;
/// The GLiNER2 schema separator standing between the prompt and the text.
const SEP_TEXT_TOKEN: &str = "[SEP_TEXT]";
/// `xberg-gliner`'s exact v2 word pattern (`src/v2/splitter.rs`).
const WORD_SPLIT_PATTERN: &str =
    r"(?i)(?:https?://[^\s]+|www\.[^\s]+)|[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}|@[a-z0-9_]+|\w+(?:[-_]\w+)*|\S";

fn word_regex() -> &'static Regex {
    static WORD_SPLIT: OnceLock<Regex> = OnceLock::new();
    WORD_SPLIT.get_or_init(|| Regex::new(WORD_SPLIT_PATTERN).expect("static pattern must compile"))
}

/// One window handed to the backend: a byte range over the caller's text.
#[derive(Debug)]
pub(crate) struct Window {
    pub start: usize,
    pub end: usize,
}

/// A windowing of the caller's text. `cut_off` is `Some` only when planning
/// had to stop: the byte where the first unplannable position begins, which
/// the caller must turn into a `ner_truncated` claim, never a silent one.
pub(crate) struct WindowPlan {
    pub windows: Vec<Window>,
    pub cut_off: Option<usize>,
}

/// The per-call budget, measured once from the model's own artifacts, so the
/// planner counts with what the backend counts with.
///
/// Relation kept against the encoder: `prompt_tokens + window text + margin ≤
/// encoder limit` — the prompt and the headroom are both reserved here, once,
/// so the planner only has to keep a window's *text* under `tokens_for_text`.
pub(crate) struct NerBudget {
    /// Tokens the schema prompt takes (labels + `[SEP_TEXT]`).
    pub prompt_tokens: usize,
    /// Tokens left for a window's text: encoder limit − prompt − margin.
    pub tokens_for_text: usize,
    /// The model's tokenizer, reloaded here to count with, not re-estimated.
    pub(crate) tokenizer: tokenizers::Tokenizer,
}

/// The encoder's sequence limit, from the same config file the candle backend
/// loads (`config.json`, or the nested `encoder_config/config.json`).
fn max_position_embeddings(model_dir: &Path) -> Result<usize, String> {
    let candidates = [
        model_dir.join("encoder_config").join("config.json"),
        model_dir.join("config.json"),
    ];
    let mut seen_any = false;
    for path in candidates {
        if !path.exists() {
            continue;
        }
        seen_any = true;
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("cannot parse {}: {e}", path.display()))?;
        if let Some(limit) = value.get("max_position_embeddings").and_then(serde_json::Value::as_u64) {
            return Ok(limit as usize);
        }
    }
    if seen_any {
        Err("config.json carries no max_position_embeddings; the window budget is unknown".to_string())
    } else {
        Err("no config.json beside the model; the window budget is unknown".to_string())
    }
}

/// Token count of one backend-style call: `items`, each a pre-split word,
/// encoded as one sequence — exactly how `encode_pretokenized` feeds the
/// tokenizer. Per-item counting makes this the same number the encoder sees,
/// prompt included.
fn encoded_token_count(tokenizer: &tokenizers::Tokenizer, items: &[&str]) -> Result<usize, String> {
    let encoding = tokenizer
        .encode(items, false)
        .map_err(|e| format!("tokenizing the window budget failed: {e}"))?;
    Ok(encoding.get_ids().len())
}

/// The schema-prompt word list, exactly `xberg-gliner`'s `build_schema_tokens`:
/// `( [P] entities ( [E] label … ) )`, one schema token per whitespace attack
/// each label splits into.
pub(crate) fn schema_items(labels: &[String]) -> Vec<String> {
    let mut schema = vec![
        "(".to_string(),
        "[P]".to_string(),
        "entities".to_string(),
        "(".to_string(),
    ];
    for label in labels {
        schema.push("[E]".to_string());
        schema.extend(label.split_whitespace().map(str::to_string));
    }
    schema.push(")".to_string());
    schema.push(")".to_string());
    schema
}

/// Load the budget for `labels` from the model directory: the prompt's token
/// count from the model's own `tokenizer.json` (measured, not estimated), the
/// encoder's limit from its config. Cached per directory and label list; the
/// cache is process-wide like the backend itself.
/// The budget cache: one entry per model dir + label list.
type BudgetsByLabelsKey = Mutex<HashMap<(PathBuf, u64), Arc<NerBudget>>>;

pub(crate) fn load_budget(model_dir: &str, labels: &[String]) -> Result<Arc<NerBudget>, String> {
    static CACHE: OnceLock<BudgetsByLabelsKey> = OnceLock::new();
    let dir = Path::new(model_dir);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for label in labels {
        std::hash::Hash::hash(&label.as_str(), &mut hasher);
    }
    let labels_key = hasher.finish();

    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(budget) = cache
        .lock()
        .map_err(|_| "budget cache poisoned".to_string())?
        .get(&(dir.to_path_buf(), labels_key))
    {
        return Ok(Arc::clone(budget));
    }

    let tokenizer_path = dir.join("tokenizer.json");
    let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
        .map_err(|e| format!("cannot load tokenizer from {}: {e}", tokenizer_path.display()))?;
    let mut prompt_items = schema_items(labels);
    prompt_items.push(SEP_TEXT_TOKEN.to_string());
    let prompt_refs: Vec<&str> = prompt_items.iter().map(String::as_str).collect();
    let prompt_tokens = encoded_token_count(&tokenizer, &prompt_refs)?;
    let max_seq = max_position_embeddings(dir)?;
    if max_seq <= prompt_tokens + TOKENS_MARGIN {
        return Err(format!(
            "encoder limit {max_seq} leaves no room beyond the schema prompt ({prompt_tokens} tokens)"
        ));
    }
    let budget = Arc::new(NerBudget {
        prompt_tokens,
        tokens_for_text: max_seq - prompt_tokens - TOKENS_MARGIN,
        tokenizer,
    });
    cache
        .lock()
        .map_err(|_| "budget cache poisoned".to_string())?
        .insert((dir.to_path_buf(), labels_key), Arc::clone(&budget));
    Ok(budget)
}

/// One piece the text splits into: a whole word with its byte and character
/// extents. Pieces are the only cut points, so no window slice ends inside a
/// word (except an outgrown one, below).
struct Segment {
    b0: usize,
    b1: usize,
    c0: usize,
    c1: usize,
}

/// Split `text` into cut-point segments: whole words normally. A single word
/// longer than `WINDOW_CHARS` (a giant URL, a digit run with no separators)
/// splits inside itself at character boundaries — windowing never needs a
/// special case for it — and text without a single word (pure whitespace)
/// gets plain `WINDOW_CHARS` segments.
fn segments(text: &str) -> Vec<Segment> {
    let regex = word_regex();
    let mut result = Vec::new();
    // Character cursor (absolute char position) and byte cursor, walked in
    // lockstep with the matches so every Segment's char extents are absolute.
    let (mut byte, mut char_at) = (0usize, 0usize);
    for hit in regex.find_iter(text) {
        if hit.start() > byte {
            // Separator characters are not segments but still count toward a
            // window's size, so their chars advance the cursor. `byte` still
            // holds the previous match's end, or 0.
            char_at += text[byte..hit.start()].chars().count();
        }
        let span = &text[hit.start()..hit.end()];
        let c0 = char_at;
        let c1 = c0 + span.chars().count();
        char_at = c1;
        byte = hit.end();
        // One word can outgrow the cap: give it cut points of its own so the
        // windowing walk can pass it.
        if c1 - c0 <= WINDOW_CHARS {
            result.push(Segment {
                b0: hit.start(),
                b1: hit.end(),
                c0,
                c1,
            });
        } else {
            let mut starts = vec![0usize];
            // `step_by` re-emits offset 0 first: the pieces are [0, k), [k,
            // 2k), … each at most `WINDOW_CHARS` characters.
            starts.extend(span.char_indices().step_by(WINDOW_CHARS).map(|(b, _)| b).skip(1));
            for (i, &start) in starts.iter().enumerate() {
                let end = starts.get(i + 1).copied().unwrap_or(span.len());
                result.push(Segment {
                    b0: hit.start() + start,
                    b1: hit.start() + end,
                    c0: c0 + span[..start].chars().count(),
                    c1: c0 + span[..end].chars().count(),
                });
            }
        }
    }
    if result.is_empty() && !text.is_empty() {
        // No words at all (whitespace): cut at char boundaries so the walk
        // still covers the text; the backend reads each window, finds nothing,
        // and every window stays within the cap.
        let (mut byte, mut char_at) = (0usize, 0usize);
        while byte < text.len() {
            // `nth(WINDOW_CHARS)` yields the byte offset where the 401st
            // character starts: exactly `WINDOW_CHARS` characters per piece,
            // on a char boundary; the last piece takes whatever remains.
            let piece_end = text[byte..]
                .char_indices()
                .nth(WINDOW_CHARS)
                .map_or(text.len(), |(offset, _)| byte + offset);
            let piece_chars = text[byte..piece_end].chars().count();
            result.push(Segment {
                b0: byte,
                b1: piece_end,
                c0: char_at,
                c1: char_at + piece_chars,
            });
            char_at += piece_chars;
            byte = piece_end;
        }
    }
    result
}

/// Tokens the backend would spend on one window's text — lowercased and
/// re-split into words, exactly how the candle path feeds its window. Per-word
/// counting is additive, so window text + the prompt (counted once in
/// `load_budget`) is the sequence the encoder can truncate; the planner
/// therefore compares this count directly with `tokens_for_text`, the limit
/// already minus prompt and margin.
fn window_token_count(tokenizer: &tokenizers::Tokenizer, slice: &str) -> Result<usize, String> {
    let lowered = slice.to_lowercase();
    let words: Vec<&str> = word_regex().find_iter(&lowered).map(|hit| hit.as_str()).collect();
    encoded_token_count(tokenizer, &words)
}

/// Plan the windows: walk the segments filling each window to the character
/// cap, shrinking it to the token budget (never below one segment), and
/// carrying `OVERLAP_CHARS` back into the next window. Planning stops (not
/// fails) at the first position where even one segment does not fit the
/// budget: `cut_off` marks it and honest truncation is reported. Fails only
/// when the very first window cannot fit, because then nothing can run.
pub(crate) fn plan_windows(text: &str, budget: &NerBudget) -> Result<WindowPlan, String> {
    let pieces = segments(text);
    let mut windows = Vec::new();
    let mut index = 0usize;
    while index < pieces.len() {
        // Fill to the char cap …
        let mut last = index;
        while last + 1 < pieces.len() && pieces[last + 1].c1 - pieces[index].c0 <= WINDOW_CHARS {
            last += 1;
        }
        // … then to the token budget.
        let slice_at = |end: usize| text.get(pieces[index].b0..pieces[end].b1).unwrap_or("");
        let mut tokens = window_token_count(&budget.tokenizer, slice_at(last))?;
        while tokens > budget.tokens_for_text && last > index {
            last -= 1;
            tokens = window_token_count(&budget.tokenizer, slice_at(last))?;
        }
        if tokens > budget.tokens_for_text {
            return if windows.is_empty() {
                Err("even the first window does not fit the token budget".to_string())
            } else {
                Ok(WindowPlan {
                    windows,
                    cut_off: Some(pieces[index].b0),
                })
            };
        }
        windows.push(Window {
            start: pieces[index].b0,
            end: pieces[last].b1,
        });
        if last + 1 >= pieces.len() {
            break;
        }
        // The next window starts at the first cut point at or after
        // `OVERLAP_CHARS` back from this window's end (snapping forward keeps
        // whole words in the overlap), or, when no pair fits, right after it.
        let target = pieces[last].c1.saturating_sub(OVERLAP_CHARS);
        let mut next = index + 1;
        while next <= last && pieces[next].c0 < target {
            next += 1;
        }
        index = next;
    }
    Ok(WindowPlan { windows, cut_off: None })
}

/// One mention together with the index of the window that found it. `None`
/// once a span merges pieces of more than one window and can no longer be
/// attributed to one.
pub(crate) type WindowMention = (Entity, Option<usize>);

/// Merge the mentions every window found. Overlapping windows re-read the
/// overlap, so the same span arrives once per window that saw it, and rival
/// labels from different windows can claim one text. Rules, per the ticket:
///
/// - same label overlapping (or touching) spans merge to their union — over
///   the caller's own text, so the redaction pattern covers the whole mention,
///   never a leak — carrying the parts' best confidence;
/// - same-label neighbours whose gap holds no word characters (a mention the
///   windows saw in pieces) merge too — but only across windows: a different
///   window re-read the same region and its piece earns the join, while gaps
///   inside one window are separate detections, not one mention's pieces;
/// - rival labels that overlap are decided by confidence: the winner keeps its
///   label and confidence and its span extends to the union of everything
///   overlapping it, so whichever label loses, no span piece is left outside
///   the redaction portions.
pub(crate) fn merge_window_entities(text: &str, mentions: Vec<WindowMention>) -> Vec<Entity> {
    // Union within a label first: windows finding the same or overlapping
    // mentions under one label become one mention each.
    let mut per_label: HashMap<String, Vec<WindowMention>> = HashMap::new();
    for mention in mentions {
        per_label
            .entry(join_key(&mention.0.category))
            .or_default()
            .push(mention);
    }
    let mut merged: Vec<WindowMention> = Vec::new();
    for mut group in per_label.into_values() {
        group.sort_by(|a, b| {
            a.0.start
                .cmp(&b.0.start)
                .then(b.0.end.cmp(&a.0.end))
                .then(prefer_confident(&a.0, &b.0))
        });
        let mut simplified: Vec<WindowMention> = Vec::with_capacity(group.len());
        for (entity, window) in group {
            match simplified
                // only the last can touch the new candidate: kept spans are
                // disjoint and sorted
                .last_mut()
            {
                // Same label: union (touching spans merge; so do neighbours
                // across windows whose gap holds no word characters — a
                // mention the windows saw in pieces must not leak the pieces
                // between them).
                Some((kept, kept_window))
                    if entity.start <= kept.end
                        || (kept_window != &window
                            && text
                                .get(kept.end as usize..entity.start as usize)
                                .is_some_and(|gap| !gap.chars().any(|c| c.is_alphanumeric()))) =>
                {
                    *kept_window = if *kept_window == window { *kept_window } else { None };
                    if entity.end > kept.end {
                        // Entity offsets are `u32` (xberg's type); the union
                        // surface is read back from the caller's own text.
                        let (start, end) = (kept.start as usize, entity.end as usize);
                        kept.end = entity.end;
                        if let Some(union_surface) = text.get(start..end) {
                            let union = union_surface.trim().to_string();
                            if !union.is_empty() {
                                kept.text = union;
                            }
                        }
                    }
                    if confidence(kept) < confidence(&entity) {
                        kept.confidence = entity.confidence;
                    }
                }
                _ => simplified.push((entity, window)),
            }
        }
        merged.extend(simplified);
    }
    // Rival labels fight only on real overlap — and by confidence: the winner
    // states the label and confidence of the most confident span in the clash,
    // and its span extends over the union so no overlapping piece drops out of
    // the redaction (sorting earliest start first keeps the union's left edge
    // decided before a later candidate arrives).
    merged.sort_by(|a, b| {
        a.0.start
            .cmp(&b.0.start)
            .then(b.0.end.cmp(&a.0.end))
            .then(prefer_confident(&a.0, &b.0))
            .then(join_key(&a.0.category).cmp(&join_key(&b.0.category)))
    });
    let mut kept: Vec<WindowMention> = Vec::new();
    for mention in merged {
        let overlapping: Vec<usize> = kept
            .iter()
            .enumerate()
            .filter(|(_, (span, _))| mention.0.start < span.end && span.start < mention.0.end)
            .map(|(index, _)| index)
            .collect();
        if overlapping.is_empty() {
            kept.push(mention);
            continue;
        }
        let mut winners: Vec<WindowMention> = Vec::with_capacity(overlapping.len() + 1);
        let mut survivors: Vec<WindowMention> = Vec::new();
        for (index, entry) in kept.drain(..).enumerate() {
            if overlapping.contains(&index) {
                winners.push(entry);
            } else {
                survivors.push(entry);
            }
        }
        winners.push(mention);
        let (union_start, union_end) = (
            winners
                .iter()
                .map(|(span, _)| span.start)
                .min()
                .expect("candidate in the clash"),
            winners
                .iter()
                .map(|(span, _)| span.end)
                .max()
                .expect("candidate in the clash"),
        );
        let best = winners
            .iter()
            .enumerate()
            .min_by(|a, b| prefer_winner(&a.1.0, &b.1.0))
            .map(|(index, _)| index)
            .expect("candidate in the clash");
        let (mut winner, mut winner_window) = winners.swap_remove(best);
        winner.start = union_start;
        winner.end = union_end;
        if let Some(union_surface) = text.get(union_start as usize..union_end as usize) {
            let union = union_surface.trim().to_string();
            if !union.is_empty() {
                winner.text = union;
            }
        }
        for (piece, window) in winners {
            winner_window = if winner_window == window { winner_window } else { None };
            if confidence(&winner) < confidence(&piece) {
                winner.confidence = piece.confidence;
            }
        }
        survivors.push((winner, winner_window));
        survivors.sort_by(|a, b| {
            a.0.start
                .cmp(&b.0.start)
                .then(b.0.end.cmp(&a.0.end))
                .then(prefer_confident(&a.0, &b.0))
        });
        kept = survivors;
    }
    kept.into_iter().map(|(entity, _)| entity).collect()
}

/// Ordering key of a category: names a label group without relying on Debug
/// formatting of `EntityCategory`.
fn join_key(category: &EntityCategory) -> String {
    match category {
        EntityCategory::Custom(label) => format!("custom:{label}"),
        other => format!("{other:?}"),
    }
}

/// The span to trust first when two spans tie on extent.
fn prefer_confident(a: &Entity, b: &Entity) -> std::cmp::Ordering {
    b.confidence
        .partial_cmp(&a.confidence)
        .unwrap_or(std::cmp::Ordering::Equal)
}

/// Which rival wins one overlapping clash: the more confident; longer on a
/// tie; the label string for determinism when both tie.
fn prefer_winner(a: &Entity, b: &Entity) -> std::cmp::Ordering {
    prefer_confident(a, b)
        .then((b.end - b.start).cmp(&(a.end - a.start)))
        .then(join_key(&a.category).cmp(&join_key(&b.category)))
}

fn confidence(entity: &Entity) -> f32 {
    // Entities carry a probability from the model; the 0 default only ever
    // matters for entities a test built unset.
    entity.confidence.unwrap_or(0.0)
}

#[cfg(test)]
#[path = "ner_windowing_tests.rs"]
mod tests;
