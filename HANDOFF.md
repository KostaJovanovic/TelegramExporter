# Handoff — 2026-09-14

Two things are open. The database export queue is done, but one chat failed.
The exporter has four fixes in the working tree. They are tested but not
committed and not built.

## Next actions

1. Close the TelegramExporter window. The queue is done, but the process
   (started 2026-09-13 20:37) is still open. Windows locks
   `dist\TelegramExporter.exe` while it runs.
2. Run `save.bat test`. I ran only `cargo test --all` (664 passed, clippy
   clean). That run does not set `TGX_REQUIRE_CORPUS=1`, so the corpus leg can
   skip without a failure.
3. Read the diff and commit it. Nothing is committed.
4. Run `save.bat build`.
5. Queue `Krovna RG za merenje` again, with **"Re-read the whole history"**
   ticked. A plain re-run gets the missing messages, but not the media of the
   48,312 messages the database already holds. A sync fetches files only for
   the messages it reads again.
6. Look in `tgx.log` for `rpc error 500` or `Telegram failed on its side`. The
   first live server error is the first real test of fix 1 below.

## The export: `L:\9 telegram export`

Database mode, one `.sqlite` per chat, media inside. The size limit was 5 MB.
Database mode writes no folder, so files over 5 MB are not stored anywhere.

| database | messages | files | size | result |
|---|---:|---:|---:|---|
| `KROVNA RADNA GRUPA ZA MEDIJE 3.0 ©®™️ ️.sqlite` | 482,387 | 24,875 | 7.69 GB | complete, 11 files missing |
| `admini x koord vol2.sqlite` | 171,354 | 9,180 | 4.57 GB | complete |
| `Krovna RG za merenje.sqlite` | 48,312 of 122,487 | 0 | 0.02 GB | **failed: `RPC_CALL_FAIL`** |
| `🟥 RJIL Narativ (Mediji).sqlite` | 42,445 | 1,840 | 0.43 GB | complete |
| `KRGD_ među(z)gradska saradnja.sqlite` | 27,011 | 1,159 | 0.18 GB | complete |
| `Svi media timovi.sqlite` | 17,205 | 946 | 0.27 GB | complete |
| `✨❗️Rektorat❗️✨.sqlite` | 16,608 | 1,196 | 0.22 GB | complete |
| `Admini.sqlite` | 14,855 | 794 | 0.23 GB | complete |
| `MAŠINCI PROTIV MAŠINERIJE.sqlite` | 13,532 | 1,478 | 0.32 GB | complete |
| `Objedinjeni Rektorat.sqlite` | 13,110 | 627 | 0.15 GB | complete |
| `Redakcija.sqlite` | 11,584 | 1,517 | 0.62 GB | complete |
| `UA KOLAB.sqlite` | 8,206 | 1,751 | 2.41 GB | complete |

"Complete" means that the last run reached the end of the history and closed.
For `admini x koord vol2`, the topic counts add up to the chat total, and the
stored files match the files fetched. I did not check the other chats that
closely.

- **KROVNA 3.0:** 11 files did not download. 9 failed on `Timeout` and 2 on
  `FILE_REFERENCE_EXPIRED`. Also, 8 extra lookups were lost to rate limits.
  The filename has an invisible character before `.sqlite`. Pick it in a file
  dialog. Do not type it.
- **Older files in the same folder:**
  - `telegram.sqlite` (2026-09-04) is the TelegramAnalyser `tga-db` corpus.
    Keep it.
  - `skitanje i snimanje.sqlite` (2026-09-04) holds 0 messages and an
    unfinished run.
  - `.tgx-scratch\` is empty.

## Exporter changes: uncommitted

Recorded in `AUDIT.md` ("What the first database queue found") and in
`CLAUDE.md` (the retry rule). There are 12 changed files in `crates/tgx-tg`,
`crates/tgx-app` and the two docs.

1. **Telegram server errors are retried.**
   - `error.rs`: the new `EnrichError::Unavailable` covers codes 500 and -503.
     It matches on the code, not the name.
   - `client.rs`: the new retry policy `Patient` wraps grammers' `AutoSleep`.
     It sends a failed request again 3 times, after 2, 4 and 8 s. It covers
     every request.
   - `engine.rs`: the history loop continues from its cursor after 30 s
     (`UNAVAILABLE_PAUSE`). It gives up after `MAX_STALLED_WAITS`.
   - `resolve_topics`: a server error now skips the chat and does not merge
     its topics into one folder. `TopicsResolution::RateLimited` is now
     `TryLater`.
   - `enrich::guarded`: a server error now counts as a lost lookup.
   - Downloads: a server error still uses up one of the 5 attempts. `Patient`
     also retries inside each attempt, so the worst case is about 70 s for
     each file.
2. **The "missing_media.txt" pointer in database mode.** With no folder, every
   failed path now goes to `tgx.log`, and the warning says so.
3. **"own names" counts.** `convert::Aliased` counts people, not messages. The
   engine resets it at the start of each chat, and the count is on
   `ExportResult::aliased`. The 18 extra spaces in the log line are gone.
4. **Folder wording.** Database mode now says "topics", not "topic folders" or
   "one folder each".

**Unverified.** No test uses a real server error. The tests give the policy
made-up errors. The -503 code for `Timeout` comes from Telegram's
documentation. The log shows only the name `Timeout`, not the code.

## TelegramAnalyser

I changed no code in TelegramAnalyser. Its working tree has uncommitted work
from before this session (`crates/tga-db/` and `crates/tga-docs/` are
untracked). I did not touch it. The new files use the same schema as
`telegram.sqlite` (`schema_version` 1), so `tga-db` should read them. I did
not try this.
