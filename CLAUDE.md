# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

A Rust/GPUI rewrite of the PySide6 exporter at `C:\Users\Kosta\Projekti\telegram`.
It reproduces **Telegram Desktop's own export format byte for byte**, plus the one
thing Desktop cannot do: forum supergroups split into one folder per topic.
`ROADMAP.md` is the long-form design record and is kept current — read the phase
you are touching before changing it.

**`HANDOFF.md` says where the work actually stands**, which is not obvious from
the phase list: the output is finished and proven, the window is barely started.
Read it before touching `tgx-app`.

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
| `wire` | **our own live export** vs a reference run | needs a signed-in account |

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
