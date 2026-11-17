# ADR-0015: The privacy gateway boundary — what the agent may not reach

- **Status:** Accepted
- **Date:** 2026-10-22
- **Deciders:** basemind maintainers
- **Related:** ADR-0011 (MCP tool surface redesign), ADR-0014 (CLI/MCP parity guard)

## Context

The privacy gateway (the *passerelle*) becomes its own component. basemind stays the engine:
detection, extraction, the `safe/` mirror and the index. The gateway owns the true values —
it keeps the keys in the system keychain — and the agent must see tokens only.

Four things in basemind sat across that line.

1. **The two tools that hand out the true values were on the agent's surface.** `redact_text`
   returns the token→original table and every detection in the clear; `vault` encrypts,
   decrypts and searches that table. Both were advertised as MCP tools and reachable through
   `basemind redact` / `basemind vault`.
2. **The scan path wrote an encrypted copy of that table under a derivable key.** The
   rehydration map was encrypted with a passphrase derived from the workspace scope and the
   data directory — a 64-bit hash of two values the reader can compute — and stored as a
   content-addressed blob in the store. Anyone who knew the scope and the data directory could
   recompute the key and decrypt the table of every redacted document in the workspace.
3. **The detection could not be told about the case's own vocabulary.** A lawyer renames the
   parties (« Dupont » becomes « Orme ») and adds the file's own terms (« Dossier Orion », an
   address). Nothing could carry that dictionary into the detection.
4. **A SIREN was classified as a phone number.** The engine files any run of seven to fifteen
   digits as a phone number, and a SIREN is nine digits. The legal caller acts on the label,
   and a phone number is not the one category it may forward in the clear; a company
   identifier is.

## Decision

### 1. The mirror is the boundary

A workspace that carries a `safe/` mirror is **protected** (`src/protected.rs`). Inside one:

- `redact_text` and `vault` are not advertised by `tools/list`, are invisible to
  `get_tool`, and a call to either is refused with the reason;
- `basemind redact` and `basemind vault` exit `2` (usage) with the same reason;
- the four upstream resources (`basemind://status`, `basemind://repo/map`,
  `basemind://outline/{path}`, `basemind://memory/{key}`) are refused and their templates are
  not advertised — what they serve is not masked by the mirror, so reading them would hand the
  agent exactly what the boundary withholds.

The marker is the mirror's presence, not a configuration key: the gateway writes `safe/` when
it writes the redacted copy, so both sides already agree on the fact. The check fails closed
(a `safe` symlink marks the workspace too).

The gateway still needs the engine, so it marks the basemind process it owns with
`BASEMIND_GATEWAY_TOKEN`. That is a capability, not a flag: the gateway generates the value
and keeps it out of the agent's sandbox environment. Without the capability, the two tools
stay available to every workspace that has no mirror — a plain code index is unaffected.

**The boundary the capability draws (and the one it does not).** The token closes the gap
between «the tool exists in the engine» and «the agent may reach it» *for a basemind process
the gateway launches from configuration*. What it cannot stop is a process the agent itself
launches: an agent able to spawn arbitrary processes runs its own basemind — with or without
the token, with or without a workspace carrying a mirror — and no environment check can
distinguish a spawner the gateway chose from one the agent chose, because both carry the
agent's own OS credentials. Moving to a credential basemind «verifies» changes nothing there:
anything the gateway can hold, the agent's same-user processes could read equally. Stopping
that case is the job of the OS sandbox the agent runtime provides (the gateway is what
provisions it), not of basemind. The capability is the cooperation contract between two
components the user administers, and it degrades into a symbol when the sandbox fails; that
residual risk is on record, and the withheld-by-default direction is what keeps the failure
of one component from exposing every workspace.

### 2. basemind keeps no key at rest

The scan path no longer encrypts or persists a rehydration map, and the passphrase derived
from the scope and the data directory is gone. The map is captured in memory for the length
of the extraction and dropped with the document. `rehydration_ref` stays `None`; the field and
its index column remain so the schema does not move, and the blob GC reaps the blobs older
workspaces left behind.

This is the "remove the rehydration from basemind" branch of the choice — which restores what
ADR-0013 (encryption at rest comes from the volume) already recorded as the state it premised:
basemind does not persist the rehydration map. The gateway's own
vault keeps the keys in the system keychain, and the gateway reaches the map through the
`redact_text` result it already receives — not through a blob in basemind's store.

### 3. The case dictionary is an input to the redaction call

`redact_text` takes a `pseudonyms` list of `[value, code_name]` pairs (the CLI takes
`--pseudonym value,code-name`). The dictionary is an equivalence class: every member of it is
rendered as the code name. A value the engine found becomes its code name instead of a token;
a value the engine did **not** find is replaced in the text directly; and a code name the
engine redacted on its own is put back. The rehydration map is left untouched, so the caller
keeps a reversible record of the true values.

The dictionary is an input to the call, not a configuration key: the scan path re-derives its
text from the index and must preserve byte offsets, and a code name has a different length
than a token.

### 4. A company identifier is a `tax_id`, not a `phone`

A finding whose matched value passes the SIREN/SIRET Luhn checksum is re-labelled `tax_id`
(`src/pii/tax_id.rs`, `src/pii/siren.rs`). The re-labelling happens *after* the engine has
run: xberg resolves an overlap in favour of the earlier, longer match and the built-in phone
pattern always comes first, so a custom pattern for the same span would lose the tie. The
value is the only thing that can tell the two apart, and it is gone once the text is
redacted — hence the two routes the re-labelling takes:

- the tool path reads the value from the pre-redaction text;
- the scan path reads it from the rehydration map (token → value).

In the tool path the token is renamed too (`[PHONE_3]` → `[TAX_ID_1]`), in the text and in the
map, so the mirror carries the label the caller acts on. The scan path renames nothing: the
token's length is part of the chunk byte offsets the index stores.

## Consequences

- The agent-facing surface depends on the workspace, not on the build. A code workspace sees
  the same tools it saw before; a `safe/` workspace sees fewer.
- The two tools are still there for the gateway, and only the gateway can hold the capability.
  A CLI flag would have been the same boundary with a name the agent could read; the token's
  value is one the agent does not have.
- The upstream resources are refused in a protected workspace. Outside one they are served as
  before, which is what a plain code index needs.
- basemind no longer stores anything an attacker with the scope and the data directory could
  decrypt. The rehydration blobs older workspaces wrote stay on disk until the GC reaps them;
  they are no longer written.
- The case dictionary changes the shape of the redacted text, not the rehydration map. A
  caller that re-hydrates by token will not find the tokens it expects for a pseudonymised
  value — the code name is in the text, and the true value is in the map under the token.
- The SIREN label is a *re*-label: a nine-digit run that passes the Luhn test is treated as a
  company identifier even when the document meant a phone number. The checksum is the filter;
  the label only decides what a caller may forward.

## Alternatives considered

- **Remove `redact_text` and `vault` outright.** Rejected: the gateway needs the engine, and
  a plain workspace has no reason to lose the tools.
- **Gate on a configuration key instead of the mirror.** Rejected: the mirror is a fact both
  sides already share; a key is one more thing the gateway has to remember to set.
- **Gate the CLI on the environment alone.** Rejected: an environment variable is inherited
  by whatever the agent spawns, so it cannot be the thing that separates the gateway from the
  agent. The capability is read from the environment, but the boundary is the workspace.
- **Let the gateway decrypt the blob basemind wrote.** Rejected: the key was derivable from
  public values. A random per-install key would have been the alternative, but it would have
  left basemind holding a secret that only the gateway needs.
- **Detect the SIREN with a custom pattern.** Rejected: the built-in phone pattern wins the
  overlap on the same span, so the custom pattern would never fire.
- **Put the dictionary in `basemind.toml`.** Rejected: the scan path would have to rewrite
  chunk text of a different length, and the offsets the index stores would no longer hold.
