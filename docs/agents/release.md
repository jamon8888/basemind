# Release runbook

The ordered sequence for cutting a release from `main`. The surface list, the
bump cadence and the tag rules live in the
[release-versioning rule](../../.ai-rulez/rules/release-versioning.md); the
policy gates live in [ADR-0027](../adr/0027-single-breaking-release.md). Both are
linked from each step rather than repeated here.

Two words carry through this document and the rule: **the bumper** is
`mise run release:sync-version`, the only sanctioned way to move a version; **the
guard** is the publish workflow's `meta` job check that fails a release whose
surfaces disagree.

## 1. Clear the gates

Read [ADR-0027](../adr/0027-single-breaking-release.md) rules 5 and 6. The gates
— plugin skew check, `tests/upgrade_from_release.rs`, CI green on Windows, and the
harden harness — must pass on the commit you intend to release, and the owner must
give the go-ahead. Green gates permit a release; they do not start one.

One of those gates is not yet implemented. The skew check — every name in
`TOOL_NAMES` of `plugin/hooks/session-start` advertised by the binary started
through the plugin launcher, and each tool's example call succeeding — has no test
anywhere in the repository. `plugin/hooks/session-start` probes that `basemind
status` runs and reports a file count, and advertises the whole `TOOL_NAMES` list
regardless of what the binary actually serves, so a tool that exists in neither
list goes unannounced rather than caught. ADR-0027 requires the check before a
release; until it exists, that gate is unmet and must be reported, not assumed.

For a release whose minor component changes, confirm `RELEASE_MINOR` in
`src/version.rs` still holds the previous minor: the bumper advances it, and that
advance is what forces every user's rescan.

**Done when:** every gate in ADR-0027 rule 5 passes on the intended commit, and
the owner has said go.

## 2. Promote the changelog

Move `## [Unreleased]` in `CHANGELOG.md` to `## [X.Y.Z] - YYYY-MM-DD`, keeping the
`### Added` / `### Changed` / `### Fixed` grouping prescribed by
[CONTRIBUTING.md](../../CONTRIBUTING.md). A minor release that rescans says so in
its heading.

**Done when:** no `## [Unreleased]` heading remains and the new heading names the
version this release will carry.

## 3. Run the bumper

```sh
mise run release:sync-version X.Y.Z
```

The version is a **positional argument** — `release:sync-version 0.40.0`, not
`VERSION=0.40.0`. The task rewrites every surface the guard checks plus the
plugin manifests it does not, regenerates the config schema snapshot, advances
`RELEASE_MINOR` when the minor changed, and re-validates every surface it touched.

Run it on `main`, on the exact tree that will be released — not on a side branch,
and not by replaying a bump commit as a patch. It requires `ai-rulez` and `jq` on
`PATH`.

Two things it depends on, both easy to break:

- The shipped plugin bundle is generated from `plugin/`, whose config carries every
  runtime. Generating from the repository root fails — that config declares only the
  Hermes runtime, whose entrypoint moved into the plugin tree.
- The four vendored crates under `crates/` carry their own version lines and are
  published under their `basemind-*` names. The root version does not apply to them.

**Done when:** it ends with `✓ All version surfaces are consistent: X.Y.Z` and
`git diff --stat` shows the surfaces moved together rather than one file.

## 4. Verify the generated bundles

```sh
mise run plugin:verify
```

Four freshness checks over the generated plugin output and public exports. A
bundle generated from an older config fails here instead of shipping a manifest
whose content lags its own version number.

**Done when:** all four checks exit 0.

## 5. Land the bump on `main`

Commit the bump and the changelog promotion together, push to `main`, and wait for
CI on that push. The release tag names a commit on `main`; a bump that lives only
on a side branch has not been released, and CI has not run against it.

**Done when:** the commit is on `origin/main` and its CI run is green.

## 6. Tag that exact commit

```sh
git tag -a vX.Y.Z -m "Release vX.Y.Z" <commit-you-just-validated>
git push origin vX.Y.Z
```

Tag the commit the bumper validated, not an ancestor of it. The guard derives the
expected version from the tag and reads the surfaces from that tag's tree, so the
two agree only when the tag points at the bump.

**Done when:** `git ls-remote --tags origin vX.Y.Z` returns the commit
landed in step 5.

## 7. Watch the publish run

```sh
gh run list --workflow=publish.yaml --branch vX.Y.Z
gh run watch <run-id>
```

The `meta` job runs the guard first, then builds, then finalizes the GitHub
release, then publishes to crates.io, npm, PyPI and Homebrew. A `meta` failure
stops everything downstream.

**Done when:** the run completes green.

## 8. Confirm what shipped

Check the GitHub release carries every platform archive and its checksums, and
that each registry reports `X.Y.Z`. The registries are independent: a green run
with one registry missing means that registry skipped, not that it published.

**Done when:** the release page and each intended registry show `X.Y.Z`.

## Recovery: the guard failed

The guard fails in roughly twenty seconds at *Verify version consistency across
all surfaces*, naming the first mismatched file, and nothing downstream runs —
no release, no registry publish. Recovery is therefore safe to redo.

1. Read the `::error::` line: it gives file, expected and got.
2. Re-run the bumper from `main` with the tag's version —
   `mise run release:sync-version X.Y.Z`. Its closing validation is the same set
   of surfaces the guard checks, so a clean run means the guard passes.
3. Land that fix on `main`, then move the tag to it:
   `git tag -f vX.Y.Z <fixed-commit> && git push -f origin vX.Y.Z`.
4. If the push does not re-trigger, dispatch from the Actions tab with the tag
   input; the workflow reads the version from that input.

Force-moving the tag is sound only while nothing published — which is exactly the
state a guard failure leaves. The run is otherwise idempotent: `meta` detects
versions already present on crates.io, npm, PyPI or in the release assets and
skips those jobs, so a re-run continues where the last one stopped rather than
double-publishing.

## The guard is narrower than the bumper

The publish guard checks a subset of the surfaces the bumper rewrites — it does
not read `kimi.plugin.json` or `.agents/plugins/marketplace.json`. A surface it
does not read can still ship a stale version.

The bumper's closing validation is the wider set, so a clean bumper run is the
stronger signal: run it, and the guard follows. When a surface is added, add it
to the bumper's validation in the same change — a surface nobody validates is a
surface that drifts silently.

## The marketplace entries carry no version

Both marketplace listings point at `./plugin` and delegate their version to that
plugin's own manifest, which the bumper regenerates from `plugin/.ai-rulez/`. The
`version` field is therefore usually absent, and both checks read it as
`.plugins[0].version // empty` — an absent field passes, a present one must match.

Reading it as plain `.plugins[0].version` yields `null`, which fails every run.
That is what the checks looked like before, and it is why a release could pass the
guard with the field absent while the bumper also aborted on it.

## Run the bumper on the base you will release from

Replaying a release commit as a diff does not move the versions it already carried.
A bump whose parent tree already had `0.39.0` contributes no `npm-package` line, so
cherry-picking it onto a tree that still read `0.38.0` leaves that surface behind —
one stale surface, guard fails, nothing published.

Land the machinery fixes on `main`, then run the bumper on the result. The bumper's
closing validation is what proves the tree is releasable, and it is the only thing
that can say so.

## References

| Need | Read |
| --- | --- |
| Which files carry the version, and the bump cadence | [Release versioning rule](../../.ai-rulez/rules/release-versioning.md) |
| Release gates, branch policy, who decides | [ADR-0027](../adr/0027-single-breaking-release.md) |
| Changelog entry format | [CONTRIBUTING.md](../../CONTRIBUTING.md) |
| Config schema regeneration | [Development workflow](development.md#change-configuration) |
| Generated plugin copies and their canonical inputs | [Integrations](integrations.md) |
| Workflow ordering invariants | `tests/publish_workflow_smoke.rs` |
