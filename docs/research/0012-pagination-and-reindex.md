# 0012 — What an index rebuild does to pagination

Research for [issue #50](https://github.com/jamon8888/basemind/issues/50), sub-ticket of the
wayfinder map #35. Answers the *how* behind spec 0012 §8 line 225–226:

> Pagination: existing `next_cursor` semantics carry `(lane ranks, last row id)`; cursors must be
> invalidated by any index rebuild (see `with_row_id`).

Versions under discussion: `lancedb 0.37.1` (pinned `Cargo.toml:170`, `Cargo.lock:8243-8246`),
which resolves `lance 10.0.0` (`Cargo.lock:7658-7660`). Spec text read from
`origin/spec/lexical-document-retrieval` (`docs/specs/0012-lexical-document-retrieval.md`), not from
`main`, because §8 lives on that branch.

---

## Direct answer

**No. There is no cursor you can mint today that survives a basemind reindex, and there is no
Lance-provided identifier that would make one possible.**

Two independent reasons, either of which is sufficient:

1. **`_rowid` from `with_row_id()` is the row *address*, not a stable identity, on any table
   basemind can produce today.** On a dataset without stable row IDs enabled — the default —
   `_rowid` *is* `(fragment_id << 32) | local_row_offset`, the physical location. Compaction
   rewrites locations, so the same logical row gets a different `_rowid`. Spec
   <https://lance.org/format/table/row_id_lineage>: *"Row addresses change when data is
   reorganized through compaction or updates."* Spec §7.3 mandates `optimize(OptimizeAction::All)`
   after every ingest batch, which compacts. A cursor carrying `_rowid` therefore breaks on the
   next ingest, not only on a reindex.
2. **A basemind "reindex" is not an index drop. It is `remove_dir_all` on the whole Lance
   directory followed by a re-ingest** (`src/lance/mod.rs:513-521`). Every row is destroyed and
   re-created. This is a stronger failure than row-id instability: a fresh table restarts
   `next_row_id` from zero, so re-minted ids **collide** with pre-wipe ids while pointing at
   different chunks. A cursor that carried even a genuinely stable row ID across a basemind
   reindex would silently resume into the wrong filing — which is exactly the §7.3 hazard
   (*"for legal work a silently missing filing is a wrong answer"*, spec line 184) turned inside
   out, into a silently *wrong* filing.

The applicability rule that follows is in [§4 below](#4-the-applicability-rule). It does not rest
on any of the three §14 triggers individually; it rests on the single existing seam that already
implements all of them.

---

## 1. `with_row_id` — what it actually guarantees

### 1.1 It is not deprecated

`with_row_id` is a required method of `lancedb::query::QueryBase` in 0.37.1:

- <https://docs.rs/lancedb/0.37.1/lancedb/query/trait.QueryBase.html> — listed among the 13
  required methods, doc text: *"Return the `_rowid` meta column from the Table."*
- Source `lancedb-0.37.1/src/query.rs:509-510` — declaration, no `#[deprecated]` attribute.
- Source `lancedb-0.37.1/src/query.rs:597-600` — the blanket impl, `self.mut_query().with_row_id = true`.
- `grep -n "deprecated" lancedb-0.37.1/src/query.rs` → **no hits**. The trait carries no
  deprecation notice on docs.rs either.

**Nothing replaces it, and it is not on a removal path.** Any claim in the spec that `with_row_id`
is deprecated would be an invention; it is not.

### 1.2 What it returns is dataset-config-dependent, and the config defaults to the useless case

`lance` distinguishes two identifiers and 0.37.1 gives you no way to tell which one you got:

- **Row address** — `row_address = (fragment_id << 32) | local_row_offset`, the physical location.
  *"Row addresses change when data is reorganized through compaction or updates."* Secondary
  indices reference rows by row address. (<https://lance.org/format/table/row_id_lineage>)
- **Row ID** — a logical identifier, stable across compaction, *only when the dataset opted in*.

The opt-in is off by default. `lance-10.0.0/src/dataset/write.rs:429`:

```rust
enable_stable_row_ids: false,   // in WriteParams::default()
```

And the API's own doc for the flag, `lance-10.0.0/src/dataset/write.rs:330-334`:

> Experimental: if set to true, the writer will use stable row ids. These row ids are stable after
> compaction operations, **but not after updates**. This makes compaction more efficient, since with
> stable row ids no secondary indices need to be updated to point to new row ids.

The spec page is blunter about the default: *"When disabled (default mode), it is exactly equal to
the row address"* and *"When stable row IDs are disabled, the `_rowid` column (if requested) is not
stable and should not be used as a persistent identifier."*

Corroborated in lance's own code. `lance-10.0.0/src/dataset/rowids.rs:158-163`:

```rust
/// Returns one entry per input address, in order. ...
/// On datasets without stable row ids, addresses are the row ids, so the input is returned unchanged.
pub async fn row_addrs_to_row_ids(...) {
    if !dataset.manifest.uses_stable_row_ids() {
        return Ok(addrs);
    }
```

### 1.3 Compaction is what breaks the address, and it is mandatory here

Indices store row addresses, so compaction must remap them. `lance-10.0.0/src/dataset/optimize.rs:2032-2034`:

```rust
let has_address_style = completed_tasks.iter().any(|t| t.row_addrs.is_some());
// Address-style results require immediate index remapping unless it is deferred.
let needs_remapping =
    !dataset.manifest.uses_stable_row_ids() && !options.defer_index_remap && has_address_style;
```

and `optimize.rs:1625-1631`:

```rust
// Capturing row addresses is only useful if something will consume them:
// an index to remap now, or a deferred remap through the FRI.
let capture_row_addrs = !dataset.manifest.uses_stable_row_ids()
    && (options.defer_index_remap || load_indices_for_remapping(dataset.as_ref()).await?.is_some());
```

Remapping indices after compaction is only necessary because their stored row addresses moved.
With stable row IDs it is skipped entirely (`optimize.rs:1742-1743` re-enables the flag on the
compaction write), and lance asserts the end-to-end property in
`lance-10.0.0/src/dataset/optimize.rs:3186-3282` (`test_stable_row_indices`): create scalar + IVF
indices, delete 110 rows, compact, then assert the index UUID set is unchanged and both a vector
and a scalar query return byte-identical results.

So: **addresses change under compaction, ids do not — if you paid for stable ids.**

### 1.4 A pure index rebuild does *not* by itself move addresses

Creating or dropping an index writes no data files. This matters for §3 below: the tokenizer
trigger is the *only* one of the three that is not already backed by a table wipe, and it is
precisely the one that leaves row addresses alone.

### 1.5 Two API sharp edges the spec should not walk into

- **No migration path in the pinned version.** The format spec says stable row IDs may be added to
  an existing dataset via `Dataset::migrate_to_stable_row_ids`
  (<https://lance.org/format/table/row_id_lineage>). **That method does not exist in
  `lance 10.0.0`.** `grep -rn "migrate_to_stable_row_ids" lance-10.0.0/ lancedb-0.37.1/` → no
  hits. Stable row IDs can only be chosen **at dataset creation**, through
  `WriteParams::enable_stable_row_ids` — reachable in 0.37.1 via
  `WriteOptions { lance_write_params: Some(WriteParams { .. }) }` (`lancedb-0.37.1/src/table.rs:293-303`,
  consumed at `lancedb-0.37.1/src/database/namespace.rs:466-471`) or the storage-option route
  `OPT_NEW_TABLE_ENABLE_STABLE_ROW_IDS` that lancedb's own tests use
  (`lancedb-0.37.1/src/table/optimize.rs:570`). Consequence: **an existing `documents` table can
  never acquire stable row IDs**; only a brand-new table — which `documents_v2` (§6) is — could.
- **`with_row_id` is not universally honoured.** `lancedb-0.37.1/src/table/query/lsm.rs:160-161`:

  ```rust
  if query.base.with_row_id {
      return unsupported("with_row_id (the LSM scanner exposes _rowaddr, not a stable _rowid)");
  }
  ```

  If basemind ever moves a table to MemWAL/LSM, `with_row_id()` becomes a hard error rather than a
  degraded answer. The unambiguous alternative is `lance::dataset::Scanner::with_row_address()`
  (`lance-10.0.0/src/dataset/scanner.rs:1812-1817`), which always means the address — but that is
  precisely the identifier with no stability guarantee. **There is no third option.**

---

## 2. What the repo does today

### 2.1 The documents path has no cursor at all

This is the first finding and it invalidates the premise of §8 line 225.

- `src/mcp/types_documents.rs:87-101` — `SearchDocumentsResponse { query, budgeted, hits,
  elapsed_us }`. **No `next_cursor` field.**
- `src/mcp/types_documents.rs:89-90`, on the `budgeted` flag:

  > `/// True when a `max_tokens` budget dropped trailing `hits`. `search_documents` has no`
  > `/// cursor; raise `max_tokens` (or omit it) to retrieve more hits.`

- `SearchDocumentsParams` (`src/mcp/types_documents.rs:10-62`) has no `cursor` field either.
- `run_search_documents` (`src/mcp/memory.rs:491-530`) threads `limit`, `mime`, `scope` and
  nothing else into `lance.search_documents`, then trims to `limit` and reranks. The only
  truncation signal is `budgeted`.
- `LanceStore::search_documents` (`src/lance/mod.rs:315-340`) is `table.vector_search(query)
  .limit(limit)` plus `only_if`. No `_rowid` is ever selected; a plain `vector_search` returns no
  row-id column.
- `grep -rn "row_id\|_rowid" --include=*.rs src/ crates/` → **no hits anywhere in the repo.**
  `grep -rn "with_rowid\|with_row_id"` over `*.rs`, `*.md`, `*.toml` → **no hits either.** The
  symbol `with_row_id` appears in this spec and nowhere in the codebase.

So there is no existing documents cursor to invalidate, and no existing `(lane ranks, last row id)`
encoding to preserve. The spec's *"existing `next_cursor` semantics"* is describing the `memory` /
code / git tiers, not the documents tier.

### 2.2 The cursors that do exist carry nothing Lance-shaped

`src/mcp/cursor.rs:1-21` states the whole design, and neither branch mentions a row id:

- **Fjall-backed** (`memory_list`, `find_references`, `find_callers`) — *"cursors hold the raw
  last-seen key bytes… Stable across rescans because Fjall keys are content-addressed."*
- **In-memory** (`search_symbols`, `list_files`, git tools) — *"`{ offset, snapshot_id }` pair…
  `snapshot_id` is whatever the calling tool uses to detect that its view changed between pages —
  the source-index tools use the `cache_generation` AtomicU32, the git-iterator tools derive it from
  the first 4 bytes of the HEAD sha."* On mismatch the response carries `cursor_invalidated = true`.

Concretely, `src/mcp/memory_ops.rs:205-300` (`list_core`): `ListQuery.cursor` is
`Option<&[u8]>` documented as *"Raw Fjall resume-key bytes from a previous page"*; the scan is a
range scan resumed with `Bound::Excluded(k.to_vec())` (`memory_ops.rs:244-247`); `next_cursor` is
*"Raw Fjall resume-key bytes for the next page"* (`memory_ops.rs:230`), set to the last emitted
**Fjall key** (`memory_ops.rs:292`). Wire form is `Cursor::encode_fjall(last_key)` at
`src/mcp/memory.rs:333` and `src/mcp/memory.rs:364`.

**A Fjall key is a memory-record key, not a Lance row identifier.** `memory_list` reads
`store.index_db` (Fjall), never the Lance table. This is why §8's parenthetical is wrong twice
over: the payload is not a row id, and a Fjall cursor is structurally immune to a Lance reindex
because it does not touch Lance.

### 2.3 The existing invalidation precedent in this repo is the pattern to follow

`cursor_invalidated: bool` already exists on the git and code responses
(`src/mcp/types_git.rs:325,367,391`; `src/mcp/helpers_git.rs:270,305,328,364,391,435,512,553`;
`src/mcp/helpers_code.rs:543,550,632`), documented at `src/mcp/types_git.rs:139-141`:

> Resume token returned by the previous call's `next_cursor`. … on HEAD movement the response
> carries `cursor_invalidated = true` and the caller must restart.

**§8 should say "extend the existing `cursor_invalidated` mechanism to the documents tier", not
"invalidate `next_cursor`"** — the repo already has a named, tested, client-visible invalidation
signal, and the documents tier is the only search tier that lacks one.

### 2.4 A reindex in this repo is a directory deletion

This is the mechanism that makes the answer to the direct question unambiguously *no*.

- `src/lance/mod.rs:184-196` — `LanceStore::open` builds an `expected` `LanceMeta`
  `{ dim, embedding_model, schema_ver }` and calls `wipe_on_mismatch(dir, &meta_path, &expected)`
  before the connection opens.
- `src/lance/mod.rs:494-523` — on any mismatch: log a warning, then
  `std::fs::remove_dir_all(&p)` / `std::fs::remove_file(&p)` for **every entry in the Lance
  directory**. No table-level drop, no index-level drop, no graceful tombstoning.
- `src/lance/mod.rs:36-53` — `LanceMeta`'s own doc: *"a mismatch on any field wipes the store."*
- `src/lance/mod.rs:57-60` — `schema_ver: MEMORY_SCHEMA_VER = RELEASE_MINOR`, so the "schema bump"
  trigger of §14 is already implemented, and it wipes.
- `src/lance/mod.rs:204-208` — the tables are then re-created from scratch by `ensure_table`.

So all three §14 triggers land on the same mechanism, and the payload of every cursor built on top
of the Lance table is destroyed by `remove_dir_all`. A `documents` row address or row id means
nothing after a wipe.

---

## 3. The three triggers: which needs invalidating

Spec §14 (line 419-421):

> Reindex triggers: any tokenizer/`FtsIndexBuilder` change, `embedding_preset` change (existing
> behaviour), schema bump. Same wipe semantics as `embedding_preset` — surfaced at scan time, not
> silently.

All three route through `wipe_on_mismatch` **provided** the corresponding field is in `LanceMeta`.
Today only two of the three are represented:

| §14 trigger | In `LanceMeta` today | Mechanism | Cursor verdict |
|---|---|---|---|
| `embedding_preset` change | yes — `embedding_model` (`mod.rs:43`) | dir wipe (`mod.rs:513-521`) | **must invalidate** |
| schema bump | yes — `schema_ver` (`mod.rs:51`, `mod.rs:60`) | dir wipe | **must invalidate** |
| tokenizer / `FtsIndexBuilder` change | **no — the field does not exist** | none today; a FTS config change would silently leave a stale index in place | **must invalidate** (and needs a new `LanceMeta` field) |

Two things follow.

**(a) The tokenizer trigger is the one that is not already handled, and it is also the one that does
not destroy row addresses.** Per §1.4, an FTS rebuild rewrites no data files, so `_rowid` values
would survive it untouched — a cursor carrying only a row id would keep paging *without error*
across an index whose tokenizer changed, and page 2 would be ordered by BM25 scores from a
different tokenizer than page 1. This is the precise mechanism of the "page 2 mixes two indexes"
failure in issue #50 §4, and it is a **silent** one: no error, no empty page, just two pages
assembled from two different scoring functions.

**(b) The other two already wipe everything, so there is nothing left to protect.** Invalidating on
them is free and correct, but it is not where the risk lives.

So: **a cursor must be invalidated on all three**, but not because three separate mechanisms are
needed — because the three triggers are not equally handled, and the one that is unhandled is the
one that fails silently. The unifying statement is not "invalidate on reindex"; it is "a cursor is
valid for exactly one generation of the index that produced it" (§5).

---

## 4. The applicability rule

> **A cursor is valid for exactly one index generation. It carries a generation token; on resume the
> token is compared against the current generation, and a mismatch yields
> `cursor_invalidated = true` with a full page 1 — never a partial page 2.**

Mapping it onto the existing code:

- The generation token is the `LanceMeta` tuple already computed in `src/lance/mod.rs:189-194`,
  extended with one field for the FTS configuration (tokenizer identity + `FtsIndexBuilder`
  settings). All three §14 triggers then bump it through one comparison, and the `wipe_on_mismatch`
  call site at `mod.rs:196` becomes the single place that mints the new generation.
- The comparison and the response field are the ones already implemented for the git/code tiers
  (`cursor_invalidated`, `src/mcp/types_git.rs:325`). No new wire vocabulary.
- The FTS-configuration field must be a **fingerprint, not the config struct itself** — a
  deterministic hash of the resolved `[documents.fts]` values (§10) — so it can be written into
  `meta.json` and compared without deserializing a builder.
- **Do not put `_rowid` in the cursor at all.** Per §1, on a default-mode table it is the row
  address and it changes under the `optimize()` that §7.3 mandates after every ingest batch; and
  after a wipe it collides. Either `enable_stable_row_ids` is set on `documents_v2` at creation
  (possible, since §6 already renames the table — and `migrate_to_stable_row_ids` does not exist
  in `lance 10.0.0`, so the rename is the *only* chance to take it), or the cursor carries
  generation + offset and no Lance identifier whatsoever.

### Why not over- or under-invalidate

- **Too broad** — invalidating on every `optimize()` or every scan. `optimize()` runs after every
  ingest batch (§7.3 line 179) and rescans are routine; a cursor that dies on each one means the
  pagination feature is never usable, which is the §7.3 "silent absence" failure in its mild form:
  the user gets page 1 forever and never learns why.
- **Too narrow** — invalidating only on `embedding_preset` / schema, and letting the tokenizer
  trigger through. Then page 1 and page 2 come from two BM25 configurations. Nothing errors. The
  caller sees a coherent-looking result assembled from an index that no longer exists. For legal
  retrieval that is a fabricated answer, which is strictly worse than the empty page §7.3 warns
  about.
- **The rule's job** is to make the failure mode *loud*. Both edges of the range produce silence;
  the generation token is what converts the tokenizer edge from silent to explicit.

---

## 5. Mismatches between spec §8 and the code

Reported, not papered over.

| # | Spec text | What the code says |
|---|---|---|
| 1 | §8: *"existing `next_cursor` semantics carry `(lane ranks, last row id)`"* (spec line 225) | The documents tier has **no cursor**. `SearchDocumentsResponse` has no `next_cursor` (`src/mcp/types_documents.rs:87-101`), and the repo states it outright at `src/mcp/types_documents.rs:89-90`: *"`search_documents` has no cursor"*. The cursors that exist are Fjall key bytes (`src/mcp/cursor.rs:5-9`, `src/mcp/memory_ops.rs:230`) or `{offset, snapshot_id}` (`src/mcp/cursor.rs:11-18`). Neither is a lane rank or a row id. |
| 2 | §8: *"(see `with_row_id`)"* | `with_row_id` appears **nowhere** in the repository — not in `src/`, not in `crates/`, not in any `.md`/`.toml`. It is being cited as if it were an in-use API. It is also **not deprecated** (`lancedb-0.37.1/src/query.rs:509`, no `#[deprecated]`, `grep deprecated` → no hits), so the parenthetical reads as a deprecation warning that does not exist. |
| 3 | §8: *"cursors must be invalidated by any index rebuild"* | A basemind reindex is not an index rebuild, it is `remove_dir_all` on the Lance directory (`src/lance/mod.rs:513-521`). There is no index object to rebuild and no row identity to preserve — so the rule as worded is *under*-specified in one direction (it does not say the store is destroyed) and *over*-specified in another (it implies row ids would otherwise survive). |
| 4 | §8 puts this in the **Query path** section, implying it applies to the fused multi-lane documents query | The fused query does not exist yet (§11 delivery phases). Whatever §8 decides here becomes the first documents-tier cursor contract, which is why it needs to be right: it should specify the *generation* contract, not a row id that cannot be made to work. |
| 5 | §14 line 419: *"`embedding_preset` change (existing behaviour)"* | Correct, and worth citing precisely: `LanceMeta.embedding_model` (`src/lance/mod.rs:43`) → `wipe_on_mismatch` (`mod.rs:196`) → `remove_dir_all` (`mod.rs:513-521`). The tokenizer trigger, by contrast, has **no** `LanceMeta` field today, so as written the spec would ship it unhandled. |

---

## 6. What could **not** be established

- **Whether `_rowid` survives an update.** The pinned crate's own doc
  (`lance-10.0.0/src/dataset/write.rs:330-332`) says stable row IDs are *"stable after compaction
  operations, but not after updates"*, while the format spec
  (<https://lance.org/format/table/row_id_lineage>, "Row ID Behavior on Updates") describes an
  implemented update workflow that preserves the logical id and remaps it to a new physical
  address, and carries its own warning that *"work to support stable row IDs in indices is in
  progress."* These two primary sources disagree for `lance 10.0.0`. I could not resolve which
  describes the shipped behaviour without reading `lance-10.0.0`'s update/merge path end to end,
  which I did not do. **This does not affect the conclusion** — basemind's reindex is a wipe, so
  neither answer helps — but it must not be relied on if `enable_stable_row_ids` is ever adopted.
- **Whether `lancedb` `Table::create_index` on `documents_v2` would internally compact.**
  I established that compaction moves addresses and that index builds write no data files
  (§1.4), but I did not trace `lancedb-0.37.1/src/table/create_index.rs` end to end to prove no
  compaction is folded into the FTS index build. If it is, the tokenizer trigger also destroys
  row addresses, which strengthens §3 but does not change the answer.
- **Whether a `_rowid`-bearing cursor is even expressible in the current wire shapes.** The
  documents response would need a new field regardless; whether that field should carry `_rowid` at
  all is the §5 decision, not a fact I could look up.
- **Behaviour on a MemWAL/LSM table** beyond the hard error quoted at
  `lancedb-0.37.1/src/table/query/lsm.rs:160-161`. basemind does not use MemWAL today.

---

## Sources

Primary, all read directly:

- `https://docs.rs/lancedb/0.37.1/lancedb/query/trait.QueryBase.html` — `with_row_id` present,
  not deprecated.
- `lancedb-0.37.1` crate source (`static.crates.io`, the exact `Cargo.lock:8243-8246` pin):
  `src/query.rs:509-510,597-600,850,898`; `src/table.rs:293-303,1618`;
  `src/table/query.rs:281-282`; `src/table/query/lsm.rs:160-161`;
  `src/database/namespace.rs:466-471`; `src/table/optimize.rs:541-585`.
- `lance 10.0.0` crate source (`static.crates.io`, the `Cargo.lock:7658-7660` resolution of
  `lancedb 0.37.1`): `src/dataset/write.rs:268-334,429`; `src/dataset/rowids.rs:74-163`;
  `src/dataset/scanner.rs:1805-1817`; `src/dataset/optimize.rs:1625-1631,1742-1743,2032-2034,3186-3282`.
- `https://lance.org/format/table/row_id_lineage` — Lance format spec, Row ID & Lineage.
- Repo: `Cargo.toml:166-170`, `src/lance/mod.rs:36-60,184-208,315-340,494-523`,
  `src/lance/schema.rs`, `src/mcp/memory.rs:296-366,491-530,637-711`,
  `src/mcp/types_documents.rs:10-101`, `src/mcp/types_git.rs:139-141,325,367,391`,
  `src/mcp/cursor.rs:1-21,44-96`, `src/mcp/memory_ops.rs:205-300`, `src/lib.rs:73-83`.
- Spec: `origin/spec/lexical-document-retrieval` — `docs/specs/0012-lexical-document-retrieval.md`
  §6 (line 99), §7.3 (lines 176-184), §8 (lines 186-226), §10 (lines 264-303), §14 (lines 394-425).

No blog post, benchmark write-up, or secondary summary was used as authority for any API claim.