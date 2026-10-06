# Research: does LanceDB 0.37.1 have FTS fuzziness, and at what cost?

- **Ticket:** [#49](https://github.com/jamon8888/basemind/issues/49) (research), child of #35
- **Date:** 2026-10-06
- **Scope:** the capability question only. Whether fuzziness is *good* for legal identifiers
  is the maintainer's decision, not this note's.
- **Pinned version under test:** `lancedb 0.37.1` (`Cargo.toml:170`, `Cargo.lock:8243-8246`),
  which pulls `lance-index =10.0.0` (`lancedb-0.37.1/Cargo.toml:310-313`) and `lance =10.0.0`.

## Answers, up front

| Question | Answer |
| --- | --- |
| 1. Does 0.37.1 expose Levenshtein / edit distance on `Query::full_text_search`? | **YES**, per-query, per-term. Not a parameter on `Query` itself — it lives inside the `FullTextSearchQuery`'s `MatchQuery` node. |
| Does it need a **separate index**? | **NO.** `InvertedIndexParams` (a.k.a. `FtsIndexBuilder`) has **zero** fuzziness-related knobs. Fuzziness is purely a query-time flag over an index that already exists. |
| 2. Does fuzziness make sense on top of the `cites` trigram index? | **NO.** Edit distance is applied to *post-tokenizer tokens*. On a 3-char trigram index, edit distance 1 matches essentially every other in-vocabulary trigram, and the expansion is then truncated at `max_expansions` in **lexicographic** order. The trigram lane cannot express typo tolerance; it can only express "match a lot". |
| 3. What recall guarantee does fuzziness give? | **None that can be asserted as an invariant.** The only guarantee is "the lexicographically smallest `max_expansions` in-vocabulary terms within the requested edit distance, per query token". Terms past the cut are dropped silently — the same failure mode §7.3 rejects in `fast_search()`. |
| 4. Does `src/search/rrf.rs` do edit distance? | **NO.** `rrf.rs` is pure RRF arithmetic — it never scores, it only sums `weight / (k + rank)`. The exact lane's logic lives in `src/search/exact.rs` and is **exact-match + key-prefix-scan**, not edit distance. |
| Is §8.3's conditional "if one is added later" satisfiable? | **YES — but not on the lane §8.3 is talking about.** It is satisfiable today with a word-tokenized index and no extra index cost. It is **void on `cites`**, where §8.3 forbids it: edit distance over trigrams degenerates (Q2). So §8.3 should be corrected, not deleted. |

---

## 1. Capability — YES, and it is a query-time parameter

### 1.1 What `Query` itself exposes

`Query::full_text_search` takes exactly one argument, `FullTextSearchQuery`. There is no
`fuzziness()` method on `Query`:

- `lancedb-0.37.1/src/query.rs:453` — `fn full_text_search(self, query: FullTextSearchQuery) -> Self;`
- docs.rs, `lancedb 0.37.1`, `lancedb::query::Query` — the method list for the 0.37.1 build contains
  `full_text_search`, `fast_search`, `only_if`, `only_if_expr`, `postfilter`, `limit`, `offset`,
  `select`, `rerank`, `with_row_id`, … and **no** fuzzy/edit-distance method.
  <https://docs.rs/lancedb/0.37.1/lancedb/query/struct.Query.html>

Fuzziness is reachable because `FullTextSearchQuery` wraps an `FtsQuery` tree whose leaf is
`MatchQuery`, and `MatchQuery` carries the fuzzy fields.

### 1.2 The parameters that exist (0.37.1)

`lancedb::index::scalar` re-exports the whole query vocabulary:

```rust
// lancedb-0.37.1/src/index/scalar.rs:63-66
pub use lance_index::scalar::FullTextSearchQuery;
pub use lance_index::scalar::InvertedIndexParams as FtsIndexBuilder;
pub use lance_index::scalar::InvertedIndexParams;
pub use lance_index::scalar::inverted::query::*;
```

So a caller needs **no new dependency** — `MatchQuery` and `FtsQuery` are reachable through
`lancedb` itself. (basemind currently depends only on `lancedb`, `Cargo.toml:170`.)

**Entry points** (`lance-index-10.0.0/src/scalar.rs:98-176`):

- `FullTextSearchQuery::new(query: String)` — `MatchQuery::new`, i.e. `fuzziness: Some(0)` (exact).
- `FullTextSearchQuery::new_fuzzy(term: String, max_distance: Option<u32>)` — the one-liner.
- `FullTextSearchQuery::new_query(query: FtsQuery)` — build the tree yourself.

**`MatchQuery` fields** (`lance-index-10.0.0/src/scalar/inverted/query.rs:283-317`), with the
defaults applied by `MatchQuery::new` at `:320-331`:

| Field | Type | `new()` default | Meaning |
| --- | --- | --- | --- |
| `terms` | `String` | — | the raw query string (tokenized by the *index's* tokenizer) |
| `fuzziness` | `Option<u32>` | `Some(0)` | max edit distance per term. `None` ⇒ auto by term length |
| `max_expansions` | `usize` | `50` | global budget of expanded terms |
| `prefix_length` | `u32` | `0` | number of leading characters held fixed |
| `operator` | `Operator` | `Or` | `And` / `Or` over terms |
| `boost` | `f32` | `1.0` | per-node boost |
| `column` | `Option<String>` | `None` | restrict to one indexed column |

Builders: `with_fuzziness`, `with_max_expansions`, `with_prefix_length`, `with_operator`,
`with_boost`, `with_column` (`:345-367`). Also `MatchQuery::auto_fuzziness(token) -> u32`
(`:370-376`): `0..=2 → 0`, `3..=5 → 1`, `_ → 2`.

Confirmed on docs.rs for the exact pinned version:
- <https://docs.rs/lancedb/0.37.1/lancedb/index/scalar/struct.MatchQuery.html> — fields
  `boost, column, fuzziness, max_expansions, operator, prefix_length, terms`; methods
  `with_boost, with_column, with_fuzziness, with_max_expansions, with_operator, with_prefix_length`,
  `auto_fuzziness`.
- <https://docs.rs/lancedb/0.37.1/lancedb/index/scalar/struct.FullTextSearchQuery.html> — fields
  `limit, query, wand_factor`; associated fns `new, new_fuzzy, new_query`.

### 1.3 The parameters are not invented by the doc reader — they are on the execution path

The `FtsQuery` → `FtsSearchParams` plumbing copies all three fuzzy fields, and the code says
why:

```rust
// lance-10.0.0/src/io/exec/fts.rs:386-394
impl MatchQueryExec {
    /// Merge the fuzzy fields from `query` into `params` so that the stored
    /// params reflect what BM25 stat collection and search will actually use.
    fn effective_params(query: &MatchQuery, params: FtsSearchParams) -> FtsSearchParams {
        params
            .with_fuzziness(query.fuzziness)
            .with_max_expansions(query.max_expansions)
            .with_prefix_length(query.prefix_length)
    }
```

`FtsSearchParams` itself (`lance-index-10.0.0/src/scalar/inverted/query.rs:12-71`) defaults to
`fuzziness: Some(0)`, `max_expansions: 50`, `prefix_length: 0`, `wand_factor: 1.0`, `limit: None`.
Note `FullTextSearchQuery::params()` (`scalar.rs:171-175`) sets only `limit` and `wand_factor` — the
fuzzy fields arrive via `effective_params`, i.e. from the `MatchQuery` node. A caller that forgets to
build the node gets exact matching silently.

### 1.4 The distance is real classic Levenshtein, over the term FST

```rust
// lance-index-10.0.0/src/scalar/inverted/index.rs:2401-2406
let fuzziness = match params.fuzziness {
    Some(fuzziness) => fuzziness,
    None => MatchQuery::auto_fuzziness(token),
};
let lev = fst::automaton::Levenshtein::new(token, fuzziness)
    .map_err(|e| Error::index(format!("failed to construct the fuzzy query: {}", e)))?;
```

Three consequences worth writing down:

1. **It is a Levenshtein automaton over the vocabulary FST, not over the raw text.** Expansion can
   only ever return terms that already exist in the index. It cannot invent a match for a term the
   index never saw.
2. **It is plain Levenshtein, not Damerau-Levenshtein.** Transpositions cost 2, not 1.
   `"Smtih" → "Smith"` is distance 2 and will *not* be caught by `fuzziness = 1`. This is confirmed
   by the official docs wording, "the classic Levenshtein distance"
   (<https://docs.lancedb.com/search/full-text-search>), and by the type used at the call site.
3. **It operates on bytes**, since the FST keys are bytes — `TokenMap::Fst(fst::Map<Vec<u8>>)`
   (`lance-index-10.0.0/src/scalar/inverted/index.rs:3066`). For accented party names
   (`ascii_folding` is on for the primary index per spec §7.1), a single changed non-ASCII
   character can cost 2 UTF-8 byte edits, i.e. a `fuzziness = 1` budget is consumed by one character.
   *(This is a reading of the byte-oriented key type plus `ascii_folding`; it was not measured —
   see "Not established".)*

### 1.5 Fuzziness requires an FST token set

```rust
// lance-index-10.0.0/src/scalar/inverted/index.rs:2408-2424
if let TokenMap::Fst(ref map) = self.tokens.tokens {
    ... map.search(lev) ...
} else {
    Err(Error::index("tokens is not fst, which is not expected".to_owned()))
}
```

`TokenSetFormat` defaults to `Fst` (`index.rs:519-523`, `#[default]` on the `Fst` variant), so a
freshly built index is fine. An index persisted in the legacy `arrow` token-set format would return a
hard error rather than degrade. Not relevant to a new build, but worth knowing if an existing store
is ever opened.

### 1.6 Official documentation agrees

<https://docs.lancedb.com/search/full-text-search> documents, verbatim:

> **Fuzzy Search** — Fuzzy search allows you to find matches even when the search terms contain typos
> or slight variations. LanceDB uses the classic Levenshtein distance to find similar terms within a
> specified edit distance.
>
> | Parameter | Type | Default | Description |
> | `fuzziness` | int | 0 | Maximum edit distance allowed for each term. If not specified, automatically set based on term length: 0 for length ≤ 2, 1 for length ≤ 5, 2 for length > 5 |
> | `max_expansions` | int | 50 | Maximum number of terms to consider for fuzzy matching. Higher values may improve recall but increase search time |

Note the doc's default is stated as `0`, and that "not specified" is the `fuzziness: None` case, which
in Rust requires deliberately passing `None` — `MatchQuery::new` pins `Some(0)`. The Rust default and
the documented default agree (`Some(0)` = exact), which is the safe direction.

### 1.7 Version provenance

- `fuzziness` / `max_expansions` exist in `lance-index 10.0.0`, which `lancedb 0.37.1` pins exactly
  (`version = "=10.0.0"`, `lancedb-0.37.1/Cargo.toml:310`).
- Upstream's own tests exercise it in-tree:
  `test_fts_udtf_fuzzy_search` (`lancedb-0.37.1/src/table/datafusion/udtf/fts.rs:568-597`, uses
  `MatchQuery::new("craziou").with_fuzziness(Some(2))`) and `test_fts_udtf_fuzzy_with_prefix_length`
  (`:1592-1625`).
- I did **not** establish the version in which fuzziness first shipped, and did not check 0.38.x.
  It does not matter here: basemind is pinned to 0.37.1 and the parameter set is present in it.

---

## 2. Interaction with the n-gram tokenizer — this is where it breaks

The decisive line is `collect_query_tokens`:

```rust
// lance-index-10.0.0/src/scalar/inverted/query.rs:803-813
pub fn collect_query_tokens(text: &str, tokenizer: &mut Box<dyn LanceTokenizer>) -> Tokens {
    let token_type = tokenizer.doc_type();
    let mut stream = tokenizer.token_stream_for_search(text);
    ...
}
```

and, for a text column, search and document tokenization are the *same* stream:

```rust
// lance-index-10.0.0/src/scalar/inverted/tokenizer/document_tokenizer.rs:110-117
impl LanceTokenizer for TextTokenizer {
    fn token_stream_for_search<'a>(&'a mut self, query_text: &'a str) -> BoxTokenStream<'a> {
        self.tokenizer.token_stream(query_text)
    }
    fn token_stream_for_doc<'a>(&'a mut self, text: &'a str) -> BoxTokenStream<'a> {
        self.tokenizer.token_stream(text)
    }
```

So on the `cites` index (spec §7.1: `base_tokenizer("ngram")`, `ngram_min_length(3)`,
`ngram_max_length(3)`), the query string is itself n-grammed and edit distance is computed **over
3-character trigrams**, not over the identifier.

The tokenization below was **executed**, not merely read. There is no Rust toolchain in this
environment, so `CodepointFrontiers` (`ngram_tokenizer.rs:187-216`), `StutteringIterator::new`
(`:133-157`), `StutteringIterator::next` (`:164-183`) and `NgramTokenStream::advance` (`:97-111`)
were ported line-for-line to Python and run at `min_gram = max_gram = 3`:

| Query | Tokens emitted |
| --- | --- |
| `362` | 1 — `['362']` |
| `3621` | 2 — `['362', '621']` |
| `2-24-1234` | 7 — `['2-2', '-24', '24-', '4-1', '-12', '123', '234']` |
| `Smith` | 3 — `['Smi', 'mit', 'ith']` |
| `ab` | **0** |
| `a` | **0** |

- A query shorter than `min_gram` is not "partially matched" — it matches nothing at all.
  `StutteringIterator::new` sets `min_gram = 1, max_gram = 0` when the frontier count is
  `<= min_gram` (`:136-144`), so `next()` hits `if self.max_gram < self.min_gram { return None }`
  (`:177-179`) and the stream ends immediately. A query shorter than the n-gram width silently yields
  **zero** tokens — no error, and no fuzzy rescue, because fuzzy expansion runs over the token list
  that is now empty.
- `Smith` → `Smi, mit, ith` is the general case, and it is the whole problem in three tokens: the
  unit being compared is a 3-character window, so "one typo in `Smith`" is not expressible as a
  distance on any single token.
- Every emitted n-gram carries `position = 0` (`ngram_tokenizer.rs:100`), so position-based phrase
  matching is not available on this lane either.

Fuzzy expansion then runs on that token list (`InvertedIndex::expand_fuzzy_tokens`,
`index.rs:1007-1044`), expanding **each token independently** and re-using its position.

**Why this makes fuzziness meaningless on `cites`:**

1. **Edit distance on trigrams is not edit distance on identifiers.** A distance-1 neighbourhood of
   the trigram `"362"` is every in-vocabulary trigram differing in one character — which for a
   numeric citation column is nearly every other 3-digit number. And the scale is wrong in the other
   direction too: a single mistyped character inside a *longer* identifier perturbs up to three
   overlapping trigrams, so recovering it would need `fuzziness ≈ 3` applied to trigrams — at which
   point the neighbourhood is the whole vocabulary. There is no distance that means "one typo in the
   identifier" when the unit being scored is a 3-character window.
2. **The default operator is `Or`, and the union of expansions is the query.** With `Or`, one
   surviving fuzzy trigram is enough to match a document. Over trigrams this is close to "match
   anything". With `And`, every original trigram must match *some* expansion, which is far stricter
   than "the identifier is within edit distance 1". There is no operator setting in between, because
   expansion happens per token before the operator is applied.
3. **`prefix_length` cannot rescue it.** The prefix narrowing at `index.rs:2410-2417` applies to the
   **token**, i.e. the trigram — `prefix_length(1)` pins the first character of `"362"`, not the
   first character of the citation number. `DocType::Text.prefix_len` is `0`
   (`document_tokenizer.rs:54-66`), so there is no base prefix to compose with either.
4. **The n-gram tokenizer already provides what the `cites` lane is for.** Its purpose (§7.1: make
   `362` reach `§ 362(a)(1)`) is substring reach, which exact trigram matching already delivers.
   Fuzziness on top of it subtracts precision without adding reach.

**Answer to question 2: no.** Fuzzy matching on top of trigrams is not a weaker version of typo
tolerance, it is a different (and much blunter) operator. A party-name lane would need its **own
index over its own column with a word tokenizer** (`simple` or `icu`), not a fuzzy flag on the
existing trigram index. §7.1's index list does not contain such an index, so adding one is a §7.1
change, not a §8.3 change.

---

## 3. Cost and recall

### 3.1 The premise in the ticket is wrong: fuzziness costs no extra index

The ticket asks "an extra index costs store size and build time" — true of an extra *lane*, but not
of fuzziness. `InvertedIndexParams` (exported as `FtsIndexBuilder`) has **no** fuzziness-related
builder at all. Grepping the whole tokenizer/params module:

```
$ grep -ci "fuzz" lance-index-10.0.0/src/scalar/inverted/tokenizer.rs
0
```

The full public builder surface is `analyzer, base_tokenizer, language, with_position,
max_token_length, lower_case, stem, remove_stop_words, custom_stop_words, ascii_folding,
ngram_min_length, ngram_max_length, ngram_prefix_only, block_size, memory_limit_mb, num_workers,
format_version` (`tokenizer.rs:639-834`). Fuzziness is a **query-time** switch over an index that is
already built, and the term FST that expansion searches is already part of every FTS index
(`TokenSetFormat::Fst` is the default, `index.rs:519-523`).

Therefore:

- **Store cost of fuzziness: 0 bytes. Build cost: 0.** Nothing new to build, nothing to re-index,
  nothing to re-optimize.
- The cost that a party-name lane *would* incur is the cost of a **new column + new FTS index with a
  word tokenizer** — a §7.1 change, independent of fuzziness.
- A useful corollary for §7.3: turning fuzziness on for a query does **not** require
  `table.optimize(OptimizeAction::All)` first, because it does not require any index that does not
  already exist.

### 3.2 What the query-time cost actually is

Per query token, per index partition:

1. One Levenshtein automaton walk over that partition's term FST (`collect_fuzzy_candidates`,
   `index.rs:2393-2424`), materialising up to `max_expansions` keys.
2. A merge of the per-partition candidate sets, then a **global truncation** to `max_expansions`
   (`expand_fuzzy_tokens`, `index.rs:1007-1044`).
3. An extra corpus-statistics pass: the BM25 scorer must cover the **union of the expanded terms**,
   not just the query tokens, or the expanded matches score 0 — see the doc comment on `bm25_base_scorer`
   (`index.rs:871-890`) and `scorer_terms` (`inverted.rs:40-67`). Its size scales with the number of
   expanded terms.
4. Reads over the posting lists of all expanded terms.

The cost is bounded and query-scoped. It is not free, but it is not a build-time or storage cost, and
it is proportional to how many terms you let it expand.

### 3.3 The recall guarantee — and why it is not one

There is **no** recall guarantee that can be asserted as an invariant. What is actually guaranteed,
per `index.rs:998-1044`:

> `params.max_expansions` caps the whole query's expansion, not any single partition's: for each
> query token the per-partition candidates (each streamed in FST key order) merge into one
> lexicographically ordered set, and the remaining budget takes a prefix of it.

So the selected terms are **the lexicographically smallest `max_expansions` in-vocabulary terms**
within the requested edit distance. Every term past the cut is dropped **silently** — no warning, no
error, no partial-result signal.

This matters because the budget is smaller than the neighbourhood. The distance-≤1 ball was
**enumerated exactly** (generate all single substitutions / insertions / deletions, then verify every
element with a real Levenshtein implementation — pure Python, no crate needed):

- 3-character token over a 10-symbol alphabet (`362`): 27 substitutions + 37 distinct insertions +
  3 deletions = **67** distinct strings. (A naive `3·9 + 4·10 + 3 = 70` double-counts 3
  insertion/deletion collisions.) **67 > 50**, so the default `max_expansions` truncates *before any
  vocabulary filtering*. On the `cites` trigram index this is the normal case.
- 6-letter word over a 26-symbol alphabet (`porter`): 150 + 176 + 6 = **332** distinct strings.
  **332 > 50** by a wide margin.
- Both figures are **upper bounds on what survives vocabulary filtering**, not measurements of what
  does. A real corpus of surnames intersects that with a much smaller set, so the budget may or may
  not bite on a word lane — **this is corpus-dependent and I did not measure it**.
- Because truncation is lexicographic, the cut is **not** "closest first". A high-frequency exact
  neighbour whose spelling sorts late is dropped in favour of an irrelevant one that sorts early.

**This is the same failure mode §7.3 rejects in `fast_search()`.** The spec refuses `fast_search()`
because "a silently missing filing is a wrong answer". Fuzzy expansion with a default
`max_expansions` introduces a *query-dependent* silent truncation with exactly the same property.
The two must be governed by the same rule: if fuzziness is ever enabled, the budget must be sized
above the worst-case in-vocabulary neighbourhood, and that sizing must be tested, not assumed.

Two further sharp edges on the same lane:

- **No distance weighting.** `expand_fuzzy_tokens` returns a flat token list with no per-candidate
  distance attached (`index.rs:1040-1043` returns `Tokens::with_positions(expanded_tokens,
  expanded_positions, ...)`). Scoring is ordinary BM25 over that list. So a distance-2 expansion can
  outrank the exact term purely on IDF and length, and the caller cannot tell from the score whether
  a hit was exact or fuzzy. *(Read from the code path; not measured.)*
- **Short queries return nothing at all.** A query token shorter than `ngram_min_length` produces zero
  tokens (§2), which is indistinguishable from "no match" at the result-set level.

### 3.4 How to test that a typo does not make a document disappear

Five tests, in increasing order of how much they actually prove. The first two are cheap; the third
is the one that matters.

**(a) Exact-form regression, per lane.** For each lane that will ship with fuzziness, assert that
the un-typo'd query returns the expected document *at the same `fuzziness` setting the production
path uses*. A lane that only passes with `fuzziness = 0` proves nothing about the fuzzy path.

**(b) Typo rescue, per lane.** Plant a fixture whose token differs from the query by one edit in a
way the setting is supposed to catch (substitution for `fuzziness = 1`; transposition needs
`fuzziness = 2` — see §1.4.2). Assert the document is in the result set. Because fuzziness is a
query-time flag, this is a pure unit test over a handful of rows — no corpus, no timing budget.

**(c) Expansion-budget saturation — the test that actually catches silent loss.** For the fixture
vocabulary, run the typo'd query at `max_expansions` = 1, 10, 50, 5000 and assert the result set
**stops changing** once the budget exceeds the in-vocabulary neighbourhood size. If the set is still
growing at the value the production path uses, expansion is being truncated and the lane is silently
dropping candidates — the §7.3 failure. This test is the direct analogue of
`index_stats().num_unindexed_rows` for the `fast_search()` ban, and it is the one §8.3 should require
if it keeps its conditional.

**(d) Corruption control, for the `cites` lane specifically.** Query the **bare** citation token, the
way §8 step 1 (Normalize) produces it — not the whole citation string, since a full-string query is
ambiguous at *any* setting. Running the §2 tokenizer over the two near-neighbours:

```
"§ 362(a)(1)" -> ['§ 3', ' 36', '362', '62(', '2(a', '(a)', 'a)(', ')(1', '(1)']
"§ 363(a)(1)" -> ['§ 3', ' 36', '363', '63(', '3(a', '(a)', 'a)(', ')(1', '(1)']
                 ^^^^^^^^^ 6 of 9 trigrams shared
```

Six of nine trigrams are common to both citations, and the same six are common to *every* pair in
the `§ 3NN` family. Because the default operator is `Or` (§2, point 2), a full-string query matches
on shared trigrams alone. That is a property of the lane's tokenizer, independent of `fuzziness` —
which is why the test below plants both rows rather than trusting a whole-string assertion. Plant
`§ 362(a)(1)` and `§ 363(a)(1)`, then assert:

1. `fuzziness = 0`, query `362` → row 1 returned, row 2 not.
2. `fuzziness = 1`, query a one-substitution typo of `362` (e.g. `462`) → row 1 returned.
3. `fuzziness = 1`, query `362` unchanged → **row 2 also returned**, because `dist("362","363") = 1`.

Step 3 is the point. On a trigram index it will fail against the expectation "row 2 not returned",
and that failure is the evidence that §8.3's first clause ("No fuzziness here") is load-bearing
rather than stylistic. Conversely, step 2 will *also* be satisfied without fuzziness if the index is
ngram — n-grams already give substring reach — so step 2 alone proves nothing; it is step 3 that
measures the corruption.

**(e) Zero-token reporting.** A guard that a query producing zero tokens is surfaced as such
(`phrase_unsupported`-style, per the §7.2 precedent) rather than returning an empty result set. This
is the concrete instance of "a silently missing filing is a wrong answer" that fuzziness does not
cause but does not fix either.

---

## 4. In-repo precedent — `src/search/rrf.rs` does no edit distance

The ticket points at `rrf.rs` (exact weight 2.0). Read it: it is **only** the fusion arithmetic.

- `WEIGHT_EXACT: f32 = 2.0`, `WEIGHT_VECTOR: f32 = 1.0`, `WEIGHT_KEYWORD: f32 = 1.0`,
  `DEFAULT_RRF_K: f32 = 60.0` (`src/search/rrf.rs:20-36`).
- `rrf_fuse_detailed` accumulates `entry.score += lane.weight / (k + rank)` over chunk ids
  (`src/search/rrf.rs:75-109`). It reads **ranks only**, never scores — that is the whole point of RRF
  ("RRF is score-scale-agnostic (it only reads ranks)", `rrf.rs:7-8`), which is exactly why it can
  blend an L2 distance, a BM25 score, and a symbol match order.
- Grepping `rrf.rs` for `levenshtein|edit|distance|fuzz|prefix` returns **one** hit, and it is the
  prose in the module doc at `rrf.rs:7`. There is no edit distance in the file.

The exact lane's actual matching logic is in **`src/search/exact.rs`**, and it is exact + prefix:

- Gate: `is_identifier_query` (`exact.rs:22-36`) — length ≥ 2, ASCII letter or `_` first, ASCII
  alphanumerics/`_` only. Natural-language phrases never reach this lane.
- Lookup: `db.symbols_by_name_lookup(query, cap)` (`exact.rs:57`), which is an **index-backed prefix
  scan** on a length-prefixed keyspace (`src/index/mod.rs:455-471`, prefix from
  `keys::symbols_by_name_prefix(name)`), so `Foo` is isolated from `Foobar`.
- Ranking: `exact.rs:79-84` — exact-name matches first (`name == query`), then longer prefix matches.
  **No similarity function is applied to the candidates.**

So the precedent is: *the repo resolves a typo'd identifier by prefix scan, not by edit distance, and
weights it above the other lanes.* If a party-name lane wants typo tolerance, the existing code gives
it **no** reusable component — the fusion side is reusable (`FusionLane` takes a ranked `&[String]`,
so a fuzzy lane would slot straight in), the matching side would be new code.

The only fuzzy machinery already in the repo is unrelated to LanceDB: `nucleo-matcher` is used by the
`code` tool's `find` mode to fuzzy-rank file **paths** client-side, before pagination
(`src/mcp/helpers_files.rs:4-9, 112-117, 154-159`, `Pattern::parse` + `Matcher::new(Config::DEFAULT.match_paths())`).
That is subsequence matching over an in-memory candidate list, not an indexed edit-distance query. It
is a precedent for *wanting* typo tolerance in a tool surface, and for nothing else.

---

## 5. Verdict on §8.3's conditional

§8.3 currently reads:

> **Exact lane.** `full_text_search` over the `cites` n-gram index (and `text_phrase` for quoted
> spans). No fuzziness here — typo tolerance corrupts statute numbers. Allow fuzziness 1 only on a
> party-name lane if one is added later.

Findings, in order of how much they bear on the sentence:

1. **The capability assumption in the spec is correct.** Fuzziness exists in the pinned crate, with
   named parameters, on the documented query path. The spec did not invent it.
2. **The conditional is satisfiable — but only off this lane.** It requires a party-name lane with a
   *word* tokenizer over its own column. Such an index does not exist in §7.1 today, so adding one
   is a §7.1 change. "If one is added later" is therefore satisfiable, but the sentence does not say
   that the lane needs its own index — which is the part an implementer will get wrong.
3. **The sentence implies a cost that does not exist.** It reads as though enabling fuzziness costs
   an extra index. It does not: zero extra bytes, zero extra build. The cost is query-time and
   budget-shaped (§3.2). Anyone pricing this lane off §8.3 will over-estimate it.
4. **The sentence omits the guard that actually matters.** The failure mode is not "fuzziness is too
   loose" — it is "`max_expansions` truncates in lexicographic order and silently drops candidates",
   which is §7.3's `fast_search()` failure wearing a different hat. If §8.3 keeps its conditional, the
   conditional should carry the saturation test (§3.4c), not just a distance of 1.

So: **§8.3 should not be removed as unsatisfiable.** It should be corrected. Two changes, both of
which are the maintainer's call:

- State that the fuzzy lane needs its **own word-tokenized index on its own column**, and that
  `cites` stays exact regardless of setting.
- Replace "fuzziness 1" with "`MatchQuery::with_fuzziness(Some(1))` **and** a `max_expansions` sized
  above the in-vocabulary edit-distance-1 neighbourhood", and require the saturation test from §3.4(c)
  in §13.

Optionally, keep a one-line note that on the `cites` trigram index fuzziness is not merely
undesirable but semantically void (§2), since that is the non-obvious part.

---

## Not established (honest gaps)

- **No Rust toolchain in this environment, so nothing was compiled or benchmarked.** I could not
  build `lancedb 0.37.1` and run any of the tests proposed in §3.4, nor measure latency, index size,
  or expansion counts. Every API claim above is read from source or docs, not measured against a
  running index.
  - Two things *were* executed, in Python, because they depend on algorithms rather than on a Rust
    toolchain: the n-gram tokenizer traces in §2 and §3.4(d), ported line-for-line from
    `lance-tokenizer-10.0.0/src/ngram_tokenizer.rs`; and the distance-≤1 ball counts in §3.3,
    enumerated and then verified element-by-element against a Levenshtein implementation. These are
    ports and pure combinatorics, **not** the compiled crate — they confirm the algorithms as read,
    not the crate as linked.
  - The Levenshtein ball counts are **worst-case alphabet counts**, not in-vocabulary counts.
- **The real size of the distance-1 in-vocabulary neighbourhood** for a party-name corpus — i.e. how
  often `max_expansions = 50` actually truncates — is corpus-dependent and unmeasured. §3.3 gives the
  worst-case arithmetic for the alphabet; the intersection with a real vocabulary is unknown.
- **The UTF-8 byte-edit consequence for accented party names** (§1.4.3) is inferred from the FST key
  type being `Vec<u8>` plus `ascii_folding`. Not confirmed by execution.
- **Whether fuzzy expansion works on the flat-scan fallback path** (unindexed rows, the thing
  `fast_search()` opts out of) was not traced. Given `collect_fuzzy_candidates` needs an index FST, I
  expect the fallback either skips expansion or errors, but I did not verify.
- **The version in which fuzziness first shipped** was not determined, and 0.38.x was not examined.
  Immaterial for a repo pinned to 0.37.1, but stated for completeness.
- **Whether fuzziness is combined with the remote / server-side path.** basemind uses local tables
  (`src/lance/mod.rs:204` connects to a local `uri`), so this was not pursued.
- **Bonus, out of this ticket's scope but material:** spec §7.1's builder calls are not callable on
  0.37.1. `FtsIndexBuilder::default()` exists (`InvertedIndexParams::default()` →
  `new("simple", English)`, `tokenizer.rs:532-536`), but `with_position` is the **only**
  `with_`-prefixed method in the entire `tokenizer.rs` (`:687`). So `with_base_tokenizer`,
  `with_language`, `with_stem`, `with_remove_stop_words`, `with_ascii_folding`,
  `with_max_token_length`, `with_custom_stop_words`, `with_ngram_min_length`, and
  `with_ngram_max_length` — all shown in §7.1 — do not exist. The real names drop the `with_` prefix:
  `base_tokenizer`, `language`, `stem`, `remove_stop_words`, `ascii_folding`, `max_token_length`,
  `custom_stop_words`, `ngram_min_length`, `ngram_max_length` (builder surface `:639-834`). Flagging
  it because §7.1 is the sibling section of the one under review and the same pin governs both.
  **Not verified by compilation** — no toolchain — but the absence of the `with_` names is a
  single-grep fact about the crate source, so it does not depend on building anything.

---

## Sources

Primary, all either the pinned crate source from crates.io, docs.rs at the pinned version, or the
repository's own code.

| # | Source |
| --- | --- |
| 1 | `lancedb 0.37.1` crate source — <https://crates.io/crates/lancedb/0.37.1> (`src/query.rs:453,574`; `src/index/scalar.rs:63-66`; `src/table/datafusion/udtf/fts.rs:568-597,1592-1625`; `Cargo.toml:310-313`) |
| 2 | `lance-index 10.0.0` crate source — `src/scalar.rs:98-176`; `src/scalar/inverted/query.rs:12-71,283-376,803-813`; `src/scalar/inverted/index.rs:519-523,871-890,998-1044,2393-2424`; `src/scalar/inverted/tokenizer.rs:532-536,639-834,910-1010`; `src/scalar/inverted/tokenizer/document_tokenizer.rs:54-66,110-126`; `src/scalar/inverted.rs:40-67` |
| 3 | `lance 10.0.0` crate source — `src/io/exec/fts.rs:386-394` |
| 4 | `lance-tokenizer 10.0.0` crate source — `src/ngram_tokenizer.rs:76-216` |
| 5 | docs.rs, `lancedb 0.37.1` — <https://docs.rs/lancedb/0.37.1/lancedb/query/struct.Query.html>, <https://docs.rs/lancedb/0.37.1/lancedb/index/scalar/struct.MatchQuery.html>, <https://docs.rs/lancedb/0.37.1/lancedb/index/scalar/struct.FullTextSearchQuery.html>. The 0.37.1 dependency panel independently lists `lance-index =10.0.0`, confirming the `=10.0.0` pin in source #1. |
| 6 | Official LanceDB docs — <https://docs.lancedb.com/search/full-text-search> ("Fuzzy Search" and "Search for Substring" sections), <https://docs.lancedb.com/indexing/fts-index> (FTS parameters) |
| 7 | basemind repo — `Cargo.toml:170`, `Cargo.lock:8243-8246`, `src/search/rrf.rs:7-36,75-109`, `src/search/exact.rs:22-36,54-84`, `src/index/mod.rs:455-471`, `src/mcp/helpers_files.rs:4-9,112-117,154-159`, `src/lance/mod.rs:204` |
| 8 | Spec branch `spec/lexical-document-retrieval` — `docs/specs/0012-lexical-document-retrieval.md` §7.1 (l.133-161), §7.3 (l.176-184), §8 step 3 (l.206-208) |

Every `path:line` reference in sources 1–4 and 7–8 was re-verified against the extracted crate
source and the working tree. Two claims in this note were additionally **executed**, in Python,
because they depend only on algorithms and not on a Rust toolchain: the §2/§3.4(d) n-gram
tokenizer traces (ported line-for-line from source #4) and the §3.3 distance-≤1 ball enumeration.
These are ports and combinatorics, not the compiled crate.

No blog post, benchmark, or third-party comparison is cited as authority for any API claim here.
Nothing in this note rests on a secondary source.