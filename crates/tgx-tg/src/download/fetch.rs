//! Fetching one file: the retry ladder, the file-reference refresh, and the
//! three artifacts a single job can owe.
//!
//! **A rate limit is not a failure.** Waiting it out and retrying is the whole
//! difference between a complete export and a thousand missing files, and the
//! typed errors in `error.rs` are what keep the two apart.
//!
//! A file reference can only be replaced by re-reading the message that
//! carried it, which is why `refreshed_media` needs the peer.

use super::*;

/// Fetch one downloadable, waiting out rate limits and retrying real failures.
///
/// `Err` means the file is not on disk and the remnant has been removed; it
/// carries the reason, which is the whole point of it being a `Result` and not
/// the `Option` it used to be. See [`MissingFile`].
///
/// **Shared by the file, its thumbnail and its preview.** The thumbnail used to
/// be fetched by a bare call: one rate limit cost it outright while the file
/// beside it was retried five times — and the JSON has already named the
/// thumbnail by then, so losing it leaves a broken `<img>`. That is the reason
/// this is one function rather than three loops.
pub(crate) async fn fetch_with_retry<D: Downloadable>(
    client: &Client,
    media: &D,
    dest: &Path,
    cancel: &Cancel,
) -> Result<i64, FetchFailure> {
    let mut attempt = 0;
    let mut waits = 0;
    loop {
        if cancel.is_cancelled() {
            let _ = std::fs::remove_file(dest);
            return Err(FetchFailure::final_(STOPPED));
        }
        attempt += 1;
        match fetch(client, media, dest).await {
            Ok(written) if written > 0 => return Ok(written),
            // Zero bytes is a failure, not a success. Remove the remnant so a
            // later existence check cannot mistake it for a saved file.
            Ok(_) => {
                let _ = std::fs::remove_file(dest);
                if attempt >= MAX_RETRIES {
                    return Err(FetchFailure::final_(format!(
                        "Telegram sent no bytes, in {attempt} attempts"
                    )));
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
                        return Err(FetchFailure::final_(format!(
                            "still rate limited after {waits} waits"
                        )));
                    }
                    // In slices, against the signal the caller actually holds.
                    // This passed a `Cancel::new()` that nobody had, so the
                    // comment promising a cancel would not be swallowed
                    // described a flag that could never be set.
                    crate::engine::sleep_in_slices_until(wait, cancel).await;
                    attempt -= 1;
                    continue;
                }
                // A stale reference answers the same way however often it is
                // asked, so the remaining attempts would buy nothing but load
                // — and there were a great many of them: roughly 7,000 files
                // × 5 attempts of guaranteed refusal in one chat, which is a
                // fair way to earn the rate limits that run also reported.
                // Hand it straight back and let the caller re-read the message.
                if e.is_stale() {
                    return Err(FetchFailure {
                        reason: e.to_string(),
                        stale: true,
                    });
                }
                if attempt >= MAX_RETRIES {
                    return Err(FetchFailure::final_(format!("{e}, in {attempt} attempts")));
                }
            }
        }
    }
}

/// Re-read the message and take the file reference it answers with.
///
/// **The only cure for [`EnrichError::Stale`], and it is a cure.** A
/// `file_reference` is a short-lived blob Telegram issues alongside the media
/// and expects handed back; it cannot be renewed in place, only re-obtained,
/// and re-obtaining it means asking for the message again. One extra request
/// buys back a file that five retries could not.
///
/// Derived through [`plan::downloadable`] rather than `Message::media`, for
/// exactly the reason `engine::payload` uses it on the way in: the two differ
/// on a link preview, and the second hands back a `Media::WebPage` that
/// grammers refuses to download. Using the same function on both passes is also
/// what guarantees the refreshed handle points at the file the JSON named.
///
/// **The refreshed file must be the file the JSON already described.** By the
/// time this runs, `result.json` and the HTML have published this job's name and
/// its `file_size`, taken from the media seen during the read. A message edited
/// in between would hand back a *different* file under the same name, and the
/// archive would then state a size it does not hold — so the size is checked
/// against the plan's, using [`plan::classify`], the same function that produced
/// the published number. A mismatch is refused: a stated gap is worse than
/// nothing only if it is silent, and a file that misdescribes itself is worse
/// than either.
///
/// `Err` carries what to say about it, because this file's `missing_media.txt`
/// line is the only account anyone gets.
pub(crate) async fn refreshed_media(
    client: &Client,
    refresh: Refresh,
    job: &DownloadJob,
) -> Result<Media, String> {
    let id = job.message_id;
    let fetched = client
        .get_messages_by_id(refresh.peer, &[id as i32])
        .await
        .map_err(|e| format!("and message {id} could not be re-read: {}", classify(&e)))?;
    let Some(msg) = fetched.into_iter().next().flatten() else {
        return Err(format!("and message {id} is no longer there to re-read"));
    };
    let tl::enums::Message::Message(m) = &msg.raw else {
        return Err(format!("and message {id} came back empty"));
    };
    handle_for(m.media.as_ref(), job, refresh.link_previews)
}

/// Whether a re-read message's media may stand in for the job's stale handle.
///
/// **Split out from the request so the guard can be tested without one.** It is
/// the half that can be wrong in a way no live run would advertise: a
/// comparison that never matches turns the whole refresh into an expensive
/// no-op, and the only symptom would be another export losing a fifth of its
/// media two hours later.
pub(crate) fn handle_for(
    media: Option<&tl::enums::MessageMedia>,
    job: &DownloadJob,
    link_previews: bool,
) -> Result<Media, String> {
    let id = job.message_id;
    let Some(media) = media else {
        return Err(format!("and message {id} no longer carries any media"));
    };
    match plan::classify(media, link_previews) {
        Some(facts) if facts.size != job.size => Err(format!(
            "and message {id} now carries a different file ({} bytes, not the {} \
             already published for this name)",
            facts.size, job.size
        )),
        Some(_) => plan::downloadable(media, link_previews)
            .ok_or_else(|| format!("and message {id} carries media nothing can download")),
        None => Err(format!(
            "and message {id} no longer carries a file the plan would fetch"
        )),
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
pub(crate) async fn fetch_thumb(
    client: &Client,
    root: &Path,
    media: &Media,
    job: &DownloadJob,
    refresh: Refresh,
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
        tally.missing.push(MissingFile::new(
            rel.clone(),
            "the message carries no thumbnail the plan could have named",
        ));
        return;
    };
    let dest = root.join(rel);
    let mut outcome = fetch_with_retry(client, &thumb, &dest, cancel).await;
    if outcome.as_ref().err().is_some_and(|f| f.stale) {
        // Its own pass, not a share of the primary's. A thumbnail carries the
        // same document's reference, so one fetched near the end of a long
        // batch can go stale even though the file it belongs to arrived — and
        // the JSON promised this path before either was attempted.
        match refreshed_media(client, refresh, job).await {
            Ok(fresh) => {
                if let Some(t) = largest_thumb(&fresh) {
                    outcome = fetch_with_retry(client, &t, &dest, cancel).await;
                }
            }
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
        }
        Err(f) => {
            tally.failed += 1;
            tally.missing.push(MissingFile::new(rel.clone(), f.reason));
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
pub(crate) async fn fetch_preview(
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
        if let Ok(written) = fetch_with_retry(client, &t, &dest, cancel).await {
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
        Err(e) => {
            let _ = std::fs::remove_file(&dest);
            tally.failed += 1;
            tally.missing.push(MissingFile::new(
                rel.clone(),
                format!("no smaller size, and the full-size copy failed: {e}"),
            ));
        }
    }
}

/// The thumbnail `plan::thumb_bytes` measured, so the file on disk is the one
/// whose size `thumbnail_file_size` reports.
///
/// Stripped sizes are excluded for the same reason the plan excludes them: they
/// are the blur preview carried inside the message, not a downloadable file,
/// and they travel as `stripped_thumbnail` on their own path.
pub(crate) fn largest_thumb(media: &Media) -> Option<PhotoSize> {
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

pub(crate) async fn fetch<D: Downloadable>(
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
///
/// **A path is always a whole line, and a reason never is.** The reason goes on
/// its own *indented* continuation line rather than beside the path, because a
/// Telegram filename may contain any separator one might pick — an em dash, a
/// colon, a bracket — and `tgx_parity::wire_leg::stated_gaps` reads this file to
/// tell a declared gap from a dangling reference. Leading whitespace is the one
/// thing a path here can never start with, so the split cannot be ambiguous.
/// Putting the reasons in the transcript instead was the alternative, and it
/// loses every gap past the twentieth: the transcript is capped, this is not.
pub fn write_missing(root: &Path, missing: &[MissingFile]) -> std::io::Result<()> {
    if missing.is_empty() {
        return Ok(());
    }
    let path: PathBuf = root.join("missing_media.txt");
    let mut body =
        String::from("These files are referenced by this export but could not be saved.\n\n");
    for m in missing {
        body.push_str(&m.path);
        body.push('\n');
        body.push_str("    ");
        body.push_str(&m.reason);
        body.push('\n');
    }
    std::fs::write(path, body)
}
