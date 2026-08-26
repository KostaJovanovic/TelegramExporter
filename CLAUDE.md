# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

A Rust/GPUI rewrite of the PySide6 exporter at `C:\Users\Kosta\Projekti\telegram`.
It reproduces **Telegram Desktop's own export format byte for byte**, plus the one
thing Desktop cannot do: forum supergroups split into one folder per topic.
`ROADMAP.md` is the long-form design record and is kept current — read the phase
you are touching before changing it.

**Phase 5 has now run, and it found four features nothing implemented.** A live
export was made and cross-examined against two other exports of the same
supergroup — Desktop's own and the Python original's. `reactions` (963 of
6,643 messages), service `action` (63 of 63), polls (7) and locations (3) were
emitted by nothing at all; 206 names resolved to `""`; the `export_results.html`
every topic page links to was never written. All are fixed. **Every one of them
was invisible to 444 passing tests and three green parity legs**, because the
legs replay a recorded `result.json` and a key the converter never writes is a
key the reference supplies for them. `AUDIT.md`'s 2026-08-27 section is the full
account, and `ROADMAP.md`'s "Still open" has what remains — read both before
assuming something is missing, and before assuming it is not.

## Commands

```powershell
save.bat                    # menu; also takes an action as an argument
save.bat test               # fmt + clippy + every suite, same as CI
save.bat build              # release build -> dist\TelegramExporter.exe
save.bat parity             # all three replay legs against corpus or drive
save.bat corpus             # cut reference\ out of the reference export
save.bat wire <export dir>  # diff a live export against the reference run
```

`save.bat` prints `[time]` per step and a total; each action funnels through one
exit path. Underneath it is plain cargo:

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

## The oracle

**Nothing in this project is verified by unit tests alone.** The output is
checked against a real Telegram Desktop export of the same chat, replayed
through our own writers and diffed. `crates/tgx-parity` is that harness, and it
was written before the code it judges.

| leg | direction | current |
|---|---|---|
| `json` | reference `result.json` → our emitter → byte diff | 4/4 topics, 6,643 messages |
| `html` | reference `result.json` → our writer → line diff vs Desktop's pages | 4/4 topics, 256,780 lines |
| `media` | reference `result.json` → our name planner → diff the tree | 830/836 (ceiling: custom emoji) |
| `wire` | **our own live export** vs a reference run | run once, 2026-08-27; see `AUDIT.md` |

The reference is `N:\telegram export\UA KOLAB TELEGRAM` (278 MB). The oracle
inside it is only 7.8 MB — no leg reads media, the media leg included, which
diffs *names* against the tree the JSON already records. `save.bat corpus` cuts
that text half into `reference/`, where `crates/tgx-parity/tests/corpus.rs`
runs all three legs as an ordinary `cargo test`, sha256-checked against
`MANIFEST.txt`.

`reference/` is gitignored: it is verbatim chat history from real people and
this repo has a CI workflow. Without it the corpus test **skips and says so** —
if you see `no corpus at …` in test output, the parity legs did not run.

A corpus cannot be trimmed to "a few interesting messages": pagination and
message joining are cumulative down a topic, so page 3 only reproduces if pages
1 and 2 were written first. Whole topics or nothing.

## Architecture

Layered, and the layering is enforced by the build rather than by convention:

```
tgx-format   Desktop's JSON schema, key order, escaping, sizes. No I/O, no network, no UI.
tgx-html     the pages, written from serialised maps. MUST NOT depend on grammers-tl-types.
tgx-media    classification, folder layout, filenames, stripped thumbnails.
tgx-tg       grammers client, topics, engine, planning, download, enrichment (+ the `tgx` CLI).
tgx-ui       the Swiss/International design system in GPUI: tokens, type scale, components.
tgx-app      the window (depends only on tgx-ui + tgx-tg).
tgx-parity   the oracle: lib + bin, depends on format/html/media but never on tgx-tg.
```

`tgx-html` taking a Telegram type would let Desktop's markup start depending on
wire shapes, and the parity harness could no longer replay recorded JSON through
it. That is the whole reason the rule exists.

**Both outputs come from one map.** `tgx-tg/src/output.rs` takes the payload that
goes into `result.json`, strips the presentation-only `_p` key, and hands the
*whole* map to the HTML writer. The two cannot drift, and the writer stays
testable with no connection — which is what makes the html leg possible.

**One pass per chat, oldest first.** `engine.rs` uses
`iter_messages(peer).reverse(true)` with a resume loop keyed on `offset_id`,
routing each message to its topic's `Output` as it arrives. Do not "improve"
this into per-topic thread fetches: `messages.getReplies` returns nothing for
the General topic, so that silently loses it and multiplies requests by the
topic count.

**Closing drains.** The JSON is streamed. A run abandoned without `close()`
leaves a file that is not truncated but **zero bytes** — the writes are still
buffered. Every path that can end an export goes through `Output::close`, with a
`Drop` impl as backstop.

**The UI is GPUI on the main thread, tokio on its own.** `tgx-app/src/bridge.rs`
is the seam; shutdown *drains*, it does not stop.

**One connection, for the whole process.** `Session::connect` hands out a handle
on a shared `Connection` (`tgx-tg/src/client.rs`); it does not open one. Every
action used to build its own, and each paid TCP, an
`InvokeWithLayer(InitConnection(help.getConfig))` and a write of every datacentre
address that answer carries *before its first useful request* — twice over a
single sign-in, and again for refresh, count and export. It also opened a second
`SqliteSession` on the file the first was still writing. Do not go back to
per-action sessions; the pool multiplexes, which is what a pool is for.

**Every network step has a timeout, because no layer underneath has one.**
`NetStream::connect` in grammers is a bare `TcpStream::connect`. Without
`client::within`, an address that is filtered rather than refused sits in the
OS's SYN retry — about 21s on Windows — with the UI reading "Working…".
`CONNECT_TIMEOUT` is deliberately under that number; `AUTH_TIMEOUT` is longer
because `auth.sendCode` can contain a whole second connection (`PHONE_MIGRATE`
means a fresh Diffie-Hellman exchange against the home DC before the code is
sent).

**Anything that needs an authorised account calls `ensure_connected` first**
(`actions::ready`). It bounds a blocked network and names an unauthorised
account, instead of letting either surface as a wire error from the middle of a
chat listing. The answer is cached on the connection, so it costs one round trip
per process, not one per action.

**A worker event repaints the window; the window never polls for one.** The
bridge's channel is `tokio::sync::mpsc` rather than `std`'s for exactly one
reason: a `std` receiver can only be polled, and polling it from `render` means
nothing is seen until some unrelated input causes a frame — which the user
experiences as having to move the mouse to make the app work. `shell/mod.rs`
awaits the receiver on GPUI's *foreground* executor and calls `cx.notify()`.
`Bridge::drain` is `#[cfg(test)]` so the polling path cannot come back.

**One writer per fact.** A chat's message count has three sources — the Count
button, the total an export looks up, and the number an export wrote — and one
setter, `Shell::set_count`. The queue owns what a run did, so no worker composes
its own summary. Both rules exist because the second writer always wins a race
you did not know you had: a finished export left the row showing one number and
sorting on another, and a worker's cheerful "Exported 3 of 3 chats" landed on
top of "Stopped" a moment after the user pressed Stop.

## Byte-exactness invariants

These are load-bearing and each was found by diffing, not by reasoning:

- `result.json` has **no trailing newline** — Desktop ends on `}`. The Python
  exporter appended one from its first commit and its harness never noticed.
- 1-space indent, raw UTF-8, and a deliberate **over-indent on `reactions`**.
- Key order comes from `tgx-format/src/order.rs`; the json leg asserts that
  re-keying a real export is a no-op.
- Media folder follows the file's **shape**, not its `media_type` — a WebM video
  sticker goes to `video_files/` while still reporting `"media_type": "sticker"`.
  Routing on `media_type` alone shifts every later filename in the folder.
- HTML: closing tags return to the opening tag's indent; the doctype is emitted
  as text, not a tag line; attributes alphabetical.
- A message whose text ends on an entity gets **a trailing empty segment, but
  only when the text is not pure ASCII** (`tgx-format/src/text.rs:159`). 98
  messages in the reference end on an entity; the 11 that carry the tail are
  exactly the 11 with a non-ASCII character in them. The entity type plays no
  part. It is the signature of a UTF-16 end offset compared against a UTF-8
  byte length — for ASCII the two agree and Desktop emits nothing.

If a diff fails, **fix the code, not the reference.** The corpus manifest exists
because editing the reference to agree with us turns the oracle into our own
output wearing Desktop's name.

## What no test here can catch

The three replay legs prove everything *downstream* of the wire and open no
sockets. `convert.rs` and `plan.rs` — TL object in, Desktop JSON out — have only
synthetic fixtures (`crates/tgx-tg/tests/`), because the reference records
Desktop's output, not Telegram's input.

This is not theoretical: `Session::connect` once took `SenderPool`'s handle and
dropped its runner, so every request was cancelled before it was sent. All tests
passed and all three legs were green. Treat green suites as saying nothing about
the wire.

The wire leg is the only check that can, and **it now tests for two absences as
well as for differences**: a media path in our JSON with no file behind it
(`dangling` — 1,546 of them on the first real run), and a field the reference
writes that we never write at all (`absent`, kept outside the `MAY_DRIFT`
allowance, which had been scoring 963 missing `reactions` as honest
run-to-run drift). Both classes are silent everywhere else in the workspace:
nothing downstream of the converter can miss a key the converter never emits.

## Porting from the Python original

`C:\Users\Kosta\Projekti\telegram` is not just a reference for *what* to build;
its comments are the record of what went wrong the first time. **Read the
original implementation, not its docstring**, before writing the Rust
equivalent — a paraphrase loses exactly the defensive details that were added
after something broke.

Concretely: `ensure_data_dir` was ported from its description and silently lost
three guards the original had — `CREATE_NO_WINDOW`, a once-per-process flag, and
an absolute `System32\icacls.exe` path (a portable exe invoking a bare name lets
a planted `icacls.exe` beside it run with the user's rights). The first two were
cosmetic-looking; the third was a security regression.

## Paths, state and credentials

State lives **beside the executable** — never AppData, never the registry —
except for a binary under `target/`, which climbs back to the workspace root,
because `cargo clean` would otherwise delete the session key and every export.
`save.bat build` therefore ships to `dist/`, so `dist/` is a folder you can copy
anywhere and it still works.

`TelegramExporterData/` holds the `api_hash` and a Telegram **session key, which
is a bearer credential**: anyone who can read it can act as the account. It is
gitignored and ACL-restricted on creation (and that protection does not exist on
FAT32/exFAT at all). Do not run a live export or read stored credentials without
the account holder saying so explicitly.

It also holds **`tgx.log`, and `tgx.prev.log` for the run before it**, written by
`tgx_tg::logging::init` — called first thing in both binaries' `main`. Until it
existed, *nothing installed a `log` implementation*, so every `log::` call in
this workspace and every one inside grammers went nowhere: the three lines that
between them time a sign-in (`connecting...`, `generating new authorization
key...`, `authorization key generated successfully`) were being written and
discarded. The first column is seconds since start, because the question the
file gets opened for is almost always "what took the time". `RUST_LOG` sets the
level and is a **plain level name**, not env_logger's filter grammar — accepting
a filter we do not implement would be worse than refusing it. Note that `log`'s
`std` feature is not a default and `set_boxed_logger` is gated behind it; it is
set in the workspace dependency, because whole-workspace feature unification
hides a per-crate omission (`cargo build --workspace` passed while
`cargo build -p tgx-tg` did not).

## Dependencies

`gpui` and `gpui-component` are pinned **exactly**, not by caret, despite their
READMEs suggesting `version = "*"`. Both are pre-1.0, so a routine `cargo update`
can break the interface with no code change of ours. Bump them deliberately, on
their own commit, with the parity legs green either side.

`tools/gen_jpeg_header.py` generates the 623-byte JPEG header baked into
`tgx-media/src/jpeg_header.rs`; `tools/extract_preview_samples.py` produced the
committed `preview_samples.json` fixture. Regenerate rather than hand-edit.

## Other agent configs

An OpenAI Codex config (`~/.codex/config.toml`) and a Gemini CLI config
(`~/.gemini/settings.json`) exist on this machine. To bring over MCP servers,
slash commands, subagents, skills or instructions, reply `/import` to see what
is importable, then `/import --yes=<digest>`. If `/import` is unavailable on
this surface, run `claude import` from a terminal.
