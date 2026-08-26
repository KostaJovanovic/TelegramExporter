# Handoff

For whoever picks this up next. Read `CLAUDE.md` first for the architecture and
the invariants; this document is about **where the work actually stands**, which
is not the same as what `ROADMAP.md`'s phase list implies.

## Where this stands

The exporter's **output is finished and proven**. The **window is not**.

| Area | State |
|---|---|
| `tgx-format`, `tgx-html`, `tgx-media` | Done. Byte-exact against a real Desktop export. |
| `tgx-tg` — client, engine, planning, download, enrichment | Built. Proven *downstream* of the wire; the wire itself is unverified — see below. |
| `tgx-parity` | Done. Four legs, plus a 7.8 MB corpus so they run without the reference drive. |
| `tgx-ui` | Tokens and a handful of components. Much of it is dead code. |
| `tgx-app` — the window | **Drawn, largely inert. This is the work.** |
| Phase 9 — the Analyser | Not started; scoped out by the user's first decision. |

359 tests across 19 suites; `cargo clippy --all-targets -D warnings` and
`cargo fmt --check` clean. Parity: `json` 4/4, `html` 4/4 (256,780 lines),
`media` 830/836 — the six are the documented custom-emoji ceiling. Release
binary 18.5 MB in `dist/`. `origin` is set to
`KostaJovanovic/TelegramExporter`; **nothing has been pushed**.

## Read `AUDIT.md` too

`AUDIT.md` is a full read of every crate, done at commit `2906e86`, with a live
baseline run. **It is the defect list for the core; this document is the defect
list for the window.** Do not treat either as complete on its own.

Three of its items are already fixed and ticked there: the GUI's 2FA being
impossible (1), the code path re-requesting a login code (1b), and `icacls` /
`explorer` launched by bare name (10). The rest are open, and several are
serious — Telegram's own thumbnails written into `result.json` but never
downloaded, every channel exported as `public_*`, an empty non-forum chat
deleting its own export folder, `FLOOD_PREMIUM_WAIT` classified as a permanent
refusal, and the media parity leg asserting nothing.

Its finding 3 also carries detail this document does not: after Stop, the Export
button re-enables, so a second concurrent export of the same queue can be
started; `ExportError::Cancelled` is never constructed anywhere; and
`engine::sleep_in_slices` justifies itself entirely by a cancel signal that does
not exist, so the slicing is currently a no-op for its stated purpose.

## Rules that must not be broken

1. **Fix the code, never the reference.** The corpus is sha256-checked in
   `MANIFEST.txt` for exactly this reason: a reference edited to agree with us is
   our own output wearing Desktop's name.
2. **`tgx-html` may not depend on `grammers-tl-types`**, and `tgx-parity` may not
   depend on `tgx-tg`. The build enforces both. Break either and the harness can
   no longer replay recorded JSON through the writers, which is the whole method.
3. **A green suite says nothing about the wire.** All three replay legs open no
   sockets. Three real bugs shipped in one day behind 359 passing tests — a dead
   connection pool, a console-subsystem binary, and a login flow that could never
   complete. Launch the app and drive it.
4. **Do not run a live export or read the stored credentials** unless the user
   says so in that message. They were asked once and declined.
5. **Port from the Python original's code, not its docstring.**
   `ensure_data_dir` was written from a description of the original and silently
   lost three guards, one of them a security regression.
   `C:\Users\Kosta\Projekti\telegram` is source, not just reference.

## Phase 5 — the wire, still unverified

Exit criterion: a live export matching the reference run on all 6,643 ids and
1,786 size-skip decisions. The user was offered this and chose to leave it
unverified; they later said they would test it themselves, and no live export has
been confirmed.

**The check is written and tested.** `tgx-parity wire <our export> <reference>`,
or `save.bat wire`. Run against the reference and itself it reports 6,643 ids and
3,135 size decisions — of which `file` is 1,786 (the roadmap's headline number),
`thumbnail` 1,287 and `photo` 62. It deliberately does not demand two identical
files: `edited`, `reactions`, `views` and `forwards` are counted and reported
separately, because two exports of one chat are taken at different times and
burying four real mismatches under four thousand reactions makes the diff
useless.

Nothing is missing to close Phase 5 except someone running an export.

## Phase 7 — what is actually wrong with the window

Every box in `ROADMAP.md`'s Phase 7 checklist is unchecked, and that is accurate.
Everything below was verified by reading the source.

### 1. The UI does not repaint when work finishes — `shell.rs:906`

`pump()` is called only from inside `render`, and every `cx.notify()` sits in a
click handler. There is no `cx.spawn`, no `Timer`, no `on_next_frame`, no
`observe`/`subscribe` — nothing marks the window dirty when a `bridge::Event`
lands. `Chats`, `Status`, `Progress`, `SignedIn`, `LoginStage`, `LoginFailed` and
`Finished` all sit in the `mpsc` queue until some unrelated input causes a frame.

Chats appearing, the login dialog advancing from Phone to Code, and export
progress therefore all wait for the user to happen to move the mouse. **Fix this
first.** The bridge is correct; the consumer is never scheduled, and most other
symptoms are judged through this one.

### 2. "Stop" does not stop — `shell.rs:537`, `actions.rs::export`

It sets `exporting = false`, clears progress, writes "Stopped", and sends nothing
to the worker. There is no cancellation token and no flag the export loop reads,
so the export runs to completion, keeps writing files, and then fires
`Event::Finished`, overwriting "Stopped" with a success message.

### 3. Nothing scrolls, anywhere

Repo-wide there is no `overflow_scroll`, no `uniform_list`, no `Scrollbar`. The
chat list (`shell.rs:621`) is a plain flex column: at ~45px a row, roughly nine
chats fit and the rest are unreachable by wheel, drag, keyboard, or filter. Every
row is also cloned and laid out each frame, including the invisible ones.

The log is worse — an unbounded `Vec<SharedString>` of which only
`.rev().take(6)` is ever rendered (`shell.rs:785`). The INCOMPLETE-export
warning, the one line the code goes out of its way to distinguish, scrolls out of
reach after six more chats finish.

### 4. There is no search bar

`Shell.filter` is read by `visible()`, `list_state()` and the
`FilterMatchedNothing` empty state — and written **only in tests**. At runtime it
is permanently empty, so one of the four carefully built empty states cannot be
reached in the shipped app.

### 5. Settings are read-only text — `shell.rs:692`

`settings_panel(&self)` takes no `Context`, so it *cannot* be interactive. Six of
~24 fields are printed as labels. Only three fields are editable anywhere in the
app — `api_id`, `api_hash`, `phone` — all in the login dialog.

Fourteen fields have no UI at all, including `media_kinds` and `download_media`,
which `plan.rs` reads. Three more have **no consumer anywhere**:
`chat_concurrency` (named only in two `engine.rs` comments), `sort_mode`, and
`group_by_type`.

### 6. Message counts can never appear

`dialogs.rs:86` hard-codes `message_count: None` and nothing ever assigns it.
There is no Count button. So `count_text` always returns `""` and the selection
footer permanently reads "N chats, at least 0 messages". Phase 7's *one writer
for a chat's count* rule has nothing to govern yet.

### 7. The queue panel does not show the queue

It is titled QUEUE and renders the log. The queue `start_export` builds is never
displayed, and the empty state says "Nothing queued" when what is empty is the
log.

### 8. No sorting, no grouping

`visible()` preserves raw `iter_dialogs` order. Neither the five buckets
`ChatKind`'s own doc-comment describes nor any of the seven sort modes exist.

### 9. Layout defects

- The right column declares 100% height plus 181px of children, so flex squeezes
  both panels and the last settings row is cut first on a short window.
- `WindowOptions` is `..Default::default()` (`main.rs:55`), so the window can be
  dragged below the measured 900×620. `tgx_ui::components::min_window()` exists
  and is never called.
- Long chat titles and a long `output_dir` lack `min_w_0()` and will push their
  neighbours off the panel — the same bug already fixed and commented in the
  login dialog's step list (`shell.rs:187`).
- The login card is a fixed 420px wide with no height cap; on a short window it
  can overflow both edges with its action button off-screen and no way to scroll.

### 10. Unrealised infrastructure

`gpui_component::init` registers 14 subsystems and exactly one — `input` — is
used. `scroll`, `list`, `table`, `checkbox`, `switch`, `select`, `progress`,
`button`, `dialog`, `clipboard`, `otp_input` (literally made for the login-code
field), `resizable` and `tooltip` are all present and unused.

In `tgx-ui`, `tokens::motion` is entirely unreferenced, `rhythm::TRACK_*` is
never applied — so the "letterspaced" micro-type is uppercase but not
letterspaced — and `soft_rule`, `section_number`, `forum_dot`, `min_window` and
`leading` are dead outside their own tests.

`tgx_tg::config::lockdown_error()` was added so the UI could check whether the
security claim it makes is true. No caller in `tgx-app` calls it; an ACL failure
reaches only `log::warn!`.

## Suggested order

1. Repaint on worker events.
2. Make Stop stop.
3. Scrolling: chat list on `uniform_list`; log and settings on
   `overflow_y_scroll` plus `Scrollbar`.
4. Search input wired to `Shell.filter`.
5. Counts: a Count action, one writer, and the *a missing count is not a count of
   zero* rule.
6. Editable settings, in the original's four sections — Destination, Format,
   Media, Performance.
7. Sorting on real values, and the five folding categories.
8. A real queue table, a real progress bar, a readable log.
9. The layout defects and the window minimum.
10. Tick Phase 7's checklist as items land.

The Python original is the specification for all of it:
`app\ui\main_window.py` (1257 lines) and `app\ui\widgets.py`. Phase 7 says *"The
layout is a port; the interaction rules are behaviour."* Its look is already
Swiss/International — `app\ui\design.py` and `motion.py` are dead code there — so
porting its structure does **not** conflict with the decision to rebuild the
design tokens from `analyser.css`. The tokens stay ours; the layout matches.

## Decisions waiting on the user

- **Phase 5** — run the live export, or leave the wire unverified.
- **`reference/`** — gitignored. Committing it gives CI the parity legs and
  publishes real chat history permanently. Their call, not the tooling's.
- **Pushing** — `origin` is configured; nothing has been pushed.
- **Phase 9**, the Analyser, was scoped out at the start and has not been
  revisited.
