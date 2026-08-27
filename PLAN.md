# Remediation plan — the 2026-08-27 second-pass audit

Numbers below are `AUDIT.md`'s. Everything in `# Code audit — 2026-08-27
(second pass)` is covered; nothing is dropped silently. Where an item is a
decision rather than a defect it is in **Decisions** at the end, not in a phase.

## How to work through this

The repo's existing rules, restated because this plan leans on them:

* **One concern per commit**, with the parity legs green either side.
* **Fix the code, not the reference.** If a diff fails, the reference is right.
* **Every fix lands with the test that would have caught it.** Several findings
  below exist *because* a test asserted the wrong half — 5 and 3 both have a
  passing test pinning the defect. Changing the assertion is part of the fix,
  not a separate tidy-up.
* A fix whose test cannot fail before the fix is not yet finished.

**Phase 0 is not optional and not reorderable.** Until it lands, every other
fix is verified against a build that may be stale (2) and a CI run that checks
none of the byte-exactness (1). Doing anything else first means doing it twice.

Sizes are relative: **S** ≈ a few lines, **M** ≈ a function and its tests,
**L** ≈ a design decision plus a live run to confirm.

---

## Phase 0 — restore the ability to verify anything

*Findings 1, 2, 8, 13, 14. All small; all in the tooling; none touch the export.*

### 0.1 — `save.bat` flag variables (finding 2) — **S**

`save.bat:37`, `:42`, `:77`.

```bat
if /i "%ACTION%"=="release" (set DO_BUILD=1 & goto save)   :: value is "1 "
```

Change all three to the quoted form, which cannot absorb the space:

```bat
if /i "%ACTION%"=="release" (set "DO_BUILD=1" & goto save)
```

Audit the rest of the file for the same shape while in there — the pattern is
`set NAME=value &`, and the fix is `set "NAME=value" &` everywhere.

**Pins it:** none available — batch has no test harness here. Verify by hand,
once: `save.bat release` must leave a `dist\TelegramExporter.exe` whose
timestamp is newer than the run. Record that check in the commit message; it is
the only evidence that will exist.

**Do this first.** Every later phase is verified by running a binary, and until
this lands the binary may not be the code.

### 0.2 — make the corpus skip impossible to miss (finding 1) — **M**

Three changes, because the honest position is different locally and on CI:

1. **Locally, a missing corpus becomes a hard failure when asked for.** Add a
   `TGX_REQUIRE_CORPUS` env var to `crates/tgx-parity/tests/corpus.rs`:
   `corpus_dir()` returning `None` while that variable is set is a `panic!`, not
   a `return`. Set it in `save.bat test` (the machine that is supposed to have a
   corpus is the machine that has one).
2. **On CI, say so where a human will see it.** `reference/` is gitignored and
   can never be present, so the legs genuinely cannot run there. Add
   `--nocapture` to the CI test step *and* a dedicated step that emits a
   GitHub annotation:
   `echo "::warning::the parity legs did not run — no corpus on CI"`.
   An annotation surfaces on the run summary; a captured `eprintln!` does not.
3. **Correct the claim.** `corpus.rs:7-12` and CLAUDE.md both state the skip is
   visible. Until (1) and (2) land they are false; after them, reword to say
   *where* it is visible, because the mechanism is no longer "the test prints".

**Pins it:** a test asserting `TGX_REQUIRE_CORPUS` panics when the directory is
absent. That test can fail before the fix.

### 0.3 — `save.bat save` must gate on the legs (finding 8) — **S**

`save.bat:494-506`. Each `if errorlevel 1 echo [warn] …` becomes

```bat
if errorlevel 1 (echo [warn] ... & set "SAVE_ERROR=1")
```

and `:runparity`'s hard-coded `exit /b 0` becomes `exit /b %SAVE_ERROR%`. Add
the media leg, which this path does not run at all. Keep a *missing* corpus
non-fatal here — a skip is legitimate on a machine without the reference; a
*failure* is not.

Note the contrast to preserve: `:parity` (`save.bat:320`) already does this
correctly. Make the commit path match it rather than inventing a third
behaviour.

### 0.4 — a leg that compared nothing must not pass (finding 14) — **S**

`crates/tgx-parity/tests/corpus.rs:59,67`. Hoist the json test's
`assert!(!topics.is_empty(), "the corpus holds no topics")` into a shared
helper and use it in all three. Separately, `media_leg::run` should return a
failure when `total == 0` rather than computing `0 >= 0` and printing "at the
known ceiling" — an empty run is not a passing run.

### 0.5 — pin the media ceiling to a set, not a count (finding 13) — **M**

`crates/tgx-parity/src/media_leg.rs:56-65`. Replace
`exact >= total.saturating_sub(6)` with an explicit exception set: record the
six reference paths (or their message ids) as a constant, and require that the
observed mismatch set is a **subset** of it. Anything else fails, whatever the
count.

This is what makes the leg able to see a *new* six-file regression, which is
the class it exists for. Expect to have to name the six first — run
`tgx-parity media reference` and read them off.

**Exit criteria for Phase 0:** `save.bat release` produces a fresh binary;
`save.bat test` fails on a machine with no corpus; `save.bat save` refuses to
commit when a leg is red; the media leg fails if a seventh file is wrong.

---

## Phase 1 — the credential and the startup path

*Findings 3, 11, 12, 25, 26, plus the CLI and non-Windows gaps. Highest
consequence per line in the whole plan: one of these can destroy stored
credentials and another can hang the app before it draws.*

### 1.1 — settings really fall back per field (finding 3) — **M**

`crates/tgx-tg/src/config.rs:217`. The merge is right; the single whole-struct
`from_value` at the end is what loses the file. Validate each key on its own:

```rust
for (key, value) in map {
    let Some(slot) = base.get(&key) else { continue };
    if !same_shape(slot, &value) { continue }
    let mut candidate = base.clone();
    candidate.insert(key.clone(), value);
    // The whole struct still has to deserialise with this field in it.
    if serde_json::from_value::<Self>(Value::Object(candidate.clone())).is_ok() {
        base = candidate;
    }
}
```

Roughly 25 deserialisations of a small struct per load — immaterial, and it
gives exactly the per-field semantics the doc already claims. Range clamps at
`config.rs:221-223` stay as a second line of defence.

**Pins it:** extend `one_bad_field_does_not_lose_the_others` with the three
cases the current version cannot reach — `"page_size": -1`,
`"size_limit_mb": 20.5`, and an `api_id` too large for `i64` — each asserting
that `api_hash` survives. All three fail before the fix.

While here, finding **"a negative number silently means unlimited"**: clamp
`size_limit_mb` and `member_limit` at zero, since a negative currently inverts
the limit in the opposite direction to the overflow the saturating multiply
already guards.

### 1.2 — `Settings::save` becomes atomic (finding 25) — **S**

`config.rs:234`. Write `settings.json.tmp` in the same directory, then
`std::fs::rename` over the target. Same-directory rename is atomic on NTFS.
Without it an interrupted write leaves a file that `load_from_str` rejects
wholesale — the same credential loss as 1.1, by a different route.

### 1.3 — the ACL lockdown must retry, and must reach the log (finding 12) — **M**

Two changes in different files:

* `config.rs:274-277` — set `RESTRICTED` **only on success**, mirroring the
  Python original (`app/config.py:81-86`). Today "tried once" and "succeeded
  once" are the same state, so a transient failure leaves the bearer credential
  at default permissions for the life of the process and `actions.rs:87`'s
  deliberate re-call does nothing.
* `logging.rs` — after `set_boxed_logger` succeeds, check
  `config::lockdown_error()` and `log::warn!` it. The existing warn at
  `config.rs:356` fires from inside `logging::init` *before* the logger is
  installed, so it goes to the no-op logger every time. Logging it after
  installation is what makes the module doc's claim true.

**Pins it:** a test that a failing lockdown leaves `lockdown_error()` `Some` and
does not set the once-flag, so a second `ensure_data_dir()` retries.

### 1.4 — bound `icacls` (finding 11) — **M**

`config.rs:336-342`'s `.output()` blocks forever, and it sits on the first line
of `main` in both binaries. Port the guard the original had (`timeout=15`):
`spawn()`, then poll `try_wait()` against a deadline, `kill()` on expiry, and
record the timeout through `set_lockdown_error` so it is visible rather than
silent.

Add `/t /c` to the invocation while there (finding 26) so the ACL reaches files
that already exist — a `dist\` folder copied from another machine carries
inherited ACEs on `session` that a directory-only change does not touch.

Also read stdout, not only stderr (`config.rs:346`): `icacls` writes most of
its failure text to stdout, so today the user's security warning degrades to
`icacls exited exit code: 1`. The original used `stderr or stdout`.

### 1.5 — the CLI checks what the GUI checks — **S**

`bin/tgx.rs` never calls `config::lockdown_error()`, so `tgx login` stores a
bearer credential in a folder it knows may be world-readable and says nothing —
on the surface most likely to be run from a stick. One check after
`logging::init()`, wording borrowed from `actions.rs:96`.

Same commit: `restrict_to_current_user` on a target that is neither Windows nor
Unix (`config.rs:371-385`) should set `lockdown_error` rather than leaving the
GUI to claim a protection nobody attempted.

---

## Phase 2 — what the export actually writes

*Findings 4, 5, 15, 17, 18, 31. These change bytes. None of them moves the three
replay legs — those feed a recorded Desktop `result.json` through our writers,
and every change here is on the converter/planner side of that boundary — so a
red leg after this phase means a mistake, not an expected shift.*

### 2.1 — `thumbnail_file_size` on the skipped branch (finding 4) — **S**

`crates/tgx-tg/src/plan.rs:392-409`. The `None` arm writes the `thumbnail`
placeholder and stops. Add the size:

```rust
None => {
    fields.insert("thumbnail".into(), json!(placeholder));
    if facts.thumb_size > 0 {
        fields.insert("thumbnail_file_size".into(), json!(facts.thumb_size));
    }
}
```

Desktop writes it in all 1,544 cases — 257 saved and 1,287 skipped. This single
key is 1,287 of the wire leg's 1,290 `absent`.

**Pins it:** a test asserting a size-skipped document with a thumbnail carries
both `thumbnail` and `thumbnail_file_size`. The existing
`a_skipped_document_records_its_thumbnail_as_skipped_too` asserts only half.

### 2.2 — `sticker_emoji` is not written empty (finding 17) — **S**

`plan.rs:420`: `if let Some(e) = &facts.sticker_emoji` → add `if !e.is_empty()`.
11 messages in the live run; zero in the reference.

### 2.3 — service actions (finding 5) — **M**

Three parts, one commit:

1. `convert.rs:741` — `A::TodoAppendTasks(_)` becomes
   `A::PollAppendAnswer(_) => no_payload("poll_append_answer")`. Verified
   against the reference: Desktop writes actor + action and **no** `answer`
   key, so `no_payload` is right.
2. Decide the two To-do actions separately. The Python names them
   `todo_append_tasks` and `todo_completions` (`serialize.py:731-738`); Desktop
   has no example in the corpus. Following the Python is the defensible default.
3. **Restore the generic fallback.** `_ => None` drops any of the other 57
   `messageAction*` constructors entirely. The Python's `snake_action()`
   (`serialize.py:817`) snake-cases the class name, which is exactly how
   Desktop names actions it predates. In Rust the variant name is reachable
   through `Debug`:

   ```rust
   // "TodoCompletions(MessageActionTodoCompletions { .. })" -> "todo_completions"
   fn snake_variant(action: &tl::enums::MessageAction) -> String { … }
   ```

   Take the identifier up to the first `(` or space, then snake-case it. Keep
   the explicit table for every action that carries a payload; the fallback is
   for the name only.

**Pins it:** the existing case at `convert.rs:1021` currently asserts the *bug*
— `TodoAppendTasks → "poll_append_answer"` — under a test named
`every_action_the_reference_holds_is_named_the_way_desktop_names_it`. Replace
it with `PollAppendAnswer`, and add a case for an unmapped action asserting it
degrades to its snake_case name rather than vanishing.

### 2.4 — the blur previews are shown (finding 31) — **M**

`crates/tgx-tg/src/convert.rs:459` `preview_of` returns `None` the moment the
media is a placeholder, which is precisely when a stripped thumbnail exists.
Add an arm before those returns: when `photo`/`file` is a placeholder **and**
`stripped_thumbnail` is present, emit

```rust
json!({ "src": stripped, "stripped": true, "width": w, "height": h })
```

sized through `preview_size` from the message's own `width`/`height`. That is
the key `writer.rs:468` has always read and nothing has ever written.

1,364 JPEGs are already on disk in the last export and none is displayed. No
leg can see this — Desktop's `result.json` has no `stripped_thumbnail` key — so
the test has to be a `tgx-tg` one: converter in, `_p.preview.stripped` out.
Add it to `crates/tgx-tg/tests/wire.rs`, beside the other `_p` assertions.

Delete the now-false clause in `convert.rs:343-344`, which lists this symptom
among those `presentation()` repairs.

### 2.5 — a failed download states its gap (finding 15) — **S**

`download.rs:159-162`. When the primary file fails, its `thumb_dest` and
`preview_dest` are never fetched and never recorded. Push them into
`tally.missing` alongside `job.dest`, so `missing_media.txt` names every path
the JSON and HTML promised.

**A constraint to state rather than fix:** filenames are decided before bytes
are fetched — that is the design, and it is where the speed comes from — so a
failed download *must* leave a reference to a file that is not there. The
archive's honesty comes from `missing_media.txt`, not from the JSON. Which
means the wire leg is currently scoring a stated gap as a dangling reference;
see 4.1.

### 2.6 — resolve the forward origins (finding 18) — **L**

94 `forwarded_from` fields came out empty across 13 people, all with correct
ids, all named by Desktop. `engine.rs:756-764` explains the gap by saying only
the sender and the chat are reachable — accurate about grammers'
`Message.peers` being `pub(crate)`, and the conclusion drawn from it ("which is
enough") is what the live run falsifies.

There is a path, and it does not need grammers to change:

* `grammers_session::Session` is a **public trait** with
  `fn peer(&self, PeerId) -> Option<PeerInfo>`, and grammers `cache_peer`s
  every peer it sees — including forward origins.
* `PeerInfo::auth()` yields the access hash, which is the missing half of a
  `PeerRef`.
* `Session::session()` (`client.rs:380`) already hands out the
  `Arc<SqliteSession>`.

So: collect the ids that resolved to `""` during the read pass, look each up in
the session store to build a `PeerRef`, and resolve the names in one batched
call at the end of the chat. `PeerInfo` carries no name, so the batch is a real
request — but one per chat, not one per message.

**Do this last in the phase and timebox the spike.** If the store turns out not
to hold them at the moment we ask, the honest outcome is to correct the comment
at `engine.rs:756` and record the gap in ROADMAP rather than leave a claim the
next live run will falsify again. Either result is acceptable; a stale comment
is not.

**Pins it:** a test that a message whose forward origin is absent from the name
book but present in the session store comes out named. If the spike fails, the
deliverable is the corrected comment instead.

---

## Phase 3 — lifecycle, cancellation and the window

*Findings 6, 7, 9, 10, 21, 22, 23, 29, and the `Bridge::new` expect. Nothing
here changes a byte of output; all of it changes what happens when something
goes wrong.*

### 3.1 — the cancel token reaches the download pool (finding 7) — **M**

`download.rs` does not import `Cancel`. Thread it: `run_all(…, cancel: &Cancel)`
→ clone into each spawned task → check before acquiring the permit and between
retries. Replace `sleep_in_slices(wait)` at `download.rs:195` with
`sleep_in_slices_until(wait, cancel)`; the current call passes a
`Cancel::new()` nobody holds, so the comment beside it is false.

Bound the retry in the same commit: `attempt -= 1` on a rate limit means a
persistent `FLOOD_WAIT` loops without limit *and* without an exit. Count
rate-limit waits separately and give them their own ceiling.

On cancel, stop starting new jobs, let in-flight ones finish, and record the
un-run jobs in `missing_media.txt` — a stated gap, consistent with 2.5.

Then correct `AUDIT.md`'s item 3, which claims this already works.

**Pins it:** a test that `run_all` with a pre-cancelled token completes without
starting the jobs, and that the un-run paths appear in the tally's `missing`.

### 3.2 — a wrong 2FA password is recoverable (finding 6) — **S**

`client.rs:359`. grammers returns `SignInError::InvalidPassword(token)` for a
wrong password and `PasswordRequired(token)` only to *ask*. Match both:

```rust
Err(SignInError::InvalidPassword(token)) | Err(SignInError::PasswordRequired(token)) => {
    self.password = Some(token);
    Err(anyhow!("that password was not accepted"))
}
```

Three lines, and the difference between a 2FA user being able to sign in on the
second attempt or having to restart the whole exchange.

**Pins it:** hard without a live account — the honest test is a unit test over a
small shim that reproduces the match arms, plus a note in the commit that the
real path is unverified until someone signs in. Say so rather than implying
coverage.

### 3.3 — `Event::Failed` stops being a global switch (finding 9) — **M**

`shell/mod.rs:526`. Give the event the run it belongs to and ignore ones that do
not match, *or* stop `sign_in`/`refresh_chats` using that variant at all — they
have `Status` and `Warn`. Fold an in-flight sign-in probe into `busy`
(`mod.rs:810`) so `03 Start export` is not clickable while one is running, and
make `start_sign_in` (`mod.rs:702`) not spawn a probe when one is already in
flight — which is also what makes the comment at `mod.rs:813` true.

Clear `self.failure` in `start_export` while here (finding 23), so a failure
from before the run stops being appended to the run's summary.

**Pins it:** a test that a `Failed` arriving during an export leaves `exporting`
true; and one that a stale `failure` does not reach the next run's status.

### 3.4 — `explorer.exe` (finding 10) — **S**

`actions.rs:653`: `%SystemRoot%\explorer.exe`, not
`%SystemRoot%\System32\explorer.exe`. Add a sibling to `config::system32` —
`config::system_root("explorer.exe")` — rather than passing a bare name, so the
absolute-path property from item 10 of the first audit is kept. `system32()`
stays correct for `icacls.exe`.

Stop discarding the result: a spawn failure should reach the journal, or the
button fails silently again the next time the path is wrong.

**Pins it:** a test that the built path ends in `explorer.exe` and does **not**
contain `System32` — the mirror of the existing icacls test, which is the test
that was never written.

### 3.5 — the export's error paths drain (finding 22) — **S**

`engine.rs:560`'s `sink.output.add(&payload)?` is a third error return that
skips `close_all`, whose doc claims every path comes through it. `Drop` keeps
the JSON valid, but the index (`export_results.html`) is not written and the
empty-folder pruning does not run — reintroducing the dead back-link the last
audit fixed. Route it through `close_all` like the other two, or correct the
comment. Prefer the former.

### 3.6 — the second Ctrl-C says what it costs (finding 21) — **S**

`bin/tgx.rs:282`. `std::process::exit(130)` runs no destructors, so every open
`Output` is abandoned with its buffer unflushed — zero bytes, which is what the
comment eleven lines above promises to prevent. At minimum print what is about
to be lost. Better: make the second press flush and exit, since `Output::close`
is cheap and the whole point of the design is that this file is never empty.

### 3.7 — a failed chat says which chat (finding 29) — **S**

`shell/mod.rs:493` has `chat_id` and `queue.title_of` in hand. Prefix the
journal line, matching every other line in the transcript. `actions.rs:366`'s
rate-limit message needs the same — it already receives `chat_title` and its
sibling branch four lines below uses it.

### 3.8 — `Bridge::new` — **S**

`shell/mod.rs:254`'s `expect` runs before `WINDOW_OPENED` is set, so a
thread-spawn failure is reported as a DirectX problem. Return the error and
surface it through the same path as any other startup failure.

---

## Phase 4 — the oracle's own calibration

*Findings 16, 24, and the two parity items below. Do this **before** the next
live export, or that run's report cannot be read.*

### 4.1 — the wire leg stops inventing the reference's side (finding 16) — **M**

`wire_leg.rs:333-350`. `skip_reason` returns `None` for three different states,
one of which is "the key is absent", and the example line renders the other
side as `y.unwrap_or("downloaded")`. Use `brief()`, which already renders
`"absent"` correctly.

Add the missing mirror: `absent` (`:379`) counts fields the reference writes and
we do not; there is **no tally for a field we write and the reference does
not**. That is the class message #509 belongs to, and it is exactly as
invisible to replay as `absent` was. Add an `extra` map built from `m.keys()`
in the other direction and include it in `Report::clean()`.

Teach it about stated gaps at the same time: a path listed in the export's own
`missing_media.txt` is a *declared* absence, not a dangling reference (see 2.5).
Score them separately or the leg will report 21 dangling paths forever on any
run with a failed download.

### 4.2 — renames and edits stop reading as converter failures (finding 24) — **M**

`wire_leg.rs:45,47,59`. `from`, `actor` and `forwarded_from` are resolved
display names, so one person renaming their profile turns every message they
ever sent into a mismatch — visible in the last run as `Tamara Blokade` vs
`Tam Fmk 📸` on three messages, and capable of being thousands.

Keep the fields — they are what caught the 206 empty names — but bucket a
disagreement by `from_id` and report *"n messages differ only in display name
(k distinct people)"*. A rename is uniform across one peer's messages; an
empty-name bug is not.

Separately, `text` is required while `edited` may drift, and an edit is *why*
`text` changes. When `edited`/`edited_unixtime` differ for an id, exempt
`text`/`text_entities` and count it as an edit.

This matters beyond tidiness: the report prints five examples per bucket, so a
rename storm can push a genuine `text` or `media_type` failure off the page.

### 4.3 — size decisions become settings-aware — **M**

The leg compares our placeholder against Desktop's, but a disagreement can mean
our size limit differed, our inclusion settings differed, or the converter was
wrong. Only the third is a defect. Split the tally into `too_large` and
`not_included`, and record the settings the run used so a uniform block is
recognisable as a settings difference rather than a scatter of failures.

### 4.4 — test the wire leg itself — **M**

`wire_leg::run` has no test; the unit tests cover `compare`, `skip_reason` and
`brief` only. Commit a small two-tree fixture under `crates/tgx-parity/tests/`
and drive `run` end to end, asserting the failure count for each documented
case — including the one-topic pairing special case at `:83-90`, which
currently degrades to comparing *any* single topic against *any* other when the
titles differ, so pointing the leg at the wrong directory produces a full
comparison of two unrelated chats instead of an error.

The leg is the only thing standing between the converter and a silent wire
regression, and it has been wrong three times. It should not itself be
untested.

---

## Phase 5 — hygiene

*Finding 33, 32, and the Low list. Individually small; worth one or two
sweeping commits rather than twenty.*

**One commit — dead code (finding 33).** Delete the eleven `pub` items reachable
only from their own tests: `stripped::dimensions`, `order::ordered_index`,
`PeerKey::raw_id`, `Tree::at`, `Tree::indent`, `Tree::raw_tag`,
`writer::css_preview_size`, `writer::preview_box_for`, `HtmlWriter::total`,
`NameBook::is_claimed`, `Presentation::is_empty`. Removing `ordered_index` drops
`indexmap` from `tgx-format` entirely — it is redundant by construction, since
`serde_json` is built with `preserve_order` and `Map` already *is* an
`IndexMap`. Also `ChatInfo::access_hash` (finding 19), which is written by
`dialogs::chat_info` and read nowhere; correct the comment at `dialogs.rs:47-52`
that credits it with a speedup the batching actually delivered.

This is the class that hid finding 2 of the first audit and finding 31 of this
one. Consider whether a `#[deny(dead_code)]`-shaped check is reachable for
`pub` items in a workspace — probably not without an external tool, in which
case say so in CLAUDE.md rather than leaving the impression that a lint covers
it.

**One commit — the JPEG offsets (finding 32).** `jpeg_header.rs:53-56` names
byte 164 the width and 166 the height; walking the committed header shows SOF0
puts **height at 163-164 and width at 165-166**. `expand` is byte-correct and
only the names are wrong — but the error has already produced a second bug in
`stripped::dimensions`, which returns `(height, width)` under a doc saying
otherwise, with a test enshrining the swap. If `dimensions` is deleted per the
commit above, fix the constants and the doc anyway; the next reader will
otherwise reintroduce it. Regenerate `jpeg_header.rs` through
`tools/gen_jpeg_header.py` rather than hand-editing, per CLAUDE.md — the
committed file no longer round-trips through its generator.

**One commit — the small correctness items.**
* `sanitize_extension` re-introducing a trailing space (`names.rs:82-90`), which
  produces a `result.json` path that is not the name on disk.
* `fit_box` overflowing `i64` on hostile dimensions (`preview.rs:26,30`).
* `desktop_reaction_indent` over-indenting unranked keys that sort after
  `reactions` (`json.rs:43-65`).
* `userpic_class` silently swallowing an over-long id and a leading `-`
  (`userpic.rs:68`), whose "gigantic id" test asserts only `1..=8` and so
  passes on the fallback.
* `error.rs:98`'s `unwrap_or(0)` → a one-second floor.
* `Output::add` incrementing `count` last (`output.rs:78-96`), which can drop
  the `,` separator if a caller ever survives an error from it.
* `tgx export ""` matching an arbitrary chat (`bin/tgx.rs:197`).
* A malformed `TG_API_ID` silently ignored (`bin/tgx.rs:40`).
* `sort_mode` given the same fallback `theme` has (`config.rs:218`).

**One commit — defence in depth on the HTML.** Route `<img src>` through
`safe_href` (`writer.rs:473,500,523,543,568`) and `PageChrome.back_href`
(`page.rs:92`); neither is reachable today, both are one call, and
`escape.rs:11-12` already states the rule as absolute. De-duplicate
`SAFE_SCHEMES` (`escape.rs:58` / `inline.rs:12`) so adding a scheme cannot
diverge them. Delete the dead check at `escape.rs:96-98`.

**One commit — the tooling nits.** `-- -D warnings` on CI's clippy line;
`--locked` on CI's cargo invocations; `save.bat`'s `pause` skipped when an
action was passed as an argument; `--force-if-includes` alongside
`--force-with-lease`; `settings_are_wired`'s substring match tightened so a
future short field name is not satisfied by an unrelated struct's field access;
`index.rs:92`'s broken doc link.

**Documentation, last.** `html_leg.rs:10-14` should list all seven values it
lifts out of Desktop's HTML, not five — everything lifted is by construction not
under test, and the size of that surface should be visible. Record in ROADMAP
that the corpus contains **zero** `hashtag`/`cashtag`/`bot_command` entities, so
the three JS-injection paths in `inline.rs:95-106` are covered by unit tests
only and never by the oracle.

---

## Decisions I cannot make for you

These are choices about what the product *is*, not defects. Each blocks or
reshapes work above.

### D1 — does this tool track Desktop, or supersede it? (finding 20)

**Already landed** in `2c63127` — `reactions_are_truncated` and
`poll_needs_refresh` now gate real `fetch_reactors` / `fetch_poll_results`
calls from `engine.rs:947,969`. So finding 20's mechanical half (five settings
offered and read by nothing) is closed, and what remains is not a fix but a
confirmation: the new behaviour is a deliberate step away from Desktop, and
nothing yet says so. The measurements:

| | Desktop | Python original | ours today |
|---|---|---|---|
| max named reactors | **3** | **11** | 3 |
| the two `min` polls | `total>0`, answers all `0` | identical | identical |

Desktop never names more than three reactors, and does not refresh a `min`
poll. The Python fetches full reactor lists and does *not* successfully refresh
polls — its own comment says that feature never worked.

So wiring `full_reactions` restores **Python** parity and departs from
Desktop's; wiring `refresh_polls` departs from **both**, on 2 of 7 polls. Both
are defensible — richer data is a legitimate goal, and it is the reason this
tool exists at all — but they are deviations of the same kind as
`link_previews`, not bug fixes.

**What the answer changes:** whether `poll` stays in the wire leg's must-match
set (it will go red on those two messages otherwise), whether these settings
default on or off, and how `enrich.rs:8-15`'s table should read — it currently
promises six enrichments and the module implements one, with `chat_metadata`,
`invite_links` and `scheduled_messages` being settings nothing reads at all.
Whatever you decide, that table needs to describe the code.

### D2 — what should a link preview's *document* do?

With `link_previews` on, a YouTube link (ćaskanje #509, #535) is exported as a
`video_file` with a `file_name` of `Spandau Ballet - Gold (HD Remastered).mp4`
and a 14 MB size — a message the sender wrote as a link becomes, in the
archive, a video they appear to have sent. Desktop writes no media at all; so
does the Python.

The setting is documented as "save the image attached to a link preview". The
document branch (`plan.rs:186-188`) goes beyond that wording. Options: restrict
`link_previews` to the photo, gate the document behind its own setting, or
document that it covers both.

Related, and worth knowing before deciding: with the setting on, **21 of the 23**
link-preview media in the last run failed to download, so the feature's main
observable effect was 21 dangling references and a shifted `photo_N` sequence.

### D3 — pin `grammers-*`? (finding 27)

`gpui` is pinned exactly because a bump breaks the interface — which is a
compile error, loud and immediate. `grammers-tl-types` is pinned by caret, and a
bump inside 0.10.x can re-shape a TL field, compile cleanly (the converter
matches only the variants it knows), and quietly change `result.json`. That is
the class CLAUDE.md says no test here can catch. `chrono` and `sha2` are in
similar positions for `date` and for the corpus manifest hash.

My recommendation is to pin them and add `--locked` to CI, but it is your call
how much dependency friction to accept.

### D4 — may `tgx-app` depend on `grammers-client`? (finding 28)

`crates/tgx-app/Cargo.toml:18` has the dependency; CLAUDE.md says the window
depends only on `tgx-ui` and `tgx-tg`; `layering.rs` enforces the other two
rules and not this one. Either add the assertion or correct the document — at
present they disagree and the test that exists to catch exactly this does not
look.

---

## Proving it is done

Unit tests and the three replay legs cannot confirm Phase 2. They open no
sockets, and every finding there is on the converter side of the replay
boundary. The only proof is a live run, which needs your account.

**The protocol, once Phases 0–4 are in:**

1. `save.bat test` — green, and now genuinely failing if the corpus is missing.
2. `save.bat parity` — all three legs at their marks (4/4, 4/4, 830/836). None
   of Phase 2 should have moved them; if one moved, stop and find out why before
   the live run.
3. `save.bat release` — and check the binary's timestamp, because until 0.1
   landed that step did nothing.
4. A live export of the same supergroup, with `link_previews` set to whatever
   D2 decides.
5. `save.bat wire "<the new export>"`.
6. **A basic group and a private chat, one each** — see V1. The supergroup
   exercises one of the three peer branches; the other two have never been run
   and are argued from the schema. Neither needs to be large. What to check is
   the header: a basic group should carry `description` / `members_count` /
   `members_listed`, and a private chat its `description` (the bio) — an empty
   header means the switch is on and the request is not being made, which is
   the bug this protocol step exists to catch.
7. `RUST_LOG=debug` on at least one of the three, so `tgx.log` carries the
   per-message and per-file lines. Everything above tells you *what* the export
   contains; this is the only thing that says what it *did*.

**Expected wire-leg numbers after Phase 2**, against the last run's:

| | before | after | why |
|---|---|---|---|
| `absent` | 1,290 | **0** | 1,287 `thumbnail_file_size` (2.1) + 3 `action` (2.3) |
| `dangling` | 21 | **0**, or 21 *stated* | 0 if `link_previews` off; otherwise reclassified by 4.1 |
| `sticker_emoji` mismatches | 11 | **0** | 2.2 |
| `forwarded_from` empty | 94 | **0** or documented | 2.6, depending on the spike |
| `from` mismatches | 5 | **bucketed as renames** | 4.2 — the people really did rename |
| `width`/`height`/`photo_file_size` extra | 23 | **0** if previews off | D2 |
| `<img>` targets missing from disk | 21 | **0** | 2.5 + D2 |
| blur previews shown | 0 of 1,364 | **1,364** | 2.4 |

Write the observed numbers into `AUDIT.md` beside the predictions. Where reality
disagrees with this table, the table was wrong and that is the finding —
which is the same rule as "fix the code, not the reference", pointed at me.

---

## What the mechanical checks do and do not prove

Three checks were added while closing the first-pass audit. Each replaces a
thing someone had to remember with a thing the build enforces, and each has a
boundary worth stating, because a check whose limits are not written down gets
read as covering more than it does — which is how five dead settings survived a
green suite in the first place.

| check | catches | cannot catch |
|---|---|---|
| `tgx-tg/tests/settings_are_wired.rs` | a `Settings` field read by nothing | a field read *badly*, or read on only one code path |
| `tgx-parity/tests/layering.rs` | a forbidden crate dependency | a layering violation that goes through a shared type |
| `wire_leg`'s `absent` / `dangling` | a key the reference has and we never write; a media path with no file | a key **neither** exporter writes |

### V1 — "wired" is not "wired for every case" — **M**

`settings_are_wired.rs` proves a setting is *named* in exporting code. It cannot
tell that `chat_metadata` was, for two commits, wired only for channels — the
basic-group and private-chat branches returned an empty map with the switch on,
and the test was green throughout because the field appeared in `enrich.rs`.
The same gate silently gave a basic group a member list of nobody flagged
`complete: true`.

Both were found by diffing the request lists of the two implementations, not by
any test. Until a live export exists for a **basic group** and for a **private
chat**, the peer coverage added in `15cc141` is argued from the schema and not
observed. Both are cheap to run and neither is in the protocol above, which
exports one supergroup.

**Do:** add a non-forum chat and a private chat to the live-run protocol, and
record the header each produced.

### V2 — a field-by-field diff is the only check for behaviour inside a wired path — **L**

Every check listed above is structural: it asks whether a thing is called, not
whether what came back was read properly. `full_reactions` is the case in point
— the request existed in the plan, the setting existed, the predicate existed,
and the emitted list was still Telegram's three-name sample. Nothing structural
could see that; it took someone comparing our reaction lists against the Python
original's and noticing they were short.

The wire leg's `absent` counter is the closest automatic equivalent, and it is
one-directional: it reports a key Desktop writes and we do not. It says nothing
about a key we both write with **different contents**, which is precisely the
reaction-cap shape.

**Do:** after the live run, diff the three exports field by field — not just
ours against Desktop, but ours against the Python original's, which is the only
artefact that carries the extensions Desktop has no opinion about
(`stripped_thumbnail`, the topic header, the roster keys). The three-way is
what found every finding in the `2026-08-27` section; two-way against Desktop
alone would have found none of the extension bugs.

### V3 — the Python original is the map, so a shared gap is invisible — **L**

Every completeness check in this repo is ultimately "does it match Desktop, or
does it match the original". A feature **neither** implementation has is
invisible to all of them, and there is no oracle for it at all — the parity
harness is built on the premise that Desktop's export is the specification.

Two known shared gaps are already recorded and are the shape of the problem:
`custom_emoji.document_id` stays numeric in both, and both miss Desktop's
trailing empty text segment until `433ae90` fixed ours (the original still has
it wrong). Neither was found by a check; both were found by reading Desktop's
bytes.

**Do:** nothing scheduled. This is stated so that "all three legs green, all
checks green" is never read as "complete". The honest claim is *complete
against Desktop's format as we have measured it*, and the measuring is
`reference/`, which is one export of one chat.

---

## What this plan does not cover

* Nothing here addresses the **`_p` blind spot in the html leg** — it still
  lifts the presentation map out of Desktop's own pages, so it proves the writer
  and not the pipeline. `crates/tgx-tg/tests/wire.rs` is the compensating
  control and 2.4 adds to it. Closing the leg's own gap is a larger piece of
  work and is already recorded as open in the previous audit.
* `custom_emoji.document_id` remains a numeric id rather than a sticker path —
  the documented media-leg ceiling, unchanged.
* `writer.rs` at 868 lines and `render_media`'s 150-line if-ladder are noted and
  deliberately not scheduled; splitting them has no test to hold it steady and
  the file is under the oracle, so the risk is all downside until something
  needs to change there.
