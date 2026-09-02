# Phase 5 — MIME and HTML rendering subsystem: checklist

Status: COMPLETE (2026-09-02)

## Scope delivered (plan §19 Phase 5, kata issue y9vb + children)

- `src/ui/rich.rs` — the app-owned rich text model (plan §13: third-party
  renderer types never leak into UI state): `RichLine`/`RichSpan`/
  `RichStyle` (bold/italic/link/code/blockquote/heading flags + per-span
  link target for Phase 8/10 open/copy), the html2text adapter
  (`html_to_rich`), the plain adapter (`plain_to_rich`), blank detection,
  and MIME body selection (`body_lines`, plan §13.2/3: **HTML-preferred**,
  plain fallback when HTML is absent or renders blank; original bodies stay
  untouched on `Message` for retry/debug; no body content is ever logged).
- `src/ui/theme.rs` — semantic style helpers for the §13 table: heading
  (bold, stronger fg), link (accent + underline), code (surface fill),
  blockquote (dim). No literal colors outside the theme.
- `src/ui/screens/reader.rs` — document lines are now `Chrome { tone, text }`
  or `Rich(RichLine)`; the body flows through `ui::rich::body_lines`; spans
  map flags → theme tokens with the link style winning inside blockquotes;
  every span is clipped at the viewport as the final safety net. The
  previous "(HTML message — plain-text rendering unavailable)" degrade is
  replaced by real rendering; the note only appears when HTML exists but
  renders blank and no plain part exists.
- `src/app/reducer.rs` — `Action::Resize` now clamps `reader_scroll` against
  the re-flowed document (`clamp_reader_scroll`), so the anchor can never
  point past the end after a re-wrap (plan §13/§19 Phase 5.6).
- **Renderer/reducer width agreement fix** — `reader::render` previously
  derived its wrap width from the body sub-rect, which re-ran layout mode
  selection on the wrong frame: below 120 columns the reader collapsed to a
  10-column wrap and the reducer's scroll clamp used a different width than
  the renderer. Both now derive the width from `reader_width(state.size)`
  (caught by the Phase 5 corpus tests at 100×30).
- **Attachment metadata bug fixed** — `map::header` matched only the dashed
  RFC header spelling, but mail_parser's serde dump (i.e. real
  `himalaya message read --json` output, per the captured fixtures) spells
  names with underscores (`content_type`, `content_disposition`). The
  Phase 5 duplicate-filenames fixture exposed it: attachment filename/MIME
  extraction silently produced `None` on real wire data. The matcher now
  normalizes `-`/`_` on both sides (message-id lookups collapsed to one).
- `src/backend/himalaya/fixtures.rs` (behind the `test-fixtures` feature) —
  the raw-`.eml` test pipeline: `mail_parser` (the maintained library
  Himalaya embeds, here as an optional dependency) parses bytes → serde
  dump → lenient `MessageReadDto` → `map::message`. Production builds
  (`cargo check`/`build` without features) never link MIME parsing (ADR
  0001: Himalaya owns MIME at runtime); `cargo test --all-features`
  enables the feature.
- `Cargo.toml` — `html2text = "0.17.1"` (html5ever-backed, wraps to width,
  rich annotations) as a runtime dependency; `mail-parser` optional +
  `test-fixtures` feature.
- `fixtures/mail/` — 16 `.eml` fixtures covering every §13 case: plain,
  multipart/alternative, malformed HTML, newsletter (tables/links/tracking
  pixels), receipt, GitHub notification, nested quotes, signature, tables,
  code/pre, long URLs, RTL (Arabic + Hebrew), emoji/CJK, empty body,
  duplicate filenames, large attachment metadata.
- `tests/rich_render.rs` — 15 corpus tests: deterministic double-render at
  three sizes for every fixture; malformed-HTML recovery; heading/link
  style spot checks on the buffer (bold; accent + underline); newsletter
  alt text with remote URLs absent; quote markers; pre whitespace; RTL/
  CJK/emoji survival; empty-body degrade; duplicate attachment chips;
  long-URL token suppression; receipt/GitHub/signature content.

## Semantic mapping in effect (plan §13 table)

| HTML | Terminal |
| --- | --- |
| Heading/strong | Bold (headings additionally use the stronger `text` fg) |
| Emphasis | Italic |
| Link | Accent + underline; target retained on the span |
| Blockquote | Dim, html2text's `> ` left marker as content |
| Lists | html2text markers (`* `, `1. `) with stable indent |
| `pre`/`code` | Whitespace preserved (html2text wraps overly-long pre lines, `Preformat(true)` marks continuations); surface fill |
| Table | html2text width-aware ASCII layout at the reader width |
| `hr` | Rendered by html2text as a rule line |
| Image | Alt text only; `src` dropped |
| Sender colors | Ignored (`Colour`/`BgColour` annotations dropped); Post theme |

## Acceptance criteria (plan §19 Phase 5)

- [x] Plain, HTML, and multipart messages render deterministically —
      `corpus_renders_deterministically` (every fixture × 3 sizes, two
      identical draws each) + 12 unit tests on the model/adapter
- [x] Malformed HTML falls back without crashing — html5ever recovery
      (`malformed_html_renders_recovered_content`,
      `malformed_html_recovers_without_panicking`), blank-render fallback to
      plain (`blank_html_falls_back_to_plain`, `empty_body_degrades_to_note`)
- [x] Remote images/resources cause no network requests — the pipeline is
      pure I/O-free text rendering; `newsletter_renders_alt_text_without_
      remote_urls` asserts banner/pixel URLs never appear and alt text does;
      no HTTP client dependency exists in `Cargo.toml`
- [x] Long URLs/Unicode/RTL never panic or corrupt terminal layout —
      `corpus_renders_deterministically_within_width` bounds every line at
      widths 20–152 (unit level); RTL/emoji fixtures render their glyphs;
      `ui::text` hard-chunks oversized tokens
- [x] Snapshot corpus covers every case in section 13 — all 16 fixture
      classes from §13 are present and asserted in `tests/rich_render.rs`

## Commands run / results

- `cargo fmt --all` — clean
- `cargo check` (production, no features) — clean, mail-parser not compiled
- `cargo check --all-targets --all-features` — clean
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test --all-features` — 232 passed, 0 failed
  (172 unit incl. 12 rich-model tests + 15 corpus + 27 contract +
  2 fake-himalaya + 16 snapshots)

## Notes for Phase 6+

- `RichSpan.link_target` is carried per span but unused by the UI yet;
  Phase 8 (open attachments) / Phase 10 (mouse) consume it.
- html2text inserts single spaces between CJK ideographs (its word-break
  model); glyphs survive, spacing is a known fidelity trade-off.
- `content()` re-renders HTML per call (render + scroll clamp). Keypress-
  driven UI makes this cheap enough for v1; memoize per (message, width) if
  latency shows up in the Phase 12.7 measurement.
- Blockquote dim styling is applied from html2text's `> ` text prefix;
  `code` spans win over the blockquote style so code blocks inside quotes
  keep the surface fill.
- The corpus intentionally renders via the real CLI-independent pipeline
  (mail-parser dump → dto → map → reader). A live-CLI smoke against the
  seeded maildir remains worthwhile at Phase 12.
