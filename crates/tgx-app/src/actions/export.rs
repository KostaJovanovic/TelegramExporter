//! Running an export: resolving a chat's topics, driving the engine, and
//! reporting what the run actually produced.
//!
//! **The queue owns what a run did.** Nothing here composes its own summary,
//! because the second writer always wins a race you did not know you had: a
//! worker's cheerful "Exported 3 of 3 chats" once landed on top of "Stopped" a
//! moment after the user pressed it.
//!
//! Topic discovery is separated from the export itself because a chat that
//! refuses its topic list must still export as one folder rather than ending
//! the queue.

use super::*;

/// What discovering one chat's topics decided.
pub(super) enum TopicsResolution {
    /// Genuinely split, with the topics Telegram returned.
    Split(Vec<dialogs::Topic>),
    /// A real refusal, not a rate limit: there is no shape to preserve, so the
    /// chat exports as one folder, the same answer a chat that was never a
    /// forum gets.
    Unsplit,
    /// Still rate-limited after the one retry [`resolve_topics`] gives it.
    /// **Not** the same answer as [`Unsplit`](Self::Unsplit) — a forum stays
    /// a forum, it just cannot be exported this run.
    RateLimited,
    /// Cancelled while waiting out the rate limit.
    Cancelled,
}

/// Resolve a forum's topics, waiting out one rate limit before giving up on
/// it — the same patience `dialogs::count_all` gives a single chat.
///
/// **Splitting by topic is this app's entire reason to exist.** Before this,
/// `export` caught *any* error from `list_topics`, `Transient` included, and
/// fell back to `vec![Topic::general()]` — so a `FloodWait` during discovery
/// silently turned a forum into a single folder, the one shape this app was
/// built not to produce. A genuine refusal has no shape left to preserve and
/// still degrades; a rate limit does not get to make that call.
///
/// `fetch` stands in for `dialogs::list_topics`, which needs a live socket to
/// answer at all — this way the distinction above can be driven with a canned
/// [`EnrichError`] in a test instead.
pub(super) async fn resolve_topics<F, Fut>(
    chat_title: &str,
    chat_id: i64,
    cancel: &Cancel,
    tx: &Events,
    mut fetch: F,
) -> TopicsResolution
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<dialogs::Topic>, EnrichError>>,
{
    let mut retried = false;
    loop {
        match fetch().await {
            Ok(t) => return TopicsResolution::Split(t),
            Err(e) if e.is_transient() && !retried => {
                retried = true;
                let secs = e.retry_after().unwrap_or_default();
                let _ = tx.send(Event::FloodWait(secs.as_secs()));
                // Sliced and cancellable: a click on Stop during the wait must
                // not be held for the whole of Telegram's two minutes.
                tgx_tg::engine::sleep_in_slices_until(secs, cancel).await;
                if cancel.is_cancelled() {
                    return TopicsResolution::Cancelled;
                }
            }
            Err(e) if e.is_transient() => {
                // The retry above already spent this chat's share of the
                // queue's patience; a second wait would hold up every chat
                // behind it for the sake of one Telegram is still throttling.
                let _ = tx.send(Event::ChatFailed {
                    chat_id,
                    message: "rate limited while listing topics — retry this \
                              chat later rather than lose its topic split"
                        .into(),
                });
                return TopicsResolution::RateLimited;
            }
            Err(e) => {
                let _ = tx.send(Event::Warn(format!(
                    "{chat_title}: could not list topics ({e}); exporting as one folder"
                )));
                return TopicsResolution::Unsplit;
            }
        }
    }
}

/// Export every selected chat, in queue order.
///
/// **Per-chat tallies live on the result, never on a shared counter** — see
/// `tgx_tg::engine::ExportResult`. This function keeps nothing across chats
/// except the queue position.
pub async fn export(
    settings: Settings,
    chats: Vec<tgx_tg::client::ChatInfo>,
    cancel: Cancel,
    tx: Events,
) {
    use tgx_tg::engine::{ChatExporter, Progress};

    let session = match Session::connect(&settings).await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(Event::Failed {
                activity: Activity::Export,
                message: e.to_string(),
            });
            let _ = tx.send(Event::Finished { stopped: true });
            return;
        }
    };
    if !ready(&session, &tx, Activity::Export).await {
        let _ = tx.send(Event::Finished { stopped: true });
        return;
    }

    // **One dialog sweep for the whole queue**, before any message is fetched.
    // Resolving each chat inside the loop paged the entire dialog list once per
    // chat — twenty full sweeps for a twenty-chat queue, to answer twenty
    // questions that one response already contains, which is exactly the
    // pattern that earns a flood wait before the export has written a byte.
    let ids: Vec<i64> = chats.iter().map(|c| c.id).collect();
    let peers = match dialogs::peer_refs_for(&session.client, &ids, &cancel).await {
        Ok(p) => p,
        Err(e) => {
            // A transport failure here is not "these chats are gone" — that
            // distinction is the whole reason `peer_refs_for` returns a
            // `Result`, and reporting it per chat would tell the user to go
            // looking for conversations they still have.
            let _ = tx.send(Event::Failed {
                activity: Activity::Export,
                message: format!("listing chats: {e}"),
            });
            let _ = tx.send(Event::Finished { stopped: true });
            return;
        }
    };

    let mut exporter = ChatExporter::new(&session.client, &settings, session.session());

    for chat in &chats {
        if cancel.is_cancelled() {
            break;
        }
        let _ = tx.send(Event::ChatStarted {
            chat_id: chat.id,
            title: chat.title.clone(),
        });

        let Some(peer) = peers.get(&chat.id).copied() else {
            let _ = tx.send(Event::ChatFailed {
                chat_id: chat.id,
                message: "no longer in the dialog list".into(),
            });
            continue;
        };

        // Whether this chat is genuinely split into topic folders. Held rather
        // than recomputed, because it decides both what the engine does and
        // what the queue is allowed to claim afterwards — and a topic count
        // reported for a chat that was never split is a wrong number that
        // reads as a right one.
        let mut split = chat.is_forum && settings.split_topics;
        let topics = if split {
            match resolve_topics(&chat.title, chat.id, &cancel, &tx, || {
                dialogs::list_topics(&session.client, peer)
            })
            .await
            {
                TopicsResolution::Split(t) => t,
                // A genuine refusal, not a rate limit: there is no shape to
                // preserve, so this one chat degrades to a single folder —
                // the same answer a chat that was never a forum gets.
                TopicsResolution::Unsplit => {
                    split = false;
                    vec![dialogs::Topic::general()]
                }
                // Still rate-limited after the one retry, or the wait for it
                // was cancelled. Neither is "this chat has no topics", so
                // neither may fall into the branch above and export a forum
                // as one folder — `resolve_topics` has already said which one
                // happened.
                TopicsResolution::RateLimited => continue,
                TopicsResolution::Cancelled => break,
            }
        } else {
            vec![dialogs::Topic::general()]
        };
        if split {
            let _ = tx.send(Event::ChatTopics {
                chat_id: chat.id,
                topics: topics.len(),
            });
        }

        let root = match tgx_tg::engine::unique_dir(
            std::path::Path::new(&settings.output_dir),
            &chat.title,
        ) {
            Ok(r) => r,
            Err(e) => {
                // The destination is free text and may be unwritable,
                // disconnected or invalid. That ends the whole queue, not
                // one chat: every later chat would fail the same way.
                let _ = tx.send(Event::Failed {
                    activity: Activity::Export,
                    message: format!("cannot write into {}: {e}", settings.output_dir),
                });
                let _ = tx.send(Event::Finished { stopped: true });
                return;
            }
        };
        let _ = tx.send(Event::Log(format!(
            "{}: writing to {}",
            chat.title,
            root.display()
        )));

        let chat_id = chat.id;
        let tx2 = tx.clone();
        let mut on_progress = move |p: Progress| match p {
            Progress::Messages { done, total, .. } => {
                let _ = tx2.send(Event::Progress {
                    chat_id,
                    done,
                    total,
                });
            }
            Progress::Total { total, .. } => {
                let _ = tx2.send(Event::ChatTotal { chat_id, total });
            }
            Progress::FloodWait { seconds } => {
                let _ = tx2.send(Event::FloodWait(seconds));
            }
            Progress::Topic { title, messages } => {
                log::info!("{title}: {messages}");
                let _ = tx2.send(Event::Log(format!("  {title}: {messages}")));
            }
            Progress::Log(msg) => {
                // **Both channels.** The window's transcript is a 2,000-line
                // ring that dies with the process; `tgx.log` survives it and is
                // the only thing left to read when someone asks the next day
                // why an export was short.
                log::info!("{msg}");
                let _ = tx2.send(Event::Log(format!("  {msg}")));
            }
            // Deliberately not sent to the window: one chat of six thousand
            // messages would flush every other line out of the ring, including
            // the INCOMPLETE warning the ring exists to preserve.
            Progress::Detail(msg) => log::debug!("{msg}"),
        };

        match exporter
            .run(chat, peer, &topics, &root, &mut on_progress, &cancel)
            .await
        {
            Ok(result) => {
                let _ = tx.send(Event::ChatDone {
                    chat_id,
                    messages: result.messages,
                    expected: result.expected,
                    // `result.topics` counts output folders, and an unsplit
                    // chat has exactly one. Only a chat that really was split
                    // has a topic count to report.
                    topics: split.then_some(result.topics),
                    media_downloaded: result.media_downloaded,
                    media_failed: result.media_failed,
                    root: result.root.clone(),
                });
                report_result(&tx, &chat.title, &result, split);
            }
            Err(tgx_tg::ExportError::Cancelled) => {
                // Not a failure. The user asked it to stop, and whatever was
                // written is closed and valid — `close_all` ran on the way out.
                let _ = tx.send(Event::Log(format!("{}: stopped", chat.title)));
                break;
            }
            Err(e) => {
                let _ = tx.send(Event::ChatFailed {
                    chat_id,
                    message: e.to_string(),
                });
            }
        }
    }

    let _ = tx.send(Event::Finished {
        stopped: cancel.is_cancelled(),
    });
}

/// Everything a finished chat has to admit to.
///
/// Each of these was counted and then never shown at some point in the
/// original, which is the same as not having counted it.
fn report_result(tx: &Events, title: &str, result: &tgx_tg::engine::ExportResult, split: bool) {
    let mb = result.bytes_downloaded as f64 / (1024.0 * 1024.0);
    let mut line = format!("{title}: {} messages", result.messages);
    // Only a chat that was split has topic folders. `result.topics` counts
    // output folders, and an unsplit chat has one — which read as
    // "1 topic folders" on every private chat.
    if split && result.topics > 0 {
        line.push_str(&format!(", {} topic folders", result.topics));
        if result.empty_topics > 0 {
            line.push_str(&format!(" ({} empty skipped)", result.empty_topics));
        }
    }
    line.push_str(&format!(", {} files ({mb:.1} MB)", result.media_downloaded));
    let _ = tx.send(Event::Log(line));

    // **A short export must not read like a complete one.** A crash at message
    // 5,609 of 6,600 produced a cheerful summary and a thousand missing
    // messages, and nothing on screen distinguished it from a whole export.
    if !result.complete() {
        let _ = tx.send(Event::Warn(format!(
            "{title}: INCOMPLETE — Telegram counted {}, {} came through",
            result.expected, result.messages
        )));
    }
    // `media_missing`, not `media_failed`: this line names a number and points
    // at a file, so it has to be the number of lines in that file. One failed
    // job takes its thumbnail and its preview with it, and the run reported 21
    // over a missing_media.txt that listed 42.
    if result.media_missing > 0 {
        let _ = tx.send(Event::Warn(format!(
            "{title}: {} files could not be fetched — see missing_media.txt",
            result.media_missing
        )));
    }
    if result.members > 0 && !result.members_complete {
        // A truncated roster that looks complete is worse than no roster:
        // wrong data that reads as right.
        let _ = tx.send(Event::Warn(format!(
            "{title}: the member list is INCOMPLETE — see participants.json"
        )));
    }
    if result.enrich_deferred > 0 {
        let _ = tx.send(Event::Warn(format!(
            "{title}: {} extra lookups were lost to rate limits — reaction \
             names, poll results or custom emoji may be missing. Re-exporting \
             this chat later will fill them in.",
            result.enrich_deferred
        )));
    }
    if !result.degraded.is_empty() {
        let _ = tx.send(Event::Log(format!(
            "{title}: Telegram data this build has no mapping for, written as \
             text: {}",
            result.degraded.join(", ")
        )));
    }
}
