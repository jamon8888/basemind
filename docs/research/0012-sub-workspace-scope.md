# Research 0012 — Is a scope finer than the repository expressible today?

Resolves wayfinder ticket [#51](https://github.com/jamon8888/basemind/issues/51).

**This is a feasibility report, not a decision.** Which grain to adopt, and who fixes it, belongs
to the integration ([hacienda-cowork #15](https://github.com/jamon8888/hacienda-cowork/issues/15),
child #18). Everything below is read off the code at `main` = `873a357`, plus the draft spec branch
`spec/lexical-document-retrieval` = `e5ecb8d`.

---

## Answer in one paragraph

A sub-repository scope is **almost** expressible today, and the code already contains a
generalisation nobody named: `doc_scope_for` (`src/scanner_docs.rs:647-669`) derives a *per-file*
scope `path:<canonical-extra-root>` for documents outside the repo. That is a second-suffix scope,
computed by path prefix, sitting in the same `scope` column as the repo scope and the `web:<host>`
scope. Nothing in the schema, the SQL predicates, or the Fjall key encoders cares what a scope
string *means* — they are opaque, length-prefixed equality keys. The reason Workstation's split is
"not expressible" is narrower than the ticket states: **the read path and the code lane have no
per-file or per-subtree scope override at all**, and the one write-side hook that exists
(`doc_scope_for`) is gated on `is_external_key`, i.e. absolute paths, so an in-repo `safe/` can
never reach it. Adding a subtree form is therefore a small change to one function plus a
scope-selecting parameter on the read side — not a new partitioning scheme. What it would *not*
give for free is a lexical/FTS lane (none exists yet), and it would need a decision about whether
`<repo>#safe` is a sibling scope or a child of `<repo>` (the two behave differently for every
prefix scan in the code).

---

## 1. The existing mechanism: `web:<host>` is a convention with a helper, not a registry

### There is no constant, no validation, no registry

The whole of `web:<host>` is one `format!` in one function:

- `src/web/ingest.rs:127-130` — `default_scope(url) = format!("web:{host}")`, with
  `url.host_str().unwrap_or("unknown")` as the only fallback.

Everything else is prose. Confirmed absences (each grepped across `src/`, `crates/`, `tests/`,
`benches/`, `schema/`, `docs/`, `website/`):

- **No constant.** `"web:"` appears as a literal only in that `format!`, in doc comments
  (`src/mcp/helpers_web.rs:78`, `src/mcp/memory.rs:481`, `src/mcp/types_web.rs:29,54,88`,
  `src/mcp/tools_memory.rs:40`, `src/mcp/types_memory.rs:147`), in `website/src/content/docs/capabilities/web-crawl.mdx:105`,
  and in tests. The other `"web:*"` literals in `src/mcp/tasks.rs:35-39` and
  `src/mcp/savings.rs:140` are **tool names** (`web:scrape`, `web:crawl`, `web:map`), a different
  namespace that happens to share the prefix.
- **No validation.** No length bound, no charset check, no `web:` prefix assertion on any caller.
  Grepping `scope.len()` / `scope.chars()` / `validate_scope` returns only `Vec::with_capacity`
  arithmetic in the key encoders. A caller-supplied scope is passed straight to
  `format!("scope = '{}'", escape_sql_literal(scope))` (`src/lance/mod.rs:338`), and the only
  sanitisation in the entire path is `escape_sql_literal`, which doubles single quotes and does
  nothing else (`src/lance/mod.rs:836-838`). Scope strings are caller-controlled SQL literal
  content today.
- **No registry of known scopes.** No `distinct scope` query, no enumeration, no allow-list
  anywhere. Nothing can list the scopes present in a LanceDB table.

### The convention is documented, and it is load-bearing

Documented in three places, consistently: `src/mcp/types_web.rs:29-31` ("LanceDB `scope` tag;
defaults to `\"web:<host>\"`. Override to share a scope across many hosts or to namespace per
project"), `src/mcp/types_documents.rs:28-34`, and the website
(`website/src/content/docs/capabilities/web-crawl.mdx:29,51,105`). The website already tells users
to pass an arbitrary `scope` to `web scrape` (`document-search.mdx:71-74` shows `"scope": "docs"`),
so free-form caller scopes are an advertised feature, not an accident.

### The convention was born from a bug, which is why it is loose

`resolve_doc_scope` (`src/mcp/memory.rs:486-488`) returns the requested value verbatim:

```rust
fn resolve_doc_scope(requested: Option<&str>, repo_scope: &str) -> String {
    requested.unwrap_or(repo_scope).to_string()
}
```

`git log -S resolve_doc_scope` points at `e8bbfb3`, *"fix(documents): let search_documents reach
the scope web pages are ingested under"*. Its message states the design intent plainly: the lanes
disagreed, scraped pages were permanently invisible (an empty, error-free ~14 ms answer), and the
fix was to let the caller name the scope rather than to unify the lanes. Verbatim passthrough is
the whole fix. #40 later recorded, on the spec branch (`e5ecb8d`, `docs/specs/0012-lexical-document-retrieval.md`
§8.2), that this same passthrough "also lets a caller name another repository's scope", and
constrained the future behaviour to "the repository's own scope or one of its own `web:<host>`
siblings". **That constraint is spec-only; it is not in `main`'s code.**

### Verdict for §1

`web:<host>` is a documented convention implemented as one `format!` string, with no constant, no
validation, no registry, and a documented intent to keep accepting arbitrary caller overrides.
The write side is fully general. The read side is general too, but only for whoever knows the
string.

---

## 2. Feasibility of a sub-repo scope

### 2a. Every scope comparison is opaque string equality — a suffix works

All nine SQL predicates compare `scope` for equality against a value supplied by the caller:

| Site | Predicate |
|---|---|
| `src/lance/mod.rs:257-259` | `replace_document` — `scope = '{}' AND path = '{}'` (delete-then-insert) |
| `src/lance/mod.rs:338` | `search_documents` — `scope = '{}'` via `only_if` |
| `src/lance/mod.rs:413-415` | `replace_code_chunks` — `scope = '{}' AND path = '{}'` |
| `src/lance/mod.rs:448-450` | `delete_code_chunks` — `scope = '{}' AND path = '{}'` |
| `src/lance/mod.rs:483` | `search_code_chunks` — `scope = '{}'` via `only_if` |
| `src/lance/mod.rs:844-851` | `memory_namespace_predicate` — `scope = '{}' AND visibility = '{}' AND agent_id = '{}'` |
| `src/lance/mod.rs:856-861` | `memory_row_predicate` — the above `AND key = '{}'` |
| `src/lance/doc_links.rs:43-45` | `replace_doc_links` — `scope = '{}' AND doc_path = '{}'` |
| `src/lance/doc_links.rs:77` | `all_doc_links` — `scope = '{}'` via `only_if` |

None parses, splits, prefixes-matches, or orders the scope. `<repo>#safe` and `<repo>/safe` behave
identically to `github.com/Foo/bar`.

The Fjall side is equally opaque, and *length-prefixed*, which is the property that makes suffixing
safe: `memory_by_key` (`src/index/keys_governance.rs:33-42`), `memory_by_key_ns_prefix` (`:47-55`),
`memory_scope_prefix` (`:61-66`), `proposal_by_id` (`:124-131`), `proposal_ns_prefix` (`:134-140`),
and the PII trio (`src/index/keys_pii.rs:14-21, 26-32, 37-42`) all write
`u16:len(scope) ‖ scope ‖ NUL …`. The comment at `keys_governance.rs:58-60` states the consequence
explicitly: "Because `scope` is length-prefixed, this prefix bounds exactly one scope's keys (a
longer scope encodes a different `u16` length, so no spillover)."

**That cuts both ways and is the single most important fact for a suffix design.** `<repo>` and
`<repo>#safe` are *disjoint* namespaces, not nested. So:

- A search for `<repo>#safe` returns only `safe/` rows. Good.
- A search for `<repo>` returns **zero** `safe/` rows. So the "exact search unchanged on originals"
  half of #18 works for free — but so does "semantic search on the whole workspace", which is the
  thing #18 wants to prevent. Nothing in the current code would union a parent with its children.
  A subtree query needs `scope = '<repo>' OR scope LIKE '<repo>#safe%'`, and **no such code exists**.

### 2b. The write-side hook that already generalises the idea

`doc_scope_for` (`src/scanner_docs.rs:647-669`) is a per-file scope resolver, called from
`src/scanner_file.rs:387`:

```rust
pub(crate) fn doc_scope_for<'a>(rel: &str, default_scope: &'a str, config: &Config)
    -> Cow<'a, str>
{
    if !crate::path::is_external_key(rel.as_bytes()) {
        return Cow::Borrowed(default_scope);          // repo-relative → repo scope
    }
    for raw_root in &config.scan.extra_roots { … if rel.starts_with(prefix) {
        return Cow::Owned(format!("path:{prefix}"));  // external root → path:<root>
    }}
    Cow::Owned(format!("path:{rel}"))
}
```

This is a scope finer than the repository, derived by path prefix, living in the same column. Its
two tests are at `src/scanner_docs_unit_tests.rs:402-421`.

**Why `safe/` cannot reach it:** the first branch returns the repo scope for any non-external key.
`is_external_key` (`src/path.rs:278`) tests for an *absolute* path; `safe/rapport.pdf.md` is
repo-relative, so it takes the early return at `scanner_docs.rs:653`. The predicate that decides
sub-scoping is "outside the repo", not "under a configured sub-root". A subtree form would be a
third branch keyed on a configured prefix — the loop body is already written, only the guard moves.

**This hook is also the concrete evidence that finer-than-repo scopes were intended and are
supported.** Its doc comment (`scanner_docs.rs:642-646`) says external-root docs are scoped
`path:<extra_root>` "so they group under the out-of-repo tree they came from", and asserts
"Retrieval is unaffected — `search_documents` has no scope filter". **That last clause is stale.**
`search_documents` *does* filter on scope (`src/lance/mod.rs:338`), so external-root documents are
written under `path:<root>` and are unreachable from the repo scope. A prior fix moved the read side
to honour a caller scope (`e8bbfb3`) but never reconciled the write side. There is also a live
inconsistency at the flush: `build_doc_rows` stamps each row with `batch.doc_scope`
(`scanner_docs.rs:624`) while `replace_document` is called with the **scan-wide** `scope`
(`scanner_docs.rs:628`), so the delete half of the delete-then-insert targets the repo scope while
the insert half writes `path:<root>` — stale rows accumulate for external roots on re-scan. Both
facts are pre-existing bugs, not blockers for a suffix, but they are the shape of the problem a
subtree scope inherits, so any design should fix them together rather than route around them.

### 2c. Sites that would need to understand the suffix

Grouped by what breaks.

**A. Read-path scope selection — no override exists today (the real blocker).**

| Site | Today | Needed for a subtree scope |
|---|---|---|
| `src/mcp/memory.rs:522` | `resolve_doc_scope(params.scope, &state.shared.scope)` — caller may name any string | must resolve `<repo>#safe` to the stored value; the passthrough at `:487` already permits it, so **documents RAG needs no change at all** |
| `src/mcp/helpers_code_search.rs:347` | `let scope = state.shared.scope.clone()` — hardcoded | the `code` lane has **no `scope` parameter** (`SearchCodeParams`, `src/mcp/types_code.rs:167-195`, has no scope field). Needs a new param |
| `src/mcp/memory.rs:173, 231, 240, 281, 350, 381, 414, 471` | `state.shared.scope` hardcoded | memory `put`/`get`/`list`/`search`/`delete` — all pinned to the repo scope |
| `src/mcp/helpers_proposals.rs:351, 428, 492, 539, 578, 616` | `state.shared.scope` | proposals |
| `src/mcp/helpers_governance.rs:488, 494, 498, 541` | `state.shared.scope` | governance / audit |
| `src/mcp/doc_links_cache.rs:50` | `lance.all_doc_links(scope)` with the repo scope | doc↔code links for a `safe/` scope would not load into the graph lane |
| `src/scanner_docs.rs:538-539`, `src/scanner_code.rs:333` | stale-delete by scope | must resolve the same subtree scope, or deletes miss |

**B. Prefix scans that would silently miss children.**

- `audit_scope_persist` (`src/mcp/helpers_governance.rs:553`) → `memory_scope_prefix(scope)`. With a
  length prefix, this returns `<repo>`'s records and *nothing* under `<repo>#safe`. The audit's own
  comment calls the scope filter "a correctness guard", so the silent miss has teeth.
- `memory_by_key_ns_prefix` (`src/index/keys_governance.rs:47`), used by `list_core`
  (`src/mcp/memory_ops.rs:242`) and `proposals_ops.rs:218`.
- `proposal_ns_prefix` (`src/index/keys_governance.rs:134`), used by `src/mcp/proposals_ops.rs:58`.
- `pii_lineage_scope_prefix` (`src/index/keys_pii.rs:37`).

**C. Prefix-based *interpretation* of a scope string — the one place a separator already matters.**

- `src/comms/scope.rs:31-33` and `src/comms/client.rs:874-876` both do
  `if key.starts_with("path:") { None } else { Some(key) }` to decide whether a scope counts as a
  *remote* for thread discovery. A `path:`-prefixed scope is dropped; a `web:`- or bare one is kept.
  A `<repo>#safe` scope derived from a `path:` repo would inherit the drop (correctly, arguably); a
  `<repo>/safe` scope derived from a remote would be kept (also arguably correct). The behaviour is
  incidental either way and there is no test pinning it.

**D. Lock keying — already suffix-safe, but the cap interacts.**

`memory_put_lock` (`src/mcp/memory.rs:129-136`) keys on
`(scope: String, vis_byte: u8, owner: String, key: String)` and holds the registry in an `LruCache`
capped at `MEMORY_PUT_LOCK_CAP = 4096` (`src/mcp/memory.rs:120`). Finer scopes multiply *distinct
lock keys* for the same logical key set. The doc comment (`:109-118`) states eviction is
correctness-safe but that a rare-eviction race window opens once more than 4096 distinct keys are
written between two racing puts on one key. Two scopes per workspace roughly doubles the key space
against a fixed 4096 budget — real, bounded, and worth knowing, though 4096 is generous relative to
a lawyer's memory count.

**E. `visibility` / `agent_id` keying — no interaction.** `namespace()`
(`src/mcp/memory.rs:153-160`) and `lance_visibility` (`:141-146`) key on visibility and agent id
only; the scope is an independent axis in both the Fjall key and the SQL predicate
(`keys_governance.rs:26-31`, `lance/mod.rs:844-851`). Nothing to change.

**F. Rehydration passphrase — derived from the scope.**

`derive_rehydration_passphrase` (`src/scanner_docs.rs:377-385`) hashes `scope` together with the
data dir into the vault key. Changing a document's scope changes its passphrase, so its existing
rehydration map (`rehydration_ref`, `src/lance/schema.rs:31`) would no longer decrypt. Any scope
re-assignment is therefore a re-encryption event for redacted documents, not a pure reindex.

**G. Config schema — would need a surface, and `MemoryConfig` is currently inert.**

`MemoryScopeStrategy` (`src/config/v1.rs:360-368`) has exactly the two variants named in the ticket
and derives only the repo-level key (`src/git/remote.rs:26-31`, used by
`src/mcp/shared_state.rs:217-219` and `src/scanner.rs:740-747`).

Worth flagging because it affects any plan that leans on config: **no code reads
`config.memory.scope_strategy`.** `MemoryConfig` (`src/config/v1.rs:329-339`) is constructed at
`:456` and defaulted at `:348-356`, and grepping `config.memory`, `.memory.enabled`, and
`scope_strategy` across `src/`, `crates/`, `tests/`, `benches/` returns only those definitions. The
field is published in `schema/basemind-config-v1.schema.json:1080-1084` and documented as working
at `website/src/content/docs/reference/configuration.mdx:195`, but setting `workdir_only` today
changes nothing — `scope_key` is called unconditionally and always prefers the remote. Same for
`default_visibility` (`:339`): unread. So `MemoryScopeStrategy` is a *documented but unenforced*
enum, and adding a suffix mechanism to it would be extending dead code. I could not establish
whether this is deliberate (a placeholder for a planned feature) or an oversight; nothing in the
repo says either way.

### 2d. Verdict for §2

**Yes, a suffixed scope works with the rest of the code as-is**, on three conditions:

1. **It is a sibling, not a child.** Length-prefixed Fjall keys and equality SQL make
   `<repo>#safe` fully disjoint from `<repo>`. No code unions them. Subtree queries need new code.
2. **The caller can name it on the read side.** `documents` already can (`resolve_doc_scope`
   passthrough). `code` cannot — `SearchCodeParams` has no `scope` field and
   `helpers_code_search.rs:347` hardcodes `state.shared.scope`. **This is the single largest gap**,
   and it matters because of §4.
3. **The scanner writes it.** `doc_scope_for` needs a subtree branch; today its guard is
   "absolute path", which `safe/…` never satisfies.

Separator choice (`#` vs `/`) is behaviourally inert in every site in §2a except the `path:`
`starts_with` checks at `comms/scope.rs:32` and `comms/client.rs:875`, where neither `web:` nor a
remote-scope prefix appears. I could not find any site where `#` and `/` would differ.

---

## 3. The cost of finer grain

The ticket asks what finer scopes cost in FTS indexes, vectors, and locks. Measured against the code,
**most of that cost is not per-scope at all** — which is the useful finding.

### 3a. FTS indexes: there are none, and none per scope

Zero `create_index`, zero `Index::` builder calls, zero FTS APIs anywhere in `src/lance/`. Grepping
for index-creation APIs across the whole crate returns nothing but `store.index` /
`store.index_db` / `GitHistoryIndex` / `InRamIndex`, all unrelated.

So **finer scopes create no additional FTS indexes, because the document table has no FTS index to
begin with.** The document lane is vector-only: `search_documents` (`src/lance/mod.rs:314-350`) does
`table.vector_search(query).limit(limit).only_if(scope_clause)` and nothing else. This is
independently confirmed by the spec branch's own PR #34 body ("la voie documentaire est
vector-only") and by the open PR #46 ("stop promising keyword search when embed = false"). The only
FTS in the codebase is `git_history/fts.rs`, a hand-rolled Fjall posting index over commit messages,
keyed by nothing scope-shaped.

The one keyword lane that exists for *code* — `bm25_search` (`src/search/bm25.rs:137`) over the
Fjall `code_bm25_postings` keyspace — is keyed `(term, chunk_id)` with **no scope component**
(`src/index/keys.rs:394-404`) and its corpus stats are stored corpus-global
(`src/index/mod.rs:53-57`: `code_bm25_n`, `code_bm25_total_len`). It is already un-scoped, so scope
finer-ness neither helps nor hurts it. **A future FTS lane, if ADR-0012 is accepted, is where the
per-scope FTS cost would land — and it does not exist to be measured yet.** I cannot estimate that
cost; it depends on an unbuilt table.

### 3b. Vectors: one shared vector index, filter-only

This is the decisive point. LanceDB store and tables are created **per workspace**, not per scope:

- `LanceStore::open(dir)` (`src/lance/mod.rs:187-234`) opens one connection and calls
  `ensure_table` for `documents`, `memory`, `code_chunks`, `doc_links` — four fixed tables.
- `dir` is `<workspace_cache>/lance` (`src/store.rs:410-419`, `src/store_layout.rs:54, 101-106`),
  keyed by the worktree root (`workspace_key` = blake3 of the canonicalised root,
  `src/store_layout.rs:93-96`).
- `ensure_table` (`src/lance/mod.rs:525-536`) is idempotent on table *name*; nothing is
  partitioned by scope.

So `scope` is a **column value**, and the scope filter is a `only_if` predicate over one shared
vector index. Splitting `<repo>` into `<repo>#safe` **does not create a second index, a second
table, or a second LanceDB directory.** The rows are the same rows; only the predicate changes.

Measurable consequences:

- **Storage: unchanged.** Same chunks, same vectors, same `dim`. A file indexed once under
  `<repo>#safe` occupies exactly what it occupied under `<repo>`. There is no duplication penalty
  *unless* the integration chooses to index originals **and** mirrors — and #18 explicitly does not
  (code is never embedded; `safe/` mirrors are `.md` and the finding in
  `docs/ner-pii-session-findings.md` is that they are indexed *as code*, not as documents).
- **Query latency: unchanged in kind, changed in selectivity.** A KNN over a flat table with a
  post-filter returns the same number of candidate rows regardless of how the predicate splits them.
  Narrowing the predicate can only *reduce* returned hits for a given `limit` — the risk is recall
  dilution (fewer than `limit` qualifying rows among the top-k), which is a ranking problem, not a
  throughput one. Without a scalar index on `scope`, LanceDB cannot pre-filter efficiently; the
  predicate is evaluated against candidates.
- **Row count: unchanged.** Finer scopes add no rows.
- **The one real storage cost is vectors that a finer scope *permits*.** #18's rule is "embed
  `safe/` only, never originals". If the current (buggy, §2b) behaviour were left alone, originals
  outside `safe/` would already be embedded under the repo scope — the split saves that embedding
  cost rather than adding any. `max_chunks_per_document` defaults to 2000
  (`src/config/documents.rs:131-133`), so per-document vector cost is bounded regardless.

### 3c. Locks: one bounded LRU, and finer scopes only widen its key space

Covered in §2d/4. The only bounded resource keyed on scope is
`MEMORY_PUT_LOCK_CAP = 4096` (`src/mcp/memory.rs:120`), over
`(scope, vis_byte, owner, key)`. Doubling scopes per workspace doubles distinct lock keys for the
same logical memories. The comment at `src/mcp/memory.rs:109-118` documents that eviction is
correctness-safe, with a narrow race window past the cap. For a lawyer's matter — tens to low
hundreds of memory keys across a handful of matters — 4096 is not a constraint. I could not
establish the intended per-workspace memory-key volume, so this stays a qualitative bound rather
than a measured one.

### 3d. Is a lawyer's matter a workable number of sub-spaces?

The evidence says **the question is mis-framed: a lawyer's matter is not a sub-space of a workspace,
it is the workspace.**

- `safe/` is defined per workspace, not per matter: `isSafeWorkspace` tests
  `existsSync(join(workspacePath, SAFE_DIR_NAME))` (`hacienda-cowork server/utils/safeWorkspace.ts:32-35`),
  and `getSafeRoots` derives `safeRoot` from the single workspace path (`:22-26`).
- The mirror is flat and mirrors the workspace tree verbatim: `toSafeMirrorPath` returns
  `` `safe/${posix}.md` `` (`hacienda-cowork server/utils/safeSync.ts:49-51`).
- `DOSSIER.md` is one file at the workspace root, described as carrying "le contexte du dossier"
  (the matter's context) — singular per workspace (`hacienda-cowork
  docs/superpowers/specs/2026-09-22-basemind-safe-integration-spec.md:126`, implementation at
  `safeWorkspace.ts:76, 88-124`).
- Cabinet mode, the other per-folder safety mechanism, is likewise workspace-keyed and explicitly
  **not** per-workspace configurable: scope is device-level "not per workspace in v1"
  (`hacienda-cowork docs/superpowers/specs/2026-09-29-cabinet-mode-spec.md:30`, decision 2).

So the live design has **one `safe/` per workspace = one matter per workspace**, and the scope split
#18 needs is a fixed **2** (repo + `safe/`), not a variable number. At N=2 the costs in §3 are
negligible: no new index, no new table, no new rows, a 2× widening of a 4096-entry lock LRU.

I could **not** establish whether Workstation intends one workspace to hold *several* matters. If it
ever does, `safe/` would need a per-matter subdirectory, which turns N=2 into N=1+*matters* — still
cheap for the reasons above, but it would make "subtree scope" a first-class requirement rather
than a one-off, and that changes the design weight from "add a branch to `doc_scope_for`" to
"define scope derivation as a function of `(repo, path)`". **That is the one input that would change
my answer, and it belongs to the integration.**

---

## 4. What Workstation actually needs

Read at [hacienda-cowork #18](https://github.com/jamon8888/hacienda-cowork/issues/18) (closed) and its
resolution comment, plus the parent map #15 and the repo's own specs.

### The decision, verbatim

Decision 4 of the resolution: *"**Search lanes (split):** Semantic / conversation RAG →
`<workspace>/safe/` strictly. Exact / filename search (`useFileSearch`, ripgrep `search-content`) →
unchanged, hits originals; code remains findable this way, never embedded."* "Explicitly out of v1"
includes "Funneling exact search through `safe/`" and "Active embedding/indexing of code".

The consolidated spec restates it as decision 24 (§6 "Couture A — RAG scopé sur `safe/`"):
"Conversation / `search_documents` / outils : `root = <workspace>/safe/`, jamais les originaux."

### It is *not* "two scopes per workspace"

Two findings push against that reading.

**First, the split is already implemented — as a path filter, not a scope.** The workspace-side
filter is `keepMirrorHitsInSafeWorkspace`
(`hacienda-cowork server/tools/builtin-tools/workstation/workspaceSearchTool.ts:30-37`), applied to
hits at `:111`, keyed on `isInsideSafeMirror` (`safeWorkspace.ts:70-74`). basemind returns hits for
the whole workspace; Workstation drops any hit whose `path` is not under `safe/`. The comment at
`:25-29` states the reason: "a hit on an original names a file the agent cannot open and whose line
numbers do not match its mirror, so it is dropped."

And the repo's own findings doc records that this is the *load-bearing* mechanism:
`hacienda-cowork docs/ner-pii-session-findings.md:252-256` — "An unscoped `code` search returns hits
on **both** `safe/contracts/lease.md.md` and the original `contracts/lease.md`. … That filter is
load-bearing — basemind indexes the whole workspace." Same doc, `:258-260`: "Safe workspaces are
searched through the `code` tool … **not** `memory documents`. Mirrors are `.md` and are indexed as
code, so LanceDB document RAG is not part of the Safe search path."

**Second, that means the operative lane is `code`, not `documents`.** `basemindSearchCode` →
`code` mode, which is `search_code` — whose semantic lane hardcodes `state.shared.scope`
(`src/mcp/helpers_code_search.rs:347`) and whose params have **no `scope` field**
(`src/mcp/types_code.rs:167-195`). So the exact lane #18 wants to leave untouched *is* the lane a
sub-repo scope would have to be expressible in. Documents RAG (the lane that already accepts a
caller scope) is, per the findings doc, not on the Safe path at all.

### So what does #18 actually require?

Precisely: **a way to make the semantic lane of `search_code` return `safe/` rows and not
originals**, with the `code`-lane scope becoming caller-selectable rather than server-fixed. That
is:

- `search_code` gains a `scope` (or scope-derivation) parameter. It has none today.
- The scanner writes `safe/` files under a scope distinct from the repo's — the subtree branch in
  `doc_scope_for`, which does not exist.
- Whatever reads those rows (`helpers_code_search.rs:347`) resolves the same scope.

Everything #18 lists as out of v1 stays unaffected, because exact/filename search never touches
LanceDB — it is `useFileSearch` / ripgrep in Workstation, not a basemind lane.

**The narrower reading of "not expressible today" is therefore correct and the ticket's framing is
too broad.** `documents` already accepts an arbitrary caller scope, so a sub-repo scope is
*expressible* there today with no basemind change. The gap is concentrated in one lane
(`search_code`'s semantic + keyword lanes), one function (`doc_scope_for`), and one derived value
(the scanner's scope), plus the read-side scope resolution in `helpers_code_search.rs`. It is a
real gap. It is not a partitioning redesign.

---

## 5. What I could not establish

1. **Whether `MemoryScopeStrategy` being unread is deliberate.** `config.memory.scope_strategy`
   (and `enabled`, and `default_visibility`) have no reader anywhere in `src/`, `crates/`, `tests/`,
   `benches/` — verified by exhaustive grep. The schema
   (`schema/basemind-config-v1.schema.json:1080-1084`) and the website
   (`configuration.mdx:195`) both advertise `workdir_only` as functional. No comment, ADR, or issue
   in the repo explains the gap. If a suffix mechanism were added to that enum it would be
   extending unread code.
2. **Per-scope FTS cost.** Unmeasurable — the document table has no FTS index (§3a). If ADR-0012 is
   accepted, its spec (`docs/specs/0012-lexical-document-retrieval.md` on
   `spec/lexical-document-retrieval`) is the place to cost this; I did not read its FTS sizing
   sections in full, only the §6 and §8.2 amendments from `e5ecb8d`.
3. **Whether the stale `doc_scope_for` comment and the flush-scope mismatch (§2b) are known bugs.**
   Both are readable from the code; neither is referenced by an issue, an ADR, or a test that would
   fail. The mismatch at `scanner_docs.rs:624` vs `:628` in particular looks like a real stale-row
   leak for external roots and appears untested.
4. **Whether Workstation ever needs >1 matter per workspace** (§3d). The current design is one
   matter per workspace. This is the input that would move the answer.
5. **Whether #40's planned constraint on `resolve_doc_scope` ("the repository's own scope or one of
   its own `web:<host>` siblings", `e5ecb8d` §8.2) is compatible with a subtree scope.** Read
   literally it is **not** — it would forbid `<repo>#safe` as a caller-named scope, since that is
   neither the repo scope nor a `web:` sibling. Nobody has reconciled the two decisions; #40 is
   closed, #51 is open, and both touch `resolve_doc_scope`. This conflict is worth surfacing
   regardless of what grain is chosen.
6. **The actual embedding dim and bytes/vector in `main`.** `src/embeddings.rs:16-24` resolves dims
   from `xberg::embeddings::EMBEDDING_PRESETS` (a dependency not vendored in this checkout), and the
   default preset is `"multilingual"` (`src/config/documents.rs:122-124`). The schema's own
   description text says `multilingual` is "multilingual-e5-base, 768-dim"
   (`schema/basemind-config-v1.schema.json:437`), but that string also claims the default is
   `balanced` while the code default is `multilingual` — a stale description. I did not run a build
   to confirm 768, and per-vector cost is moot anyway under §3b (no per-scope index to duplicate).

---

## 6. Feasibility summary

| Question | Answer |
|---|---|
| Is `web:<host>` a constant, a convention, or an accident? | A **documented convention**, one `format!` at `src/web/ingest.rs:129`, with no constant, no validation, no registry. Free-form caller overrides are an advertised feature. |
| Would `<repo>#safe` work with the rest of the code as-is? | **Yes for storage and predicates** — every scope comparison is opaque equality and every Fjall key is length-prefixed. **No for reads** — `search_code` has no scope parameter (`helpers_code_search.rs:347`, `types_code.rs:167-195`), and the scanner has no subtree branch (`scanner_docs.rs:652-653`). |
| Sibling or child? | **Sibling.** Length-prefixed keys make `<repo>` and `<repo>#safe` disjoint, so `<repo>#safe%` is not matched by `<repo>`'s prefix scan and no code unions them. A subtree query is new code, not a reinterpretation. |
| Cost of finer grain | **Near zero at N=2.** Scope is a column in one shared per-workspace table (`lance/mod.rs:208-214`, `store.rs:410-419`): no new index, no new table, no new rows. No FTS index exists to multiply (`src/lance/`). Only bounded cost is a wider `MEMORY_PUT_LOCK_CAP = 4096` LRU key space (`memory.rs:120`). |
| What does #18 need? | A caller-selectable scope on the **`code`** lane (`search_code`), which is the lane Safe actually uses (`ner-pii-session-findings.md:258-260`) — not a second scope per workspace in the abstract. `documents` already accepts an arbitrary scope. |
| Blocking? | Not for design; **yes** for #18 as written, and it must be reconciled with #40's already-decided constraint on `resolve_doc_scope`, which as written forbids a subtree scope. |

**Not a decision, restated:** the grain, the separator, and who derives the scope belong to the
integration. This report establishes that the mechanism exists, what it would touch, and what it
would cost — not which to pick.