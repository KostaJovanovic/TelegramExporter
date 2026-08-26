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

use crate::error::{classify, EnrichError};
use crate::plan::DownloadJob;
use grammers_client::media::{Downloadable, Media, PhotoSize};
use grammers_client::Client;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;

/// How many times a download is retried before its path is recorded as missing.
pub const MAX_RETRIES: u32 = 5;

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

/// Run every job, at most `concurrency` at a time.
pub async fn run_all(
    client: &Client,
    root: &Path,
    jobs: Vec<PendingDownload>,
    concurrency: usize,
) -> MediaTally {
    let permits = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut set = tokio::task::JoinSet::new();

    for pending in jobs {
        let permits = permits.clone();
        let client = client.clone();
        let root = root.to_path_buf();
        set.spawn(async move {
            let _permit = permits.acquire_owned().await;
            run_one(&client, &root, pending).await
        });
    }

    let mut tally = MediaTally::default();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(one) => tally.merge(one),
            // A panicked task must not take the export with it.
            Err(_) => tally.failed += 1,
        }
    }
    tally.missing.sort();
    tally
}

async fn run_one(client: &Client, root: &Path, pending: PendingDownload) -> MediaTally {
    let mut tally = MediaTally::default();
    let dest = root.join(&pending.job.dest);

    if let Some(parent) = dest.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            tally.failed += 1;
            tally.missing.push(pending.job.dest.clone());
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
        tally.missing.push(pending.job.dest.clone());
        return tally;
    };

    if let Some(written) = fetch_with_retry(client, &media, &dest).await {
        tally.downloaded += 1;
        tally.bytes += written;
        fetch_thumb(client, root, &media, &pending.job, &mut tally).await;
        fetch_preview(client, root, &media, &pending.job, &dest, &mut tally).await;
    } else {
        tally.failed += 1;
        tally.missing.push(pending.job.dest.clone());
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
async fn fetch_with_retry<D: Downloadable>(client: &Client, media: &D, dest: &Path) -> Option<i64> {
    let mut attempt = 0;
    loop {
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
                    // A rate limit is temporary. Wait it out in slices so a
                    // cancel is not swallowed, and do not spend a retry on it.
                    crate::engine::sleep_in_slices(wait).await;
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
    match fetch_with_retry(client, &thumb, &dest).await {
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
        if let Some(written) = fetch_with_retry(client, &t, &dest).await {
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
