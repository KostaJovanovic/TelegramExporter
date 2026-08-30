# CLAUDE.md

Guidance for Claude Code (claude.ai/code) working in this repository.

A Rust/egui exporter for Telegram chats. It reproduces **Telegram Desktop's own
export format byte for byte**, plus the one thing Desktop cannot do: forum
supergroups split into one folder per topic.

`ROADMAP.md` holds the design record and what is still open. `AUDIT.md` holds
what past reviews and live runs found. `DEFECTS.csv` is the register of known
output differences against Desktop, with a `caught_by` column naming what would
have found each — eleven are open, and several are invisible to every leg. Read
the relevant part before changing the area it covers.

## Commands

```powershell
save.bat                    # menu; also takes an action as an argument
save.bat test               # fmt + clippy + every suite, same as CI
save.bat build              # release build -> dist\TelegramExporter.exe
save.bat parity             # all three replay legs against corpus or drive
save.bat corpus             # cut reference\ out of the reference export
save.bat wire <export dir>  # diff a live export against the reference run
save.bat clean              # report target\ and empty it
```

Underneath it is plain cargo:

```powershell
cargo test --all
cargo test -p tgx-html                          # one crate
cargo test -p tgx-tg --lib config::tests        # one module
cargo test -p tgx-html --test preview_parity    # one integration suite
cargo test -p tgx-tg the_last_target_wins       # one test by name substring
cargo clippy --all-targets --all-features -- -D warnings
cargo run -p tgx-app --bin TelegramExporter     # the window
cargo run -p tgx-tg  --bin tgx -- login|chats|export "<title>"
```

CI (`windows-latest`) runs fmt, clippy with `-D warnings`, `cargo test --all`,
a release build, and fails if the binary exceeds 30 MB.

`save.bat clean` exists because `target\` is a cache cargo never collects; it
reached 45 GB once. `[profile.dev.package."*"] debug = false` in the root
`Cargo.toml` handles the recurring half — do not remove it.

## The oracle

**Nothing here is verified by unit tests alone.** The output is checked against
a real Telegram Desktop export of the same chat, replayed through our own
writers and diffed. `crates/tgx-parity` is that harness.

| leg | direction | current |
|---|---|---|
| `json` | reference `result.json` → our emitter → byte diff | 4/4 topics, 6,643 messages |
| `html` | reference `result.json` → our writer → line diff vs Desktop's pages | 4/4 topics, 256,780 lines |
| `media` | reference `result.json` → our name planner → diff the tree | 830/836 (ceiling: custom emoji) |
| `wire` | our own live export vs a reference run | run once, 2026-08-27; see `AUDIT.md` |

The reference export is `N:\telegram export\UA KOLAB TELEGRAM`. No leg reads
media — the media leg diffs *names* against the tree the JSON records — so
`save.bat corpus` cuts only the 7.8 MB text half into `reference/`, where
`crates/tgx-parity/tests/corpus.rs` runs all three legs as an ordinary
`cargo test`, sha256-checked against `MANIFEST.txt`.

`reference/` is gitignored: it is verbatim chat history from real people and
this repo has a CI workflow. Without it the corpus test skips, so:

- `save.bat` sets **`TGX_REQUIRE_CORPUS=1`**, turning a missing corpus into a
  panic. `set TGX_REQUIRE_CORPUS=0` opts back out.
- CI runs `cargo test --all -- --nocapture` and raises a `::warning::`.
- A plain `cargo test --all` still skips silently, so a fresh clone can run the
  suite.

A corpus cannot be trimmed to "a few interesting messages": pagination and
message joining are cumulative down a topic, so page 3 only reproduces if pages
1 and 2 were written first. Whole topics or nothing.

**If a diff fails, fix the code, not the reference.** Editing the reference to
agree with us turns the oracle into our own output wearing Desktop's name.

## Architecture

Layered, enforced by `crates/tgx-parity/tests/layering.rs` rather than by
convention:

```
tgx-format   Desktop's JSON schema, key order, escaping, sizes. No I/O, network or UI.
tgx-html     the pages, written from serialised maps. MUST NOT depend on grammers-*.
tgx-media    classification, folder layout, filenames, stripped thumbnails.
tgx-tg       grammers client, topics, engine, planning, download, enrichment (+ the `tgx` CLI).
tgx-ui       the design system in egui: tokens, type scale, components, theme.
tgx-app      the window. Depends only on tgx-ui + tgx-tg.
tgx-parity   the oracle: lib + bin. Depends on format/html/media, never on tgx-tg.
```

A Telegram type in `tgx-html` would let Desktop's markup depend on wire shapes,
and the harness could no longer replay recorded JSON through it. That is the
whole reason the rule exists.

**Both outputs come from one map.** `tgx-tg/src/output.rs` takes the payload
that goes into `result.json`, strips the presentation-only `_p` key, and hands
the whole map to the HTML writer. The two cannot drift, and the writer stays
testable with no connection — which is what makes the html leg possible.

**One pass per chat, oldest first.** `engine.rs` uses
`iter_messages(peer).reverse(true)` with a resume loop keyed on `offset_id`,
routing each message to its topic's `Output` as it arrives. Do not convert this
to per-topic thread fetches: `messages.getReplies` returns nothing for the
General topic, so that silently loses it and multiplies requests by the topic
count.

**Closing drains.** The JSON is streamed, so a run abandoned without `close()`
leaves a file that is not truncated but **zero bytes**. Every path that can end
an export goes through `Output::close`, with a `Drop` impl as backstop.

**egui on the main thread, tokio on its own.** `tgx-app/src/bridge.rs` is the
seam; shutdown drains, it does not stop.

**A worker event repaints the window.** Otherwise a result sits in the channel
until something else causes a frame, which the user experiences as having to
move the mouse to make the app work. `bridge::Events` is what guarantees it: it
wraps the sender and calls `ctx.request_repaint()` after every send, so no call
site has to remember to. `Shell::update` drains the whole batch at the top of
each frame and rebuilds the rows once.

The rule predates egui. Under GPUI the mechanism was the opposite — the channel
had to be *awaitable*, `Bridge::drain` was `#[cfg(test)]` so a poll could not
come back, and `shell/mod.rs` awaited on the foreground executor. Polling from a
frame is now the only way to read a channel, and `Events` is what makes it
correct. **Do not add a send path that bypasses it.**

**One connection for the whole process.** `Session::connect` hands out a handle
on a shared `Connection` (`tgx-tg/src/client.rs`); it does not open one.
Per-action sessions each paid TCP, an `InvokeWithLayer(InitConnection)` and a
write of every datacentre address before their first useful request, and opened
a second `SqliteSession` on a file the first was still writing.

**Every network step has a timeout,** because no layer underneath has one:
`NetStream::connect` in grammers is a bare `TcpStream::connect`, so a filtered
address sits in the OS's SYN retry (~21s on Windows) with the UI reading
"Working…". `CONNECT_TIMEOUT` is deliberately under that; `AUTH_TIMEOUT` is
longer because `auth.sendCode` can contain a whole second connection
(`PHONE_MIGRATE` means a fresh DH exchange against the home DC first).

**Anything needing an authorised account calls `ensure_connected` first**
(`actions::ready`). It bounds a blocked network and names an unauthorised
account instead of surfacing either as a wire error mid-listing. Cached on the
connection: one round trip per process, not per action.

**One writer per fact.** A chat's message count has three sources and one
setter, `Shell::set_count`. The queue owns what a run did, so no worker composes
its own summary. The second writer always wins a race you did not know you had.

## Byte-exactness invariants

Each was found by diffing, not by reasoning:

- `result.json` has **no trailing newline** — Desktop ends on `}`.
- 1-space indent, raw UTF-8, and a deliberate **over-indent on `reactions`**.
- Key order comes from `tgx-format/src/order.rs`; the json leg asserts that
  re-keying a real export is a no-op.
- Media folder follows the file's **shape**, not its `media_type` — a WebM video
  sticker goes to `video_files/` while still reporting `"media_type": "sticker"`.
  Routing on `media_type` alone shifts every later filename in the folder.
- HTML: closing tags return to the opening tag's indent; the doctype is emitted
  as text, not a tag line; attributes alphabetical.
- A message whose text ends on an entity gets **a trailing empty segment, but
  only when the text is not pure ASCII** (`tgx-format/src/text.rs`). It is the
  signature of a UTF-16 end offset compared against a UTF-8 byte length; for
  ASCII the two agree and Desktop emits nothing. The entity type plays no part.

## What no test here can catch

The three replay legs prove everything *downstream* of the wire and open no
sockets. `convert.rs` and `plan.rs` — TL object in, Desktop JSON out — have only
synthetic fixtures (`crates/tgx-tg/tests/`), because the reference records
Desktop's output, not Telegram's input.

This is not theoretical. `Session::connect` once took `SenderPool`'s handle and
dropped its runner, so every request was cancelled before it was sent, with all
tests passing and all three legs green. Separately, four whole features —
`reactions`, service `action`, polls, locations — were emitted by nothing at all
under 444 passing tests, because a key the converter never writes is a key the
reference supplies for the legs. **Treat green suites as saying nothing about
the wire.**

The wire leg is the only check that can, and it tests for two absences as well
as for differences: a media path in our JSON with no file behind it (`dangling`)
and a field the reference writes that we never write (`absent`, kept outside the
`MAY_DRIFT` allowance). Nothing downstream of the converter can miss a key the
converter never emits.

## Paths, state and credentials

State lives **beside the executable** — never AppData, never the registry —
except for a binary under `target/`, which climbs back to the workspace root,
because `cargo clean` would otherwise delete the session key and every export.
`save.bat build` ships to `dist/`, so `dist/` can be copied anywhere and still
work.

`TelegramExporterData/` holds the `api_hash` and a Telegram **session key, which
is a bearer credential**: anyone who can read it can act as the account. It is
gitignored and ACL-restricted on creation (a protection that does not exist on
FAT32/exFAT). Do not run a live export or read stored credentials without the
account holder saying so explicitly.

`ensure_data_dir` needs three guards that are easy to drop: `CREATE_NO_WINDOW`,
a once-per-process flag, and an **absolute `System32\icacls.exe` path** — a
portable exe invoking a bare name lets a planted `icacls.exe` beside it run with
the user's rights.

It also holds **`tgx.log`, and `tgx.prev.log` for the run before it**, written
by `tgx_tg::logging::init`, called first thing in both binaries' `main`. Until
it existed nothing installed a `log` implementation, so every `log::` call here
and inside grammers went nowhere. The first column is seconds since start,
because the question the file gets opened for is almost always "what took the
time". `RUST_LOG` is a **plain level name**, not env_logger's filter grammar.
`log`'s `std` feature is not a default and `set_boxed_logger` is gated behind
it; it is set in the workspace dependency, because whole-workspace feature
unification hides a per-crate omission.

**An export narrates itself at two levels.** `Progress::Log` is stage
commentary — settings in force, the count and where it came from, topic folders,
the roster and whether it was capped, each resume after a rate limit, the read
total against what Telegram promised, and per media batch the queue, throughput
and every file that did not arrive. It goes to `tgx.log` and the window's
transcript. `Progress::Detail` is one line per message and per file, goes to
`tgx.log` and the CLI **only**, and is emitted at all only under
`RUST_LOG=debug` (`engine::detail_wanted` gates the formatting so an ordinary
run does not pay for it). It never reaches the window because the transcript is
a 2,000-line ring whose purpose is that the INCOMPLETE warning can still be
scrolled to, and one chat of six thousand messages would flush it.

**A detail line reports the message's text *length*, never its text.** The log
sits beside the executable and an export is other people's conversation;
`a_detail_line_never_carries_the_message_text` is what keeps it that way.

## Dependencies

`winresource` is pinned **exactly**, not by caret: it is pre-1.0, so a routine
`cargo update` can change its interface with no code change of ours, and what it
breaks is a build script — the failure surfaces in a crate whose source did not
change. Bump it deliberately, on its own commit, with the parity legs green
either side.

`gpui` and `gpui-component` carried the same pin and the same warning, and went
with the UI swap. `eframe` takes a caret: it is past 0.1, ships a changelog and
moves on a schedule.

**`eframe` is `default-features = false` with `glow`, and it must stay that
way.** `wgpu` drags in a shader compiler for a window that is a table, a
transcript and a row of buttons; `default_fonts` would merge egui's own Ubuntu
and Hack in as fallbacks, so a missing glyph would render in a typeface nobody
chose. `tgx-ui` and `tgx-app` both declare it and must agree.

`egui_extras` is the egui project's own companion crate, held at the same
version as `eframe` and taken `default-features = false` — its image, SVG,
datepicker and syntect extras are each opt-in and none is wanted. It is here for
`TableBuilder`, which lays out the queue: four fixed count columns and one
stretch column naming the chat, with a floor under the stretch. Laying that out
by hand meant the same five-term subtraction written twice under a comment
asking the next reader to re-check the sum.

`tools/gen_jpeg_header.py` generates the 623-byte JPEG header baked into
`tgx-media/src/jpeg_header.rs`; `tools/extract_preview_samples.py` produced the
committed `preview_samples.json` fixture. Regenerate rather than hand-edit.

## Other agent configs

An OpenAI Codex config (`~/.codex/config.toml`) and a Gemini CLI config
(`~/.gemini/settings.json`) exist on this machine. To bring over MCP servers,
slash commands, subagents, skills or instructions, reply `/import` to see what
is importable, then `/import --yes=<digest>`. If `/import` is unavailable on
this surface, run `claude import` from a terminal.
