//! The #71 windowing, in units: planning covers the whole text with bounded
//! windows and overlap; the token budget shrinks windows for dense text and
//! truncates honestly when nothing fits; merged mentions union under one
//! label and keep the more confident rival.

use xberg::types::entity::{Entity, EntityCategory};

use super::{NerBudget, SEP_TEXT_TOKEN, WINDOW_CHARS, merge_window_entities, plan_windows, schema_items};

/// A tokenizer that costs one token per character: a BPE whose vocabulary is
/// the printable ASCII set with no merges — every word tokenizes to its own
/// characters, so budget arithmetic is exact and test text stays ASCII. (The
/// merge tests below need no tokenizer at all.)
fn per_char_tokenizer() -> tokenizers::Tokenizer {
    let mut vocab: ahash::AHashMap<String, u32> = ahash::AHashMap::new();
    vocab.insert("[UNK]".to_string(), 0);
    let mut id = 1u32;
    for byte in b' '..=b'~' {
        vocab.insert((byte as char).to_string(), id);
        id += 1;
    }
    let model = tokenizers::models::bpe::BPE::builder()
        .vocab_and_merges(vocab, Vec::new())
        .unk_token("[UNK]".to_string())
        .build()
        .expect("per-char BPE builds");
    tokenizers::Tokenizer::new(model)
}

/// Budget builder for a per-character tokenizer: the prompt's token cost is
/// its own character count, and `headroom` is the character budget a
/// window's text gets — the production relation is `tokens_for_text =
/// encoder limit − prompt − margin`, so a window must fit inside `headroom`
/// on its own.
fn budget_with_text_chars(headroom: usize) -> NerBudget {
    let prompt_items = schema_items(&["person".to_string(), "location".to_string()]);
    let prompt_chars: usize = prompt_items.iter().map(String::len).sum::<usize>() + SEP_TEXT_TOKEN.len();
    NerBudget {
        prompt_tokens: prompt_chars,
        tokens_for_text: headroom,
        tokenizer: per_char_tokenizer(),
    }
}

fn entity(category: EntityCategory, text: &str, start: u32, end: u32, confidence: Option<f32>) -> Entity {
    Entity {
        category,
        text: text.to_string(),
        start,
        end,
        confidence,
    }
}

/// A plain ~20 000-character prose body plans into more than one window; every
/// window stays within the caller's gate (`WINDOW_CHARS` characters); each
/// neighbour re-reads the tail instead of skipping it; nothing is truncated.
#[test]
fn plans_prose_into_bounded_overlapping_windows() {
    let text = "Alice Smith met Bob Jones in Paris near Acme Corp. ".repeat(400);
    let plan = plan_windows(&text, &budget_with_text_chars(usize::MAX / 2)).expect("plans a plain body");
    assert!(
        plan.windows.len() > 10,
        "many windows for ~20k chars: {}",
        plan.windows.len()
    );
    for window in &plan.windows {
        let count = text
            .get(window.start..window.end)
            .expect("window cut on char boundaries")
            .chars()
            .count();
        // The caller's gate reads window sizes in characters.
        assert!(
            count <= WINDOW_CHARS,
            "window {window:?} is {count} chars, past the cap"
        );
    }
    for pair in plan.windows.windows(2) {
        assert!(
            pair[1].start < pair[0].end,
            "neighbours must re-read the tail rather than skip: {pair:?}"
        );
        assert!(pair[1].start > pair[0].start, "each window must move on: {pair:?}");
    }
    assert_eq!(plan.cut_off, None, "a budget this generous keeps every window");
}

/// A person name at the ticket's offsets (500 / 700 / 5 000 / 19 000 of a
/// 20 000-character body) sits inside a window that reaches it.
#[test]
fn planning_reaches_the_far_offsets() {
    let filler = "Alice met Bob Jones near Acme in the lower town. ";
    let text = filler.repeat(420);
    let plan = plan_windows(&text, &budget_with_text_chars(usize::MAX / 2)).expect("plans a plain body");
    let read_to = plan.windows.iter().map(|window| window.end).max().unwrap_or(0);
    for offset in [500, 700, 5_000, 19_000] {
        assert!(
            offset < read_to,
            "offset {offset} must be read, range stops at {read_to}"
        );
    }
}

/// Token-dense text (the ticket's dense case: digit runs, e-mail addresses,
/// identifiers) costs far more tokens per character than prose. With a real
/// tokenizer the character cap alone would read past the encoder; the budget
/// shrinks the windows before the cap does.
#[test]
fn dense_text_shrinks_windows_to_the_token_budget() {
    // Fifteen-token words: the 400-character cap would read ~26 of them past
    // the budget; the budget allows two.
    let dense = "0123456789abcde ".repeat(200);
    let plan = plan_windows(&dense, &budget_with_text_chars(31)).expect("dense text still plans");
    assert!(plan.windows.len() > 20, "budget-shrunk windows: {}", plan.windows.len());
    for window in &plan.windows {
        let slice = dense
            .get(window.start..window.end)
            .expect("window cut on char boundaries");
        assert!(
            slice.split_whitespace().count() <= 2,
            "window {window:?} reads {slice:?} past the budget"
        );
        // …and stays an honest report about the characters it does read.
        assert!(slice.chars().count() <= WINDOW_CHARS);
    }
    assert_eq!(plan.cut_off, None);
}

/// The forced-truncation case: a budget that one overgrown word blows leaves
/// the windows before it runnable, and marks where reading would have to
/// stop, so the caller can claim only partial coverage — `ner_truncated`,
/// never a silent cut.
#[test]
fn a_budget_that_cannot_cover_a_window_stops_with_cut_off() {
    // A 900-character URL: one splitter match, so one piece over the char cap
    // splits it — and a 400-character piece costs ~400 tokens the budget
    // cannot pay.
    let mut text = "Alice Smith met Bob Jones in Paris near Acme Corp. ".repeat(30);
    text.push_str("https://very-long-unreadable.example.org/");
    text.push_str(&"a".repeat(500));
    text.push_str(" and the end");
    let plan = plan_windows(&text, &budget_with_text_chars(320)).expect("the prose head still plans");
    let cut = plan
        .cut_off
        .expect("a 400-character piece cannot fit: planning stops, not fails");
    assert!(
        plan.windows.iter().all(|window| window.start < cut),
        "windows stay before {cut}"
    );
    assert!(
        plan.windows.len() > 1,
        "the prose head was planned: {}",
        plan.windows.len()
    );
}

/// A budget the very first window cannot fit fails outright: nothing can run,
/// and the caller reports NER as not-run (pattern-only, or refusal).
#[test]
fn a_hopeless_budget_fails_planning() {
    let text = "Alice Smith met Bob Jones in Paris near Acme Corp.";
    let plan = plan_windows(&text, &budget_with_text_chars(1));
    assert!(plan.is_err(), "no window can run on a hopeless budget");
}

/// Duplicates from the overlap re-read: the same mention found in two windows
/// collapses to one, keeping the more confident duplicate.
#[test]
fn overlapping_windows_merge_their_duplicates() {
    let entities = vec![
        (
            entity(EntityCategory::Person, "Alice Smith", 0, 11, Some(0.62)),
            Some(0),
        ),
        (
            entity(EntityCategory::Person, "Alice Smith", 0, 11, Some(0.97)),
            Some(1),
        ),
        (
            entity(EntityCategory::Organization, "Acme Corp", 31, 40, Some(0.9)),
            Some(1),
        ),
    ];
    let kept = merge_window_entities("Alice Smith met Bob Jones near Acme Corp", entities);
    assert_eq!(kept.len(), 2, "the duplicate collapses into its twin: {kept:?}");
    assert_eq!(kept[0].text, "Alice Smith");
    assert_eq!(kept[0].confidence, Some(0.97), "the more confident duplicate wins");
    assert_eq!(kept[1].text, "Acme Corp");
}

/// Rival labels claim one span; the more confident one keeps it — the longer
/// span on a tie — so one mention redacts once under one label.
#[test]
fn rival_labels_keep_the_more_confident_claim() {
    let entities = vec![
        (entity(EntityCategory::Location, "Smith", 6, 11, Some(0.6)), Some(0)),
        (
            entity(EntityCategory::Person, "Alice Smith", 0, 11, Some(0.97)),
            Some(0),
        ),
        (entity(EntityCategory::Organization, "Acme", 31, 35, Some(0.9)), Some(0)),
    ];
    let kept = merge_window_entities("Alice Smith met Bob Jones and Acme", entities);
    assert_eq!(kept.len(), 2, "{kept:?}");
    assert_eq!(kept[0].start, 0);
    assert_eq!(kept[0].end, 11);
    assert_eq!(kept[0].confidence, Some(0.97));
    assert_eq!(kept[1].text, "Acme");
}

/// The ticket's leak case, in miniature: the lower-confidence rival sorts
/// first, but the more confident one wins the clash — and its span extends
/// over the union, so no overlapping piece is left outside the redaction
/// ("Acme Corp Jones" under Person; "Jones" can no longer leak).
#[test]
fn a_low_confidence_rival_starting_first_extends_the_confident_winner() {
    let entities = vec![
        (
            entity(EntityCategory::Organization, "Acme Corp", 0, 9, Some(0.5)),
            Some(0),
        ),
        (entity(EntityCategory::Person, "Corp Jones", 5, 15, Some(0.99)), Some(0)),
    ];
    let kept = merge_window_entities("Acme Corp Jones signs", entities);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].start, 0);
    assert_eq!(kept[0].end, 15);
    assert_eq!(kept[0].text, "Acme Corp Jones");
    assert_eq!(kept[0].confidence, Some(0.99), "the more confident rival wins");
}

/// Same-label pieces the different windows saw separately union to one
/// mention spanning both: the second window re-read the region between them,
/// so the join is earned — the windowed saw a split mention, and its pieces
/// must not leak the characters in between.
#[test]
fn same_label_pieces_from_different_windows_union_leaking_nothing() {
    let entities = vec![
        (entity(EntityCategory::Location, "Rue", 11, 14, Some(0.7)), Some(0)),
        (
            entity(EntityCategory::Location, "de la Paix", 15, 25, Some(0.8)),
            Some(1),
        ),
    ];
    let kept = merge_window_entities("date at 12 Rue de la Paix, Paris", entities);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].start, 11);
    assert_eq!(kept[0].end, 25);
    assert_eq!(kept[0].text, "Rue de la Paix");
    assert_eq!(kept[0].confidence, Some(0.8), "the union carries the best confidence");
}

/// Inside one window the same gap rule would weld *unrelated* detections
/// ("Rue" and "de la Paix" are mention pieces only because the windows re-read
/// the region): one window's neighbours separated only by separator characters
/// stay separate spans.
#[test]
fn same_window_neighbours_keep_their_own_spans() {
    let entities = vec![
        (entity(EntityCategory::Location, "Rue", 11, 14, Some(0.7)), Some(0)),
        (
            entity(EntityCategory::Location, "de la Paix", 15, 25, Some(0.8)),
            Some(0),
        ),
    ];
    let kept = merge_window_entities("date at 12 Rue de la Paix, Paris", entities);
    assert_eq!(kept.len(), 2, "{kept:?}");
    assert_eq!(kept[0].text, "Rue");
    assert_eq!(kept[1].text, "de la Paix");
}

/// A partial-overlap mention from another window extends the kept span to the
/// union: the tail it claims must be read (and redacted) whole.
#[test]
fn partial_ranges_union_over_the_callers_text() {
    let entities = vec![
        (entity(EntityCategory::Person, "Dubreuil", 13, 21, Some(0.7)), Some(0)),
        (
            entity(EntityCategory::Person, "Hélène Dubreuil Martin", 5, 28, Some(0.9)),
            Some(1),
        ),
    ];
    let kept = merge_window_entities("Pour Hélène Dubreuil Martin, un token", entities);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].start, 5);
    assert_eq!(kept[0].end, 28);
    assert_eq!(kept[0].text, "Hélène Dubreuil Martin");
    assert_eq!(kept[0].confidence, Some(0.9));
}
