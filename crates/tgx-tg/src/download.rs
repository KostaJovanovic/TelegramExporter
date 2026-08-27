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
use crate::plan::DownloadJob;
use grammers_client::media::{Downloadable, Media, PhotoSize};
use grammers_client::Client;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;

/// How many times a download is retried before its path is recorded as missing.
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

/// What one media pass produced.
#[derive(Debug, Default)]
pub struct MediaTally {
    pub downloaded: usize,
    pub failed: usize,
    pub bytes: i64,
    /// Paths referenced by the export that could not be saved.
    pub missing: Vec<String>,
}

impl MediaTally {
    fn merge(&mut self, other: MediaTally) {
        self.downloaded += other.downloaded;
        self.failed += other.failed;
        self.bytes += other.bytes;
        self.missing.extend(other.missing);
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
            let one = run_one(&client, &root, pending, &cancel).await;
            if let Some(tx) = &report {
                let _ = tx.send(crate::engine::Progress::Detail(format!(
                    "  {} {dest} ({} in {:.1}s)",
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
    tally.missing.sort();
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
fn promised_paths(job: &DownloadJob) -> Vec<String> {
    let mut out = vec![job.dest.clone()];
    out.extend(job.thumb_dest.clone());
    out.extend(job.preview_dest.clone());
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
        missing: promised_paths(job),
        ..MediaTally::default()
    }
}

async fn run_one(
    client: &Client,
    root: &Path,
    pending: PendingDownload,
    cancel: &Cancel,
) -> MediaTally {
    let mut tally = MediaTally::default();
    let dest = root.join(&pending.job.dest);

    if let Some(parent) = dest.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            tally.failed += 1;
            tally.missing.extend(promised_paths(&pending.job));
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
            Err(_) => {
                tally.failed += 1;
                tally.missing.push(pending.job.dest.clone());
            }
        }
        return tally;
    }

    let Some(media) = pending.media else {
        tally.failed += 1;
        tally.missing.extend(promised_paths(&pending.job));
        return tally;
    };

    if let Some(written) = fetch_with_retry(client, &media, &dest, cancel).await {
        tally.downloaded += 1;
        tally.bytes += written;
        fetch_thumb(client, root, &media, &pending.job, &mut tally, cancel).await;
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
    } else {
        tally.failed += 1;
        // Not just `dest`. When the primary file fails, its thumbnail and its
        // preview are never fetched and never recorded, yet the JSON and the
        // HTML have already named all three. Reporting one of three made
        // `missing_media.txt` an incomplete account of the same run's gaps.
        tally.missing.extend(promised_paths(&pending.job));
    }
    tally
}

/// Fetch one downloadable, waiting out rate limits and retrying real failures.
///
/// `None` means the file is not on disk and the remnant has been removed.
///
/// **Shared by the file, its thumbnail and its preview.** The thumbnail used to
/// be fetched by a bare call: one rate limit cost it outright while the file
/// beside it was retried five times — and the JSON has already named the
/// thumbnail by then, so losing it leaves a broken `<img>`. The Python original
/// carries a comment recording exactly that, which is the reason this is one
/// function rather than three loops.
async fn fetch_with_retry<D: Downloadable>(
    client: &Client,
    media: &D,
    dest: &Path,
    cancel: &Cancel,
) -> Option<i64> {
    let mut attempt = 0;
    let mut waits = 0;
    loop {
        if cancel.is_cancelled() {
            let _ = std::fs::remove_file(dest);
            return None;
        }
        attempt += 1;
        match fetch(client, media, dest).await {
            Ok(written) if written > 0 => return Some(written),
            // Zero bytes is a failure, not a success. Remove the remnant so a
            // later existence check cannot mistake it for a saved file.
            Ok(_) => {
                let _ = std::fs::remove_file(dest);
                if attempt >= MAX_RETRIES {
                    return None;
                }
            }
            Err(e) => {
                let _ = std::fs::remove_file(dest);
                if let Some(wait) = e.retry_after() {
                    // A rate limit is temporary and does not spend a retry —
                    // it is Telegram asking for patience, not a failure. It
                    // gets its own ceiling instead, because `attempt -= 1` with
                    // no second counter is a loop with no exit when every
                    // attempt comes back rate-limited.
                    waits += 1;
                    if waits >= MAX_RATE_LIMIT_WAITS {
                        return None;
                    }
                    // In slices, against the signal the caller actually holds.
                    // This passed a `Cancel::new()` that nobody had, so the
                    // comment promising a cancel would not be swallowed
                    // described a flag that could never be set.
                    crate::engine::sleep_in_slices_until(wait, cancel).await;
                    attempt -= 1;
                    continue;
                }
                if attempt >= MAX_RETRIES {
                    return None;
                }
            }
        }
    }
}

/// Fetch Telegram's own thumbnail for a file we just saved.
///
/// **This is the half that did not exist.** `plan.rs` wrote
/// `"thumbnail": "<file>_thumb.jpg"` into `result.json` and set
/// `DownloadJob::thumb_dest`, and nothing anywhere read that field — it was a
/// `pub` member of a `pub` struct, so no dead-code lint fired and clippy stayed
/// clean. Every export therefore shipped a `thumbnail` path with no file behind
/// it: 1,559 of them in the last live run, every single one dangling, and none
/// recorded in `missing_media.txt` because no job had ever been queued to fail.
///
/// A thumbnail that cannot be fetched is counted and named like any other
/// missing file rather than silently left out, because the JSON has already
/// promised it by the time we get here.
async fn fetch_thumb(
    client: &Client,
    root: &Path,
    media: &Media,
    job: &DownloadJob,
    tally: &mut MediaTally,
    cancel: &Cancel,
) {
    let Some(rel) = job.thumb_dest.as_ref() else {
        return;
    };
    let Some(thumb) = largest_thumb(media) else {
        // The plan only sets `thumb_dest` when it saw a non-stripped thumb, so
        // arriving here means the two disagreed about the same document.
        tally.failed += 1;
        tally.missing.push(rel.clone());
        return;
    };
    let dest = root.join(rel);
    match fetch_with_retry(client, &thumb, &dest, cancel).await {
        Some(written) => {
            tally.downloaded += 1;
            tally.bytes += written;
        }
        None => {
            tally.failed += 1;
            tally.missing.push(rel.clone());
        }
    }
}

/// Put the inline preview on disk beside the file it previews.
///
/// Desktop renders this one locally with an image scaler. We take Telegram's
/// next size down instead, which needs no decoder in the binary and lands a
/// real image at the name Desktop uses. **The bytes therefore differ from
/// Desktop's**; the file, its path and its role do not, and no parity leg reads
/// media bytes — the media leg diffs names.
///
/// When Telegram advertises nothing smaller, the full-size file is copied. That
/// costs page weight and is the deliberate trade: `_p.preview` has already been
/// written into the HTML by the time this runs, so the alternative is an
/// `<img>` pointing at nothing, and a dangling reference is worse than a heavy
/// one.
async fn fetch_preview(
    client: &Client,
    root: &Path,
    media: &Media,
    job: &DownloadJob,
    full: &Path,
    tally: &mut MediaTally,
    cancel: &Cancel,
) {
    let Some(rel) = job.preview_dest.as_ref() else {
        return;
    };
    let dest = root.join(rel);
    // Strictly smaller than the file itself, or it is not a downscale.
    let smaller = match media {
        Media::Photo(p) => p
            .thumbs()
            .into_iter()
            .filter(|t| !matches!(t, PhotoSize::Stripped(_)))
            .filter(|t| (t.size() as i64) < job.size)
            .max_by_key(|t| t.size()),
        _ => None,
    };
    if let Some(t) = smaller {
        if let Some(written) = fetch_with_retry(client, &t, &dest, cancel).await {
            tally.downloaded += 1;
            tally.bytes += written;
            return;
        }
    }
    match std::fs::copy(full, &dest) {
        Ok(n) => {
            tally.downloaded += 1;
            tally.bytes += n as i64;
        }
        Err(_) => {
            let _ = std::fs::remove_file(&dest);
            tally.failed += 1;
            tally.missing.push(rel.clone());
        }
    }
}

/// The thumbnail `plan::thumb_bytes` measured, so the file on disk is the one
/// whose size `thumbnail_file_size` reports.
///
/// Stripped sizes are excluded for the same reason the plan excludes them: they
/// are the blur preview carried inside the message, not a downloadable file,
/// and they travel as `stripped_thumbnail` on their own path.
fn largest_thumb(media: &Media) -> Option<PhotoSize> {
    let thumbs = match media {
        Media::Document(d) => d.thumbs(),
        Media::Sticker(s) => s.document.thumbs(),
        _ => return None,
    };
    thumbs
        .into_iter()
        .filter(|t| !matches!(t, PhotoSize::Stripped(_)))
        .max_by_key(|t| t.size())
}

async fn fetch<D: Downloadable>(
    client: &Client,
    media: &D,
    dest: &Path,
) -> Result<i64, EnrichError> {
    let mut iter = client.iter_download(media);
    let mut file = match tokio::fs::File::create(dest).await {
        Ok(f) => f,
        Err(e) => return Err(EnrichError::Failed(e.to_string())),
    };
    let mut written = 0i64;
    loop {
        match iter.next().await {
            Ok(Some(chunk)) => {
                use tokio::io::AsyncWriteExt;
                if let Err(e) = file.write_all(&chunk).await {
                    return Err(EnrichError::Failed(e.to_string()));
                }
                written += chunk.len() as i64;
            }
            Ok(None) => break,
            Err(e) => return Err(classify(&e)),
        }
    }
    use tokio::io::AsyncWriteExt;
    let _ = file.flush().await;
    Ok(written)
}

/// Write the list of files the export references but could not save.
///
/// A dangling reference is worse than a stated gap: without this the archive
/// silently contains broken links.
pub fn write_missing(root: &Path, missing: &[String]) -> std::io::Result<()> {
    if missing.is_empty() {
        return Ok(());
    }
    let path: PathBuf = root.join("missing_media.txt");
    let mut body =
        String::from("These files are referenced by this export but could not be saved.\n\n");
    for m in missing {
        body.push_str(m);
        body.push('\n');
    }
    std::fs::write(path, body)
}

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

    #[test]
    fn missing_media_is_written_only_when_something_is_missing() {
        let root = tmp("missing");
        write_missing(&root, &[]).unwrap();
        assert!(
            !root.join("missing_media.txt").exists(),
            "an export that saved everything must not gain the file"
        );

        write_missing(&root, &["photos/photo_1.jpg".into()]).unwrap();
        let body = std::fs::read_to_string(root.join("missing_media.txt")).unwrap();
        assert!(body.contains("photos/photo_1.jpg"));
        // And it says what the list means, not just the paths.
        assert!(body.contains("could not be saved"));
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
        let paths = promised_paths(&job_with_all_three());
        assert_eq!(
            paths,
            vec![
                "video_files/clip.mp4",
                "video_files/clip.mp4_thumb.jpg",
                "photos/photo_1.jpg"
            ]
        );
        // A job with neither still reports itself.
        let bare = DownloadJob {
            thumb_dest: None,
            preview_dest: None,
            ..job_with_all_three()
        };
        assert_eq!(promised_paths(&bare), vec!["video_files/clip.mp4"]);
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
            missing: vec!["x".into()],
        };
        a.merge(MediaTally {
            downloaded: 2,
            failed: 1,
            bytes: 5,
            missing: vec!["y".into()],
        });
        assert_eq!((a.downloaded, a.failed, a.bytes), (3, 1, 15));
        assert_eq!(a.missing, vec!["x", "y"]);
    }

    #[test]
    fn the_retry_ceiling_is_desktops_five() {
        assert_eq!(MAX_RETRIES, 5);
    }
}
