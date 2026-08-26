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

- [ ] **1. Two-factor sign-in through the GUI can never complete.**
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

- [ ] **1b. The code path re-requests a login code before submitting one.**
  `actions.rs:91-96` calls `request_code` again, then `sign_in(&secret)` — the
  code the user typed is submitted against a possibly-new `phone_code_hash`.
  Falls out of the same fix.

- [ ] **2. Telegram's own thumbnails are written into `result.json` but never
  downloaded.** `crates/tgx-tg/src/plan.rs:370` inserts
  `"thumbnail": "<path>"`; line 405 populates `DownloadJob.thumb_dest` — and
  **nothing ever reads it** (`grep thumb_dest` returns writes only). Every
  export therefore carries dangling `thumbnail` references, and because they
  never reach the pool they are not recorded in `missing_media.txt` either —
  exactly the "a dangling reference is worse than a stated gap" failure
  `download.rs` argues against. The wire leg counts **1,287** `thumbnail`
  decisions, so this is not a rare path.
  **Fix:** fetch the thumb in `download::run_one`, or stop writing the key.

- [ ] **3. `Stop` does not stop, and re-enables `Export`.**
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

- [ ] **4. `FLOOD_PREMIUM_WAIT` is classified as a permanent refusal.**
  `crates/tgx-tg/src/error.rs:87` tests `rpc.name.contains("FLOOD_WAIT")`.
  `"FLOOD_PREMIUM_WAIT_60"` does **not** contain that substring. The comment
  directly above (line 85) claims it does. This is precisely the
  rate-limit-mistaken-for-a-refusal class the module exists to make
  unrepresentable, and no test exercises `classify` against a real RPC name.
  **Fix:** match `name.contains("FLOOD")` *and* `name.contains("WAIT")`, or list
  the names explicitly — plus a table test over the real error strings.

- [ ] **5. Every channel and supergroup is exported as `public_*`.**
  `crates/tgx-tg/src/engine.rs:191` hardcodes `chat.kind.export_type(true)`.
  `ChatInfo` carries no `public`/`username` field, so the `false` branch is
  unreachable in production — a private supergroup writes
  `"type": "public_supergroup"`. The unit test at `client.rs:264` covers the
  function, not the call.
  **Fix:** carry `username.is_some()` on `ChatInfo` from `dialogs::chat_info`.

- [ ] **6. An empty non-forum chat deletes its own export folder — including
  `participants.json`.** `crates/tgx-tg/src/engine.rs:474` does
  `remove_dir_all(&sink.output.root)` when a sink wrote zero messages. In the
  split-topics case that is a topic subfolder (intended). In the non-split case
  `Output::new(root, …)` at line 220 means `sink.output.root == root`, so a chat
  with no messages wipes the whole chat directory, discarding the roster written
  at line 265 and releasing the `unique_dir` reservation.
  **Fix:** only prune when the sink's root is a topic subfolder of `root`.

- [ ] **7. Stripped thumbnails consume the `file_N` counter.**
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

- [ ] **8. The media parity leg asserts nothing.**
  `crates/tgx-parity/tests/corpus.rs:75` is
  `let _ = media_leg::run(&topics).expect(...)`. 830/836 could drop to 0/836 and
  `cargo test` stays green; the number lives in stdout, which cargo captures.
  The module docstring twelve lines above warns about exactly this ("the classic
  way a suite goes quietly green").
  **Fix:** assert a floor — `assert!(r.exact >= 830)`.

---

## Medium

- [ ] **9. The live HTML is missing the entire presentation layer.**
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

- [ ] **10. `icacls` and `explorer` are launched by bare name.**
  `crates/tgx-tg/src/config.rs:262` and `crates/tgx-app/src/actions.rs:262`.
  Windows `CreateProcess` search order includes the application directory and
  the current directory before `PATH`, and this app is explicitly designed to be
  copied onto USB sticks and run from arbitrary folders — a planted
  `icacls.exe` runs with the user's rights at exactly the moment the app is
  securing its credential store.
  **Fix:** `%SystemRoot%\System32\icacls.exe`, and an absolute `explorer.exe`.

- [ ] **11. A rate limit during topic discovery silently collapses a forum into
  one folder.** `crates/tgx-app/src/actions.rs:172-177` catches *any* error from
  `list_topics` — `Transient` included — and falls back to
  `vec![Topic::general()]`. Splitting by topic is the app's entire reason to
  exist; a FloodWait should defer, not silently change the output shape.

- [ ] **12. `peer_ref_for` in the GUI swallows errors and hides rate limits.**
  `crates/tgx-app/src/actions.rs:254`:
  `while let Ok(Some(d)) = iter.next().await` treats any error as end-of-list,
  then reports `"{chat} is no longer in the dialog list"` — a confidently wrong
  message that skips the chat. The CLI version (`bin/tgx.rs:229`) propagates
  properly.
  Separately this walks every dialog once **per queued chat** — O(chats ×
  dialogs) requests — and `ChatInfo.access_hash` exists to avoid that but is
  hardcoded to `0` in all three branches of
  `crates/tgx-tg/src/dialogs.rs:43-76` and read nowhere.

- [ ] **13. Protocol-relative and UNC hrefs survive the allowlist.** Verified:
  `safe_href("//evil.example/x")` → `Some("//evil.example/x")` and
  `safe_href(r"\\evil.example\share")` → accepted. In an archive opened as
  `file://`, both resolve to `file://evil.example/…`, which on Windows is a UNC
  path — clicking one initiates an SMB connection to an attacker-chosen host
  (NTLM leak). Telegram `text_link` entities carry arbitrary URLs.
  **Fix:** reject a leading `//` or `\\` in the relative-URL branch of
  `crates/tgx-html/src/escape.rs:96`.

- [ ] **14. `list_topics` has no loop bound.**
  `crates/tgx-tg/src/dialogs.rs:144` breaks only when a page adds nothing. If
  offsets are not honoured the same page is pushed repeatedly, `out.len()` grows
  every iteration, and the loop never terminates. `fetch_participants` has a
  cap; this does not.

- [ ] **15. The panic hook blames the GPU for every panic.**
  `crates/tgx-app/src/main.rs:41` installs a global hook whose message is always
  *"This build needs a GPU with working DirectX drivers."* A panic anywhere —
  including inside an export — is reported as a driver problem and overwrites
  `startup-error.log`.
  **Fix:** scope it to startup, or make the text conditional.

---

## Low / documentation

- [ ] **`gpui-component` is not pinned.** `crates/tgx-app/Cargo.toml:22` is
  `"0.5.1"` (caret). The workspace `Cargo.toml` comment says *"Pinned exactly,
  not by caret… a routine `cargo update` can break the interface"*, and
  ROADMAP's retired-risk table says *"Both are in and both are pinned exactly."*
  Only `gpui = "=0.2.2"` actually is.
- [ ] **The "enforced by the build" dependency rules do not exist.** README
  claims `tgx-html` may not depend on `grammers-tl-types`, and the analyser may
  not depend on `tgx-tg`, are *"enforced by the build rather than by
  convention."* There is no `build.rs`, no `deny.toml`, no test and no CI step.
  Either add the check or reword the claim.
- [ ] **`topics::sanitize_component` truncates after trimming**
  (`crates/tgx-media/src/topics.rs:51`), so a 120+ character topic title can end
  on `.` or a space — Windows silently drops it, so the folder on disk stops
  matching the recorded name. `names::sanitize_filename` documents and fixes
  this exact bug; `topics.rs` does not.
- [ ] **Message-block joining measures a duration in local wall-clock.**
  `crates/tgx-html/src/join.rs:93` computes the 900 s / 3 s gap from the naive
  `date` field. `crates/tgx-format/src/lib.rs:31` states the rule: *"anything
  measuring a duration reads `date_unixtime`, which stays monotonic across a DST
  change where `date` does not."* At a fall-back the gap goes negative and the
  block splits. The corpus is a single December range, so it cannot catch this.
  Parity impact unknown — Desktop may well use wall-clock too; measure before
  changing.
- [ ] **`save.bat --force` force-pushes with no prompt** (line 34 → 155 → 186),
  uses `--force` rather than `--force-with-lease`, and is documented in neither
  the menu nor the README.
- [ ] **`peer.rs:105`'s doc contradicts its own test** — it says
  "Nađa Gavrilović arh blokade fotograf" renders as `Nf`; the test at line 270
  asserts `NG`.
- [ ] **The CLI echoes the 2FA password** — `crates/tgx-tg/src/bin/tgx.rs:64`
  uses a plain `read_line`, so the cloud password lands in terminal scrollback.
- [ ] **Binary-size figures disagree across docs**: README 17.8 MB, ROADMAP
  18.4 MB, `ci.yml` "~10 MB"; the Python baseline is 42 MB in `Cargo.toml` and
  46.4 MB in README / `save.bat`.
- [ ] **Minor overflow / truncation.** `Settings::size_limit_bytes` multiplies
  an untrusted `i64` by 1 MiB without saturation; `client.rs:70` casts
  `api_id as i32` silently; `json::header_prelude` byte-slices
  `head.len() - 2` and emits invalid JSON for an empty header map (unreachable
  today).

---

## Suggested order

1. **3** — Stop actually cancels. User-visible on day one, and the cancellation
   token it needs is a prerequisite for doing **11** properly.
2. **1** — GUI 2FA. Anyone with two-step verification cannot use the window.
3. **2** — thumbnails. Every export currently ships broken links.
4. **4**, **5**, **6**, **7** — small, mechanical, one test each.
5. **8** can jump the queue: until it asserts, the suite can go quietly green
   while the rest is in flight.
