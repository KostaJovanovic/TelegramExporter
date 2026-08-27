# Three exports of one supergroup — 2026-08-27

The same forum supergroup, exported three times, then compared against each
other on disk. Not a replay, not a fixture: three real trees, 3,342 + 3,342 +
1,896 files, sitting side by side under `N:\telegram export\`.

| export | what made it | when | size |
|---|---|---|---|
| `UA KOLAB TELEGRAM` | Telegram Desktop itself | 2026-08-25 16:04–16:31 | 286.93 MB |
| `UA KOLAB PYTHON` | the PySide6 original | 2026-08-26 03:22 (+11 h) | 286.99 MB |
| `UA KOLAB RUST` | this workspace | 2026-08-27 05:22 (+37 h) | 340.17 MB |

Desktop is the oracle in every comparison below. Rust-vs-Python appears only
where Desktop has no opinion, because it cannot split a forum into per-topic
folders and therefore has no `export_results.html` and no `participants.json`
to be right or wrong about.

One structural note that trips up every diff: Desktop's export is really **four
separate Desktop runs**, one per topic, and it buries media under
`<topic>/chats/chat_562953540103937/topic_<N>/`. Rust and Python put media
directly under the topic folder. Every media path in this document has been
normalised across that difference before being compared.

The group stayed active between runs, so Rust's export carries 44 messages
(ids 7944–7987) that Desktop's does not. Those are excluded from every
comparison except where noted.

---

## The short version

**Rust reproduces Desktop's format far better than the Python original does,
and gets some of the bytes wrong that Python gets right.**

The three replay legs are green and stayed green throughout: feed Desktop's own
`result.json` back through our writers and the JSON comes out byte-identical on
4 of 4 topics, the HTML line-identical across 256,780 lines, and the media
planner reproduces 830 of 836 filenames. That is worth exactly what it has
always been worth, and no more — see "Why nothing caught this" at the end.

Against Desktop's own live output:

|  | Python | Rust |
|---|---|---|
| messages missing | none | none |
| fields Desktop writes that we never do | **2,576** | **0** |
| fields we write that Desktop never does | **6,390** | 104 |
| field-level value mismatches | **11,650** | 2,805 |
| static assets byte-identical (of 220) | 218 | **220** |
| broken links in the output | **0** | **42** |

Rust is the more faithful *format*. Python is the more complete *export* — it
downloaded everything it promised, and it ships an analytics report Rust has no
answer to at all.

---

## What Rust gets right that Python gets wrong

These are the wins, and several of them are the kind of detail that only ever
gets found by diffing.

**No trailing newline.** Desktop ends `result.json` on `}`. Rust does too.
Python appends a `\n` in all four topics — the same defect its own harness
never noticed, still present.

**The trailing empty segment.** 98 messages in this corpus end on an entity.
Desktop emits a trailing empty text segment on exactly the 11 that contain a
non-ASCII character, and on none of the 87 that are pure ASCII. Rust reproduces
that 11 out of 11. **Python reproduces it 0 out of 11.** This is the UTF-16
end-offset signature documented in `CLAUDE.md`, and it is invisible in the HTML,
which is why it survived in Python for so long.

**Desktop's gapped filename counter.** Desktop advances `photo_N` for *every*
photo it sees, including the ones it refuses to download, and burns a number
when it de-duplicates. In `foto video` that leaves 63 gaps in a ladder reaching
479. Rust reproduces the gap semantics; in `editorijal` its photo filenames are
identical to Desktop's, name for name, gap at index 17 included. Python
renumbers densely from 1 and drifts to **−61** by the end of `foto video`, which
means every photo filename it writes after the first skipped one is wrong.

**Filename stems.** Desktop writes `voice_messages/audio_1@….ogg` and
`video_files/video_6@….mp4`. Rust matches both. Python writes `file_N@…` for
both.

**Static assets.** All 220 CSS/JS/PNG files byte-identical to Desktop,
including Desktop's CRLF line endings. Python normalises the CSS and JS to LF —
identical content, 1,982 bytes of difference, two files that no longer hash the
same.

**Media routing.** Zero folder mismatches across 836 aligned media messages in
all three exports. The rule that a file follows its *shape* rather than its
declared `media_type` is confirmed live: four `video/webm` files reporting
`"media_type": "sticker"` land in `video_files/` in Desktop and in Rust.

**Rendering the awkward media types.** Desktop renders a WebM video sticker as
a generic file block and an animated GIF as a plain `media_video` block. Rust
matches both exactly. Python invents `<img class="sticker">` for the first and
an `animated_wrap`/`gif_play` structure for the second.

**Structural invariants.** Closing tags return to the opening tag's indent,
attributes alphabetical, doctype emitted as text — zero violations across
128,581 tag lines, in all three exports. The `<head>` block is byte-identical
across all 27 HTML files in all three trees; one sha256 covers the lot,
including Desktop's oddity of putting `<title>` at column 0.

**Pagination.** Page boundaries fall on the same message in all three, and the
per-page message-id sequences are byte-identical in order, not merely equal as
sets.

**And 6,643 messages of agreement.** Not one id is missing from either
exporter. All 63 service messages present with an identical `action` census.
All 7 polls byte-identical. Both locations present. `id`, `date`,
`date_unixtime`, `edited`, `from_id`, `actor`, `action`, `forwarded_from_id`,
`reply_to_message_id`, `file_size`, `photo_file_size`, `width`, `height`,
`duration_seconds`, `mime_type`, `media_type` and `file_name` are exact on
every single message in both exporters. All 6,580 timestamp `title=` attributes
match, offset spelling included. All 3,518 reply-to blocks point at the right
message on the right page.

---

## What Rust gets wrong

Ordered by how much it costs.

### 1. Photo previews are the wrong size — 54 MB, and nothing could see it

`photos/*_thumb.jpg` is 72.87 MB in Rust against 18.68 MB in Desktop. The
full-size photos are byte-identical; **100 % of the gap is the previews.**

Desktop scales every preview to 520 px on the long edge, re-encoding as
progressive 4:4:4 with a Qt sRGB profile. Rust downloads a smaller `PhotoSize`
variant from Telegram instead, which `download.rs:412 fetch_preview` documents
as a deliberate trade — it means the binary needs no image decoder. The problem
is the selector:

```rust
.filter(|t| (t.size() as i64) < job.size).max_by_key(|t| t.size())
```

It chooses by **byte size**, not by pixel box, so the result lands on
Telegram's standard boxes and never on Desktop's 520:

| | preview long edge |
|---|---|
| Desktop | 520 ×560, 480 ×3, 500 ×1, 232 ×1 |
| Python | the same |
| **Rust** | **800 ×403, 1280 ×148, 320 ×13, 232 ×1** |

It is wrong in both directions. 552 previews come out larger than Desktop's —
median 3.1×, worst 22× — and **13 come out smaller**. A 642×480 source gets
520×388 from Desktop and 320×239 from Rust. So this is not "we ship more
pixels", it is "we ship the wrong ones".

**Nothing in the workspace can detect this.** The photo preview is named in no
`result.json` — all 154 `_thumb.jpg` references in each JSON belong to
documents and stickers, never photos. It appears only as an `<img src>` in the
HTML, and there the *string* is identical. The json leg cannot see a filename
that never appears; the html leg compares a string that matches. Fifty-four
megabytes of divergence, invisible by construction to every leg we have. Only
a filesystem comparison finds it.

### 2. Link previews become photos that were never downloaded

Desktop writes no media key at all for `MessageMediaWebPage`. Rust promotes the
preview's image to a real top-level `photo`, with `photo_file_size`, `width` and
`height` beside it — 21 messages — and on two more (`ćaskanje` 509 and 535) it
manufactures an entire `file` / `file_name` / `media_type: video_file` /
`mime_type` / `duration_seconds` / `thumbnail` block out of a YouTube link.

Three consequences, in increasing order of annoyance:

- All 21 fail to download, leaving **21 dangling references in the JSON and 42
  broken `<img>` tags in the HTML**. Desktop and Python have zero of either.
- Each phantom photo advances the `photo_N` counter, so **474 later photo
  filenames shift**. This single defect is roughly half of Rust's total field
  mismatches.
- `plan.rs:219` already documents it — *"that difference cost 21 files a run…
  one hundred percent of them, every run"* — and carries a fix. **The export on
  disk predates that fix.**

`link_previews` is opt-in and defaults off (`config.rs:102`); this run had it on.

### 3. Twenty-eight names resolve to the empty string

`inviter` is `""` on **all 26** `join_group_by_link` service messages, and
`members[0]` is `""` on 2 of 24 `invite_members`. Desktop and Python resolve
every one of them. The rendered HTML is Desktop's markup minus the trailing
name, ending on a stray space.

This is the residue of the 206-empty-names bug. It is smaller, and it is not
closed. `participants.json` is clean — 43 of 43 names resolved, no nulls — so
the roster path is fine and the service-message path is not.

### 4. No media de-duplication

Desktop reuses one file when the same document appears twice, collapsing 46
references onto 20 basenames, and writes a second thumbnail with a ` (1)`
suffix instead of a second photo. Neither Rust nor Python does this — both are
strictly 1:1. It costs 9 redundant photos (3.33 MB) and shifts 74 `file` and 74
`thumbnail` paths.

Python shares this one.

### 5. Custom emoji keep their numeric id

Desktop downloads the emoji's document and rewrites `document_id` to point at
the file; Rust leaves the bare number. Ten reaction entries and one text
entity. This is also why three `AnimatedSticker*.tgs` files that Desktop and
Python both fetch are never even planned in Rust. `convert.rs:608` names this
as the known ceiling, and it is the same six files behind the media leg's
830 of 836 — so this one is documented and expected, not a surprise.

Python resolves these correctly.

### 6. Reactions carry too much

Desktop caps the recent-reactor list at **three**, and when there are more
reactors than avatars it appends `<span class="count">TOTAL</span>`. In all 95
spans where Desktop emits that count, the number is exactly right.

Rust emits the full reactor list — up to eleven — and **never emits a count
span**. Desktop also omits `recent` entirely on 19 reactions; Rust always
writes it. 209 messages differ. Desktop's list is always a strict subset of
ours, so this is more data rather than wrong data, but it is not Desktop's
format.

### 7. Sticker previews are copies of the sticker

`sticker (1)_thumb.webp` hashes identically to `sticker (1).webp`. Every
sticker in every topic. It is the same `fetch_preview` branch as finding 1 —
when Telegram advertises nothing smaller, the full file gets copied — and for
stickers that branch is always taken. It also produces
`stickers/AnimatedSticker_thumb.tgs`, a Lottie file offered as an image preview.

### 8. Userpics

Two separate small defects, both cosmetic, both byte-level mismatches:

- Desktop's colour classes are `{1,2,4,5,6,7,8,19}`. Rust's are
  `{1,2,4,5,6,7,8,18}` — it emits `userpic18` 494 times, a class Desktop never
  uses, and never emits `userpic19`. `style.css` only defines 1–8, so both are
  unstyled and it looks identical.
- 29 initials are wrong, and **Rust disagrees with itself**: the same person
  renders `A` in one place and `AR` in another. One code path uses Desktop's
  rule (first name initial + last name initial), another splits the display
  name on whitespace. Python gets all 41 right.

### 9. Coordinates are not rounded

Desktop writes `44.857507`; Rust writes `44.857507352853844`. Two messages, and
it shows up twice each — once in `location_information` and once in the map URL
in the HTML. Python matches Desktop. The only floats in the entire corpus.

### 10. `export_results.html` drops three fields it already has

`topic_closed` is `true` for two topics in Rust's own JSON and the page shows
only `pinned`. The topic creator is populated and not rendered. The invite link
is populated and not rendered — that last one is arguably right on purpose,
since it is a live credential to a private group, but the first is a plain
defect.

### 11. Metadata disagrees with the file on disk — four photos

Four photos are served as `photoSizeProgressive`. Desktop assembles it. Rust
records the progressive size's dimensions in `photo_file_size`, `width` and
`height`, then downloads the largest plain `photoSize` instead. `plan.rs:90`
and `download.rs` are choosing different objects. In three of the four cases the
*lower*-resolution file is the *larger* file.

Desktop has zero recorded-vs-actual mismatches. Python has the same four.

---

## What the Python original gets wrong

For completeness, and because some of these are why the rewrite exists.

- **1,287 messages are missing `thumbnail` and `thumbnail_file_size`.** Desktop
  writes a `"(File exceeds maximum size…)"` placeholder; Python omits the keys
  and writes `null` where it does write one. This alone is half of its 2,576
  absent fields.
- **A trailing newline on every `result.json`.**
- **The trailing empty segment, missed on all 11 messages.**
- **Dense photo renumbering**, drifting −61.
- **`file_N` stems** where Desktop writes `audio_N` and `video_N`.
- **23 extra message keys**, 6,390 field instances — `grouped_id` (2,120),
  `outgoing` (897), `forwarded_date` and its unixtime twin (1,628 between them),
  `edit_hide` (995), `replies_count` (434), `sticker_set`, `voice_waveform`,
  `from_rank`, `mentioned`, `link_preview`, `reply_to_quote*`. Genuinely useful
  data. Not Desktop's format.
- **WebM stickers and animated GIFs rendered wrong**, and 24 link-preview
  blocks Desktop does not emit.
- **CSS and JS normalised to LF.**
- `sticker_emoji: ""` on 11 stickers where Desktop omits the key;
  `new_icon_emoji_id` dropped on both `topic_edit` messages; `recent` dropped
  from all 10 custom-emoji reactions; `sticker_set.id` emitted as a string
  where every other id in the corpus is an integer; an invented `answer` on
  three `poll_append_answer` messages.

And the thing it has that we do not: **`report.html` and `analysis/`**. 558 KB
of self-contained report with 64 inline SVG charts — timeline, hour-of-day and
day-of-week rhythm, a 45-row per-person table, reply latency, a 28-node contact
graph, emoji and sticker and link and mention breakdowns, arrivals, per-topic
stats, six superlative records — plus a 770 KB per-message digest and a spec
for annotating the timeline with events extracted from it. Roughly two dozen
analyses, 1.33 MB.

**No parity leg will ever report this missing, because Desktop never produced
it either.** It is a feature gap, not a fidelity gap, and it is the single
largest difference between the two exporters that is not about bytes. Worth
noting the shape of it too: `report.html` and `digest.jsonl` between them
concentrate every participant's name, activity profile, social graph and
verbatim message text into two files that are trivially easy to forward.

---

## What neither gets right

**The forwarded header, on 117 of 814 messages.** Desktop shows the forwarded
userpic and origin name on some joined messages and suppresses it on others.
Rust and Python agree with each other exactly and disagree with Desktop on 117 —
59 where Desktop shows it and neither of us does, 58 the other way.

Desktop's discriminator **is not recoverable from `result.json`**. It is not the
sender, not the forward origin (`joined` already implies same origin, 573 of
573), not the media type, not the reply, not the gap between message dates.
Whatever Desktop is keying on, it is something it knows at render time that its
own export does not record. This is an open question, not a defect either
exporter introduced, and it may not be answerable from the data we have.

Both also share the flattened media path (a consequence of one folder per
topic), `stripped_thumbnail` as an extra key, the extra `thumbnails/` tree with
its extra `media_wrap` block for over-size files, and the uncapped reaction
avatars.

One thing that looks like a defect and is not: **21 of the 49 users have two
different display names inside Desktop's own export**, and the split does not
follow dates — 19 of the 21 overlap in time. Desktop is resolving names
inconsistently from `min` versus full peer objects. Rust and Python each
collapse to one name per user and always pick the same one as each other. The
five message-level `from` differences hit both equally. This is Desktop being
inconsistent, not us being wrong.

---

## Where the megabytes went

Rust's export is 53.23 MB larger than Desktop's. Every one of them is
accounted for:

| | Desktop | Python | Rust | R − D | why |
|---|---|---|---|---|---|
| `photos/*_thumb.jpg` | 18.68 | 18.98 | **72.87** | **+54.18** | finding 1 |
| `stickers/*_thumb.webp` | 9.30 | 0.96 | 1.71 | −7.59 | Desktop stores 384 px **PNGs** mislabelled `.webp` |
| photos, full size | 157.10 | 160.59 | 160.59 | +3.49 | 9 photos Desktop de-duplicates, + 4 progressive |
| files, full size | 84.11 | 85.37 | 85.37 | +1.25 | 2 documents Desktop did not save |
| `thumbnails/` | 0 | 1.05 | 1.06 | +1.06 | no Desktop equivalent, inherited from Python |
| everything else | 24.74 | 20.04 | 17.57 | +0.75 | assets, HTML, JSON |

Python lands within **0.06 MB** of Desktop overall: its 520 px previews come
out 1.6 % over Desktop's, and its smaller sticker previews almost exactly
cancel its nine duplicate photos.

---

## Why nothing caught this

All three replay legs were green the entire time, and they were green for the
right reason: fed Desktop's own data, our writers reproduce Desktop's output
byte for byte. That is a true statement about the writers and says nothing at
all about what the converter puts into them.

Sort this document's Rust findings by where they hide:

- **Findings 2, 3, 5, 6, 9, 11** are wire-side. The converter emits a field
  Desktop does not, or fails to resolve one Desktop does. A replay leg cannot
  see either, because the reference supplies the very key the converter never
  writes. The **wire leg is the only thing in the workspace that catches
  these**, and it caught all of them.
- **Findings 1, 4, 7** are filesystem-side. The bytes on disk are wrong while
  every string in the JSON and the HTML is right. **Nothing in the workspace
  catches these** — not even the wire leg, which compares fields rather than
  files. They needed a tree walk with `sha256sum` and `exiftool`.
- **Findings 8 and 10** are rendering-side, on data the replay legs never
  exercise because Desktop's recorded output does not contain the shapes that
  trigger them.

The wire leg has earned its keep. The gap it does not cover is *the file behind
the name* — a path that is spelled identically in both exports and points at
different bytes. Finding 1 is 54 MB of exactly that, and the natural home for
catching it is a fourth check that hashes the media tree rather than reading
the JSON that describes it.

---

## Reproducing this

```powershell
cargo run --release -p tgx-parity -- json  "N:\telegram export\UA KOLAB TELEGRAM"
cargo run --release -p tgx-parity -- html  "N:\telegram export\UA KOLAB TELEGRAM"
cargo run --release -p tgx-parity -- media "N:\telegram export\UA KOLAB TELEGRAM"
cargo run --release -p tgx-parity -- wire  "N:\telegram export\UA KOLAB RUST"   "N:\telegram export\UA KOLAB TELEGRAM"
cargo run --release -p tgx-parity -- wire  "N:\telegram export\UA KOLAB PYTHON" "N:\telegram export\UA KOLAB TELEGRAM"
cargo run --release -p tgx-parity -- wire  "N:\telegram export\UA KOLAB RUST"   "N:\telegram export\UA KOLAB PYTHON"
```

The last one is the direct head-to-head: Python writes 6,396 field instances
Rust does not, Rust writes 2,680 Python does not, and 2,578 of those are the
`thumbnail` pair that Desktop *does* write.

Everything beyond the legs — the size accounting, the preview dimensions, the
escape census, the userpic analysis — came from reading the three trees
directly. All of it is read-only; nothing under `N:\` was modified.

**Run performance, for the record** (`dist/TelegramExporterData/tgx.log`):
6,687 messages read in 65.4 s, then 3,105 media files and ~316 MB in ~179 s at
1.4–2.5 MB/s, throttled throughout by `upload.getFile` FLOOD_WAITs. 21
downloads failed — all of them finding 2, all correctly declared.
