# Code audit — 2026-08-26

Full read of every crate at `2906e86`, plus a live baseline run. Findings are
ordered by severity, each with the file and line that carries it and the reason
it matters. Tick a box when the fix lands *and* a test covers it.

## Baseline

Established before anything else, so a "finding" is never just a stale build:

| check | result |
|---|---|
| `cargo clippy --all-targets --all-features` | clean |
| `cargo test --all` | **357 passed, 0 failed** (19 suites) |
| `cargo run -p tgx-parity -- json reference` | **4 of 4 topics byte-identical** |
| `cargo run -p tgx-parity -- html reference` | **4 of 4 topics, 256,780 lines** |
| `cargo run -p tgx-parity -- media reference` | **830 of 836**, the six being the documented custom-emoji ceiling |
| tracked secrets | none — `dist/TelegramExporterData/session` and `settings.json` are correctly ignored |
| `unwrap()` / `panic!` outside tests | none |

Every README claim that can be checked from this machine checks out. The pure
layers — `tgx-format`, `tgx-html`, `tgx-media` — are genuinely pinned by the
oracle. **Almost every real problem below is in the layers the oracle cannot
reach**: `tgx-tg`'s wire-facing half and `tgx-app`.

---

## Critical

- [x] **1. Two-factor sign-in through the GUI can never complete.** *Fixed in
  `5f14fa8`: one `Session` is now held in `actions::PENDING` across the steps,
  kept through `NeedPassword` and a mistyped code, dropped on success and on
  Cancel.*
  `crates/tgx-app/src/actions.rs:83` builds a *fresh* `Session` on every login
  step; line 111 then calls `session.check_password(&secret)`.
  `check_password` (`crates/tgx-tg/src/client.rs:132`) does
  `self.password.take()`, and `password` is only ever set by `sign_in` **on the
  same session object** — which was dropped when the previous task ended. A new
  session always has `password: None`, so every 2FA attempt returns
  `"two-factor sign-in is not pending"`. Provable from the code; no account
  needed.
  `client.rs:113-117` documents this exact failure *as fixed* ("Dropping it here
  is what made two-factor sign-in impossible to complete") — the app layer
  reintroduces it one crate up. The CLI (`crates/tgx-tg/src/bin/tgx.rs:70-90`)
  holds one session across all steps and is correct, which is why nobody hit it.
  **Fix:** keep the `Session` alive across login steps — own it on the shell, or
  move the whole exchange into one task that awaits the user's input.

- [x] **1b. The code path re-requests a login code before submitting one.**
  *Fixed in `5f14fa8`. It was worse than a possibly-new `phone_code_hash`:
  Telegram treats a second `auth.sendCode` as starting over, so it invalidated
  the code the user was typing, and repeated attempts drew `AUTH_RESTART`, which
  is now also retried once where it is raised.*
  `actions.rs:91-96` calls `request_code` again, then `sign_in(&secret)` — the
  code the user typed is submitted against a possibly-new `phone_code_hash`.
  Falls out of the same fix.

- [x] **2. Telegram's own thumbnails are written into `result.json` but never
  downloaded.** *Fixed: `download::fetch_thumb`
  (`crates/tgx-tg/src/download.rs:170`) is called from `run_one` at line 124,
  so `thumb_dest` is read at last. The audit had half of it. Desktop does not
  write a **path** in `thumbnail` for a size-skipped file — it writes the
  placeholder, the same string it writes in `file`. Counted over the
  reference's 1,786 skipped files: 1,287 carry the placeholder, 499 carry no
  `thumbnail` key at all, and 0 carry a path. `plan.rs:382` now writes
  `TOO_LARGE` there and reserves no name, so a skip promises nothing it has no
  job to fetch. The old test `a_skipped_document_keeps_its_thumbnail_record`
  asserted the wrong half of that and is now
  `a_skipped_document_records_its_thumbnail_as_skipped_too`
  (`plan.rs:607`).*
  `crates/tgx-tg/src/plan.rs:370` inserts
  `"thumbnail": "<path>"`; line 405 populates `DownloadJob.thumb_dest` — and
  **nothing ever reads it** (`grep thumb_dest` returns writes only). Every
  export therefore carries dangling `thumbnail` references, and because they
  never reach the pool they are not recorded in `missing_media.txt` either —
  exactly the "a dangling reference is worse than a stated gap" failure
  `download.rs` argues against. The wire leg counts **1,287** `thumbnail`
  decisions, so this is not a rare path.
  **Fix:** fetch the thumb in `download::run_one`, or stop writing the key.

- [x] **3. `Stop` does not stop, and re-enables `Export`.** *Fixed:
  `crates/tgx-tg/src/cancel.rs` is the token, checked in the read loop, in
  `sleep_in_slices` and in the download pool, and `ExportError::Cancelled` is
  now constructed. `Shell::stop` (`crates/tgx-app/src/shell/mod.rs:792`) sets
  it and **leaves `exporting` true** until the worker's `Finished` arrives —
  the interface says "Stopping…" in between. The token resets when a run
  starts, never when one ends, so a cancel landing after the last message does
  not carry into the next run.*
  `crates/tgx-app/src/shell.rs:486` sets `exporting = false` and status
  `"Stopped"` but touches nothing on the tokio side — the export keeps reading
  history and writing files. Line 392 gates the Export button on
  `!self.exporting`, so after Stop a second concurrent export of the same queue
  can be started.
  Related, and part of the same hole: `ExportError::Cancelled`
  (`crates/tgx-tg/src/error.rs:74`) is **never constructed anywhere**, and
  `engine::sleep_in_slices` justifies itself entirely with *"a flat sleep
  swallows a click on Cancel for the whole two minutes"* — there is no cancel
  signal to observe between slices, so the slicing is currently a no-op for its
  stated purpose.
  **Fix:** a cancellation token checked in the read loop, in `sleep_in_slices`
  and in the download pool; `stop()` sets it and leaves `exporting` true until
  `Finished`/`Failed` arrives.

---

## High

- [x] **4. `FLOOD_PREMIUM_WAIT` is classified as a permanent refusal.**
  *Fixed: `crates/tgx-tg/src/error.rs:96` is now
  `name.contains("FLOOD") && name.contains("WAIT")`, which catches the slow-mode
  spellings too. The two new tests drive `classify` through `RpcError::from`
  with the real wire strings rather than hand-built names —
  `every_spelling_of_a_wait_is_a_wait` (`error.rs:145`) over `FLOOD_WAIT_31`
  and `FLOOD_PREMIUM_WAIT_60`, and `a_refusal_is_still_a_refusal`
  (`error.rs:167`) so the widened match did not swallow the other direction.*
  `crates/tgx-tg/src/error.rs:87` tests `rpc.name.contains("FLOOD_WAIT")`.
  `"FLOOD_PREMIUM_WAIT_60"` does **not** contain that substring. The comment
  directly above (line 85) claims it does. This is precisely the
  rate-limit-mistaken-for-a-refusal class the module exists to make
  unrepresentable, and no test exercises `classify` against a real RPC name.
  **Fix:** match `name.contains("FLOOD")` *and* `name.contains("WAIT")`, or list
  the names explicitly — plus a table test over the real error strings.

- [x] **5. Every channel and supergroup is exported as `public_*`.** *Fixed:
  `ChatInfo` carries `public`, set in `dialogs::chat_info`
  (`crates/tgx-tg/src/dialogs.rs:53`) from `has_username`
  (`dialogs.rs:126`), which reads **both** `username` and the `usernames`
  vector — a channel that bought a second name keeps the first in `usernames`
  and would otherwise have flipped to private. `engine.rs:213` now calls
  `chat.kind.export_type(chat.public)`. The same pass stopped hardcoding
  `access_hash` to `0`; see 12 for what that unlocked.*
  `crates/tgx-tg/src/engine.rs:191` hardcodes `chat.kind.export_type(true)`.
  `ChatInfo` carries no `public`/`username` field, so the `false` branch is
  unreachable in production — a private supergroup writes
  `"type": "public_supergroup"`. The unit test at `client.rs:264` covers the
  function, not the call.
  **Fix:** carry `username.is_some()` on `ChatInfo` from `dialogs::chat_info`.

- [x] **6. An empty non-forum chat deletes its own export folder — including
  `participants.json`.** *Fixed: `close_all` prunes only when
  `sink.output.root != root` (`crates/tgx-tg/src/engine.rs:663`). An empty
  chat now leaves an empty export rather than no export.*
  `crates/tgx-tg/src/engine.rs:474` does
  `remove_dir_all(&sink.output.root)` when a sink wrote zero messages. In the
  split-topics case that is a topic subfolder (intended). In the non-split case
  `Output::new(root, …)` at line 220 means `sink.output.root == root`, so a chat
  with no messages wipes the whole chat directory, discarding the roster written
  at line 265 and releasing the `unique_dir` reservation.
  **Fix:** only prune when the sink's root is a topic subfolder of `root`.

- [x] **7. Stripped thumbnails consume the `file_N` counter.** *Fixed:
  `names::layout` gained `"thumbnails" => ("thumbnails", ".jpg")`
  (`crates/tgx-media/src/names.rs:111`) and `synth_prefix` gained
  `"thumbnails" => "thumb"` (`names.rs:128`), so the name is
  `thumb_N@stamp.jpg` and `files/` numbering is untouched. The prefix was read
  off the Python exporter's own output, not chosen —
  `thumbnails\thumb_10@08-03-2026_23-17-42.jpg`.*
  `crates/tgx-tg/src/plan.rs:412` calls `reserve_name("thumbnails", …)`, but
  `layout("thumbnails")` falls through to `("files", "")`, so `synth_prefix`
  returns `"file"` — the same counter real unnamed documents use. Verified with
  a throwaway test: the thumbnail takes `file_1@s.jpg` and the next real
  document becomes `file_2@s.pdf`. The comment on that branch claims the
  opposite ("the stripped thumbnail gets its own counter"). Every skipped file
  shifts `files/` numbering, breaking Desktop parity — and **the media leg
  cannot see it**, because a Desktop `result.json` has no `stripped_thumbnail`
  key to replay.
  **Fix:** give `thumbnails` its own entry in `layout` and `synth_prefix`.

- [x] **8. The media parity leg asserts nothing.** *Fixed:
  `crates/tgx-parity/tests/corpus.rs:77` is
  `assert_eq!(failures, 0, "media names fell below the known ceiling")`, the
  same shape the json and html legs already had. The floor is not restated in
  the test: the leg already knows its own ceiling — 830 of 836, the six being
  custom emoji — and returns `1` when a run falls below it, so the number lives
  in one place instead of two that can drift.*
  `crates/tgx-parity/tests/corpus.rs:75` is
  `let _ = media_leg::run(&topics).expect(...)`. 830/836 could drop to 0/836 and
  `cargo test` stays green; the number lives in stdout, which cargo captures.
  The module docstring twelve lines above warns about exactly this ("the classic
  way a suite goes quietly green").
  **Fix:** assert a floor — `assert!(r.exact >= 830)`.

---

## Medium

- [x] **9. The live HTML is missing the entire presentation layer.** *Fixed:
  `convert::presentation` (`crates/tgx-tg/src/convert.rs:354`) builds the map
  and `engine::payload` inserts it (`engine.rs:601`), last, because it reads
  the finished map — the media paths and sizes the plan decided are what the
  preview points at. It covers `from_name`, `forwarded_from_name`,
  `forwarded_date`, `group`, `initials`, `colours`, `reactions_chosen` and
  `preview`. The size of what it was costing was only measurable once a live
  export existed: **zero `<img>` elements in the whole archive, against
  Desktop's 649** over the same chat. `Output::close` still strips `_p` before
  the JSON is written, so this reaches the HTML writer and nothing else. The
  leg's own blind spot is unchanged and is now stated in ROADMAP — see the
  2026-08-27 section below.*
  `grep '"_p"' crates/tgx-tg/src/` returns nothing — **the engine never emits
  the `_p` map**. `crates/tgx-tg/src/convert.rs:27-34`'s `NameBook` populates
  `.html`, `.initials` and `.colour` and *nothing reads them*;
  `NameBook::html_name` is called only from its own test.
  In a real export that means: no inline `<img>` previews at all
  (`render_media` always falls through to a media row), userpic initials derived
  from the display string instead of the name *fields*, no custom name colours,
  no forward-date tooltips, no `reaction active`, and the `stripped_thumbnail`
  files that *are* written to disk referenced by nothing.
  The html leg reads 4/4 because `crates/tgx-parity/src/html_leg.rs:324-356`
  lifts `_p` out of Desktop's own HTML — **it proves the writer, not the
  pipeline**. ROADMAP's "everything downstream is pinned byte for byte, so the
  exposure is narrow and named" does not cover this gap.
  **Fix:** build `_p` in `engine::payload` from the `NameBook` it already
  maintains; note in ROADMAP that inline preview *rendering* remains open.

- [x] **10. `icacls` and `explorer` are launched by bare name.** *Both fixed:
  `icacls` in `e6dbe77` via `config::system32`, `explorer` likewise. The audit
  was right that this is the moment the app is securing its credential store.*
  `crates/tgx-tg/src/config.rs:262` and `crates/tgx-app/src/actions.rs:262`.
  Windows `CreateProcess` search order includes the application directory and
  the current directory before `PATH`, and this app is explicitly designed to be
  copied onto USB sticks and run from arbitrary folders — a planted
  `icacls.exe` runs with the user's rights at exactly the moment the app is
  securing its credential store.
  **Fix:** `%SystemRoot%\System32\icacls.exe`, and an absolute `explorer.exe`.

- [x] **11. A rate limit during topic discovery silently collapses a forum into
  one folder.** *Fixed: `actions::resolve_topics`
  (`crates/tgx-app/src/actions.rs:336`) separates the two answers. A genuine
  refusal has no shape left to preserve and still degrades to one folder; a
  `Transient` waits and retries once, and if it is still rate-limited the chat
  is failed rather than exported in the wrong shape. It takes `fetch` as a
  parameter so the distinction can be driven with a canned `EnrichError` in a
  test instead of a live socket (`actions.rs:713`).*
  `crates/tgx-app/src/actions.rs:172-177` catches *any* error from
  `list_topics` — `Transient` included — and falls back to
  `vec![Topic::general()]`. Splitting by topic is the app's entire reason to
  exist; a FloodWait should defer, not silently change the output shape.

- [x] **12. `peer_ref_for` in the GUI swallows errors and hides rate limits.**
  *Fixed, both halves. It is now `dialogs::peer_refs_for`, plural: **one dialog
  sweep for the whole queue** before any message is fetched
  (`crates/tgx-app/src/actions.rs:416`), so a twenty-chat queue pages the
  dialog list once rather than twenty times — the pattern that earns a flood
  wait before the export has written a byte. It returns a `Result`, and a
  transport failure fails the run with the error rather than telling the user
  per chat to go looking for conversations they still have. "No longer in the
  dialog list" is now said only when the sweep succeeded and the id was not in
  it. `ChatInfo.access_hash` carries the real hash — see 5.*
  `crates/tgx-app/src/actions.rs:254`:
  `while let Ok(Some(d)) = iter.next().await` treats any error as end-of-list,
  then reports `"{chat} is no longer in the dialog list"` — a confidently wrong
  message that skips the chat. The CLI version (`bin/tgx.rs:229`) propagates
  properly.
  Separately this walks every dialog once **per queued chat** — O(chats ×
  dialogs) requests — and `ChatInfo.access_hash` exists to avoid that but is
  hardcoded to `0` in all three branches of
  `crates/tgx-tg/src/dialogs.rs:43-76` and read nowhere.

- [x] **13. Protocol-relative and UNC hrefs survive the allowlist.** *Fixed:
  the relative-URL branch of `crates/tgx-html/src/escape.rs:96` rejects a
  leading `//` or `\\`, and line 101 rejects the mixed forms (`/\`, `\/`)
  that Windows resolves the same way. A single leading slash is still an
  ordinary relative path and still passes (`escape.rs:257`) — that case is
  reachable from real media paths.*
  Verified:
  `safe_href("//evil.example/x")` → `Some("//evil.example/x")` and
  `safe_href(r"\\evil.example\share")` → accepted. In an archive opened as
  `file://`, both resolve to `file://evil.example/…`, which on Windows is a UNC
  path — clicking one initiates an SMB connection to an attacker-chosen host
  (NTLM leak). Telegram `text_link` entities carry arbitrary URLs.
  **Fix:** reject a leading `//` or `\\` in the relative-URL branch of
  `crates/tgx-html/src/escape.rs:96`.

- [x] **14. `list_topics` has no loop bound.** *Fixed:
  `MAX_TOPIC_PAGES = 200` (`crates/tgx-tg/src/dialogs.rs:402`), named after
  `enrich`'s own cap, which this loop was the only one missing. The page is
  also keyed on the topic id rather than merely appended
  (`dialogs.rs:428`), so a server that ignores the offset stops adding rows
  instead of growing `out.len()` every iteration.*
  `crates/tgx-tg/src/dialogs.rs:144` breaks only when a page adds nothing. If
  offsets are not honoured the same page is pushed repeatedly, `out.len()` grows
  every iteration, and the loop never terminates. `fetch_participants` has a
  cap; this does not.

- [x] **15. The panic hook blames the GPU for every panic.** *Fixed:
  `panic_message(window_opened, panic)` (`crates/tgx-app/src/main.rs:70`)
  chooses the text off a `WINDOW_OPENED` flag set the moment `open_window`
  returns `Ok`. A panic before that still names the DirectX requirement; a
  panic after it says the app stopped unexpectedly, because the renderer has
  plainly been drawing and naming the GPU would send someone to fix a driver
  that is fine. Three tests, including one asserting the flag starts `false`.*
  `crates/tgx-app/src/main.rs:41` installs a global hook whose message is always
  *"This build needs a GPU with working DirectX drivers."* A panic anywhere —
  including inside an export — is reported as a driver problem and overwrites
  `startup-error.log`.
  **Fix:** scope it to startup, or make the text conditional.

---

## Low / documentation

- [x] **`gpui-component` is not pinned.** *Fixed:
  `crates/tgx-app/Cargo.toml:28` is `gpui-component = "=0.5.1"`. Both are now
  what the workspace comment and ROADMAP's retired-risk table always claimed.*
- [x] **The "enforced by the build" dependency rules do not exist.** *Fixed:
  `crates/tgx-parity/tests/layering.rs` reads the manifests and fails on
  `tgx-html` reaching `grammers-tl-types`, or the analyser reaching `tgx-tg`.
  It runs in `cargo test --all`, so the claim is now checked rather than
  reworded.*
- [x] **`topics::sanitize_component` truncates after trimming.** *Fixed:
  `crates/tgx-media/src/topics.rs:58` cuts at 120 characters and **then**
  trims `.` and ` ` again, mirroring `names::sanitize_filename`. Two tests
  drive the two boundaries — a 120th character that is a dot and one that is a
  space — and both assert the result is 119 characters, not 120.*
- [x] **Message-block joining measures a duration in local wall-clock.**
  *Fixed: `crates/tgx-html/src/join.rs:131` takes the gap from
  `date_unixtime` whenever both messages carry it, falling back to `date`
  only when one does not. Measured on the case the corpus cannot reach: at a
  fall-back, two messages 90 s apart come out −3510 s off `date`, fall outside
  `0..=900`, and Desktop's single sender block splits in two.*
- [x] **`save.bat --force` force-pushes with no prompt.** *Fixed: every push
  path is `--force-with-lease` (`save.bat:191`, `save.bat:402`), both prompts
  say what that overwrites and what it does not, and `--force` is documented in
  the menu (`save.bat:69`).*
- [x] **`peer.rs:105`'s doc contradicts its own test.** *Fixed, and the doc was
  the one that was right: Desktop paints `Nf` 281 times in the reference's
  HTML and `NG` nowhere. The test had been handing the whole tail in as the
  surname; it now splits the fields the way the export does
  (`crates/tgx-format/src/peer.rs:278`).*
- [x] **The CLI echoes the 2FA password.** *Fixed: `prompt_hidden`
  (`crates/tgx-tg/src/bin/tgx.rs:80`) turns off console echo through
  `windows-sys`, which is already a dependency — `rpassword` would have been a
  new crate for three functions' worth of FFI.*
- [x] **Binary-size figures disagree across docs.** *Fixed: 19.5 MB against the
  Python build's 46.4 MB, everywhere — README, ROADMAP, `Cargo.toml`,
  `ci.yml` and `save.bat`. `ci.yml` prints the measured number on every build
  so the next disagreement is visible rather than archaeological.*
- [x] **Minor overflow / truncation.** *All three fixed:
  `Settings::size_limit_bytes` uses `saturating_mul`
  (`crates/tgx-tg/src/config.rs:186`); `client.rs:113` is
  `i32::try_from(settings.api_id)` with an error naming `settings.json`, not
  an `as` cast; and `json::header_prelude` handles the empty map explicitly
  (`crates/tgx-format/src/json.rs:92`) rather than cutting two bytes off `{}`
  and opening the file on `,\n "messages": [`.*

---

## Suggested order

*Discharged. Every box above is ticked; the order below is kept as the record of
how it was worked, not as a plan.*

1. **3** — Stop actually cancels. User-visible on day one, and the cancellation
   token it needs is a prerequisite for doing **11** properly.
2. **1** — GUI 2FA. Anyone with two-step verification cannot use the window.
3. **2** — thumbnails. Every export currently ships broken links.
4. **4**, **5**, **6**, **7** — small, mechanical, one test each.
5. **8** can jump the queue: until it asserts, the suite can go quietly green
   while the rest is in flight.

---

# What the first live export found — 2026-08-27

The audit above was a **read** of the code plus a baseline that opened no
sockets. This section is the other half: a real export was run, and its output
was cross-examined against two other exports of the same supergroup — Telegram
Desktop's own, and the Python original's. Three runs of one chat, diffed
field by field.

**Every finding here is wire-only, and none of them is catchable by the three
replay legs.** That is not a coincidence and it is not a surprise: the legs
replay a recorded `result.json`, so they can only judge what the converter
already put in the map. A key the converter never writes is a key the reference
JSON supplies for them, and they read green.

The count that makes the point: reactions on 963 messages, actions on 63,
7 polls, 3 locations, 206 names and one linked-to index page — **all of them
present in the reference, all of them emitted by nothing at all, and 444 tests
plus three green legs over the top.**

### Fields the converter never emitted

- [x] **`reactions` — 963 of 6,643 messages carry them in the reference; a live
  export had none.** Now `convert::reactions_of`
  (`crates/tgx-tg/src/convert.rs:516`). The over-indent invariant, the
  `reactions_chosen` presentation key and the whole `enrich` path that fetches
  the *full* reactor list when the three-name cap bites were all in place and
  all downstream of a key nothing wrote.
- [x] **Service `action` — 63 of 63, all nine kinds.** Now
  `convert::service_action` (`convert.rs:662`), with the payload fields
  Desktop carries beside them: `inviter`, `members`, `title`, `new_title`,
  `message_id`, `new_icon_emoji_id`. A service message reached the JSON as a
  typed row with no verb in it.
- [x] **Polls — 7 of 7.** Now `convert::poll_of` (`convert.rs:584`), inserted
  from `engine::payload` (`engine.rs:579`). `plan::classify` only answers
  "what would we download", so a poll fell straight through it and the message
  arrived as bare text.
- [x] **Locations — 3 of 3.** Now `convert::location_of` (`convert.rs:630`),
  same fall-through, plus `live_location_period_seconds` where the TL object
  carries a period.

### Names that resolved to the empty string

- [x] **206 fields came out `""` in a live export** — 103 `from`, 96
  `forwarded_from`, 7 `actor`. The `NameBook` was filled only from the
  participant roster, and `learn_user` had **no caller at all**, so anybody who
  posted and then left the group had no name anywhere to resolve from. Now
  `engine::learn_peers` (`crates/tgx-tg/src/engine.rs:515`) harvests the sender
  and the chat off every message as it arrives — the people who were missing
  are by definition people who posted, so they arrive as the sender of their
  own messages. The chat is learned too, because a migration notice's actor
  *is* the chat and had no other source.
  Worth stating plainly: 206 is the number **with `member_roster` on**. With it
  off — a supported setting — it would have been every name in the export.

### Files that were referenced and never written

- [x] **`export_results.html` was never written, although all 9 topic pages
  link to it.** Every page opens with `<a href="../export_results.html">` and
  the target was absent on a real run. New `crates/tgx-html/src/index.rs`,
  wired in `engine::write_index` (`engine.rs:693`) off the same `split` branch
  that sets `back_href`, so the link and the file cannot drift apart. Only
  topics that produced a folder are listed — an empty one had its directory
  removed, and listing it would be the same dead link again.
- [x] **The inline preview is a third artifact, and nothing planned it.**
  `<full name>_thumb.jpg` (Telegram's thumbnail), the stripped thumbnail in
  `thumbnails/`, and `<stem>_thumb<ext>` (the preview the HTML's `<img>` points
  at) are three different files. `names::claim_rendered_preview`
  (`crates/tgx-media/src/names.rs:289`) had existed since Phase 4 and was
  **never called**. Now planned as `DownloadJob.preview_dest`
  (`crates/tgx-tg/src/plan.rs:441`) and fetched by `download::fetch_preview`
  (`crates/tgx-tg/src/download.rs:214`). The name is read off the job rather
  than derived again in `engine::payload` (`engine.rs:563`), because deriving
  it would miss a `(1)` collision suffix and point the `<img>` at a file the
  pool never writes.
  **A deviation worth recording rather than hiding:** Desktop renders this file
  locally with an image scaler. We take Telegram's next size down, or copy the
  full file when there is none. The path is Desktop's and the role is Desktop's;
  the **bytes are not**. No leg compares them — the media leg diffs names — so
  nothing here goes red, and that is exactly why it is written down.

### A Desktop quirk that only three exports side by side could name

- [x] **Desktop appends a trailing empty text segment when the message text is
  not pure ASCII.** Over the whole reference, 98 messages end on an entity and
  exactly 11 carry the empty tail — the same 11 whose text contains a
  non-ASCII character. 98 of 98, no exceptions, no false positives. The entity
  *type* plays no part: `mention` and `link` occur on both sides of the split,
  so "always append when the text ends on an entity" would have been right 11
  times and wrong 87.
  This is the signature of a **UTF-16 end offset compared against a UTF-8 byte
  length**: for ASCII the two numbers agree and Desktop's "is anything left?"
  test comes out false; for anything else the byte length is larger and it
  emits the leftover, which is empty. Reproduced in
  `crates/tgx-format/src/text.rs:159`, with the reference message ids in the
  tests.

### The wire leg was broken three ways

The one check written specifically to catch all of the above could not have.
Each of these was found by running it and disbelieving the answer:

- [x] **It paired topics by folder name.** Desktop uses the bare topic title
  and we prefix the topic id, so `ćaskanje` and `0001 - ćaskanje` never met.
  Eight folders came back "only in ours" / "we did not export this topic" —
  a report that reads like a total export failure, having compared **zero**
  messages. `topics_by_name` (`crates/tgx-parity/src/wire_leg.rs:463`) now
  keys on the title, and pairs a lone topic a side outright, since a chat can
  be renamed between two runs.
- [x] **`MAY_DRIFT` let a field vanish entirely.** `reactions`, `edited`,
  `views` and `forwards` genuinely move between two runs minutes or months
  apart, so they are counted rather than raised — but "present with a different
  value" and "absent from ours" were scored the same way. That is how 963
  missing reactions read as *"two runs, two points in time"*. A field the
  reference writes and we never do is now its own `absent` tally
  (`wire_leg.rs:381`), outside the drift allowance.
- [x] **It never checked that a media path had a file behind it.** 1,546
  dangling thumbnail references in a live export were invisible to it. Paths
  are now resolved against the tree and reported as `dangling`
  (`wire_leg.rs:366`), with up to five examples — a dangling reference is worse
  than a stated gap, which is the argument `download.rs` already made and the
  leg was not enforcing.

### Still open after all of it

Stated rather than ticked, because neither is fixed:

- **The html leg still synthesises `_p` itself.** `html_leg.rs:426` lifts the
  presentation map out of Desktop's own pages and feeds it back in, so the leg
  proves **the writer, not the pipeline** — which is precisely why finding 9
  above could sit green for the whole of Phases 3–7. The pipeline half is now
  covered by `crates/tgx-tg/tests/wire.rs` instead, converter in and map out.
  The leg's blind spot itself is unchanged.
- **`custom_emoji.document_id` stays a numeric id rather than a sticker path.**
  This is the documented media-leg ceiling: 830 of 836, the six being custom
  emoji, and a JSON replay cannot see them at all.
