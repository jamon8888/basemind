# ADR-0013: Encryption at rest comes from the volume, enforced as a deployment requirement

- **Status:** Accepted
- **Date:** 2026-10-06
- **Accepted:** 2026-10-06, via wayfinder ticket jamon8888/basemind#45
- **Deciders:** jamon8888 (proposer and decider)
- **Related:** ADR-0012 (multi-lane document retrieval), and the `[pii]` series #4–#12

## Context

The store is written **in clear**. `src/lance/` contains no encryption of any kind, and the
redaction map is not even persisted by basemind: with `strategy = "token_replace"`,
`extract_doc` calls `redact_capturing_rehydration_map` (`src/extract/doc.rs:490`) and the map is
returned to the caller, who encrypts it separately with `basemind vault-encrypt`. So the pseudonymised
`text` column is readable by anyone who opens the file, and pseudonymisation is the only protection
layer standing between stored documents and a disk image.

Two facts make an in-application answer impossible rather than merely undesirable:

- **LanceDB OSS has no at-rest encryption.** It is an Enterprise feature; on object stores the OSS
  answer is bucket-level SSE.
- **Lance requires plaintext to work.** FTS indexes are built over column contents and vector search
  runs over the raw vectors, so column-level encryption would mean indexing ciphertext — which
  removes the capability the encryption was meant to preserve.

The surface that needs protecting is wider than the documents table. `cache_root()` contains the
global content-addressed blob store `cache/blobs/` — which holds **extracted** text, extracted and
embedded once for every byte-identical file on the machine (`src/store_layout.rs:108-112`) — one
Lance store per workspace, the `registry/registry.msgpack` snapshot, and the `.chunk.msgpack`
embedding sidecars. Encrypting only the per-workspace Lance directory would protect the least
interesting part of the surface and leave the extracted text readable.

The default location is `$HOME` — `~/.local/share/basemind` on Linux,
`~/Library/Application Support/basemind` on macOS, `%APPDATA%\basemind\data` on Windows
(`src/store_layout.rs:69-77`) — overridable by `$BASEMIND_DATA_HOME`, which is documented as the
test-isolation seam used by `init_isolated_cache` (`src/store_layout.rs:229-241`).

## Decision

Encryption at rest is a **deployment requirement on the volume below `cache_root()`**, not a
basemind feature, and it is verified rather than assumed.

1. **Scope.** The whole of `cache_root()`: `cache/blobs/`, every per-workspace Lance store, the
   registry snapshot, and the `.chunk.msgpack` sidecars. Encrypting a subdirectory is not an
   accepted configuration.
2. **Enforcement posture.** When `redaction.enabled = true` and the volume cannot be verified as
   encrypted, the **scan warns loudly**. A hard refusal exists only behind an explicit opt-in. The
   default is deliberately not refusal: on Linux `/home` is frequently a separate unencrypted
   partition while `/` is LUKS, so refusing by default would break a stock Linux install, and
   `$BASEMIND_DATA_HOME` is the test seam, so refusal would break every test and every
   temporary-directory workflow.
3. **Verification point.** One reusable check, invoked **at scan** — where the write happens — whose
   verdict is also surfaced as a line in `basemind doctor`. `doctor` keeps its daemon scope
   (pid, version, uptime, `--probe`, `--clear-fatal`, `src/main.rs:188-219`) and gains one store
   line rather than being replaced by a new subcommand.
4. **Platform coverage.** macOS and Windows are **authoritative**: FileVault via `fdesetup status`,
   BitLocker via `manage-bde`. Linux is **best-effort** — resolve the mount containing the data
   home and determine whether the backing device is `dm-crypt`, which is a heuristic.
5. **`unknown` is never `not encrypted`.** Detection that cannot answer reports that it could not
   verify. Treating an unknown as clear would warn on every container, CI runner and network mount,
   and a warning that fires always is a warning nobody reads.
6. **The retrieval constraint binds any replacement.** Encryption must not make retrieval unusable.
   This is what eliminated column-level encryption, and it is the acceptance criterion for whatever
   is retained here.

## Consequences

Easier: the store is protected by a mechanism that already exists on every supported platform and
that basemind does not have to maintain, audit or get wrong. ADR-0012's `cites` precondition is
simplified — its clear-text column holds public authority only (#44), and at rest it is covered by
the volume rather than by pseudonymisation.

Harder: basemind acquires a deployment responsibility it cannot verify on Linux, and a warning
whose absence of meaning must be preserved (see `unknown`). Support must be able to answer "is my
data home encrypted", which is a new question the project did not previously have to answer.
Multi-machine and shared-workspace deployments must place `cache_root()` on encrypted storage
themselves; nothing in basemind enforces it remotely.

No schema bump, no blob-layout change, no new dependency, no feature gate. `cfg!(target_os)` and
`std::process::Command` are both already in use (`src/shells/launcher.rs`, `src/config/root_guard.rs`),
so the platform split adds no new pattern.

## Alternatives considered

- **Column-level encryption inside Lance** — rejected: Lance builds FTS indexes and runs vector
  search over plaintext, so this removes retrieval to protect storage. It also fails the constraint
  in decision 6 by construction.
- **Encrypted-object-store backend (LanceDB Enterprise)** — rejected: it changes the distribution
  story, and the community distribution has to remain fully usable without hosted or proprietary
  services.
- **Refuse to scan when the volume is unencrypted, by default** — rejected: it breaks stock Linux
  installs, where `/home` is routinely unencrypted, and it breaks the test seam. Kept as an opt-in.
- **Ship basemind-managed disk encryption** — rejected: a second security mechanism to maintain,
  with no retrieval benefit.
