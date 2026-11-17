# ADR-0014: Tier routing is declared by the caller, not inferred by basemind

- **Status:** Proposed
- **Date:** 2026-10-07
- **Deciders:** jamon8888 (proposer and decider)
- **Related:** ADR-0012 (multi-lane document retrieval), tickets #64 and
  [hacienda-cowork#73](https://github.com/jamon8888/hacienda-cowork/issues/73)

## Context

basemind has two retrieval tiers over one scanner. `documents` stores chunked prose with byte
spans and, after ADR-0012, lexical, exact and phrase lanes. `code` stores tree-sitter chunks with
symbol kinds and line ranges, and already fuses vector, BM25 and exact-symbol lanes by RRF
(`src/mcp/helpers_code_search.rs:167-174`). Which tier a file lands in is decided by one line:

```rust
// src/scanner_file.rs:198-209
let lang = match lang::detect(Path::new(rel)) {
    Some(l) => l,
    None => {
        #[cfg(feature = "documents")]
        {
            if matches!(source, ScanSource::WorkingTree) {
                return process_doc(root, rel, filters, store, config, scope, embed);
            }
        }
        return FileResult::bare(rel.to_string(), FileStatus::SkippedNoLang);
    }
};
```

The documents pipeline is reached **only** when language detection fails. `process_doc` has exactly
one call site in the tree, and it is this one.

That is a workable rule for a codebase, and it is what the code tier relies on: markdown is in
`OVERRIDE_LANGUAGES` (`src/lang.rs:62`) with a hand-written query, so `README.md` and Obsidian
vaults are indexed as code, and `tests/scan_smoke.rs:988`
`markdown_headings_and_obsidian_references_are_indexed` asserts headings resolve as
`SymbolKind::Heading` and `[[wikilinks]]` become graph callees. That test is the guarantee, and
this ADR does not weaken it.

It stops working as soon as a file is prose *that happens to be markdown*. One integration writes a
redacted legal corpus to `safe/<original-name>.md` — every file ends in `.md` by construction — and
every pleading, contract and memo is therefore chunked by tree-sitter as source. Its search results
come back as `symbol: Heading | lang: markdown` with line numbers, for text that has no functions
and no classes.

Two things make this a decision rather than a bug report.

**It is not fixable inside basemind alone, and it is not fixable by a basemind default.** Making
markdown default to the documents tier would break Obsidian vaults and READMEs, which work today.
Making it configurable inside basemind still requires basemind to know what the caller means, and
the caller is the only party that knows.

**The obvious knob is inert.** `[languages]` is a `BTreeMap<String, LanguageConfig>` with an
`enabled` bool (`src/config/v1.rs:26, 308-311`) and **no reader anywhere in `src/`**. An operator
disabling `markdown` would reasonably expect rerouting and would get nothing. This is the same
class of defect as `MemoryScopeStrategy`, recorded in spec 0012 §6.2: a config key that loads,
changes nothing, and says nothing.

## Decision

**The caller declares which paths belong to the documents tier. basemind does not infer it, does
not default it, and does not hardcode any integration's directory name.**

The declaration is basemind configuration, read per workspace root (`src/config/mod.rs:67-76`), so
it survives a rescan. The `admin rescan` call carries no declaration field
(`src/mcp/types_admin.rs:24-34`) and does not need one.

Two rejections, recorded so they are not relitigated:

- **Markdown defaults to documents.** Rejected: breaks the code tier's markdown handling, which
  `markdown_headings_and_obsidian_references_are_indexed` proves works.
- **The caller changes the file extension to something undetected.** Rejected: it makes a display
  convention load-bearing for retrieval, and the mirror path is user-visible.

## Consequences

Routing becomes basemind configuration rather than an emergent property of file extensions, which
means basemind gains a key it cannot validate on its own: the caller can name a prefix that does
not exist, and nothing in basemind will say so.

The declaration must be honoured on **all three** scan sources. `src/scanner_file.rs:198-209` gates
`process_doc` on `matches!(source, ScanSource::WorkingTree)`, so an implementation placed only on
that branch silently does nothing for `Staged` and `Rev` scans — pre-commit and git-source. A
declaration that appears to work and does not is the same failure class as the inert
`[languages]` key.

Moving a file between tiers must purge the rows it left behind. One direction happens to work today
by accident: a doc-routed file returns `DocIndexed`, which is not inserted into `seen`
(`src/scanner_drive.rs:371-373`), so the path stays in `stale` and its code chunks are purged. The
reverse does not: `scanner_drive.rs:376-382` inserts into `doc_seen` for code-tier `Updated`
results, so `doc_stale` never sees the path and its document rows are never purged. An incremental
`scan_paths` scan purges nothing on a tier flip at all (`src/scanner.rs:605-614`).

Routing `safe/` to the documents tier costs the mirror its code-tier structure — `SymbolKind::Heading`
and `[[wikilink]]` resolution — in exchange for `heading_path`, byte spans, and ADR-0012's lanes. For
legal prose that cross-references between filings, that trade is not settled by this ADR and is left
to the integration.

**The caller's search surface is its own responsibility.** basemind serving a document tier does not
mean the caller can read it. One integration's agent reaches the corpus through a single tool that
queries `code_chunks` exclusively (`helpers_code_search.rs:342-348`), so routing its files to the
documents tier removes every result rather than changing their shape. A caller must provide a
documents-tier read path **before** declaring a prefix, or the declaration is a silent outage.

`heading_path` does not exist yet — it lands in ADR-0012's Phase 2 — and adding it changes the Arrow
column set, which is gated by `MEMORY_SCHEMA_VER = RELEASE_MINOR` (`src/lance/mod.rs:56-60`) and so
wipes and rebuilds the whole LanceDB directory for every existing workspace. That cost lands on the
deployer, not on the caller who asked for routing.

## Alternatives considered

- **Make `resolve_doc_scope` constrain documents to the repository and its `web:<host>` siblings.**
  Rejected here, and worth recording why: the write side deliberately supports arbitrary scopes
  (`src/mcp/types_web.rs:29-30`, `src/mcp/helpers_web.rs:74-75`, *"honour it verbatim"*).
  Constraining reads alone makes every custom-named web scope **written but permanently unreadable**
  — the exact invisible-data failure that `resolve_doc_scope`'s own comment
  (`src/mcp/memory.rs:480-484`) exists to prevent. It needs both sides in view and is a separate
  decision.
- **Wire up `[languages]` as the routing knob.** Rejected: it has no reader, its shape is a
  per-language map rather than a path rule, and reusing a key that looks like it works would
  reproduce the `MemoryScopeStrategy` defect.
