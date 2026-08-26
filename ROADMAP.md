# Telegram Exporter — Rust / GPUI rewrite

A full-scope roadmap for rewriting `C:\Users\Kosta\Projekti\telegram` in Rust on
[GPUI](https://docs.rs/gpui/latest/gpui/).

**Scope decisions taken before this document was written:**

| Decision | Choice |
|---|---|
| Apps in scope | Exporter first, to full parity. Analyser as a later phase in the same workspace. |
| Design language | The Swiss/International language, rebuilt from the ground up against `file-analyser/web/assets/css/analyser.css` as the source of truth — not a port of `app/ui/theme.py`. `app/ui/design.py` and `motion.py` (the unwired Apple direction) are **not** carried over. |
| UI primitives | `gpui-component` for `Input`/`Scrollbar`/`Dropdown` state machinery. All visual chrome hand-built so the design language stays ours. |

---

## Status

| Phase | State | Evidence |
|---|---|---|
| **0** Ground, spike, pin | **done** | Rust 1.97 MSVC; both reference exports on `N:\`; workspace + pinned toolchain |
| **1** Oracle harness | **done** | all three legs: `tgx-parity json\|html\|media <root>` |
| **2** `tgx-format` | **done** | JSON leg: **4 of 4 topics byte-identical**, 6,643 messages, 3.2 MB |
| **3** `tgx-html` | **done** | HTML leg: **4 of 4 topics reproduced exactly, 256,780 lines** |
| **4** `tgx-media` | **done** | Media leg: **830 of 836 filenames**, the six being the documented custom-emoji ceiling |
| **5** `tgx-tg` | **built; verification deferred by decision** | compiles and runs; `tgx login \| chats \| export`. Exit criterion needs a live account — see below |
| **6** `tgx-ui` | **done** | tokens from `analyser.css`, components, empty states; **gpui builds and renders on Windows** |
| **7** `tgx-app` | **built** | window opens and stays up; nav bar, chat list, settings, queue, status bar all live |
| **8** Packaging | **done** | release binary **17.8 MB** (Python: 46.4 MB), assets embedded, CI on `windows-latest` |

### Phase 5: verification deferred, by decision

**Decided 2026-08-26: the exporter ships built-but-unproven at the wire.**
The live-export check was offered and declined. It is not forgotten, it is a
named open item — nothing below is a surprise waiting to be discovered.

What that costs, precisely: `convert.rs` and `plan.rs` are proven to map a
given TL object correctly (16 wire tests in `crates/tgx-tg/tests/wire.rs`),
but nothing proves Telegram sends the shapes those tests assume. Everything
downstream of them is pinned byte for byte by the three parity legs, so the
exposure is narrow and named.

To close it later, two commands and one diff:

```powershell
$env:TG_API_ID="..."; $env:TG_API_HASH="..."
cargo run -p tgx-tg --bin tgx -- login
cargo run -p tgx-tg --bin tgx -- export "UA KOLAB TELEGRAM"
```

then compare the result against our own earlier run on message ids and
size-skip decisions.

### Why it could not be signed off here

The engine, the client, the converter, the topic routing, media planning,
the download pool, the enrichment layer and the streamed output are all in
place, and both a CLI and a window drive them:

```powershell
cargo run -p tgx-app --bin TelegramExporter      # the window
cargo run -p tgx-tg  --bin tgx -- chats
cargo run -p tgx-tg  --bin tgx -- export "UA KOLAB TELEGRAM"
```

The exit criterion — *a live export matching our own reference run on all
6,643 ids and 1,786 size-skip decisions* — needs credentials and a signed-in
account. Using them is the account holder's call, not mine, so this is a
decision rather than a limitation of the code.

**341 tests across 16 suites**; `cargo clippy` and `cargo fmt --check` clean.

### Risks that are now retired

| # | Risk | Outcome |
|---|---|---|
| 1 | gpui/gpui-component are pre-1.0 | Both are in and both are pinned exactly. `gpui-component` 0.5.1 declares `gpui ^0.2.2`, so it builds against the pin. |
| 3 | JSON escaping differs silently | Pinned by the json leg **and** by unit tests checked against CPython's actual output. |
| 8 | **GPUI needs a working GPU** | **gpui builds on Windows in 1m45s and the window renders here** — launched for 10s with empty stderr. |

### Still open

* **Risk 4 (DirectWrite colour fringing)** is unmeasured. GPUI renders through
  DirectWrite, which is exactly what the Qt original abandoned over fringing on
  light text over near-black. The measurement task in Phase 6 has not been run.
* **Risk 2 (`N:\` disappears)** — reduced, not closed, and the step that is
  left is a decision rather than work. See below.

Run the legs with:

```powershell
cargo run -p tgx-parity -- json  "N:\telegram export\UA KOLAB TELEGRAM"
cargo run -p tgx-parity -- html  "N:\telegram export\UA KOLAB TELEGRAM"
cargo run -p tgx-parity -- media "N:\telegram export\UA KOLAB TELEGRAM"
```

### The corpus, and why it is not committed

`tgx-parity corpus <export root>` cuts a standalone corpus into `reference/`:

```powershell
cargo run -p tgx-parity -- corpus "N:\telegram export\UA KOLAB TELEGRAM"
```

It copies only `result.json` and `messages*.html` — 7.8 MB of a 278 MB export,
and the whole oracle. None of the three legs reads a byte of media, the media
leg included: it diffs *names* against the tree the JSON already records.
Against the corpus all three return exactly what they return against the drive:
**json 4/4, html 4/4 (256,780 lines), media 830/836**.
`crates/tgx-parity/tests/corpus.rs` then runs them as ordinary `cargo test`,
after checking every file against a sha256 manifest — because the tempting way
to close a failing diff is to edit the reference, and a reference edited to
agree with us is our own output wearing Desktop's name.

Two things the cut cannot do:

* **It cannot take a slice.** The HTML writer's pagination and joining are
  cumulative down a topic, so page 3 is only reproducible if pages 1 and 2 were
  written first. A corpus is whole topics or nothing — which is why the tool
  ends with a coverage survey naming what each topic uniquely holds. On this
  export the answer is lopsided: `ćaskanje` alone owns polls, voice messages,
  location, custom emoji and seven service actions, while the other three
  contribute no shape that is not already somewhere else.
* **It cannot decide whether to commit itself.** `reference/` is in
  `.gitignore`, because a corpus is verbatim messages from real people and this
  repository has a CI workflow — pushing it publishes them, permanently. So
  here the corpus test *skips*, and says out loud what it did not check.

That is the honest state of Risk 2: the corpus is one command away and the
tests are already written against it, but until `reference/` is committed, CI
proves the unit tests rather than the parity legs. Committing it is a privacy
decision, and not one the tooling should make quietly on someone's behalf.

### What the harness caught that the Python suite could not

* **A one-byte difference in every `result.json` ever written.** Desktop ends
  the file on `}`; the Python exporter appends a newline. Its harness only ever
  replayed JSON *through the HTML writer*, so the JSON emitter was covered by
  tests encoding what we believed. The JSON leg found it on its first run.
* **The doctype is emitted as text, not a tag line** — which is why `<html>`
  follows it with no blank line. Guessed wrong; the reference corrected it.
* **A closing tag returns to its opening tag's indent**, not one level deeper.
  Guessed wrong; the reference corrected it.
* **The poll renderer was missing entirely.** The HTML leg located it in one
  run: 3 of 4 topics exact, and the first differing line named the element.
* **Media folders route on the file's *shape*, not its `media_type`.** Four
  WebM video stickers report `media_type: sticker` and belong in `video_files/`.
  Routing on `media_type` alone also handed them the `stickers/` collision
  counter, so every later sticker shifted — one rule, ten wrong filenames. The
  media leg went 826 → 830 of 836 when it was fixed.
* **`<full name>_thumb.jpg` and `<stem>_thumb<ext>` are two different files.**
  The first is Telegram's own thumbnail and reaches the JSON; the second is the
  downscale Desktop renders for the HTML. Both exist on disk in a real export
  and conflating them loses one.

---

## 0. What is actually being rewritten

The Python app is ~19,000 lines across two programs. The exporter is not a thin
Telethon wrapper — it is a **reimplementation of Telegram Desktop's export
format, verified byte for byte**, plus a superset of the data Desktop discards.

| | lines | what it is |
|---|---|---|
| `app/tg/` | ~4,000 | engine, serialisation, media naming, HTML writer, enrichment |
| `app/ui/` | ~3,200 | Qt interface (+ 1,000 unwired lines of a second design system) |
| `app/` | ~800 | portable config, asyncio bridge, diagnostics |
| `analyser/` | ~3,500 | finished export → one self-contained `report.html` |
| `tests/` | ~3,800 | plain scripts, not pytest |

### The thing that makes this rewrite hard is not the UI

It is that the output is pinned to an oracle. `tools/diff_reference.py` replays
a real Desktop export's own `result.json` through the writer and diffs the
pages: **256,780 lines, 4 of 4 topics, zero differences.** Media naming is
separately pinned at **830 of 836 filenames** against a real run of our own
exporter over the same supergroup.

That number is the acceptance criterion for the rewrite. A Rust exporter that
produces *nice* HTML is a failure; it has to produce *those bytes*.

**Corollary that drives the whole phase order:** in Python, the parity harness
arrived late and immediately found five media-naming bugs that no unit test had
caught, because unit tests encode what you believed. In Rust the harness is
built **first**, before a single line of the writer, so every module below is
developed against a byte diff instead of an assumption. This is Phase 1 and it
is not negotiable.

### The reference corpus is a hard dependency

| path | what it is |
|---|---|
| `N:\telegram export\UA KOLAB TELEGRAM` | Telegram Desktop's own export. 4 topics, 6,643 messages, 9 pages. The oracle for JSON and HTML. |
| `N:\telegram export\UA KOLAB` | Our Python exporter's run over the same supergroup. The oracle for media naming and size-skip decisions. |

**Phase 0 task:** verify both are still on disk, hash them, and copy them
somewhere durable. If `N:\` disappears, the rewrite loses its ability to prove
anything and drops from "verified" to "asserted".

---

## 1. Stack

All versions checked against docs.rs / crates.io in August 2026.

| Concern | Crate | Version | Notes |
|---|---|---|---|
| UI | `gpui` + `gpui_platform` | 0.2.2 (2026-08-15) | Windows via Win32 + DirectWrite. Taffy layout, Tailwind-ish `Styled`. **Pre-1.0, breaking changes between versions — pin exact, never `"*"` as the README suggests.** |
| UI primitives | `gpui-component` | latest | longbridge. 60+ components, `Input`/`InputState`, `Scrollbar`, `Dropdown`. Ships in a commercial product. Also pre-1.0. |
| Telegram | `grammers-client` | 0.10.0 (2026-07-02) | Pure-Rust MTProto. `iter_messages().reverse(true)` **exists** — the single-pass design survives. `Message.raw` is a public field, so the full TL object is reachable. |
| Raw TL | `grammers-tl-types` | 0.10.0 | For everything the high-level client does not cover — forum topics, poll results, reaction lists, custom emoji, invites, scheduled messages. |
| Session | `grammers-session` | 0.10.0 | File-backed session at `TelegramExporterData/session`. |
| Async | `tokio` | 1.x | Required by grammers. Runs on its own thread; GPUI keeps the main thread. |
| JSON | `serde_json` | 1.x, `preserve_order` | `IndexMap`-backed maps + a custom `Formatter` for Desktop's 1-space indent. |
| Images | `image` | 0.25+ | Replaces Pillow. Only used for the downscaled `<img>` preview. |
| HTML parsing (tests) | `html5ever` or `lol_html` | — | The security suite must **parse**, never substring-match. `html5ever` already arrives as a grammers dependency. |
| Embedded assets | `rust-embed` | 8.x | Desktop's `style.css`, `script.js`, 42 PNGs, and the Geist TTFs. Replaces PyInstaller `datas` / `sys._MEIPASS`. |

### Two verified facts that de-risk the plan

1. **`MessageIter::reverse(true)`** — "Changes the order to oldest-to-newest."
   The entire export engine rests on a single oldest→newest pass; had this been
   missing, we would have had to reimplement Telethon's `add_offset` trick over
   raw `messages.GetHistory`. It is not missing. `MessageIter::total()` also
   exists, which is the `Count messages` button.
2. **`Message.raw` is public.** `Message` has 51 fields and Desktop's format uses
   15; the whole "beyond Desktop's format" layer needs the other 36. A
   high-level wrapper with getters only would have forced a raw-API rewrite.

### Two facts that add risk

1. **grammers is not audited.** The maintainer explicitly asks that
   `grammers-crypto` and the auth half of `grammers-mtproto` be reviewed for
   security-critical use. This app holds a bearer credential for the user's
   Telegram account. Phase 0 carries a read of those two crates.
2. **GPUI needs a GPU.** The Qt app ran anywhere. A GPUI app on Windows needs
   working DirectX/Vulkan drivers. This is a *new hardware requirement*, and it
   needs a legible failure rather than a blank window — see Phase 8.

---

## 2. Workspace layout

Crate boundaries are drawn where the Python modules already have no coupling —
`html_writer.py` takes a dict, not a Telethon object, and the analyser shares
nothing with `app/tg`. Both facts become compile-time guarantees here.

```
telegram_rust/
  Cargo.toml                  workspace
  crates/
    tgx-format/               no I/O, no network, no UI
      text.rs                 UTF-16 entity offsets, surrogate snapping
      order.rs                Desktop's key order + our extras
      json.rs                 the emitter, byte-pinned
      peer.rs                 typed peer keys, userpic colour, initials
      size.rs                 truncating byte formatter
      serialize.rs            TL message -> ordered map
    tgx-html/                 dict -> pages. No Telegram types anywhere.
      tree.rs                 Desktop's emission model
      escape.rs               esc(), safe_href(), _js_str, message-number guard
      writer.rs               joining, previews, media rows, replies, pagination
      assets/                 Desktop's css/js/images, verbatim
    tgx-media/                classification, naming, stripped thumbnails
    tgx-tg/                   grammers client, topics, engine, enrichment
    tgx-ui/                   the design system: tokens, type, components
    tgx-app/                  the exporter binary
    tgx-parity/               the oracle harness (bin, not a test)
    tgx-analyse/              PHASE 9 — export -> report.html
    tgx-analyse-app/          PHASE 9 — its small window
  reference/                  the parity corpus, cut by `tgx-parity corpus` (gitignored)
```

**One rule the layout enforces:** `tgx-html` may not depend on
`grammers-tl-types`. In Python this was a convention held by a comment; here the
build breaks. Same for `tgx-analyse`, which must not pull `tgx-tg` — the Python
note "a build that suddenly matches the exporter's size means something started
importing `app/tg`" becomes a dependency assertion.

---

## Phase 0 — Ground, spike, and pin

**Goal: prove the stack works on this machine before committing to it.**

- [ ] Locate, hash and back up both reference exports off `N:\`.
- [ ] Cargo workspace skeleton, `rust-toolchain.toml` pinning a stable version.
- [ ] `cargo run` opens a GPUI window on Windows 11. Confirm the renderer
      initialises and report which backend it picked.
- [ ] `gpui-component` compiles alongside it; render one `Input` and type into it.
- [ ] Spike `grammers`: connect, sign in with the existing api_id/api_hash,
      `iter_dialogs`, and pull 10 messages from the reference supergroup with
      `.reverse(true)`. Print `Message.raw` for one message with a poll, one
      forward and one album to confirm field coverage.
- [ ] Read `grammers-crypto` and the auth path of `grammers-mtproto`.
- [ ] Write `DECISIONS.md` with the exact pinned versions and why.

**Exit:** a window opens, ten real messages print, and the reference corpus is
safe. If GPUI will not render here, that is discovered now and not in Phase 6.

---

## Phase 1 — The oracle, before the code it judges

**Goal: a harness that can say "wrong" long before anything is right.**

`tgx-parity` is a binary, not a test, because it needs a path argument and it
prints a burn-down list.

- [ ] **JSON leg (new — Python never tested this direction).** Read the
      reference `result.json`, feed the parsed values back through our emitter,
      and byte-diff against the original file. This tests key order, indent
      width, `ensure_ascii=False`, the `reactions` over-indent, and escaping —
      independently of everything upstream of it.
- [ ] **HTML leg (port of `tools/diff_reference.py`).** Reference `result.json`
      → our writer → line-diff against Desktop's `messages*.html`. Carry over
      the five lift-outs: untrimmed display name, forward's original timestamp,
      self-chosen reaction, album membership, and each preview's filename.
- [ ] **Media-naming leg.** Replay the reference's message sequence through the
      naming rules and compare against the 836 filenames on disk.
- [ ] **Golden corpus.** Extract a deterministic slice of the reference — a few
      hundred messages covering every media type, every service action reached,
      polls, forwards (visible and hidden), both reply-link forms, albums,
      custom emoji, spoilers, and every size-skip branch — and commit it under
      `reference/`. This is what CI runs; the full 6,643-message diff stays a
      local command.
- [ ] Escaping-parity spike: enumerate where Python `json.dumps(ensure_ascii=False)`
      and `serde_json` can disagree — control characters, U+2028/U+2029, DEL,
      lone surrogates — and pin each with a test. This is the likeliest place
      for a silent byte difference to hide.

**Exit:** the harness runs and reports `0 of 4 topics exact`, `0 of 836 names`.
Failing correctly is the deliverable.

---

## Phase 2 — `tgx-format`: the serialisation core

No network, no UI, no filesystem. Everything here is pinned by Phase 1's JSON leg.

- [ ] `text.rs` — **UTF-16 entity offsets.** Telegram counts in UTF-16 code
      units. Convert to `Vec<u16>`, slice, `String::from_utf16`. Port
      `_snap_cuts` (a boundary landing between surrogate halves moves back to
      the start) and `_drop_lone_surrogates`. *Rust improvement:*
      `String::from_utf16` returns `Err` where Python silently produced an
      unencodable string that killed the export three thousand messages later.
- [ ] `text.rs` — `plain_text`: unwrap `TextWithEntities` **until a string
      appears.** `MessageActionPollAppendAnswer.answer` nests two levels deep and
      that is what ended the first real export at message 5,609.
- [ ] `order.rs` — `_DESKTOP_ORDER + _EXTRA_ORDER + (text, text_entities, reactions)`.
      Key order is part of the format; a differently ordered map diffs on every line.
- [ ] `json.rs` — `serde_json` with `preserve_order`, custom `Formatter` for
      1-space indent, raw UTF-8 output, the deliberate `reactions` over-indent
      (relies on `reactions` being last, which `ORDER` guarantees), and the
      `unmapped` fallback so an unknown TL constructor writes its type name
      instead of ending the export.
- [ ] `peer.rs` — typed peer keys (`user123`/`chat123`/`channel123`; the three id
      spaces collide as bare integers). Userpic colour `(1,8,5,2,7,4,6)[bare_id % 7]`,
      and the hidden-forward variant keyed on the *message* id. Initials from
      `first_name[0] + last_name[0]`, with the first-space split for a peer known
      only by a name string.
- [ ] `size.rs` — sizes **truncate**, never round. 1,940,744 B is `1.8 MB`.
      1,950 samples, zero mismatches.
- [ ] `serialize.rs` — the message mapping. Desktop's 15 keys, the `_EXTRA_ORDER`
      block, and the `MessageAction*` coverage (41 of 67 types carry a payload
      Desktop discards).

**Design note.** Keep `serde_json::Map<String, Value>` at the boundary rather
than a typed struct. The format is defined by Desktop's *emission*, not by a
schema; the action types map irregularly, and `_EXTRA_ORDER` exists precisely so
new keys sit in a block Desktop never writes. Typed structs belong inside the
module, not at its edge.

**Exit:** Phase 1's JSON leg reads byte-identical on the full reference.

---

## Phase 3 — `tgx-html`: the writer

- [ ] `tree.rs` — Desktop's emission model: one-space indent, blank lines
      between tags, alphabetical attributes.
- [ ] `escape.rs` — everything interpolated goes through `esc()`. `safe_href()`
      allowlists http/https/mailto/tel/ftp plus relative paths, decides against a
      copy with control characters *and* whitespace stripped, but **returns the
      form that still has its spaces** (`stickers/sticker (55).webp`). Every href
      the writer emits goes through it — the four in `_render_media` and the one
      in `_file_row` included. `_message_number` rejects anything that is not a
      whole number, because `onclick="return GoToMessage(n)"` interpolates code,
      not text. `_js_str` quotes the hashtag/cashtag/bot-command arguments.
- [ ] `writer.rs` — joining (same sender within 900s; consecutive forwards only
      within 3s; a joined forward repeats its header unless it shares an album),
      preview sizing (`KeepAspectRatio` with integer division, stored at 2× the
      CSS box, never upscaled), `MIN_INLINE_PHOTO`, both reply-link forms,
      duration and size row rules, tooltips carrying the local zone's *standard*
      offset all year, pagination at 1,000.
- [ ] `assets/` — copy `app/tg/webassets/` byte for byte. Do not reimplement the
      stylesheet; that was already tried (6.2 KB against Desktop's 43 KB, no
      images) and it does not converge.
- [ ] Bot-keyboard markup (`bot_buttons_table`, cells as `div`s, not table cells).
- [ ] Port `test_security.py` as a real parser-based suite with `ALLOWED_HANDLERS`
      as an exact list, including the case-per-href-field payloads that carry a
      genuine `javascript:` target rather than markup — the vacuous-pass bug that
      left five unguarded hrefs green.

**Exit:** `tgx-parity` reports **4 of 4 topics reproduced exactly**, 256,780 lines.

---

## Phase 4 — `tgx-media`: naming and thumbnails

Every rule here was measured, not assumed. Port the numbers with the code.

- [ ] `MediaPlanner`, one per output folder — that is what gives each topic its
      own `photo_1`, `photo_2`.
- [ ] **A name is reserved for every message carrying media, written or not.**
      `_reserve_name` runs *before* the skip checks. Reading `photo_N` as "the
      Nth photo-bearing message" matches 557 of 557; as "the Nth written" it
      matches 64.
- [ ] Per-prefix counters: `photo_`, `video_`, `audio_`, else `file_`.
- [ ] A file Telegram named leaves every counter alone but is still **claimed** —
      and so is the `_thumb.jpg` beside it, or a document named `clip.mp4_thumb.jpg`
      lands exactly where `clip.mp4`'s thumbnail is about to be written.
- [ ] The collision suffix is claimed **only when the file is written**. Claiming
      it up front drops the match rate from 830/836 to 809/836.
- [ ] `_by_id` dedupe, typed by class. A repeat still consumes a number — the
      reference's photos folder runs 1..479 holding 439 files.
- [ ] `_extension` sanitises the extension too, or a sender can inject separators
      and guarantee their file is never saved.
- [ ] `unique_dir` — the exclusive directory creation **is** the reservation.
      Rust's `create_dir` already errors on an existing directory, which is the
      semantic Python had to ask for explicitly.
- [ ] `stripped_photo_to_jpg` — `header[623] + payload + 0xFFD9`, exactly two
      patched bytes at offsets 164 and 166. Port the byte-for-byte test.
      `stripped_thumb_jpeg` checks for the `FFD8` magic and returns `None`
      rather than writing a broken file. Measured at 0.9 µs each; 1,848 of the
      reference's 2,684 media messages get a preview Desktop shows as bare text.
- [ ] A skipped file keeps its `thumbnail` record with `thumbnail_file_size`;
      a skipped *photo* never gets one (62 of 62). A stripped thumbnail has its
      own counter and its own `thumbnails/` folder so it cannot consume a `photo_N`.
- [ ] Folder follows the file's *shape*, not its `media_type` — a WebM video
      sticker goes to `video_files/` while still reporting `"media_type": "sticker"`.
- [ ] `render_preview` via the `image` crate, degrading to no preview rather than
      failing the export. Only the *dimensions* are parity-relevant; the bytes
      are not compared.

**Exit:** 830 of 836 filenames reproduced, with the six custom-emoji exceptions
documented rather than chased.

---

## Phase 5 — `tgx-tg`: client and engine

- [ ] Session at `TelegramExporterData/session` via `grammers-session`. Step-wise
      login: `request_login_code` → `sign_in` → `check_password`.
- [ ] Dialog listing → `ChatInfo` (name, type, last activity, forum flag).
- [ ] **Forum topics** — no high-level API. Raw
      `tl::functions::messages::GetForumTopics` through `Client::invoke`.
      *(Corrected: this document originally said `channels::GetForumTopics`.
      In grammers-tl-types 0.10.0 the `channels` module has only `ToggleForum`
      and `ToggleViewForumAsMessages`; every topic call — `GetForumTopics`,
      `GetForumTopicsById`, `CreateForumTopic`, `EditForumTopic`,
      `DeleteTopicHistory` — lives under `messages`.)*
      Signature: `{ peer, q: Option<String>, offset_date, offset_id,
      offset_topic, limit }`.
- [ ] `topic_id_for` routing, verbatim: no reply header → General; `forum_topic`
      set → `reply_to_top_id` else `reply_to_msg_id`; a plain reply with no flag
      → General; the service message creating a topic → that topic.
- [ ] **The single pass.** `iter_messages(peer).reverse(true).offset_id(last_id)`
      inside a resume loop, so a long FloodWait mid-history resumes instead of
      aborting. Do not "improve" this into per-topic thread fetches:
      `messages.getReplies` returns nothing for General, so that approach
      silently loses it and multiplies requests by the topic count.
- [ ] Filenames decided synchronously, bytes fetched by a bounded tokio pool
      behind them. Downloads validated — a `None` return or zero bytes written is
      a failure, the remnant is deleted, the path goes to `missing_media.txt`.
      Five retries.
- [ ] **Typed errors.** `enum ExportError { FloodWait(Duration), Refused, ... }`.
      In Python, four separate places collapsed a temporary rate limit into a
      permanent refusal because both landed in one `except Exception` — and in two
      of them it made every line of the retry guard unreachable. *This entire
      class of bug is unrepresentable in Rust and it is the single biggest
      correctness win of the rewrite.* Write the enum before the enrichment code,
      not after.
- [ ] Enrichment, each independently switchable and each degrading to nothing:
      full reaction list (fires when `sum(counts) > len(recent_reactions)` — the
      three-name cap is per *message*, not per reaction), poll refresh (build
      **real** TL request objects; a missing required `poll_hash` is a compile
      error in Rust where Python swallowed it at runtime and would have shipped a
      no-op), chat info, participants, invites, scheduled messages.
- [ ] `fetch_participants` returns a roster with a `complete` flag, separate from
      `capped`. A truncated roster that looks complete is worse than no roster:
      wrong data that reads as right.
- [ ] The `_guarded` wrapper: retry once, wait up to 120s in one-second slices,
      emit a `flood_wait` progress event, count what is still lost. The slicing
      and the event are not polish — at 120s, silence is indistinguishable from a
      hang and a flat sleep swallows a click on Cancel for two minutes.
- [ ] `_resolve_reactors` and `_plan_custom_emoji` mark ids missing *before* the
      call so a permanent failure is not retried per message, and a FloodWait
      *lifts* that mark.
- [ ] **Per-chat tallies live on `ExportResult`, never on the exporter.** One
      exporter serves the whole queue. In Rust this is a `&mut ExportResult`
      parameter and the chat is a parameter of `handle`, so the borrow checker
      enforces what the Python comment could only request. Both were real bugs
      that wrote one chat's data into another chat's export as fact.
- [ ] Counting: `MessageIter::total()`, but only for a chat the list has not
      already counted. `0` is a count; test `is_none`, not falsiness.
- [ ] Shutdown **drains**, it does not stop. Cancel, then await, then close.
      A bare stop abandons in-flight tasks and leaves a **zero-byte**
      `result.json` — the writes are still buffered.

**Exit:** a live export of the reference supergroup completes end to end and
agrees with `N:\telegram export\UA KOLAB` on all 6,643 message ids and all
1,786 size-skip decisions.

---

## Phase 6 — `tgx-ui`: the Swiss design system, from the ground up

Source of truth: `C:\Users\Kosta\Projekti\file-analyser\web\assets\css\analyser.css`.
Not `app/ui/theme.py` — that was a Qt-constrained approximation, and roughly a
third of its comments are apologies for what a Qt stylesheet cannot express.
GPUI can express most of them, so the tokens come from the CSS directly.

**Tokens.** Both appearances carry the same keys; no colour literal lives
outside the table.

| token | light | dark |
|---|---|---|
| `bg` | `#ffffff` | `#0a0a0a` |
| `fg` | `#0a0a0a` | `#e8e8e8` |
| `muted` | `#6b6b6b` | `#888888` |
| `hairline` | `#0a0a0a` | `#333333` |
| `rule` | `#e6e6e6` | `#262626` |
| `surface` | `#f4f4f4` | `#141414` |
| `accent` | `#e60023` | `#ff3347` |
| `accent_fg` | `#ffffff` | `#ffffff` |
| `shadow` | `rgba(0,0,0,.14)` | `rgba(0,0,0,.6)` |

Type scale `--t-mega … --t-micro` (200 / 112 / 56 / 28 / 16 / 16 / 13 / 11 / 10
at their clamp ceilings), rhythm `--lh-tight 1.2` / `--lh-body 1.5` /
`--lh-prose 1.65`, tracking `--ls-caps .08em` / `--ls-micro .15em`, durations
`--dur-fast .12s` through `--dur-slow .3s`, `--radius: 0`, `--gap: 24px`.
Geist with `ss01`, `cv11`, `tnum`; Geist Mono for every number.

- [ ] Port the token table as a Rust type with both appearances. Same keys,
      enforced by the type rather than by a test.
- [ ] Fonts: reuse the **already-built** `app/ui/fonts/*.ttf` (Geist + Geist Mono,
      merged Latin+Cyrillic — Qt had no CSS `unicode-range` equivalent, which is
      why they were merged). They are a committed build artefact; copy them, do
      not regenerate. Register via GPUI's text system.
- [ ] **Measure text rendering on Windows.** GPUI uses DirectWrite, and the
      Python project spent real effort on colour fringing of light text on
      near-black — DirectWrite ignored `NoSubpixelAntialias` entirely (81% of
      inked pixels fringed) and the fix was abandoning it for FreeType. Measure
      the same way: share of inked pixels carrying a colour cast, on a **rendered
      window**, not an offscreen paint — under FreeType those two paths disagreed
      0% against 90%. If GPUI fringes, find out in Phase 6 and not at release.
- [ ] **Hairlines at fractional DPI.** A 1px rule is the design's core primitive
      and it must land on a device pixel, not blur across two. Explicit task with
      a visual check at 100 / 125 / 150 / 200% scaling.
- [ ] The drifting grid backdrop: 48px SVG tile drawn at 72px, translating
      `(72px, 72px)` over 24s linear, `0.07` ink on light and `0.12` on dark. In
      Qt this was a static tiled pixmap; in GPUI it can actually drift, which is
      what the site does.
- [ ] Components: `NavButton` (numbered `01`–`03` for the sequence — Sign in,
      Refresh chats, Start export — and unnumbered, right-pushed and
      label-sized for the tools; a cell without a number does not pay the number
      gap), `Rule`, letterspaced uppercase micro-label, `ChatRow`,
      `SettingsRow`, `NumberField` (no stepper arrows; wheel only after click),
      `ListView` with painted empty states.
- [ ] Theme switch as a token swap, not a rebuild.

**What GPUI hands back that Qt refused**, each of which deletes a workaround:
letter-spacing as a real property; actual transitions instead of snapping;
`box-shadow`; no `WA_TransparentForMouseEvents` footgun; no
stylesheet-beats-`setFont` precedence trap; no Fusion-style requirement; and
list virtualization (`UniformList`) the Qt tree never had.

---

## Phase 7 — `tgx-app`: the shell

The layout is a port; the *interaction rules* below are behaviour, hard-won, and
survive the toolkit change unchanged. Treat them as a checklist, because every
one of them is a bug that was found the expensive way.

- [ ] **Async bridge.** Tokio runtime on a dedicated thread for the whole app
      lifetime (grammers requires it); GPUI owns the main thread. Submit
      futures, receive events over a channel, marshal to the UI with `cx.spawn`.
      Direct analogue of `worker.py::AsyncBridge`, including the drain-on-shutdown
      contract. **The UI thread never blocks on a future.**
- [ ] Chat list on `UniformList`. A row paints tick box, title, mono caption and
      right-set count; a forum is a painted red dot, never a suffix on the stored
      title, because presentation in the string is what the filter then searches.
      Ticks held by chat id so they survive sorting, grouping and filtering.
- [ ] Sorting on real values, not displayed text (`100` must not order before
      `99`), with an explicit un-inversion helper so the fixed category order and
      the alphabetical tie-break hold in both directions.
- [ ] Five category buckets plus a flat mode; categories fold; a search re-opens
      any closed category that matched, so a filter never looks like it found
      nothing.
- [ ] **One writer for a chat's count.** Three sources — the Count button, the
      total an export looks up, the number it actually wrote — and one setter.
      Painting, sorting and the selection footer must read the same value, or a
      finished export leaves the row showing one number and sorting on another.
- [ ] **A missing count is not a count of zero.** It paints blank, sorts last,
      and every place that sums says *at least N*.
- [ ] A cancelled or failed export writes no count at all. A truncated run must
      not leave its own length behind as the size of the chat.
- [ ] `ExportResult` keeps the number the run started with, so the summary can
      say *Telegram counted 6,643; 6,640 came through*. Without it a short export
      reads exactly like a complete one.
- [ ] **One progress bar, two claimants.** The export claims it; while claimed,
      every counting handler returns without touching it. The count's progress
      signal fires from a `finally` for every chat however it ended, or the bar
      stops short and reads as stuck rather than finished.
- [ ] **Four empty states, not one.** Not signed in / signed in but nothing
      loaded / filter matched nothing / account has no chats. Track *signed in*
      separately from *list is empty* — they need opposite instructions. The
      message is painted signage: nothing focusable, nothing clickable. A short
      panel drops the hint and keeps the headline. The filter's empty state
      quotes what was typed.
- [ ] A message that names a screen names a screen that exists. There is no
      Settings page; credentials are the first page of sign-in.
- [ ] All / None / Invert / Only forums act on visible rows and are **disabled**
      over an empty list. A button that is enabled and does nothing teaches that
      the interface is unreliable.
- [ ] Minimum window 900×620; the queue's Chat column gets a minimum section
      width (it is the stretch section and the only one that says which row is
      which); auto-sized columns get a gutter.
- [ ] A path longer than its field shows its **start**, not its end, and mirrors
      into the tooltip on every change — not once at construction, because Browse
      writes back afterwards.
- [ ] Login dialog: credentials → phone → code → 2FA. **One dialog, ever** —
      raise the existing one rather than making a second, disconnect on close,
      and say nothing on success beyond the status bar. Two stacked modals is
      what "the app froze the moment it logged me in" actually was.
- [ ] Settings persistence with per-field type validation: an unknown key is
      dropped (so a file written by a newer build still loads) and a wrong *type*
      falls back to the default rather than being coerced. `serde` with
      `#[serde(default)]` per field gives this directly.
- [ ] Quitting mid-export cancels **and waits**, so `result.json` is valid.

---

## Phase 8 — Packaging and release

- [ ] Single portable `TelegramExporter.exe`. All state in `TelegramExporterData/`
      beside it — never AppData, never the registry.
- [ ] ACL-restrict the data folder to the current user on creation; say so in the
      log when it fails, and document that it does not exist on FAT32/exFAT. The
      session key is a bearer credential: anyone who can read it can act as the
      account.
- [ ] Assets embedded with `rust-embed`. No `_MEIPASS` branch, no spec file, no
      `datas` list — the whole class of "the frozen build cannot find its fonts"
      bug disappears.
- [ ] Size budget: compare against the 42 MB PyInstaller build. A GPUI binary
      links a GPU stack; 20–40 MB is the expectation. Record the number so a
      sudden jump is visible, the way the Git-Bash-vs-PowerShell OpenSSL
      discovery was (+2.5 MB from the wrong build shell).
- [ ] **GPU failure has to be legible.** If the renderer cannot initialise, show
      a real message naming the driver requirement. A blank window is the worst
      possible outcome and it is the default one.
- [ ] Icon, version resource, and a decision on code signing (unsigned today).
- [ ] CI on `windows-latest`: build release, run the golden-corpus parity tests
      from `reference/`. The full 6,643-message diff stays a local command
      against `N:\`.
- [ ] Port `save.bat`'s discipline: run every suite first, refuse to commit on a
      failure with an explicit override, re-run the parity diff.

---

## Phase 9 — The Analyser

Deferred by decision, but scoped now so the crate boundaries in Phase 2 are
drawn correctly. It shares the export reader and the design tokens with the
exporter and **nothing else** — no Telegram, no MTProto, no GPUI in the report
path itself.

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
- [ ] The three "deliberately not a naive count" notes must survive the port:
      forum top-level messages are not replies (2,702 of 3,518 would otherwise
      be, dragging the median response time out to days), reactions given are a
      floor and the report prints how much is attributed, and a short member list
      says so.
- [ ] A small GPUI window (pick a folder → watch a bar → open the report) reusing
      `tgx-ui`.

---

## Risk register

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| 1 | **GPUI is pre-1.0 with breaking changes between versions**, and `gpui-component` is pre-1.0 on top of it | A routine dependency bump can break the UI | Pin exact versions; never `"*"`, despite the README. Bump deliberately, on its own commit, with the parity suite green before and after. Keep `tgx-ui` thin enough to re-target. |
| 2 | **`N:\` disappears** | The rewrite loses its oracle and drops to "asserted" | Phase 0 backs both exports up and hashes them. `tgx-parity corpus` cuts a 7.8 MB standalone corpus that all three legs run against with identical results, and `tests/corpus.rs` runs them from `cargo test`. Committing `reference/` is left to a deliberate privacy decision — see "The corpus, and why it is not committed". |
| 3 | **JSON escaping differs silently** between `json.dumps` and `serde_json` | A byte-level diff on a small fraction of messages, invisible until someone diffs | Phase 1's JSON leg exists specifically for this, and runs before the emitter is written. |
| 4 | **DirectWrite colour fringing** on light text over near-black | The exact problem Qt had; the fix there was abandoning DirectWrite entirely | Measure in Phase 6 with the established method. If it reproduces, the options are a GPUI text-rendering setting, a slightly lifted background, or upstreaming. Discovering it late is the real risk. |
| 5 | **Text input** — GPUI ships none; Zed's editor is not published | The login dialog cannot accept a phone number | Resolved: `gpui-component`'s `Input`/`InputState`. Fallback is `EntityInputHandler` directly, roughly one extra milestone. |
| 6 | **grammers is unaudited** and this app holds a bearer credential | Security | Phase 0 reads `grammers-crypto` and the auth path. Session file stays ACL-restricted. |
| 7 | **grammers may lag Telethon on TL layers** | A constructor Telegram added is missing | `Message.raw` plus `Client::invoke` reach anything the high-level API misses. The `unmapped` fallback means an unknown constructor writes its type name instead of ending an export — port it early, not late. |
| 8 | **GPUI needs a working GPU** | A machine the Qt app ran on may show a blank window | Phase 8's legible failure path. Document the requirement in the README. |
| 9 | **Scope illusion** — "it's just a rewrite" | The format work is most of the value and none of the visible progress | Phase order puts the invisible work first and gates each phase on a diff, not a demo. |
| 10 | **No `py-spy` equivalent** for a hung GPUI app | The Qt freeze was diagnosed with sampled stack dumps and Windows' hung-window flag | Build the diagnostic hooks in Phase 7 rather than after the first hang. `tokio-console` covers the async side; the render thread needs its own. |

---

## What Rust and GPUI actually buy

Worth stating, because a rewrite that only moves a working app to another
language is not worth doing.

- **A whole class of bug becomes unrepresentable.** The most damaging pattern in
  the Python codebase is `except Exception` collapsing a temporary rate limit
  into a permanent refusal. It happened in four places, and in two of them it
  made the retry guard *entirely unreachable* — the guard could not retry, could
  not wait, could not count. A `FloodWait(Duration)` variant cannot be silently
  absorbed by a handler written for a refusal.
- **The borrow checker enforces the comments.** "Per-chat tallies live on the
  result, never on the exporter" and "the chat being read is a parameter, not a
  field" are both borrow-checked once the types are right. Both were real bugs
  that wrote one chat's data into another chat's export as fact.
- **UTF-16 handling fails loudly.** `String::from_utf16` returns `Err` where
  Python produced an unencodable string that killed an export thousands of
  messages later.
- **Crate boundaries replace conventions.** "`html_writer` must not import
  Telethon" and "the analyser must not import `app/tg`" become build errors
  instead of comments and a binary-size heuristic.
- **Real list virtualization** via `UniformList`, which the Qt tree never had.
- **No GIL** in the download pool; no interpreter in the binary.
- **The design language stops fighting the toolkit.** Letter-spacing, real
  transitions, shadows and an animated backdrop are all first-class.

### And what it costs

- Two pre-1.0 dependencies in the UI path.
- Text input, scrollbars and dropdowns are a dependency or a build, not a given.
- No `py-spy`, no offscreen Qt screenshot harness — the equivalent tooling has
  to be written.
- A GPU requirement the Qt app did not have.
- Compile times, against a language where editing a file was the dev loop.

---

## Sequencing summary

```
0  spike + pin        -> a window opens, ten real messages print
1  oracle harness     -> reports 0 of 4 exact.  The burn-down list.
2  tgx-format         -> JSON leg byte-identical
3  tgx-html           -> 4 of 4 topics, 256,780 lines
4  tgx-media          -> 830 of 836 filenames
5  tgx-tg             -> live export matches our own reference run
6  tgx-ui             -> tokens, type, hairlines, drift
7  tgx-app            -> the shell + every interaction rule
8  packaging          -> one portable exe, CI green
9  tgx-analyse        -> report.html
```

Phases 2–4 have no UI and no network and can be developed and verified entirely
offline against the reference. Phase 6 has no Telegram dependency and can run in
parallel with 2–5 if there is appetite for it. Phases 5 and 7 are the only two
that need a live account.

**The one rule:** no phase is done because it looks done. Each one is done when
its diff is clean.
