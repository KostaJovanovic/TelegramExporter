# Telegram Exporter (Rust)

Exports Telegram chats in **Telegram Desktop's own export format** (HTML +
`result.json`), with the one thing Desktop cannot do: forum supergroups are
split into one folder per topic, named after the topic.

A Rust/GPUI rewrite of [the PySide6 original](../telegram). See
[ROADMAP.md](ROADMAP.md) for the plan and its current state.

## Verified against a real Desktop export

The output is not merely *similar* to Desktop's — it is byte-identical, and
that is checked rather than asserted:

```powershell
cargo run -p tgx-parity -- json  "N:\telegram export\UA KOLAB TELEGRAM"
cargo run -p tgx-parity -- html  "N:\telegram export\UA KOLAB TELEGRAM"
cargo run -p tgx-parity -- media "N:\telegram export\UA KOLAB TELEGRAM"
```

| leg | what it proves | result |
|---|---|---|
| `json` | `result.json` re-emitted from its own data is byte-identical | **4 of 4 topics**, 6,643 messages, 3.2 MB |
| `html` | the pages rendered from that data match Desktop's line for line | **4 of 4 topics, 256,780 lines** |
| `media` | the filenames Desktop chose are reproduced from the message sequence | **830 of 836** (the six are custom emoji, invisible to a JSON replay) |

The harness was built **before** the code it judges. That ordering is the whole
method, and it paid for itself on the first run — see *What the harness caught*
in ROADMAP.md.

## Running it

```powershell
cargo run -p tgx-app --bin TelegramExporter      # the window
cargo run -p tgx-tg  --bin tgx -- login          # sign in (session is saved)
cargo run -p tgx-tg  --bin tgx -- chats          # list every chat
cargo run -p tgx-tg  --bin tgx -- export "Dev Team"
```

You need your own `api_id` / `api_hash` from
[my.telegram.org](https://my.telegram.org) → **API development tools**. Put
them in `TelegramExporterData/settings.json`, or pass them as `TG_API_ID` and
`TG_API_HASH`.

You sign in as *yourself*, not as a bot — bots cannot read chat history.

## Layout

```
crates/
  tgx-format/   Desktop's JSON schema. No I/O, no network, no UI.
  tgx-html/     the pages, written from serialised maps. No Telegram types.
  tgx-media/    filenames, folder layout, stripped thumbnails.
  tgx-tg/       the client and the export engine (+ the `tgx` CLI).
  tgx-ui/       the Swiss/International design system, in GPUI.
  tgx-app/      the window.
  tgx-parity/   the oracle.
```

Two dependency rules are enforced by the build rather than by convention:
`tgx-html` may not depend on `grammers-tl-types`, and the analyser (when it
lands) may not depend on `tgx-tg`. In the Python original both were comments
plus a binary-size heuristic.

## Output

A **forum supergroup** produces:

```
Dev Team/
  0001 - General/
    result.json  messages.html  css/  js/  images/
    photos/  video_files/  voice_messages/  files/ …
  0042 - Backend/
  0043 - Design 🎨/
```

Each topic folder is a complete, self-contained export — what Desktop would
produce if that topic were its own chat, including per-folder media numbering.
Folders carry the topic id so two topics with the same name, or a topic renamed
mid-history, can never collide.

Messages are always written oldest first.

## Security

An exported archive opens as a **local file**, so anything surviving into it
runs with that file's origin, and every string in an export is
attacker-controlled — a display name, a sticker emoji, an ID3 tag, a filename,
a link target.

* Everything interpolated into markup is escaped.
* Every href goes through an allowlist (`http`, `https`, `mailto`, `tel`,
  `ftp`, plus relative paths). The check runs against a copy with control
  characters *and whitespace* removed, because browsers ignore both inside a
  scheme — but the accepted URL keeps its spaces, since Desktop really does
  write `stickers/sticker (55).webp`.
* **Escaping is not enough inside a JavaScript expression.** Desktop's markup
  contains `onclick="return GoToMessage(12)"`; an id interpolated there is
  code, so anything that is not a whole number drops the link instead.

`TelegramExporterData/` holds your `api_hash` and a Telegram session key, which
is a **bearer credential**: anyone who can read it can act as your account.
Portability was a deliberate choice, so it stays beside the executable — but
the folder is ACL-restricted to your user on creation, and the app says so in
the log if that fails. **The protection does not exist on FAT32/exFAT at all.**
Revoke a session any time via Telegram → Settings → Devices.

## Building

```powershell
cargo test --all
cargo build --release -p tgx-app
```

The release binary is **~9.8 MB**, against the Python build's 46.4 MB. Desktop's
stylesheet, script and 42 icons are embedded in it, so there is no `_MEIPASS`
branch, no spec file and no `datas` list.

**It needs a GPU.** GPUI renders through DirectX on Windows; the Qt original ran
anywhere. If the renderer cannot start, the app says so on the console rather
than showing a blank window.

## Requirements and limits

- Your own `api_id`/`api_hash`. There is no way around this.
- Only chats your account can already see. Nothing here bypasses access controls.
- Deleted messages are gone from Telegram's servers and cannot be recovered.
- Large forums take a while: the history pass is inherently sequential, bounded
  by Telegram's paging.
