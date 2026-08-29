# AUDIT.md — what reviews and live runs actually found

The findings record. `ROADMAP.md` holds what is still open; this holds what was
found, what it cost, and the handful of places where reasoning about the code
turned out to disagree with running it.

Two code audits were done, 2026-08-26 and 2026-08-27, and between them they
raised 33 findings across every crate. All are closed; the items that were
*decided* rather than fixed have moved to `ROADMAP.md`'s "Still open". They are
not reproduced here, because a closed finding list is the kind of document that
gets read as current.

The section that matters is the next one.

---

# What the first live export found — 2026-08-27

The audits above were a **read** of the code plus a baseline that opened no
sockets. This is the other half: a real export was run, and its output
cross-examined field by field against other exports of the same supergroup —
Telegram Desktop's own, and one from the previous implementation.

**Every finding here is wire-only, and none is catchable by the three replay
legs.** That is not a coincidence. The legs replay a recorded `result.json`, so
they can only judge what the converter already put in the map. A key the
converter never writes is a key the reference JSON supplies for them, and they
read green.

The count that makes the point: reactions on 963 messages, actions on 63, 7
polls, 3 locations, 206 names and one linked-to index page — **all present in
the reference, all emitted by nothing at all, under 444 tests and three green
legs.**

### Fields the converter never emitted

- [x] **`reactions` — 963 of 6,643 messages carry them in the reference; a live
  export had none.** Now `convert::reactions_of`. The over-indent invariant, the
  `reactions_chosen` presentation key and the whole `enrich` path that fetches
  the *full* reactor list when the three-name cap bites were all in place and
  all downstream of a key nothing wrote.
- [x] **Service `action` — 63 of 63, all nine kinds.** Now
  `convert::service_action`, with the payload fields Desktop carries beside
  them: `inviter`, `members`, `title`, `new_title`, `message_id`,
  `new_icon_emoji_id`. A service message reached the JSON as a typed row with no
  verb in it.
- [x] **Polls — 7 of 7.** Now `convert::poll_of`, inserted from
  `engine::payload`. `plan::classify` only answers "what would we download", so
  a poll fell straight through it and the message arrived as bare text.
- [x] **Locations — 3 of 3.** Now `convert::location_of`, same fall-through,
  plus `live_location_period_seconds` where the TL object carries a period.

### Names that resolved to the empty string

- [x] **206 fields came out `""`** — 103 `from`, 96 `forwarded_from`, 7 `actor`.
  The `NameBook` was filled only from the participant roster, and `learn_user`
  had **no caller at all**, so anybody who posted and then left the group had no
  name anywhere to resolve from. Now `engine::learn_peers` harvests the sender
  and the chat off every message as it arrives — the people who were missing are
  by definition people who posted, so they arrive as the sender of their own
  messages. The chat is learned too, because a migration notice's actor *is* the
  chat and had no other source.

  Worth stating plainly: 206 is the number **with `member_roster` on**. With it
  off — a supported setting — it would have been every name in the export.

### Files that were referenced and never written

- [x] **`export_results.html` was never written, although all 9 topic pages link
  to it.** Every page opens with `<a href="../export_results.html">` and the
  target was absent on a real run. New `tgx-html/src/index.rs`, wired in
  `engine::write_index` off the same `split` branch that sets `back_href`, so the
  link and the file cannot drift apart. Only topics that produced a folder are
  listed — an empty one had its directory removed, and listing it would be the
  same dead link again.
- [x] **The inline preview is a third artifact, and nothing planned it.**
  `<full name>_thumb.jpg` (Telegram's thumbnail), the stripped thumbnail in
  `thumbnails/`, and `<stem>_thumb<ext>` (the preview the HTML's `<img>` points
  at) are three different files. `names::claim_rendered_preview` had existed
  since the media work landed and was **never called**. Now planned as
  `DownloadJob.preview_dest` and fetched by `download::fetch_preview`. The name
  is read off the job rather than derived again in `engine::payload`, because
  deriving it would miss a `(1)` collision suffix and point the `<img>` at a file
  the pool never writes.

  **A deviation recorded rather than hidden:** Desktop renders this file locally
  with an image scaler. We take Telegram's next size down, or copy the full file
  when there is none. The path is Desktop's and the role is Desktop's; the
  **bytes are not**. No leg compares them — the media leg diffs names — so
  nothing here goes red, and that is exactly why it is written down.

### A Desktop quirk only side-by-side exports could name

- [x] **Desktop appends a trailing empty text segment when the message text is
  not pure ASCII.** Over the whole reference, 98 messages end on an entity and
  exactly 11 carry the empty tail — the same 11 whose text contains a non-ASCII
  character. 98 of 98, no exceptions, no false positives. The entity *type*
  plays no part: `mention` and `link` occur on both sides of the split, so
  "always append when the text ends on an entity" would have been right 11 times
  and wrong 87.

  This is the signature of a **UTF-16 end offset compared against a UTF-8 byte
  length**: for ASCII the two numbers agree and Desktop's "is anything left?"
  test comes out false; for anything else the byte length is larger and it emits
  the leftover, which is empty. Reproduced in `tgx-format/src/text.rs`, with the
  reference message ids in the tests.

### The wire leg was broken three ways

The one check written specifically to catch all of the above could not have.
Each was found by running it and disbelieving the answer:

- [x] **It paired topics by folder name.** Desktop uses the bare topic title and
  we prefix the topic id, so `ćaskanje` and `0001 - ćaskanje` never met. Eight
  folders came back "only in ours" / "we did not export this topic" — a report
  that reads like a total export failure, having compared **zero** messages.
  `topics_by_name` now keys on the title, and pairs a lone topic a side outright,
  since a chat can be renamed between two runs.
- [x] **`MAY_DRIFT` let a field vanish entirely.** `reactions`, `edited`, `views`
  and `forwards` genuinely move between two runs minutes or months apart, so they
  are counted rather than raised — but "present with a different value" and
  "absent from ours" were scored the same way. That is how 963 missing reactions
  read as *"two runs, two points in time"*. A field the reference writes and we
  never do is now its own `absent` tally, outside the drift allowance.
- [x] **It never checked that a media path had a file behind it.** 1,546 dangling
  thumbnail references were invisible to it. Paths are now resolved against the
  tree and reported as `dangling`, with up to five examples — a dangling
  reference is worse than a stated gap, which is the argument `download.rs`
  already made and the leg was not enforcing.

---

# Where the remediation plan was wrong

The four places the plan disagreed with the code once it was opened. These are
the ones a future reader would otherwise repeat.

- **`icacls /c` must not be added.** The plan said to pass `/t /c` so the ACL
  reaches files that already exist. `/t` is right; `/c` is not. Measured on this
  machine: `icacls <missing path> /t /c` exits **0** while printing "Failed
  processing 1 files", and the same command without `/c` exits **3**. `/c` would
  have turned every partial failure into a reported success — precisely the state
  `lockdown_error` exists to prevent — and parsing "Failed processing" out of the
  text is not an option, because icacls is localised.
- **Desktop's action names are not the snake-cased constructor.** The plan said
  an explicit table should own the actions carrying a payload and a snake_case
  fallback could supply the rest of the names. It cannot: Desktop writes
  `create_group` for `ChatCreate`, `clear_history` for `HistoryClear`,
  `joined_telegram` for `ContactSignUp`, `delete_group_photo` for
  `ChatDeletePhoto`. Thirteen names are therefore a measurement against Desktop,
  not a derivation, and the table owns them. The fallback covers the rest.
- **A cancelled media batch needed a second change nobody scheduled.** Recording
  every un-run job as a stated gap is right, but the transcript is a 2,000-line
  ring whose whole purpose is that the INCOMPLETE warning can still be scrolled
  to — and a Stop during the 1,781-file folder would have flushed it with "not
  saved" lines. The listing is capped at twenty with a pointer to the file.
- **CI's missing `-- -D warnings` was already covered** by the `RUSTFLAGS: -D
  warnings` at the top of the workflow — verified with a scratch crate rather
  than reasoned about. It is spelled out anyway, because that chain is three
  deniable steps long.

---

# The three numbers to check on the next live run

Each was measured on the last run rather than reasoned about. Write the observed
numbers in beside them; where reality disagrees, the prediction was wrong and
that is the finding.

| | before | predicted after | observed |
|---|---|---|---|
| `absent` fields | 1,290 | **0** — 1,287 `thumbnail_file_size`, 3 `action` | — |
| blur previews displayed | 0 of 1,364 | **1,364** | — |
| empty `forwarded_from` | 94 across 13 people | **0**, at a cost of 13 requests | — |

The run that produces them should also cover **a basic group and a one-to-one
chat**, which no live run has ever exercised — see `ROADMAP.md`'s "Still open".

---

# Why nothing caught any of this

The structural lesson, which is the reason this file exists at all.

The three replay legs prove everything *downstream* of the wire and open no
sockets. They feed Desktop's own `result.json` through our writers and diff the
result, so they are a complete check on the writers and **no check at all on the
converter**: a key `convert.rs` never emits is a key the reference supplies on
its behalf. Four whole features were missing under 444 passing tests and three
green legs, and every leg was reading exactly what it was built to read.

`convert.rs` and `plan.rs` — TL object in, Desktop JSON out — have only
synthetic fixtures, because the reference records Desktop's *output*, not
Telegram's *input*. There is no oracle for that direction short of running it.

The wire leg is the only check that can see it, which is why it now tests for
two absences as well as for differences: a media path with no file behind it
(`dangling`), and a field the reference writes that we never write (`absent`,
kept outside the `MAY_DRIFT` allowance that had been scoring 963 missing
reactions as honest run-to-run drift).

**Treat a green suite as saying nothing about the wire.**
