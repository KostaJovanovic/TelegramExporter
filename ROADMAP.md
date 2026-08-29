# Telegram Exporter — design record

What is built, what is next, and what is deliberately still open. `AUDIT.md`
holds what reviews and live runs actually found. `CLAUDE.md` holds the working
rules.

The product is a Rust exporter that reproduces **Telegram Desktop's export
format byte for byte**, plus the one thing Desktop cannot do: forum supergroups
split into one folder per topic.

---

## Status

| Area | State | Evidence |
|---|---|---|
| Oracle harness | **done** | three legs: `tgx-parity json\|html\|media <root>` |
| `tgx-format` | **done** | JSON leg: 4 of 4 topics byte-identical, 6,643 messages |
| `tgx-html` | **done** | HTML leg: 4 of 4 topics reproduced exactly, 256,780 lines |
| `tgx-media` | **done** | Media leg: 830 of 836 filenames; the six are the custom-emoji ceiling |
| `tgx-tg` | **run at the wire, 2026-08-27** | a live export, diffed against a Desktop export of the same supergroup. It found four features nothing emitted. See `AUDIT.md`. |
| `tgx-ui` / `tgx-app` | **done, and about to be replaced** | every interaction rule is implemented and the window was driven to confirm it. See "The egui swap". |
| Packaging | **done** | release binary 20.4 MB, assets embedded, icon and version resource, CI on `windows-latest`, 30 MB ceiling |

Baseline on the `refactor` branch, 2026-08-29: **574 tests green, three legs
green, corpus sha256-verified**, fmt and clippy clean.

---

## In progress — the refactor, from 2026-08-29

A behavior-preserving cleanup. **Exported bytes must not change**; the parity
legs are the contract and stay green at every step. If a step would change what
gets written, it is out of scope.

- [x] Baseline recorded and branched.
- [x] Detached from the PySide6 original: `CLAUDE.md` rewritten, 198 lines, no
      Python references. Telegram Desktop remains the format target and the
      oracle — that is the product, not an inheritance.
- [x] `ROADMAP.md` and `AUDIT.md` recovered. They were deleted in `ac09f2c`
      while `CLAUDE.md` still instructed readers to consult them.
- [x] **D4 closed.** `tgx-app` carried an unused `grammers-client` dependency
      while `CLAUDE.md` claimed the window depends only on `tgx-ui` + `tgx-tg`,
      and `layering.rs` enforced two rules but not that one. The dependency is
      removed and `tgx_app_does_not_depend_on_grammers` now asserts it.
- [ ] Purge the remaining Python references from code comments (60 across 27
      files). Each becomes a statement of the invariant on Desktop's terms, or
      goes. This includes re-grounding the escaping tests in
      `tgx-format/src/json.rs`, whose stated authority is CPython's
      `json.dumps` rather than anything Desktop does.
- [ ] Split the oversized modules. `engine.rs` and `convert.rs` are 1,749 lines
      each; `shell/mod.rs`, `download.rs`, `config.rs` and `list.rs` are all
      over 800. Order is `tgx-format`/`tgx-html`/`tgx-media` first, because the
      legs cover them directly, then `tgx-tg`, then `tgx-app` last.
- [ ] Thin the comments. 22.4% of the workspace is comment lines, much of it
      narration of past incidents rather than the rule that came out of them.
      Keep the invariant and one line of why; drop the story.
- [ ] Audit the tests and cut what restates the implementation or cannot break.
- [ ] The harness itself, last, once nothing else is moving. `wire_leg.rs` is
      1,114 lines.

The workspace stays at seven crates. The layering it encodes is load-bearing —
`tgx-html` must not know Telegram's wire types, or the harness could no longer
replay a recorded `result.json` through it.

---

## Next — the egui swap

**Decided 2026-08-29: GPUI and `gpui-component` are replaced by egui.** Pure
Rust, immediate mode, no web stack. Sequenced *after* the refactor above, which
means roughly 4,500 lines of view code get refactored and then rewritten; that
cost was weighed and accepted.

What survives untouched: the exporter crates know nothing about the UI, and
inside `tgx-app` the framework-independent half — `list.rs` (fold, sort and
filter, with a single GPUI reference in 875 lines), `queue.rs`, `actions.rs`,
`journal.rs` — is roughly 3,500 lines that ports as-is.

What has to be rebuilt: `shell/*`, `main.rs`, `theme.rs`, `settings_form.rs`
and the whole of `tgx-ui` — about 4,500 lines.

Constraints carried into it:

- **The design tokens and the bundled Geist fonts both stay.** The look should
  be near-identical: same palette, spacing scale, type scale and typeface, at a
  cost of 338 KB in the binary. egui must load the family properly.
- **A worker event still repaints the window; the window never polls.** egui is
  immediate-mode, so the trap changes shape rather than disappearing: the app
  must request a repaint when a `Progress` message arrives, and must *not* sit
  in a continuous-repaint loop burning the GPU while an export runs.
  `bridge.rs` stays the seam, and `Bridge::drain` stays `#[cfg(test)]`.
- **One writer per fact** survives unchanged — `Shell::set_count`, and the queue
  owning what a run did.
- The transcript stays a 2,000-line ring, so the INCOMPLETE warning can still be
  scrolled to.

Expected side effect: the dependency graph is 544 crates today, most of it
GPUI's renderer, shaper and asset stack. It should fall by an order of
magnitude, and the binary with it.

---

## Still open, and deliberately

Stated rather than hidden. None of these is a defect being ignored.

**Blind spots in the oracle**

- **The html leg proves the writer, not the pipeline.** `html_leg.rs` lifts the
  presentation-only `_p` map out of Desktop's own pages and feeds it back in, so
  the leg reads 4 of 4 whether or not anything upstream builds that map. Nothing
  did, for a long stretch: a live export had **zero `<img>` elements against
  Desktop's 649**, with the leg green over it. `convert::presentation` now
  builds it and `crates/tgx-tg/tests/wire.rs` covers converter-in/map-out, but
  the leg's blind spot is structural — lifting the map is what lets it run with
  no connection, which is the whole reason it exists.
- **Three entity types are absent from the corpus.** Counted across all 6,643
  messages: 3,916 `plain`, 130 `mention`, 53 `link`, 7 `mention_name`, 5 `bold`,
  2 `email`, 1 `phone`, 1 `custom_emoji` — and **zero `hashtag`, zero `cashtag`,
  zero `bot_command`**. Those three are exactly the inline types that build a
  `ShowHashtag(...)` / `ShowCashtag(...)` / `ShowBotCommand(...)` JavaScript
  call (`tgx-html/src/inline.rs`), the one place where chat text is interpolated
  into a JS argument rather than into markup. The `js_str` + `esc` guard there
  is covered by **unit tests only and by no leg at all**. Closing it needs a
  second corpus from a chat that uses hashtags and bot commands.
- **The inline preview's bytes are ours, not Desktop's.** `<stem>_thumb<ext>` is
  a third artifact, distinct from Telegram's `_thumb.jpg` and from the stripped
  thumbnail. Desktop renders it locally with an image scaler; we take Telegram's
  next size down, or copy the full file when there is none. Path and role match,
  bytes do not, and the media leg diffs names — so nothing goes red.
- **`custom_emoji.document_id` stays a numeric id, not a sticker path.** This is
  the media leg's 830-of-836 ceiling. A JSON replay cannot see custom emoji.
- **Two of the three peer shapes have never been run.** `enrich::fetch_chat_info`
  and the roster branch on `InputPeer::Chat` (basic group) and `InputPeer::User`
  (private chat) are argued from `api.tl` and have never been exercised: every
  live run has been the same forum supergroup, which takes the
  `InputPeer::Channel` arm. Closing it needs a live export of a basic group and
  of a one-to-one chat, not a test.

**Known output defects against Desktop**

From the 2026-08-27 filesystem comparison of a live export against Desktop's own
(`DEFECTS.csv` is the register, with severity, source location and — the useful
column — what would have caught each). These are *defects*, not decisions —
they are listed here because none is fixed and several are invisible to every
leg. Ordered by cost. The four marked **verified 2026-08-29** were re-checked
against the current code during the refactor; the rest are as recorded on
2026-08-27 and have not been re-confirmed.

1. **Photo previews are the wrong size — 54 MB of divergence.**
   `photos/*_thumb.jpg` is 72.87 MB against Desktop's 18.68 MB; the full-size
   photos are byte-identical, so 100% of the gap is previews. Desktop scales
   every preview to 520 px on the long edge. We pick a `PhotoSize` variant by
   **byte size rather than pixel box** (`download.rs:650`), so results land on
   Telegram's standard boxes and never on 520: 552 previews come out larger than
   Desktop's (median 3.1×, worst 22×) and **13 come out smaller**. Not "more
   pixels" — the wrong ones. **Invisible to every leg by construction:** a photo
   preview is named in no `result.json`, appears only as an `<img src>` in the
   HTML, and there the string matches. Only a filesystem comparison finds it.
   *(verified 2026-08-29)*
2. **Link previews become photos that were never downloaded.** Desktop writes no
   media key at all for `MessageMediaWebPage`. We promote the preview image to a
   top-level `photo` (21 messages), and on two more manufacture a whole
   `file` / `media_type: video_file` / `duration_seconds` block out of a YouTube
   link. All 21 fail to download, leaving 21 dangling JSON references and 42
   broken `<img>` tags. Each phantom photo advances the `photo_N` counter, so
   **474 later filenames shift** — roughly half of all field mismatches.
   `plan.rs` documents this and carries a fix; the compared export predates it,
   so this needs re-confirming on the next live run. Ties into D2 below.
3. **Twenty-eight names still resolve to the empty string.** `inviter` is `""`
   on all 26 `join_group_by_link` service messages, and `members[0]` on 2 of 24
   `invite_members`. The residue of the 206-empty-names bug: `participants.json`
   is clean at 43 of 43, so the roster path is fine and the service-message path
   is not.
4. **Sticker previews are byte-identical copies of the sticker.**
   `sticker (1)_thumb.webp` hashes the same as `sticker (1).webp`, every sticker
   in every topic — the same selector as (1), taking the branch that copies the
   full file when Telegram advertises nothing smaller. It also writes
   `AnimatedSticker_thumb.tgs`, a Lottie file offered as an image preview.
   *(verified 2026-08-29)*
5. **Reactions carry too much.** Desktop caps the recent-reactor list at
   **three** and appends `<span class="count">TOTAL</span>` when there are more
   reactors than avatars — correct in all 95 spans it emits. We emit the full
   list, up to eleven, and **never emit a count span**; Desktop also omits
   `recent` entirely on 19 reactions where we always write it. 209 messages
   differ. More data rather than wrong data, but not Desktop's format.
   *(verified 2026-08-29)*
6. **No media de-duplication.** Desktop reuses one file when the same document
   appears twice, collapsing 46 references onto 20 basenames. We are strictly
   1:1 — 9 redundant photos (3.33 MB) and 74 `file` plus 74 `thumbnail` paths
   shifted.
7. **Userpics, two cosmetic byte-level mismatches.** Desktop's colour classes
   are `{1,2,4,5,6,7,8,19}`; ours are `{1,2,4,5,6,7,8,18}` — we emit `userpic18`
   494 times, a class Desktop never uses. `style.css` defines only 1–8 so both
   render unstyled. Separately **29 initials are wrong and we disagree with
   ourselves**: the same person renders `A` in one place and `AR` in another,
   because one path uses Desktop's rule (first + last initial) and another
   splits the display name on whitespace.
8. **Coordinates are not rounded.** Desktop writes `44.857507`; we write
   `44.857507352853844` (`convert.rs:752`). Two messages, twice each — once in
   `location_information`, once in the map URL. The only floats in the corpus.
   *(verified 2026-08-29)*
9. **`export_results.html` drops three fields it already has.** `topic_closed`
   is `true` for two topics in our own JSON and the page shows only `pinned`.
   The topic creator is populated and not rendered. The invite link is populated
   and not rendered — that last is arguably right on purpose, being a live
   credential to a private group, but the first is a plain defect.
10. **Metadata disagrees with the file on disk for four photos** served as
    `photoSizeProgressive`.

**Undecided by choice**

- **D1 — does this tool track Desktop, or supersede it?** `fetch_reactors` and
  `fetch_poll_results` are now wired, which is a deliberate step *away* from
  what Desktop records. That direction has not been ratified.
- **D2 — what should a link preview's document do?** With `link_previews` on, a
  YouTube link is exported as a `video_file` with a real filename and a 14 MB
  size, so a message the sender wrote as a link becomes, in the archive, a video
  they appear to have sent. Desktop writes no media at all here.
- **D3 — pin `grammers-*`?** `gpui` is pinned because a bump breaks the
  interface, which is a loud compile error. `grammers-tl-types` is pinned by
  caret, and a bump inside 0.10.x can re-shape a TL field, compile cleanly (the
  converter matches only the variants it knows), and quietly change
  `result.json`. That is precisely the class no test here catches.
- **A sender's name is stamped as it stands now, not as it stood per message.**
  Desktop records the name a person had *at each message*; we hold one name per
  peer. Two users in the reference carry two names each in Desktop's export and
  one in ours. Changing this interacts with the wire leg's rename bucketing,
  which would otherwise file the difference as a rename.
- **~20 service-action payloads are not emitted.** Only the names are.
  `custom_action`'s message, `phone_call`'s duration, `gift_code`'s slug and the
  rest would have to be reproduced against grammers' TL shapes with no oracle,
  and the wire leg's `extra` tally would score any wrong key.

**Environmental**

- **Hairlines at fractional DPI** are verified at 100% scaling only. At 100% a
  1px rule is exactly one device row of `#333333` with no bleed. 125 / 150 /
  200% still need somebody to change the display scaling and look.
- **No diagnostic hook for a hung window.** `startup-error.log` covers a failure
  to start and distinguishes a panic after the window opened, but neither is a
  freeze: a wedged render thread leaves nothing behind to read.
- **The binary is unsigned.** Deliberate — there is no certificate — but a user
  who *downloads* it meets SmartScreen on first run. Copying `dist\` over a
  share or a stick carries no mark-of-the-web and does not trip it.
- **`chat_concurrency` has no UI**, deliberately: the engine exports one chat at
  a time, and a control that is enabled and does nothing teaches that the
  interface is unreliable. The setting still loads and clamps, so an existing
  file opens.

---

## Deferred — the Analyser

Scoped now so the crate boundaries stay drawn correctly. It shares the export
reader and the design tokens with the exporter and **nothing else** — no
Telegram, no MTProto, and no UI framework in the report path itself.

- [ ] `read.rs` — find `result.json` wherever it sits, so both our layout and
      Desktop's `chats/chat_<id>/` load without a flag. Two decisions every
      metric inherits: anything with a clock face on it reads local `date`;
      anything measuring a duration reads `date_unixtime`, which stays monotonic
      across a DST change. A sender is a typed peer key, never a name.
- [ ] `identity.rs` — one person is one peer id, not one display name.
- [ ] `metrics/` — activity, content, conversation, graph, people, extras.
      Output a plain nested structure so the report's table-twin of every chart
      is mechanical and `stats.json` needs no adapter.
- [ ] `charts.rs` — inline SVG. Colour is a CSS variable, never a hex, so the
      light/dark switch is a class and not a re-render. One baseline per mark;
      never two measures on one plot with two scales.
- [ ] `report.rs` — **one file.** Fonts base64'd into the stylesheet, charts
      inline, ~20 lines of script. About 550 KB for a 6,600-message archive, of
      which 260 KB is the type.
- [ ] `events.rs` — the annotation layer and the `--digest` brief. Everything in
      `events.json` is untrusted and escaped like any other outside string; an
      event citing no messages is marked, and low confidence is drawn hollow
      rather than hidden.
- [ ] Three counts that must not become naive ones: forum top-level messages are
      not replies (2,702 of 3,518 would otherwise be, dragging the median
      response time out to days), reactions given are a floor and the report
      prints how much is attributed, and a short member list says so.
- [ ] A small window — pick a folder, watch a bar, open the report — reusing
      `tgx-ui`.

---

## Risk register

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| 1 | **`N:\` disappears** | The exporter loses its oracle and drops to "asserted" | Both exports are backed up and hashed. `save.bat corpus` cuts a 7.8 MB standalone corpus that all three legs run against with identical results, and `tests/corpus.rs` runs them from `cargo test`. Committing `reference/` stays a deliberate privacy decision: it is verbatim chat history from real people and this repo has a CI workflow. |
| 2 | **grammers is unaudited** and this app holds a bearer credential | Security | The session file stays ACL-restricted on creation. Do not run a live export or read stored credentials without the account holder saying so. |
| 3 | **grammers may lag Telethon on TL layers** | A constructor Telegram added is missing | `Message.raw` plus `Client::invoke` reach anything the high-level API misses, and the `unmapped` fallback means an unknown constructor writes its type name instead of ending an export. |
| 4 | **A TL field re-shapes inside a caret bump** | `result.json` changes quietly; no test catches it | Open as D3 above. |
| 5 | **The UI needs a working GPU** | A machine the app should run on shows a blank window | Survives the egui swap: egui still renders through wgpu or glow. Keep the legible failure path and the README note. |
| 6 | **Colour fringing on light text over near-black** | The exact problem the Qt original had, which was fixed there by abandoning DirectWrite | Measured at **0.0% of inked pixels** under GPUI with `tools/measure_fringing.py`, taken on a *rendered window* rather than an offscreen paint — under FreeType those two disagreed 0% against 90%. **Must be re-measured after the egui swap**: it is a property of the rasteriser, and the rasteriser is what changes. |
| 7 | **No diagnostic hook for a hung window** | A freeze leaves nothing to read | `tokio-console` covers the async side; the render thread needs its own. Unbuilt — see "Still open". |
| 8 | **Scope illusion** — "it's just a rewrite" | The format work is most of the value and none of the visible progress | Every phase gates on a diff, not a demo. |

Retired: GPUI and `gpui-component` being pre-1.0 (both pinned exactly, and both
about to be removed); GPUI shipping no text input (`gpui-component`'s `Input`
resolved it, and egui has `TextEdit` built in); JSON escaping differing silently
(pinned by the json leg).
