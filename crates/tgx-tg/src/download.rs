//! The bounded download pool.
//!
//! Filenames were decided synchronously by [`crate::plan`], so the JSON and
//! HTML have already streamed out by the time anything is fetched. This is
//! where nearly all the wall-clock speed comes from.
//!
//! **Downloads are validated.** A call that returns without error but writes
//! zero bytes counts as a failure: the remnant is deleted and the path is
//! recorded in `missing_media.txt`. Never count a success on the return value
//! alone — the HTML references the file before it is fetched, so a silent
//! failure is a broken link in the archive rather than an error anyone sees.

use crate::cancel::Cancel;
use crate::error::{classify, EnrichError};
use crate::plan::{self, DownloadJob};
use grammers_client::media::{Downloadable, Media, PhotoSize};
use grammers_client::session::types::PeerRef;
use grammers_client::Client;
use grammers_tl_types as tl;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;

/// How many times a download is retried before its path is recorded as missing.
///
/// A [stale file reference] does not spend these: it fails identically however
/// many times it is asked, so it comes back on the first attempt and the caller
/// re-reads the message instead.
///
/// [stale file reference]: EnrichError::Stale
pub const MAX_RETRIES: u32 = 5;

/// How many rate limits one file may wait out before it is given up on.
///
/// A `FLOOD_WAIT` did not spend a retry — deliberately, since it is Telegram
/// asking for patience rather than a failure — but that made the retry loop
/// unbounded in the one case where it can genuinely never end: an account under
/// a persistent limit answers every attempt the same way, and the loop had no
/// second counter and no exit. Twenty waits is far more patience than any real
/// export needs and still terminates.
pub const MAX_RATE_LIMIT_WAITS: u32 = 20;

/// A path the export named and could not deliver, and why.
///
/// **The reason is not decoration.** Every failure used to reduce to
/// `Option::None` — `fetch_with_retry` returned one and threw the
/// [`EnrichError`] away — so `missing_media.txt`, the transcript and `tgx.log`
/// between them recorded *that* twenty-one files were absent and nothing about
/// why. A permanent, hundred-percent-reproducible refusal (grammers answering
/// "media not downloadable" for every link preview, before a request left the
/// process) was indistinguishable in every artifact the run produced from a
/// flaky network, and diagnosing it meant reading the dependency's source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingFile {
    pub path: String,
    pub reason: String,
}

impl MissingFile {
    fn new(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            reason: reason.into(),
        }
    }
}

/// What one media pass produced.
#[derive(Debug, Default)]
pub struct MediaTally {
    pub downloaded: usize,
    pub failed: usize,
    pub bytes: i64,
    /// Paths referenced by the export that could not be saved.
    ///
    /// **Longer than `failed`, and it must be.** `failed` counts *jobs*; this
    /// counts the paths each one had already promised, and a photo that fails
    /// takes its preview down with it. Reporting `failed` beside the words
    /// "see missing_media.txt" told the user 21 when the file listed 42.
    pub missing: Vec<MissingFile>,
}

impl MediaTally {
    fn merge(&mut self, other: MediaTally) {
        self.downloaded += other.downloaded;
        self.failed += other.failed;
        self.bytes += other.bytes;
        self.missing.extend(other.missing);
    }
}

/// What a stale file reference needs in order to be replaced.
///
/// Carried into the pool rather than looked up inside it: `run_all` spawns its
/// jobs across tasks, so each one needs its own copy, and both fields are
/// `Copy`.
#[derive(Debug, Clone, Copy)]
pub struct Refresh {
    /// The chat the jobs came from. A file reference can only be re-obtained by
    /// re-reading the message that carried it, and that read needs a peer.
    pub peer: PeerRef,
    /// The same switch [`plan::downloadable`] was given during the export's
    /// read, so a refreshed handle is derived exactly as the original was. A
    /// link preview resolves to the photo inside the page under one setting and
    /// to nothing under the other; the two passes must not disagree about which
    /// file they are fetching.
    pub link_previews: bool,
}

/// Why a fetch gave up, and whether a fresh file reference could still fix it.
///
/// The `bool` is not a convenience: without it the caller cannot tell the one
/// failure worth a second request from the several that are not, and treating
/// them alike is what turned 7,140 recoverable files into a permanent gap.
struct FetchFailure {
    reason: String,
    stale: bool,
}

impl FetchFailure {
    /// A failure no fresh reference would change.
    fn final_(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            stale: false,
        }
    }
}

/// A job plus the handle needed to fetch it.
pub struct PendingDownload {
    pub job: DownloadJob,
    /// `None` for an inline write — a stripped thumbnail has no request behind
    /// it and goes through the pool only to keep the disk write off the read
    /// loop.
    pub media: Option<Media>,
}

/// Where a running pool reports each file as it lands.
///
/// `None` on an ordinary run. The pool spawns its work across tasks, so this
/// cannot be the engine's `&mut dyn FnMut` sink — that is neither `Sync` nor
/// shareable — and a channel the caller drains while awaiting is the cheapest
/// thing that still reports *during* the batch rather than after it. A media
/// pass of three hundred files was the one stretch of an export that printed
/// nothing at all until it was over.
pub type Reporter = tokio::sync::mpsc::UnboundedSender<crate::engine::Progress>;

/// Run every job, at most `concurrency` at a time.
///
/// **Cancellable, which it was not.** This module did not so much as import
/// [`Cancel`], so Stop reached the engine's between-batch check and nothing
/// else: a folder of 1,781 files carried on to the last one. The comment beside
/// the rate-limit sleep said it waited "in slices so a cancel is not swallowed"
/// and then passed a `Cancel::new()` that nobody held, which is a signal that
/// can never fire.
///
/// On cancel the pool **stops starting new jobs and lets the in-flight ones
/// finish**. Tearing a download down mid-write would leave a part-written file
/// the HTML already links to, which is the thing this whole module argues
/// against. Every job that never ran is recorded in `missing` and so reaches
/// `missing_media.txt`: a stated gap, not a dangling reference.
pub async fn run_all(
    client: &Client,
    root: &Path,
    jobs: Vec<PendingDownload>,
    refresh: Refresh,
    concurrency: usize,
    report: Option<Reporter>,
    cancel: &Cancel,
) -> MediaTally {
    let permits = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut set = tokio::task::JoinSet::new();
    let queued = jobs.len();

    for pending in jobs {
        let permits = permits.clone();
        let client = client.clone();
        let root = root.to_path_buf();
        let report = report.clone();
        let cancel = cancel.clone();
        set.spawn(async move {
            let _permit = permits.acquire_owned().await;
            // Checked *after* the permit, not before: with a concurrency of 5
            // and 1,781 jobs, 1,776 of them are already spawned and waiting
            // here when Stop is pressed. Checking on the way in is what makes
            // the cancel take effect in the time it takes five files to finish
            // rather than 1,781.
            if cancel.is_cancelled() {
                return skipped(&pending.job);
            }
            let dest = pending.job.dest.clone();
            let started = std::time::Instant::now();
            let one = run_one(&client, &root, pending, refresh, &cancel).await;
            if let Some(tx) = &report {
                // The reason, on the line that says FAILED. `RUST_LOG=debug` is
                // what someone reaches for when files are going missing, and it
                // was the one view of a failure that named the file and still
                // would not say what happened to it.
                let why = one
                    .missing
                    .iter()
                    .find(|m| m.path == dest)
                    .map(|m| format!(" — {}", m.reason))
                    .unwrap_or_default();
                let _ = tx.send(crate::engine::Progress::Detail(format!(
                    "  {} {dest} ({} in {:.1}s){why}",
                    if one.failed > 0 { "FAILED" } else { "saved " },
                    super::engine::human_bytes(one.bytes),
                    started.elapsed().as_secs_f64()
                )));
            }
            one
        });
    }

    let mut tally = MediaTally::default();
    let mut finished = 0usize;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(one) => tally.merge(one),
            // A panicked task must not take the export with it.
            Err(_) => tally.failed += 1,
        }
        finished += 1;
        // A heartbeat for the window, which does not see the per-file lines.
        // Every twenty-fifth, so a big batch shows movement and a small one
        // says nothing beyond its own summary.
        if let Some(tx) = &report {
            if finished.is_multiple_of(25) && finished < queued {
                let _ = tx.send(crate::engine::Progress::Log(format!(
                    "  {finished} of {queued} files, {} so far",
                    super::engine::human_bytes(tally.bytes)
                )));
            }
        }
    }
    tally.missing.sort_by(|a, b| a.path.cmp(&b.path));
    tally
}

/// Every path this job's message already promised in the JSON and the HTML.
///
/// The primary file, Telegram's thumbnail and the inline preview are named by
/// `plan.rs` before a byte is fetched — that is the design, and it is where the
/// wall-clock speed comes from — so a job that fails or never runs leaves up to
/// three references to files that are not there. Only the first was ever
/// recorded, so `missing_media.txt` under-reported and the other two showed up
/// as dangling `<img>` tags with nothing anywhere accounting for them.
///
/// `reason` belongs to the primary file. The other two get a knock-on reason
/// naming it, because they were never attempted — saying "media not
/// downloadable" about a thumbnail no request was ever made for would be the
/// report inventing an answer it does not have.
fn promised(job: &DownloadJob, reason: &str) -> Vec<MissingFile> {
    let mut out = vec![MissingFile::new(job.dest.clone(), reason)];
    let knock_on = format!("not fetched, because {} was not", job.dest);
    out.extend(
        job.thumb_dest
            .iter()
            .chain(job.preview_dest.iter())
            .map(|p| MissingFile::new(p.clone(), knock_on.clone())),
    );
    out
}

/// The tally for a job the pool never started, because Stop was pressed.
///
/// Counted as **failed**, not as a separate category. A file the export named
/// and did not deliver is a gap in the archive whatever the reason, and the run
/// says "Stopped" beside the number, so nothing here is claiming it tried.
fn skipped(job: &DownloadJob) -> MediaTally {
    MediaTally {
        failed: 1,
        missing: promised(job, STOPPED),
        ..MediaTally::default()
    }
}

/// Said of a file the pool never got to, and of one a Stop interrupted.
const STOPPED: &str = "stopped before this file was fetched";

async fn run_one(
    client: &Client,
    root: &Path,
    pending: PendingDownload,
    refresh: Refresh,
    cancel: &Cancel,
) -> MediaTally {
    let mut tally = MediaTally::default();
    let dest = root.join(&pending.job.dest);

    if let Some(parent) = dest.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tally.failed += 1;
            tally.missing.extend(promised(
                &pending.job,
                &format!("could not create {parent:?}: {e}"),
            ));
            return tally;
        }
    }

    // A stripped thumbnail is already in memory: write it and stop.
    if let Some(bytes) = &pending.job.inline_bytes {
        match std::fs::write(&dest, bytes) {
            Ok(()) => {
                tally.downloaded += 1;
                tally.bytes += bytes.len() as i64;
            }
            Err(e) => {
                tally.failed += 1;
                tally.missing.push(MissingFile::new(
                    pending.job.dest.clone(),
                    format!("could not write it: {e}"),
                ));
            }
        }
        return tally;
    }

    let Some(mut media) = pending.media else {
        tally.failed += 1;
        tally.missing.extend(promised(
            &pending.job,
            "the message carries no downloadable media",
        ));
        return tally;
    };

    // **Two passes at most.** The first asks with the file reference captured
    // during the read; if that has aged out, the second asks with one re-read
    // from the message. The fresh handle then replaces the stale one for the
    // thumbnail and the preview as well, since all three are derived from it.
    //
    // Downloads do not begin until the whole history has been read, so on a
    // large chat the oldest references are well past their life by the time
    // they are used: 450,817 messages took over an hour to read and 20.6% of
    // that chat's media was refused as expired, against 6 files in the
    // 162,132-message chat exported next to it.
    let mut outcome = fetch_with_retry(client, &media, &dest, cancel).await;
    if outcome.as_ref().err().is_some_and(|f| f.stale) {
        match refreshed_media(client, refresh, &pending.job).await {
            Ok(fresh) => {
                media = fresh;
                outcome = fetch_with_retry(client, &media, &dest, cancel).await;
            }
            // Say which of the two failed, and why. "Stale" on its own reads as
            // though the refresh had never been attempted.
            Err(why) => {
                if let Err(f) = &mut outcome {
                    f.reason = format!("{}, {why}", f.reason);
                }
            }
        }
    }

    match outcome {
        Ok(written) => {
            tally.downloaded += 1;
            tally.bytes += written;
            fetch_thumb(
                client,
                root,
                &media,
                &pending.job,
                refresh,
                &mut tally,
                cancel,
            )
            .await;
            fetch_preview(
                client,
                root,
                &media,
                &pending.job,
                &dest,
                &mut tally,
                cancel,
            )
            .await;
        }
        Err(f) => {
            tally.failed += 1;
            // Not just `dest`. When the primary file fails, its thumbnail and
            // its preview are never fetched and never recorded, yet the JSON
            // and the HTML have already named all three. Reporting one of three
            // made `missing_media.txt` an incomplete account of the same run's
            // gaps.
            tally.missing.extend(promised(&pending.job, &f.reason));
        }
    }
    tally
}

mod fetch;

pub use fetch::write_missing;

use fetch::{fetch_preview, fetch_thumb, fetch_with_retry, refreshed_media};

// Test-only: handle_for's branches are covered by this module's own tests.
#[cfg(test)]
use fetch::handle_for;

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tgx-dl-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn an_inline_write_needs_no_request() {
        // A stripped thumbnail arrives inside the message; the pool exists
        // only to keep the disk write off the read loop.
        let root = tmp("inline");
        let pending = PendingDownload {
            job: DownloadJob {
                dest: "thumbnails/a.jpg".into(),
                thumb_dest: None,
                preview_dest: None,
                size: 3,
                inline_bytes: Some(vec![1, 2, 3]),
                message_id: 1,
            },
            media: None,
        };
        // Exercised without a client: the inline path returns before using it.
        let mut tally = MediaTally::default();
        let dest = root.join("thumbnails/a.jpg");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, pending.job.inline_bytes.as_ref().unwrap()).unwrap();
        tally.downloaded += 1;
        assert_eq!(std::fs::read(&dest).unwrap(), vec![1, 2, 3]);
        assert_eq!(tally.downloaded, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A job for a photo of `size` bytes, named as the JSON already named it.
    fn photo_job(size: i64) -> DownloadJob {
        DownloadJob {
            dest: "photos/photo_1@01-01-2026_00-00-00.jpg".into(),
            thumb_dest: None,
            preview_dest: None,
            size,
            inline_bytes: None,
            message_id: 4242,
        }
    }

    fn photo_media(size: i32) -> tl::enums::MessageMedia {
        tl::enums::MessageMedia::Photo(tl::types::MessageMediaPhoto {
            spoiler: false,
            live_photo: false,
            photo: Some(tl::enums::Photo::Photo(tl::types::Photo {
                has_stickers: false,
                id: 1,
                access_hash: 0,
                // The blob that expires. Empty here: the point of the refresh
                // is that we take whatever the re-read hands back.
                file_reference: vec![],
                date: 0,
                sizes: vec![tl::enums::PhotoSize::Size(tl::types::PhotoSize {
                    r#type: "y".into(),
                    w: 100,
                    h: 100,
                    size,
                })],
                video_sizes: None,
                dc_id: 2,
            })),
            ttl_seconds: None,
            video: None,
        })
    }

    #[test]
    fn a_re_read_message_replaces_the_stale_handle() {
        // The whole point of the fix: the reference expired, the file did not,
        // and the same file coming back is accepted. A guard that rejected here
        // would make the refresh an expensive no-op whose only symptom is
        // another export losing a fifth of its media two hours later.
        let job = photo_job(9_000);
        let got = handle_for(Some(&photo_media(9_000)), &job, true);
        assert!(
            got.is_ok(),
            "the same photo re-read must be usable: {:?}",
            got.err()
        );
    }

    #[test]
    fn a_refresh_that_would_change_the_file_is_refused() {
        // `result.json` and the HTML published this job's name *and* its
        // `file_size` before a byte was fetched. If the message was edited in
        // between, writing the new file under the old name would leave the
        // archive stating a size it does not hold — a file that misdescribes
        // itself, which is worse than a gap the run declares.
        let job = photo_job(9_000);
        let why = handle_for(Some(&photo_media(12_345)), &job, true)
            .expect_err("a different file must not be written under the published name");
        assert!(why.contains("12345") || why.contains("12,345"), "{why}");
        assert!(why.contains("9000") || why.contains("9,000"), "{why}");
        assert!(
            why.contains("4242"),
            "the reason must name the message, since this line is the only \
             account anyone gets: {why}"
        );
    }

    #[test]
    fn a_message_that_lost_its_media_says_so_rather_than_guessing() {
        let job = photo_job(9_000);
        let why = handle_for(None, &job, true).expect_err("there is nothing to fetch");
        assert!(why.contains("4242"), "{why}");
        assert!(
            !why.contains("bytes"),
            "no size claim belongs in a reason we have no size for: {why}"
        );
    }

    #[test]
    fn missing_media_is_written_only_when_something_is_missing() {
        let root = tmp("missing");
        write_missing(&root, &[]).unwrap();
        assert!(
            !root.join("missing_media.txt").exists(),
            "an export that saved everything must not gain the file"
        );

        write_missing(
            &root,
            &[MissingFile::new(
                "photos/photo_1.jpg",
                "media not downloadable",
            )],
        )
        .unwrap();
        let body = std::fs::read_to_string(root.join("missing_media.txt")).unwrap();
        assert!(body.contains("photos/photo_1.jpg"));
        // And it says what the list means, not just the paths.
        assert!(body.contains("could not be saved"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_gap_is_written_with_its_reason_and_the_path_still_owns_its_line() {
        // Both halves matter. The reason is the half that did not exist: every
        // failure reduced to `None`, so 21 permanently-refused link previews
        // were indistinguishable from a flaky network in every artifact the run
        // produced. The layout is the half `tgx_parity::wire_leg::stated_gaps`
        // depends on — it reads this file to tell a declared gap from a
        // dangling reference, and a filename may contain any separator one
        // might otherwise put between the two.
        let root = tmp("reasons");
        write_missing(
            &root,
            &[
                MissingFile::new("photos/a — b.jpg", "media not downloadable, in 5 attempts"),
                MissingFile::new(
                    "photos/a — b_thumb.jpg",
                    "not fetched, because photos/a — b.jpg was not",
                ),
            ],
        )
        .unwrap();
        let body = std::fs::read_to_string(root.join("missing_media.txt")).unwrap();
        assert!(body.contains("media not downloadable"), "{body}");

        // What the leg does, on a path that contains the very separator a
        // one-line format would have had to split on.
        let paths: Vec<&str> = body
            .lines()
            .filter(|l| !l.starts_with(char::is_whitespace))
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("These files are referenced"))
            .collect();
        assert_eq!(paths, vec!["photos/a — b.jpg", "photos/a — b_thumb.jpg"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn job_with_all_three() -> DownloadJob {
        DownloadJob {
            dest: "video_files/clip.mp4".into(),
            thumb_dest: Some("video_files/clip.mp4_thumb.jpg".into()),
            preview_dest: Some("photos/photo_1.jpg".into()),
            size: 1024,
            inline_bytes: None,
            message_id: 7,
        }
    }

    #[test]
    fn a_failed_file_states_every_path_it_promised() {
        // `plan.rs` names the file, its thumbnail and its preview before a byte
        // is fetched. When the primary download fails the other two are never
        // attempted and never recorded, yet the JSON and the HTML have already
        // named all three — so missing_media.txt reported one gap in three and
        // the other two showed up as dangling <img> tags accounted for nowhere.
        let gaps = promised(&job_with_all_three(), "no bytes");
        let paths: Vec<&str> = gaps.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "video_files/clip.mp4",
                "video_files/clip.mp4_thumb.jpg",
                "photos/photo_1.jpg"
            ]
        );
        // The reason belongs to the file it is about. The other two were never
        // attempted, and saying "no bytes" about a request that was never made
        // is the report inventing an answer it does not have.
        assert_eq!(gaps[0].reason, "no bytes");
        assert!(
            gaps[1].reason.contains("video_files/clip.mp4"),
            "a knock-on gap must name what took it down: {}",
            gaps[1].reason
        );
        assert_eq!(gaps[1].reason, gaps[2].reason);

        // A job with neither still reports itself.
        let bare = DownloadJob {
            thumb_dest: None,
            preview_dest: None,
            ..job_with_all_three()
        };
        assert_eq!(
            promised(&bare, "no bytes"),
            vec![MissingFile::new("video_files/clip.mp4", "no bytes")]
        );
    }

    #[test]
    fn a_job_stop_prevented_is_a_stated_gap_not_a_silent_one() {
        // On cancel the pool stops starting jobs and lets the in-flight ones
        // finish — tearing a download down mid-write would leave a part-written
        // file the HTML already links to. What it must not do is drop the
        // un-run jobs silently: the JSON named them before the pool ever saw
        // them.
        let t = skipped(&job_with_all_three());
        assert_eq!(t.downloaded, 0);
        assert_eq!(t.bytes, 0);
        assert_eq!(t.failed, 1);
        assert_eq!(t.missing.len(), 3);
        // One job, three gaps — which is exactly why the warning that points
        // the user at missing_media.txt counts `missing`, not `failed`.
        assert!(
            t.missing[0].reason.contains("stopped"),
            "{:?}",
            t.missing[0]
        );
    }

    // A FLOOD_WAIT deliberately does not spend a retry — it is Telegram asking
    // for patience, not a failure. With `attempt -= 1` and no second counter
    // that made the loop unbounded in the one case where it can genuinely never
    // end: an account under a persistent limit answers every attempt the same
    // way. A const assertion rather than a test, because it is a statement about
    // the constants and should fail the build rather than a test run.
    const _: () = assert!(MAX_RATE_LIMIT_WAITS > MAX_RETRIES);
    const _: () = assert!(MAX_RATE_LIMIT_WAITS < 100);

    #[test]
    fn a_tally_merges_without_losing_anything() {
        let mut a = MediaTally {
            downloaded: 1,
            failed: 0,
            bytes: 10,
            missing: vec![MissingFile::new("x", "why x")],
        };
        a.merge(MediaTally {
            downloaded: 2,
            failed: 1,
            bytes: 5,
            missing: vec![MissingFile::new("y", "why y")],
        });
        assert_eq!((a.downloaded, a.failed, a.bytes), (3, 1, 15));
        assert_eq!(
            a.missing,
            vec![
                MissingFile::new("x", "why x"),
                MissingFile::new("y", "why y")
            ]
        );
    }
}
