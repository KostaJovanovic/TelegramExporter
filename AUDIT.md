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

---

# Code audit — 2026-08-27 (second pass)

Full read of every crate at `433ae90`, plus a **second live export** (`N:\telegram
export\UA KOLAB RUST`, 6,687 messages, 3,105 files) put through the wire leg and
then cross-examined field by field against Desktop's export and the Python
original's. Where a finding could be checked against the oracle rather than
argued from the code, it was, and the number is quoted.

Three findings below are proved by a runnable repro rather than by reading:
the libtest capture behaviour (1), the batch trailing-space behaviour (2), and
the settings-loading behaviour (3). Each took a throwaway program because each
contradicts something the repo currently asserts.

## Baseline

| check | result |
|---|---|
| `save.bat test` (at `a96519b`, before `433ae90` landed) | **509 passed, 0 failed** (19 suites), fmt and clippy clean |
| `tgx-parity json/html/media reference` | 4/4, 4/4, 830/836 — all at their documented marks |
| **`tgx-parity wire "…UA KOLAB RUST"`** | **RED**: 136 field mismatches, **1,290 absent fields**, **21 dangling paths** |

The three replay legs are green and the one leg that opens a real export is not.
That is the same shape as the 2026-08-27 section above, and for the same reason:
the replay legs cannot see a key the converter never writes.

**A note on provenance.** The live run was produced by `dist\TelegramExporter.exe`
built at 01:16, which predates `433ae90`. Every wire finding below was therefore
re-checked against current source before being written down; all of them still
hold. One apparent finding did not survive that check and is recorded here so it
is not rediscovered: the export's own narration is absent from `tgx.log`, but
that is the stale binary — `Progress::Log → log::info!` landed in `433ae90`
itself.

---

## Critical

- [ ] **1. The oracle does not run in CI, and the skip is invisible.**
  `crates/tgx-parity/tests/corpus.rs:27-41`, `.github/workflows/ci.yml:41`.
  `reference/` is gitignored, so on CI `corpus_dir()` always returns `None`,
  all four corpus tests return early, and the run reports `ok`. The design rests
  on the skip being *visible* — `corpus.rs:7-12` and `CLAUDE.md` both promise
  "the skip prints exactly what it did not check". **It does not.** libtest
  captures stdout *and* stderr and discards both for a test that passes.
  Verified with a throwaway `rustc --test` binary: a passing test whose body is
  `eprintln!` + `println!` prints neither string — only `test … ok`.
  So CI verifies the unit tests and the layering rules, verifies **none** of the
  byte-exactness the project exists for, and says so nowhere. Every class the
  three legs were written to catch is unguarded there, permanently, behind a
  green check.
  **Fix:** `--nocapture` on the CI test step is the one-word version; better is
  a `TGX_REQUIRE_CORPUS=1` that turns the skip into a failure on any machine
  that is supposed to have a corpus.

- [ ] **2. `save.bat release` has never built anything, and exits 0.**
  `save.bat:42` and `save.bat:77` are `(set DO_BUILD=1 & goto save)`. `cmd`
  takes the value up to the `&` **including the space**, so `DO_BUILD` is
  `"1 "`, and the test at `save.bat:207` is `=="1"`. It never matches.
  Verified: a batch file of exactly this shape prints `DO_BUILD=[1 ]` and the
  comparison reports no match; `set "DO_BUILD=1"` on its own line matches.
  `release` and menu option 6 therefore run test + commit + push, skip the
  build, print `[time] total` and exit **zero** — indistinguishable from a
  release that worked. `dist\` keeps whatever was there before, which is how
  the export analysed in this audit came to be produced by a binary 46 minutes
  older than the source that was being read.
  `save.bat:37`'s `FORCE_MODE` has the identical defect (`save.bat:160`), so
  `--force` silently falls through to the ordinary prompt. That one fails
  *safe*; the build does not.
  **Fix:** `set "DO_BUILD=1"` as its own statement, both sites.

- [ ] **3. One out-of-range number in `settings.json` discards the whole file,
  and the window then writes the defaults back over the credentials.**
  `crates/tgx-tg/src/config.rs:217`. `same_shape` (`config.rs:245`) compares
  JSON *kinds* only, so `Number` matches `Number` — it cannot reject a number
  of the right kind and the wrong range. `{"page_size": -1}` (the field is
  `usize`) or `{"size_limit_mb": 20.5}` (it is `i64`) therefore passes the
  shape gate, is inserted into `base`, and then fails `from_value` for the
  **entire struct** — `#[serde(default)]` fills in absent fields, it does not
  rescue present-and-failing ones. `unwrap_or(defaults)` drops everything else
  in the file with it.
  Verified with a standalone reproduction of `load_from_str`:

  | `settings.json` holds | `api_hash` survives |
  |---|---|
  | `"page_size": "nonsense"` — wrong **kind** | yes (this is the case the test covers) |
  | `"page_size": -1` — wrong **range** | **no** |
  | `"size_limit_mb": 20.5` — wrong **number kind** | **no** |

  This contradicts the doc at `config.rs:192` ("falling back **per field**
  rather than per file") and the test `one_bad_field_does_not_lose_the_others`
  (`config.rs:528`), which only ever supplies a wrong *type*. The clamps at
  `config.rs:221-223` run after deserialisation and cannot help.
  It escalates: `Settings::load()` hands back `api_id: 0`, `api_hash: ""`, and
  the window persists settings on any change (`shell/mod.rs:651`,
  `shell/settings.rs:318`, `shell/signin.rs:300`), writing the defaults over
  the file. **One hand-typed `-1` destroys the stored API credentials, the
  phone number and the output directory.**
  **Fix:** deserialise field by field, or range-check inside `same_shape`.

---

## High

- [ ] **4. `thumbnail_file_size` is never written for a size-skipped file —
  1,287 of them.** `crates/tgx-tg/src/plan.rs:398` sets the key only inside the
  `Some(dest)` arm. Measured over the reference: Desktop writes `thumbnail_file_size`
  in **every** case where `thumbnail` is present — 257 saved **and 1,287
  skipped**, 1,544 of 1,544. Ours: 257 written, 1,302 omitted. This is the
  whole of the wire leg's `absent` tally bar three. The comment at
  `plan.rs:381-391` reasoned the skipped branch out correctly for the
  `thumbnail` *path* and dropped the size key with it.
  **Fix:** write it in the `None` arm too, gated on `facts.thumb_size > 0`.

- [ ] **5. `poll_append_answer` is mapped to the wrong TL constructor, and the
  Python original's generic fallback was dropped.**
  `crates/tgx-tg/src/convert.rs:741` is
  `A::TodoAppendTasks(_) => no_payload("poll_append_answer")`.
  `messageActionPollAppendAnswer` is its **own constructor** in the schema
  (`api.tl`), distinct from `messageActionTodoAppendTasks` and
  `messageActionTodoCompletions`. Three service messages (ćaskanje #5609,
  #5612, #5615) came out of the live run with **no `action` key at all**;
  Desktop names all three `poll_append_answer`, and so does the Python export.
  Two defects, one arm:
  * the mapping names a to-do action as a poll action, and leaves the real poll
    action unmapped;
  * `_ => None` means any of the **57 other** `messageAction*` constructors
    vanishes silently. The Python original does not do this — `snake_action()`
    (`app/tg/serialize.py:817`) turns any unmapped class name into its
    snake_case spelling, which is exactly how Desktop names actions it was
    written before. That is the guard the port lost, and it is why the Python
    export got these three right and we did not.
  The test at `convert.rs:1021` **pins the wrong behaviour** — it asserts
  `TodoAppendTasks → "poll_append_answer"` under the name
  `every_action_the_reference_holds_is_named_the_way_desktop_names_it`.
  Desktop's payload for this action is actor + action and nothing else
  (verified: all three reference messages carry no `answer` key), so the
  replacement arm is `A::PollAppendAnswer(_) => no_payload("poll_append_answer")`.

- [ ] **6. One mistyped two-factor password makes the rest of the sign-in
  unreachable.** `crates/tgx-tg/src/client.rs:343-364`. `check_password` does
  `self.password.take()` at line 345, then matches only
  `SignInError::PasswordRequired(token)` to put the token back. grammers
  returns **`SignInError::InvalidPassword(PasswordToken)`** for a wrong
  password (`grammers-client-0.10.0/src/client/auth.rs:459-462`); `PasswordRequired`
  is what `sign_in` returns to *ask* for one. So a wrong password falls to the
  catch-all `Err(e)` arm with the token already consumed, and every retry
  answers `"two-factor sign-in is not pending"` — the user has to restart the
  whole sign-in and request a fresh code.
  The docstring immediately above (`client.rs:341-342`) promises the opposite:
  "On a wrong password grammers hands the token back, so the user can try again
  rather than restarting the whole sign-in." grammers does; this does not.
  This is audit item 1's failure re-entering by the one door that fix did not
  cover — `actions::PENDING` correctly holds the `Session` across a mistyped
  *code*, and the token it is holding has already been destroyed.
  **Fix:** add `Err(SignInError::InvalidPassword(token))` to the arm that
  restores `self.password`.

- [ ] **7. The cancel token never reaches the download pool.**
  `crates/tgx-tg/src/download.rs` does not import `Cancel` at all. Audit item 3
  above records the fix as "checked in the read loop, in `sleep_in_slices` and
  in the download pool" — the first two hold, the third does not.
  During the media pass the only check is *between topic folders*
  (`engine.rs:662`); the live run's `foto video` was a single uninterruptible
  batch of 1,781 files. Worse, `fetch_with_retry` (`download.rs:192-197`) waits
  out a rate limit with `sleep_in_slices`, which is
  `sleep_in_slices_until(total, &Cancel::new())` (`engine.rs:1050`) — a signal
  nobody holds and nothing can ever set — and then does `attempt -= 1`, so a
  persistent `FLOOD_WAIT` loops **without bound and without an exit**. The
  comment on those very lines claims "wait it out in slices so a cancel is not
  swallowed". Every other wait site threads the real token.
  **Fix:** pass `&Cancel` through `run_all` → `run_one` → `fetch_with_retry`;
  count the rate-limit attempts.

- [ ] **8. `save.bat save` runs two parity legs, gates on neither, and never
  runs the third.** `save.bat:494-506`. Each leg's failure is `if errorlevel 1
  echo [warn] …`; `SAVE_ERROR` is never set and `:runparity` ends `exit /b 0`.
  The commit and the push proceed, and the script exits zero. The media leg is
  not run on this path at all. Combined with finding 1 — CI cannot see the legs
  either — **no gate anywhere in the project stops a byte-exactness regression
  from being committed and pushed.** The `:parity` action (`save.bat:320-327`)
  gets this right; the pre-commit path is the weaker of the two, and it is the
  one that runs by default.

- [ ] **9. `Event::Failed` is a process-wide kill-switch, and an export can be
  started twice.** `crates/tgx-app/src/shell/mod.rs:526-532` clears `exporting`
  and `counting` for *any* worker's failure, because the event carries no run
  identity. `busy` (`mod.rs:810`) is `exporting || counting` and does **not**
  include an in-flight sign-in probe, and `start_sign_in` (`mod.rs:702`) spawns
  one unconditionally even when already signed in. So: press 01, press 03, let
  the probe time out — `exporting` goes false while the export worker is still
  running, `03 Start export` re-enables, and a second press calls
  `cancel.reset()` (clearing the running export's token), wipes the queue rows
  and starts a second concurrent export of the same chats into a second folder.
  **Fix:** tag `Failed` with the run it belongs to, or stop the sign-in and
  refresh paths from using that variant, and fold an in-flight probe into `busy`.

- [ ] **10. "Open output folder" silently does nothing.**
  `crates/tgx-app/src/actions.rs:653` launches
  `tgx_tg::config::system32("explorer.exe")`, i.e.
  `%SystemRoot%\System32\explorer.exe`. **That file does not exist** — verified
  on this machine; `explorer.exe` lives at `%SystemRoot%\explorer.exe`.
  `CreateProcess` fails and the result is dropped by `let _ =`. Both callers
  are dead: the `Open output folder` tool (`shell/mod.rs:864`) and clicking a
  finished queue row (`shell/run.rs:157`). `open_folder` creates the directory
  first, so the side effect happens and the visible effect does not.
  The Python original had it right (`app/ui/main_window.py:75-83`). The
  absolute-path rule from audit item 10 is still correct and must be kept —
  `system32()` remains right for `icacls.exe`, whose test is the only one that
  exists. This is that fix applied one directory too deep.

- [ ] **11. `icacls` is spawned with no timeout, on the first line of `main`.**
  `crates/tgx-tg/src/config.rs:336-342` calls `.output()`, which blocks until
  the child exits, forever. The Python bounds it — `timeout=15`
  (`app/config.py:70-75`). This is a **fourth** guard lost from the one function
  CLAUDE.md already records losing three.
  `ensure_data_dir` is reached from `logging::init`, which is the first
  statement of `main` in both binaries. If `icacls.exe` blocks — data folder on
  a disconnected share or a stalled removable volume, an AV filter holding the
  ACL write — `TelegramExporter.exe` never reaches `Application::new().run()`:
  no window, no `startup-error.log`, no `tgx.log`, and no stderr, because it is
  a GUI-subsystem binary. A process in Task Manager and nothing else.

- [ ] **12. A failed ACL lockdown can never reach the log, and is never
  retried.** Two defects that compound, both on the credential folder.
  * `config.rs:356`'s `log::warn!` is the only report to the log, and the
    lockdown runs exactly once per process (`config.rs:274-277`) — from inside
    `logging::init` (`logging.rs:118`), which is **before**
    `log::set_boxed_logger` at `logging.rs:158`. The facade still holds the
    no-op logger, so the warning is discarded. The module doc (`config.rs:8-9`)
    claims "the app says so in the log when that fails rather than leaving you
    to assume it worked". In every real run it does not.
  * `RESTRICTED.set(())` at `config.rs:276` runs unconditionally, so "tried
    once" and "succeeded once" are the same state. Python sets its flag only in
    the success branch (`app/config.py:81-86`) and retries until it works, which
    is what `main_window.py:208`'s "attempted lazily" depends on. A transient
    failure therefore leaves the **session key — a bearer credential** — at
    default permissions for the life of the process, and `actions.rs:87`'s
    deliberate re-call is a no-op.
  The GUI does surface `lockdown_error()` (`actions.rs:96`); the CLI never
  calls it, so `tgx login` stores the credential in a folder it knows may be
  world-readable and says nothing — on the surface most likely to be run from a
  stick or a shared machine.

- [ ] **13. The media leg tolerates *any* six wrong filenames, not *these*
  six.** `crates/tgx-parity/src/media_leg.rs:56-65` computes
  `expected_ceiling = total.saturating_sub(6)` and passes if `exact >=
  expected_ceiling`. The six known misses are one custom-emoji bug's cascade,
  not six independent tolerances, so the allowance is spent on whatever fails
  first. Any new naming regression touching six or fewer files — the four WebM
  video stickers, a collision-suffix bug at the end of a folder, an
  extension-sanitisation change — passes while printing "at the known ceiling".
  The number is absolute rather than proportional, so a ten-file corpus passes
  at 4/10. Audit item 8 made this leg assert; it asserts the wrong thing.
  **Fix:** pin the exception *set* — the six reference paths — and require the
  mismatch set to be a subset of it.

- [ ] **14. Two of the three leg tests pass on an empty topic list.**
  `crates/tgx-parity/tests/corpus.rs:59` and `:67`. `html_leg::run(&[])` and
  `media_leg::run(&[])` both return `Ok(0)`; the media leg goes further and
  reports "at the known ceiling" because `0 >= 0`. Only the json test carries
  `assert!(!topics.is_empty())` (`corpus.rs:54`). `topic_folders`
  (`lib.rs:32-41`) returns `Ok(())` both when it gives up at depth 3 and when it
  hits a directory carrying its own `result.json`, so an empty vector is
  reachable from a corpus laid out one level deeper. Same shape as the `let _ =`
  bug audit item 8 fixed, one level up.

---

## Medium

- [ ] **15. A failed download leaves its rendered preview dangling, and
  `missing_media.txt` does not name it.** `crates/tgx-tg/src/download.rs:159-162`
  pushes only `job.dest` into `missing`; `thumb_dest` and `preview_dest` are
  fetched inside the success branch and are simply skipped when the primary
  file fails. Verified on the live export: **21 `<img src>` targets in the HTML
  point at files that do not exist**, and the two `missing_media.txt` files
  name the 21 *photos* rather than the 21 `_thumb.jpg` previews. The module
  docstring's "a dangling reference is worse than a stated gap" is broken by
  its own error path.

- [ ] **16. The wire leg renders a key the reference does not have as the word
  `"downloaded"`, and has no mirror for `absent`.**
  `crates/tgx-parity/src/wire_leg.rs:333-350`. `skip_reason` returns `None` for
  three different states — a real path, a non-string, and *the key being
  absent* — and the example line prints the other side as
  `y.unwrap_or("downloaded")`. That is how the live run reported
  `509 file: ours "(File exceeds maximum size…)", reference "downloaded"` for a
  message whose reference JSON is a plain YouTube link with **no media keys at
  all**. `brief()` (`:422`) already knows how to render this as `"absent"`.
  The structural half matters more: `absent` (`:379`) iterates the reference's
  keys only. There is **no tally for a field we write and the reference does
  not** — which is the class message 509 belongs to, and which is exactly as
  invisible to replay as `absent` was. It surfaced here by accident, for three
  media keys, wearing a false label.

- [ ] **17. `sticker_emoji: ""` is written where Desktop writes no key.**
  `crates/tgx-tg/src/plan.rs:420` inserts whenever `facts.sticker_emoji` is
  `Some`, and a sticker with an empty `alt` gives `Some("")`. 11 messages in the
  live export; **zero** in the reference.
  **Fix:** `if let Some(e) = … if !e.is_empty()`.

- [ ] **18. 94 `forwarded_from` fields still resolve to the empty string, and
  the reason the last fix gave for stopping there is not true.**
  `crates/tgx-tg/src/engine.rs:756-764` says only the sender and the chat are
  reachable "— which is enough: the names that were missing belonged to people
  who had posted". The live export disproves it: 94 messages across 13 distinct
  people, all forwards, all with a correct `forwarded_from_id`, all named
  correctly by Desktop. A forward's origin is by definition someone who did
  *not* post here. `convert.rs:253-274` already prefers `fwd.from_name` when the
  lookup is empty; these are the case where Telegram sends neither.
  The peers *are* in the response — grammers keeps the whole set on
  `Message.peers` and the field is `pub(crate)`
  (`grammers-client-0.10.0/src/message/message.rs:40`), so the reachability
  claim is accurate and the conclusion drawn from it is not.
  **Fix:** collect unresolved `forwarded_from_id`s and resolve them in one
  batch — `Client::resolve_peer` (`client/chats.rs:621`) exists — or accept the
  gap and correct the comment.

- [ ] **19. `ChatInfo::access_hash` is written and never read.**
  `crates/tgx-tg/src/client.rs:407`, populated at `dialogs.rs:56/84/103`. Every
  other mention in the workspace is `access_hash: 0` in a fixture. Audit item 12
  records carrying the real hash as part of its fix — but nothing constructs a
  `PeerRef` from it, and `peer_refs_for` still pages the entire dialog list. The
  O(chats × dialogs) → O(dialogs) win came from batching the sweep; the field
  bought nothing. This is the same dead-`pub`-field class as `thumb_dest` in
  item 2, which no lint catches for the same reason.

- [ ] **20. `enrich`'s table promises six enrichments and the module implements
  one — and two of the missing five would move the export *away* from Desktop.**
  `crates/tgx-tg/src/enrich.rs:8-15` tabulates full reaction lists, poll
  refresh, chat info, participants, invites and scheduled messages, with
  measured hit rates. Only `fetch_participants` has a caller;
  `reactions_are_truncated` and `poll_needs_refresh` are called **only by their
  own tests**, and `chat_metadata`, `invite_links` and `scheduled_messages` are
  settings that nothing reads.
  Measured before recommending anything, because the obvious fix is wrong:

  | | Desktop | Python | ours |
  |---|---|---|---|
  | max named reactors on any reaction | **3** | **11** | 3 |
  | the two `min` polls (#728, #7100) | `total>0`, all answers `0` | identical | identical |

  Desktop never names more than three reactors — 95 entries across 77 messages
  have `count > len(recent)` and it leaves them sampled — and Desktop does *not*
  refresh a `min` poll, writing `total_voters: 8` above two zeroed answers. The
  Python original fetches full reactor lists (hence 11) and does **not**
  successfully refresh polls either; its own comment says that feature "silently
  never worked".
  So wiring `full_reactions` restores *Python* parity and steps away from
  Desktop's; wiring `refresh_polls` steps away from both, on 2 of 7 polls, and
  `poll` is in the wire leg's must-match set. Both may still be wanted — richer
  data is a legitimate goal — but each is a deliberate deviation of the same
  kind as `link_previews`, and the docs and the leg should say so rather than
  presenting them as bug fixes.

- [ ] **21. The CLI's second Ctrl-C guarantees the zero-byte `result.json` the
  design exists to prevent.** `crates/tgx-tg/src/bin/tgx.rs:282-284` calls
  `std::process::exit(130)`, which runs no destructors: every live `Output` is
  abandoned with its `BufWriter` unflushed, and per `output.rs:9-12` that means
  **zero bytes, not a truncated file**. The comment at `tgx.rs:267-270` promises
  the opposite for the Ctrl-C path, and nothing on screen distinguishes the two
  presses — the first says "closing the export so nothing is left empty", the
  second silently destroys it. Someone who presses twice because the first
  seemed slow loses the run.

- [ ] **22. An I/O error mid-message skips `close_all`, so the index the last
  audit added is not written.** `crates/tgx-tg/src/engine.rs:560`'s
  `sink.output.add(&payload)?` is a third error return, and `close_all`'s
  docstring (`engine.rs:880`) claims "every path that can end an export comes
  through here, including the two error returns above". `Drop` keeps the JSON
  valid, so this is not the zero-byte failure — but `close_all` is also what
  prunes empty topic folders, collects `degraded`, emits `Progress::Topic` and
  writes `export_results.html` (`engine.rs:936`). A disk filling up therefore
  produces topic pages whose back-link points at a file that was never written:
  precisely the regression `write_index` was added for.

- [ ] **23. A stale failure is appended to the next run's summary.**
  `crates/tgx-app/src/shell/mod.rs:530` sets `failure`, and only
  `Event::Finished` (`:520`) consumes it — but `sign_in`, `refresh_chats` and
  `count_chats` all emit `Failed` and none emits `Finished`, and `start_export`
  does not clear it. Open the app with the network down, then export three
  chats successfully, and the status bar reads `Exported 3 of 3 chats:
  connection timed out`. The field's own doc says "held only until the run ends".

- [ ] **24. Two of the wire leg's must-match fields move for reasons that are
  not defects.** `crates/tgx-parity/src/wire_leg.rs:45,47,59` require `from`,
  `actor` and `forwarded_from` — resolved display names, not wire data. Someone
  renaming their profile between two exports turns **every message they ever
  sent** into a mismatch; the live run shows this already (`1662/1664/1669
  from: ours "Tamara Blokade", reference "Tam Fmk 📸"`). Separately, `text` is
  required while `edited` is allowed to drift (`:66`) — but an edit is *why*
  `text` changes, so the two allowances contradict each other, and the test that
  guards them only checks the sets are disjoint.
  This matters because the report prints five examples per bucket: a rename
  storm can push a genuine `text` or `media_type` failure off the page. Keep the
  fields, but bucket a `from` disagreement by `from_id` so one rename reads as
  one rename, and exempt `text` for an id whose `edited` differs.

- [ ] **25. `Settings::save` is not atomic.** `config.rs:234-238` truncates then
  writes. Interrupted, it leaves a partial `settings.json`, which
  `load_from_str` rejects at `config.rs:200` and replaces with defaults — the
  `api_hash` gone with no message. Write to a sibling `.tmp` and rename.

- [ ] **26. The lockdown does not cover the files already in the folder.**
  `config.rs:336-340` has no `/t`, so it rewrites the directory's DACL and not
  its children's. `(OI)(CI)` fixes files created *after* this point; a `session`
  that already carries an inherited `Users:(R)` ACE keeps it, and on Windows a
  full-path open is checked against the file's own DACL. Reachable whenever the
  data folder predates a successful lockdown — a `dist\` copied to another
  machine, restored from backup, or unzipped. Present in the Python original
  too, so not a port regression, but it is the one thing the function exists to
  do. `/t /c` closes it.

- [ ] **27. `grammers-*` is pinned by caret while `gpui` is pinned exactly.**
  `crates/tgx-tg/Cargo.toml:14-16`. The asymmetry runs backwards to
  consequence: a `gpui` break is a compile error, loud and immediate, while a
  `grammers-tl-types` bump inside 0.10.x can re-shape a TL field, compile
  cleanly — the converter matches only the variants it knows — and quietly
  change what lands in `result.json`. That is the class CLAUDE.md says no test
  here can catch. `chrono` (`date`/`date_unixtime`, both must-match) and `sha2`
  (the corpus manifest hash) are in the same position. CI has no `--locked`.

- [ ] **28. `tgx-app` depends on `grammers-client` directly, and the test that
  exists to catch that does not look.** `crates/tgx-app/Cargo.toml:18` against
  `CLAUDE.md`'s "`tgx-app` … depends only on tgx-ui + tgx-tg".
  `crates/tgx-parity/tests/layering.rs` enforces two of the three documented
  rules and claims to enforce the set. Either add the third assertion or correct
  the document — at present they disagree and nothing notices.

- [ ] **29. A failed chat's reason is unattributed.**
  `crates/tgx-app/src/shell/mod.rs:493` has `chat_id` in hand and
  `queue.title_of` beside it, but journals the bare message. Every other
  transcript line is `"{title}: …"`. It bites hardest on the message audit item
  11 exists to deliver — `actions.rs:366` sends "rate limited while listing
  topics — retry this chat later" with no title, while the sibling branch four
  lines below does prefix it. The queue's STATUS column cannot serve as the
  fallback: it is 68 px and truncates to about one word. Three rate-limited
  forums in a twenty-chat queue leave the user unable to tell which three to
  re-run.

- [ ] **30. Two processes share one `tgx.log` with no coordination.**
  `logging.rs:117-129`. Running the `tgx` CLI — which CLAUDE.md documents as the
  way to exercise the wire — while the window is open renames the window's live
  log onto `tgx.prev.log`, destroying the genuinely previous run, creates a
  fresh `tgx.log`, and then both processes write at independent offsets into it.
  Also `logging.rs:115`'s "calling this twice is a no-op" is wrong twice over:
  the rotation and truncation happen *before* `set_boxed_logger`, and the
  refusal is returned as `Err`, not swallowed.

---

## Low / documentation

- [ ] **A negative number silently means "unlimited".** `config.rs:178-190`:
  `size_limit_mb: -5` yields `None` — download everything — and `member_limit:
  -1` disables the roster cap. The saturating-multiply guard beside it was
  written because untrusted input must not invert a limit; a negative inverts it
  just as well, in the other direction.
- [ ] **`Output::add` can desynchronise its own separator.** `output.rs:78-96`
  increments `count` last, so an error from the HTML writer after the JSON block
  was written leaves `count` stale and the next `add` omits the `",\n"`. Not
  reachable today — the only caller propagates with `?` — but it is a latent
  trap in the struct whose documented job is that the file is always valid.
- [ ] **`icacls`'s diagnostic is read from stderr only** (`config.rs:346`).
  `icacls` writes most failure text to **stdout**, so the user's security
  warning degrades to `icacls exited exit code: 1` with no indication whether it
  was FAT32, a bad grantee or a permissions problem. The Python read
  `stderr or stdout` (`app/config.py:77`).
- [ ] **`tgx export ""` exports an arbitrary chat.** `bin/tgx.rs:197-205`:
  `contains("")` is true for the first element of the list. Reject an empty
  title before searching.
- [ ] **A malformed `TG_API_ID` is silently ignored.** `bin/tgx.rs:40`'s
  `unwrap_or(settings.api_id)` means `TG_API_ID=1234x` falls back to whatever is
  on disk while the user is told they have no credentials.
- [ ] **`Bridge::new`'s `expect` is diagnosed as a GPU fault.**
  `shell/mod.rs:254` is the crate's only non-test `expect`, and it runs inside
  the window-content closure — before `WINDOW_OPENED` is set — so a
  thread-spawn failure is reported as "this build needs a GPU with working
  DirectX drivers". Item 15's misdiagnosis class, by another route.
- [ ] **`sort_mode` has no fallback though `theme` was given one**
  (`config.rs:218-220`) for exactly the same reason.
- [ ] **CI's clippy line omits `-- -D warnings`** (`ci.yml:39`) and relies on
  `RUSTFLAGS` instead; the gate works today but is one plausible edit from
  silently becoming a report.
- [ ] **`save.bat` cannot run non-interactively** — `pause` at `save.bat:512`
  sits on the single exit path, contradicting the header's "can be chained", and
  the menu spins forever on EOF.
- [ ] **`--force-with-lease` without `--force-if-includes`** (`save.bat:191`,
  `:402`): a `git fetch` between integration and push satisfies the lease and
  permits the clobber it exists to prevent.
- [ ] **`settings_are_wired`'s "is it read?" check is a bare substring match**
  on `.{field}`, so a future short field name (`.id`, `.size`, `.text`) is
  satisfied by any unrelated struct's field access. Fails safe in the other
  direction, which is the right way round.
- [ ] **`html_leg` lifts seven values out of Desktop's own HTML, and documents
  five** (`html_leg.rs:10-14` vs `:408-413`). Everything lifted is by
  construction not under test; the table should show the true size of that
  surface.
- [ ] **The CLI's 2FA password is never zeroised** (`bin/tgx.rs:116-130`) and is
  copied again by `trim().to_string()`, after the function goes to real trouble
  to keep it out of scrollback.
- [ ] **`Output::add` deep-clones every payload to drop one key**
  (`output.rs:81-85`) — 6,643 full-map clones per chat for a filter that could
  be applied during serialisation.
- [ ] **`error.rs:98`'s `unwrap_or(0)`** yields `Transient(Duration::ZERO)`, so
  a caller "waits" zero seconds and retries at once. Nothing spins today because
  every caller has its own bound; a one-second floor would make that
  unconditional.
- [ ] **`restrict_to_current_user` is a no-op on non-Windows, non-Unix targets**
  and never sets `lockdown_error`, so the GUI affirmatively claims a protection
  that was never attempted (`config.rs:371-385`).
- [ ] **`app_dir()` falls back to `"."`** when `current_exe()` fails
  (`config.rs:28-31`), putting the session key in whatever the working directory
  happens to be.
- [ ] **A signed-in user can spawn unbounded sign-in probes.**
  `shell/mod.rs:702` spawns `actions::sign_in` unconditionally and only opens
  the dialog when signed out, so the guard the comment at `:813` describes
  ("disabled while the dialog is up") never engages.
- [ ] **`shell/signin.rs:300,325` swallow a settings save failure** with
  `let _ =`, where every other write path journals a warning. A read-only disk
  means the credentials work this session and are gone next launch.

---

## The pure layers

`tgx-format`, `tgx-html` and `tgx-media` came out of this pass **with no
critical and no high findings**, which is what the oracle covering them is
supposed to buy. The escaping in particular held up: `javascript:` in every
casing, `data:`, `vbscript:`, `blob:`, `file:`, tab/LF/CR/NUL inside and before
a scheme, `j a v a s c r i p t:`, NBSP separators, `//host`, `\\host`, `/\host`,
`\/host`, a leading bidi mark, and the encoded colons `javascript&#58;` and
`javascript%3a` were all tried against `safe_href` and all refused. Item 13's
fix holds. No unescaped value reaches markup anywhere in the crate, and there
is no `unwrap`/`expect`/slicing outside tests bar the documented-infallible
ones.

The one substantial finding is numbered with the rest; the remainder are small.

- [ ] **31. Every blur preview is written to disk and none is ever shown.**
  `crates/tgx-html/src/writer.rs:468` gates the inline stripped-thumbnail image
  on `pv.get("stripped") == Some(true)`, and **nothing in the workspace writes a
  `stripped` key**. The only producer of `_p.preview` is `convert::preview_of`
  (`convert.rs:459`), which emits `{src, width, height}` and returns `None` the
  moment `file`/`photo` is a placeholder (`convert.rs:462,474,478`) — which is
  exactly the case a stripped thumbnail exists for.
  Measured on the live export:

  | topic | `stripped_thumbnail` in JSON | `<img src="thumbnails/…">` |
  |---|---|---|
  | ćaskanje | 32 | 0 |
  | foto video | 1,195 | 0 |
  | editorijal | 136 | 0 |
  | bitno pročitaj | 1 | 0 |
  | **total** | **1,364** | **0** |

  All 1,364 JPEGs are on disk. `plan.rs:458` reserves the name, `stripped::expand`
  builds a real JPEG, the pool writes it, `result.json` points at it, and the
  page renders a grey `media_file` row instead. For a file too large to
  download this is the only image that will ever exist.
  This is the same reader-with-no-producer shape as `thumb_dest` in item 2 and
  `_p` in item 9, and `convert.rs:343-344` already lists "the
  `stripped_thumbnail` files we do write on disk referenced by nothing" among
  the symptoms `presentation()` repairs. It does not repair this one.
  No leg can see it: the html leg replays Desktop's `result.json`, which has no
  `stripped_thumbnail` key at all.
  **Fix:** one arm in `preview_of` — when the media is a placeholder and
  `stripped_thumbnail` is present, emit `{src: that, stripped: true, …}`.

- [ ] **32. `WIDTH_OFFSET` and `HEIGHT_OFFSET` name the wrong JPEG fields.**
  `crates/tgx-media/src/jpeg_header.rs:53-56` and its doc at `:10`. Walking the
  committed 623-byte header: SOF0 begins at `0x9e`, so byte **164 is the height
  low byte** and **166 is the width low byte** — the constants are swapped.
  `expand` (`stripped.rs:33-34`) is still byte-correct because it writes
  `stripped[1]→164, stripped[2]→166` exactly as tdesktop does; only the names
  are wrong. But the mislabelling has already produced a second bug:
  `stripped::dimensions` (`stripped.rs:42`) returns `(stripped[1], stripped[2])`
  documented as "width and height", which is `(height, width)`, and its test
  enshrines the swap. Latent only because `dimensions` has no callers — the
  first `<img>` sized from it gets a transposed box on every non-square
  thumbnail.

- [ ] **33. Eleven `pub` items are reachable only from their own tests**, the
  class that hid item 2 and item 31: `stripped::dimensions`,
  `order::ordered_index`, `PeerKey::raw_id`, `Tree::at`, `Tree::indent`,
  `Tree::raw_tag`, `writer::css_preview_size`, `writer::preview_box_for`,
  `HtmlWriter::total` (written, never read), `NameBook::is_claimed`,
  `Presentation::is_empty`. Two knock-ons: `ordered_index` is the **only** user
  of `indexmap` in `tgx-format` and is redundant by construction, since
  `serde_json` is built with `preserve_order` and `Map` already *is* an
  `IndexMap`; and `Tree::raw_tag`'s doc says "used for the doctype" when the
  doctype goes through `Tree::text` in both callers — and must, or a blank line
  appears before `<html>`.

Smaller, all verified:

- [ ] **`sanitize_extension` can reintroduce the trailing space
  `sanitize_filename` exists to strip** (`names.rs:82-90` vs `:239-247`): a
  document named `"invoice "` yields `files/invoice.invoice `, Windows drops the
  trailing space on create, and `result.json` points at a name that is not on
  disk — the dangling reference the truncation-trim was written to prevent.
- [ ] **`fit_box` can overflow `i64`** (`preview.rs:26,30`): `520 * width` with
  `width` taken straight off the wire panics in debug and wraps in release.
  `preview_size` clamps the low end only.
- [ ] **`desktop_reaction_indent` over-indents any key that sorts after
  `reactions`** (`json.rs:43-65`). `order.rs:189` puts *unranked* keys after it,
  so a message carrying `reactions` plus a key nobody has classified yet gets
  that key shifted one space. The doc's "reactions is always the last key" is
  true of ranked keys only.
- [ ] **`<img src>` gets attribute escaping but no scheme vetting**
  (`writer.rs:473,500,523,543,568`), the one attacker-shaped slot that skips
  `safe_href` — and worse than an href, since an `<img>` loads without a click,
  so `//host` on a `file://` page is an unprompted UNC fetch. Not reachable
  today: `src` only ever comes from a planner-claimed path, which cannot begin
  `//`. One `.and_then(safe_href)` closes it permanently. `PageChrome.back_href`
  (`page.rs:92`) skips it too, and is a constant in practice.
- [ ] **`SAFE_SCHEMES` is duplicated** in `escape.rs:58` and `inline.rs:12`;
  adding a scheme to one silently diverges the other.
- [ ] **`userpic_class` silently swallows an over-long id and a sign**
  (`userpic.rs:68`): a 20-digit id fails to parse and falls back to palette
  entry 1, and the digit filter drops a leading `-`. Its "gigantic id" test
  asserts only that the result is in `1..=8`, so it passes on the fallback.
- [ ] **`escape.rs:96-98` is dead** — subsumed by the `matches!` on the next
  line. **`index.rs:92` links to `HtmlWriter::finish`**, which is called `close`.
  **`jpeg_header.rs` was hand-edited after generation** (16 bytes per row where
  `tools/gen_jpeg_header.py` writes 12), so it no longer round-trips through its
  generator, against CLAUDE.md's "regenerate rather than hand-edit".
- **Coverage gap worth knowing:** the corpus contains **zero**
  `hashtag`/`cashtag`/`bot_command` entities, so the three JS-injection paths in
  `inline.rs:95-106` are covered by unit tests only and never by the oracle.

---

## Suggested order

1. **2** and **1** first, and in that order — until `release` builds and the
   corpus legs are visible, every other fix is being verified against a binary
   and a CI run that may not reflect it. Both are one-line changes.
2. **3**, then **12** — the two that can destroy or expose the credential.
3. **4**, **5**, **17**, **31** — the output defects, each small and each
   pinned by a number from the reference or from the live run. **6** belongs
   here too: it is three lines and it is the difference between 2FA users being
   able to sign in or not.
4. **7**, **9**, **21**, **22** — the cancel and lifecycle holes.
5. **8**, **13**, **14**, **16** — the oracle's own calibration. Worth doing
   before the next live run, so that run's report can be trusted.
6. **20** is a decision rather than a fix: choose whether this tool tracks
   Desktop or supersedes it, then make the docs and the leg agree.

---

# What the 2026-08-27 remediation actually landed

`PLAN.md` scheduled the section above into five phases. This records what was
done, what it cost, and — more usefully — the four places where **the plan
turned out to be wrong when the code was opened**, since those are the ones a
future reader will otherwise repeat.

## Where the plan was wrong

- **`icacls /c` must not be added.** 1.4 said to pass `/t /c` so the ACL reaches
  files that already exist. `/t` is right; `/c` is not. Measured on this machine:
  `icacls <missing path> /t /c` exits **0** while printing "Failed processing 1
  files", and the same command without `/c` exits **3**. `/c` would have turned
  every partial failure into a reported success — precisely the state
  `lockdown_error` exists to prevent — and parsing "Failed processing" out of the
  text is not an option, because icacls is localised.
- **Desktop's action names are not the snake-cased constructor.** 2.3 said the
  explicit table should own the actions that carry a payload and a snake_case
  fallback could supply the rest of the names. It cannot: Desktop writes
  `create_group` for `ChatCreate`, `clear_history` for `HistoryClear`,
  `joined_telegram` for `ContactSignUp`, `delete_group_photo` for
  `ChatDeletePhoto`. Thirteen names were ported from the Python original, whose
  table *is* a measurement against Desktop. The fallback covers the rest.
- **A cancelled media batch needed a second change nobody scheduled.** Recording
  every un-run job as a stated gap is right, but the transcript is a 2,000-line
  ring whose whole purpose is that the INCOMPLETE warning can still be scrolled
  to — and a Stop during the 1,781-file folder would have flushed it with "not
  saved" lines. The listing is capped at twenty with a pointer to the file.
- **Two `--force-with-lease` sites, not three**, and the third hit was prose.
  Separately, CI's missing `-- -D warnings` was **already covered** by the
  `RUSTFLAGS: -D warnings` at the top of the workflow — verified with a scratch
  crate rather than reasoned about. It is spelled out anyway, because that chain
  is three deniable steps long.

## What each phase closed

| phase | items | landed in |
|---|---|---|
| 0 — restore the ability to verify | 1, 2, 8, 13, 14 | `44738ed`, `eacef6f`, `ecf48bd`, `196f66c` |
| 1 — the credential and startup path | 3, 11, 12, 25, 26, CLI | `023cb53`, `750a390` |
| 2 — what the export writes | 4, 5, 17, 18, 31 (15 with 3.1) | `fba97f5`, `ccf4811`, `4778446`, `048fc73` |
| 3 — lifecycle and cancellation | 6, 7, 9, 10, 21, 22, 23, 29, `Bridge::new` | `4778446`, `750a390`, `82b6c55` |
| 4 — the oracle's own calibration | 16, 24, and the leg's own tests | `5b98057` |
| 5 — hygiene | 19, 32, 33, the Low list | `63c08a8`, `582ec61`, and the crate sweeps |

## The three numbers to check on the next live run

`PLAN.md`'s prediction table stands. The three that matter most, because each
was measured from the last run rather than reasoned about:

| | before | predicted after |
|---|---|---|
| `absent` fields | 1,290 | **0** — 1,287 `thumbnail_file_size`, 3 `action` |
| blur previews displayed | 0 of 1,364 | **1,364** |
| empty `forwarded_from` | 94 across 13 people | **0**, at a cost of 13 requests |

Write the observed numbers in beside them. Where reality disagrees, the
prediction was wrong and that is the finding.

## Still open, and deliberately

- **D1–D4 in `PLAN.md` are decisions, not defects**, and none is made here.
- **A sender's name is stamped as it stands now, not as it stood per message.**
  Desktop records the name a person had *at each message*; we hold one name per
  peer. Two users in the reference carry two names each in Desktop's export and
  one in ours (`user6375985771`, `user7884540830`). This is a design change —
  one name per peer *per message* — and it interacts with the wire leg's new
  rename bucketing, which would otherwise file the difference as a rename.
- **The Python's ~20 service-action payloads are not ported.** Only the names
  are. `custom_action`'s message, `phone_call`'s duration, `gift_code`'s slug
  and the rest would have to be reproduced against grammers' TL shapes with no
  oracle, and the wire leg's new `extra` tally would score any wrong key. The
  Python's `_action_fields` is the specification when someone takes it on.
- **Two of the three peer shapes have never been run.** `enrich::fetch_chat_info`
  and the roster branch on `InputPeer::Chat` (a basic group) and
  `InputPeer::User` (a private chat) are argued from `api.tl` and have never
  been exercised against Telegram: every live run so far has been the same
  forum supergroup, which takes the `InputPeer::Channel` arm. So `chat_metadata`
  and `member_roster` are proven on one peer type in three. This is the same
  shape as the bug the code's own comment records — "the switch was on, the
  request was never made, and nothing said so" — one layer further down, because
  *wired* and *wired for every peer* are not the same claim. Closing it needs a
  live export of a basic group and of a one-to-one chat, not a test.
- **The `_p` blind spot in the html leg** is unchanged: it still lifts the
  presentation map out of Desktop's own pages, so it proves the writer and not
  the pipeline. `crates/tgx-tg/tests/wire.rs` is the compensating control and
  gained the stripped-preview case.
