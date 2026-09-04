# Bundling Himalaya with Post (ticket xjgx)

Research question: Post currently requires a system-installed `himalaya`
CLI (checked at startup, refusing with an actionable error when missing).
What would it take to ship Himalaya **along with** the app, so the binary
"just works" after installing Post alone? Definition of done for this
ticket is this document: options, pros/cons, and a high-level migration
plan. No code changes are proposed for v1.

## 0. Ground facts (verified)

- Himalaya is a single, self-contained Rust binary; no runtime
  dependencies. License is Apache-2.0 OR MIT — bundling and redistribution
  are permitted with attribution (include the license texts).
- Official release artifacts cover the platforms Post targets and more
  (v2.1.0): `aarch64-darwin` (5.7 MB tgz), `x86_64-darwin`, `x86_64-linux`,
  `aarch64-linux`, plus armv6/7, i686, Windows. Cargo features compiled
  in: smtp, imap, jmap, gmail, msgraph, m2dir, pimdir, maildir,
  rustls-ring — i.e. the pre-built binary covers every backend Post needs.
- Post already gates on the executable at startup (`himalaya --version`,
  config validation) and pins behavior via JSON-schema/fixture contract
  tests (ADR 0001 finding 14), so a bundled binary is version-controllable
  with existing machinery.
- Post's process model (one CLI invocation per operation, argv-only) is
  orthogonal to where the binary lives; only the resolution path changes.

## 1. Options

### A. Status quo — system-installed CLI (current)

User installs himalaya themselves (`brew`, `cargo install`, install.sh);
Post refuses to start without it.

- **Pros:** zero packaging work; users control their own himalaya version
  (it may already serve other tooling); no signing/licensing burden.
- **Cons:** a hard pre-install step; version drift between "whatever the
  user has" and the version Post was tested against; the #1 onboarding
  friction for a non-CLI-savvy user.

### B. Sidecar binary in Post's release artifacts (recommended)

Post's release pipeline ships `himalaya` next to `post` per platform. At
startup Post looks for `himalaya` in the executable's own directory first,
then falls back to `PATH`.

- **Pros:** one-download install; exact version coupling (Post is tested
  against the bundled binary, killing version drift); no runtime
  extraction, no first-run surprises; keeps the ADR 0001 process boundary
  untouched — the adapter code does not change at all.
- **Cons:** release pipeline grows (download asset, verify checksum,
  package per platform, per-arch); Post's artifact carries ~6 MB extra;
  macOS code-signing/notarization should cover both binaries; Post's
  release cadence now gates on bumping a himalaya pin.

### C. Embed the binary in `post` and extract on first run

`include_bytes!` the himalaya binary; write it to Post's data directory
(0o755) at first launch.

- **Pros:** literally one file to distribute.
- **Cons:** all of B's packaging work *plus* first-run extraction:
  binary grows ~6 MB, an executables-writing step that macOS Gatekeeper
  and enterprise AV policies dislike, cache-invalidation logic on upgrade
  (extracted copy must always match the Post version), and a data-dir
  dependency at startup. Strictly worse than B unless the distribution
  channel demands a single file.

### D. In-process backend via the Pimalaya crates (no CLI at all)

Implement `MailBackend` directly over `email-lib`/transport crates —
"builtin backend" in the strongest sense. Removes the process boundary,
spawn latency, and JSON DTO layer entirely.

- **Pros:** fastest possible operations and true session reuse; no
  external binary; typed errors end to end.
- **Cons:** contradicts ADR 0001 decision 1 (no Pimalaya library
  dependency; revisit only with measured evidence and explicit user
  approval, plan §22); Post would re-implement behaviors it currently gets
  from the CLI for free — send-outcome classification, pagination mapping,
  trash-first semantics, JSON error shapes (ADR 0001 findings 1–13);
  upgrade churn moves from a pinned binary to an API; the measured
  evidence (5 ms/invocation on maildir; IMAP handshake addressable via
  sirup, see cache.md §1.2) does not currently justify it.

## 2. Recommendation

- Keep **A** for development and source builds.
- Adopt **B** when Post ships its first real release artifacts (the
  natural moment is the v1 release, after the Phase 12 gate).
- Keep **D** parked behind the plan §22 evidence gate; revisit only if
  measured IMAP behavior makes the CLI model unusable *and* the user
  approves the scope change (cache.md documents the cheaper sirup
  alternative for the handshake cost).

## 3. Migration plan for option B (high level)

1. **Pin**: add `HIMALAYA_VERSION` (single constant, e.g. in
   `Cargo.toml` `[package.metadata]` or a build script env). The startup
   version check compares the bundled binary's `--version` against it.
2. **CI bundling job**: matrix job downloads the official
   `himalaya.<target>.tgz` matching each release target, verifies the
   published checksum, and packages `himalaya` beside the `post` binary
   into the per-platform artifacts.
3. **Runtime resolution**: `HimalayaCliBackend`'s program resolution
   becomes: (a) explicit config override (already possible via the
   program string), (b) `himalaya` sibling of the current executable,
   (c) `PATH`. A new contract test pins the order.
4. **Legal**: distribute the Apache-2.0/MIT license texts + attribution
   notice inside every artifact.
5. **Upgrade cadence**: bumping the pin re-runs the existing JSON-schema
   regeneration and fixture contract suites (fixtures/himalaya/schemas)
   before the release is cut — version drift is caught by CI, not users.
6. **Rollback**: resolution falls back to `PATH`, so a broken bundle
   degrades to today's behavior (clear startup error), never a crash.

## 4. Risks

| Risk | Mitigation |
| --- | --- |
| Arch mismatch (aarch64 vs x86_64 mac) | CI matrix ties the himalaya asset to the exact release target |
| macOS Gatekeeper on a second unsigned binary | notarize/staple the whole artifact; document `xattr -d com.apple.quarantine` fallback |
| Upstream breaking JSON/CLI changes | existing schema/fixture contract tests run against the pinned version before release (ADR 0001 finding 14) |
| Artifact bloat complaints | ~6 MB per artifact; option C avoided; gzip keeps tarballs at release sizes already shown in §0 |
| License compliance drift | license texts added by the same CI step that bundles the binary |
