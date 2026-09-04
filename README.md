# Telegram Exporter

Keep your own copy of your Telegram conversations — as web pages you can open
in any browser, and as JSON if you want the raw data.

Forum supergroups come out as **one folder per topic**, each a complete archive
in its own right. That's the part Telegram's own exporter won't do for you.

## Before you start

You need an `api_id` and `api_hash` of your own. Sign in at
[my.telegram.org](https://my.telegram.org), open **API development tools**, and
register an app. It's free and takes a minute.

You sign in as yourself, with your phone number and a login code. Bots can't
read chat history, so there's no bot-token shortcut.

## First run

```powershell
save.bat build          # → dist\TelegramExporter.exe
```

Launch the exe. It asks for your API keys and phone, sends a code to Telegram,
and remembers the session — you won't do this again.

Then: pick chats from the list, add them to the queue, press Export. The
transcript tells you what's happening; Stop works at any moment, and whatever
was already written stays on disk.

## What you end up with

Everything lands in `Exports/`, beside the app:

```
Dev Team/
  0001 - General/
    messages.html        ← start here
    result.json
    photos/  video_files/  voice_messages/  files/  stickers/ …
  0042 - Backend/
  0043 - Design 🎨/
```

Open `messages.html` and you have the conversation as it looked: text,
replies, media, reactions, polls, joins and leaves, oldest message first. Media
lives in the folder next to the pages, so you can move the whole thing to a
drive, zip it, hand it to someone — it reads offline, forever, with nothing
installed.

Topic folders are prefixed with the topic's id. Two topics sharing a name, or
one renamed halfway through its life, can't collide.

## Settings that change the outcome

Most defaults you can leave alone. These are the ones people actually want to
change:

| | Default | |
|---|---|---|
| **Size limit** | 20 MB | Anything bigger is skipped. `0` means no limit — be aware a busy group can run to tens of gigabytes |
| **Download media** | on | Turn off for text-only pages, which is enormously faster |
| **Where exports go** | `Exports/` | Point it anywhere |
| **Parallel downloads** | 5 | 4–6 is the sweet spot; pushing higher just gets you throttled |
| **Split forums by topic** | on | |
| **Member list** | first 10,000 | Large public channels stop serving the list well before that |
| **Messages per page** | 1000 | |
| **Format** | Classic | Classic writes Desktop's folders; Database writes one accumulating file. See below |

### The database

**Database** is an alternative to the Classic export, not an addition to it.
Classic writes a fresh folder every run, in Desktop's format. Database gives
each chat its own `<chat name>.sqlite` beside your exports and merges each run
into it, media included — so re-running syncs instead of exporting the whole
chat again. Export a supergroup and a private chat and you get two files, each
holding only that conversation.

What that buys you is the thing a folder export cannot do. Export a chat today
and again next month, and the second export simply won't contain whatever was
deleted in between — it is a photograph of the chat as it is now. The database
keeps both: new messages are added, an edited message keeps its earlier
versions, and a message Telegram no longer returns stays in the file, marked
with the date it went missing. **It can only keep what it has already seen**, so
the first export is what starts the record.

A Database run reads what is new plus the newest few hundred messages again,
looking for edits and deletions, rather than walking the whole history. That is
the one to run regularly. It honours the same media settings as Classic, and
because it only re-reads the newest messages, that is also as far back as it
will pick up a file an earlier run skipped — **Re-read the whole history**
catches the rest.

Two options are off on purpose, because they'd make the archive diverge from
what Telegram Desktop writes: saving link-preview images, and labelling
contacts by the name *they* chose instead of the name you saved them under.
Both are there if you want them.

## Looking after your account

`TelegramExporterData/`, next to the exe, holds your `api_hash` and your
Telegram session key. That key **is** access to your account — anyone who can
read the folder can act as you. It's restricted to your Windows user when it's
created, though that offers nothing on a FAT32 or exFAT drive.

If you're ever unsure, revoke the session from Telegram → **Settings →
Devices**.

And exports are other people's messages as much as yours. Store them like it.

## What it won't do

- Reach chats your account can't already see. It has exactly your access, no more.
- Bring back messages deleted before you first exported them. Those are gone
  from Telegram's servers too. Turn on **Database** and it will keep anything
  deleted *after* a run has seen it.
- Finish a huge forum quickly. History arrives in one sequential pass, at the
  rate Telegram hands it over.
- Run without a GPU — the window renders through DirectX. If it can't start,
  it says so instead of showing you an empty frame.

## From the terminal

```powershell
cargo run -p tgx-tg --bin tgx -- login
cargo run -p tgx-tg --bin tgx -- chats
cargo run -p tgx-tg --bin tgx -- export "Dev Team"
cargo run -p tgx-tg --bin tgx -- export --db "Dev Team"   # database instead of folders
```

`--full` makes a `--db` run walk the whole history rather than the trailing
window — needed for older deletions, and for files an earlier run skipped.

`TG_API_ID` and `TG_API_HASH` work instead of the settings file.

## Building

`save.bat` with no arguments gives you a menu; `save.bat test` runs formatting,
lints and the full suite, and `save.bat build` produces the release exe in
`dist/`. That folder is self-contained — copy it to a stick and it works there.

Rust, [egui](https://github.com/emilk/egui) and
[grammers](https://github.com/Lonami/grammers). MIT licensed.
