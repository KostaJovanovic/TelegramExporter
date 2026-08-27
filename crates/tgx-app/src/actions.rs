//! What the nav bar's cells actually do.
//!
//! Every one of these submits to the tokio side and returns immediately — **the
//! UI thread never blocks on a future.** Results come back as
//! [`Event`](crate::bridge::Event)s, which wake the window rather than waiting
//! to be found on its next frame.

use crate::bridge::{Activity, Event, Events};
use tgx_tg::cancel::Cancel;
use tgx_tg::client::{LoginStep, Session};
use tgx_tg::config::Settings;
use tgx_tg::dialogs;
use tgx_tg::error::EnrichError;

/// Connect and report whether the session is already authorised.
///
/// **Reaching Telegram without being signed in is a success, not an error** —
/// the caller opens the sign-in dialog on it. Conflating the two is what put
/// two modal dialogs on top of each other in the original, which the user
/// experienced as the app freezing the moment it logged them in.
pub async fn sign_in(settings: Settings, tx: Events) {
    let _ = tx.send(Event::Status("Connecting…".into()));
    let session = match Session::connect(&settings).await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(Event::Failed {
                activity: Activity::SignIn,
                message: e.to_string(),
            });
            return;
        }
    };
    // **Asked, not read from the cache.** This is the one call whose whole job
    // is to re-check the stored session, and a cached answer would make the
    // button do nothing visible. Every *other* action uses `ensure_connected`,
    // which reuses what this learned.
    match session.refresh_authorization().await {
        Ok(true) => match session.me().await {
            Ok(name) => {
                let _ = tx.send(Event::SignedIn(name));
            }
            Err(e) => {
                let _ = tx.send(Event::Failed {
                    activity: Activity::SignIn,
                    message: e.to_string(),
                });
            }
        },
        Ok(false) => {
            // Reached Telegram, not signed in yet.
            let _ = tx.send(Event::Status("Not signed in".into()));
        }
        Err(e) => {
            let _ = tx.send(Event::Failed {
                activity: Activity::SignIn,
                message: e.to_string(),
            });
        }
    }
}

/// What every action that needs an authorised account does first.
///
/// Two failures used to look the same from here and neither said what it was.
/// A blocked network hung in the operating system's TCP retry, because nothing
/// in the stack had a timeout; and an unauthorised account got as far as its
/// real request and came back with a wire error from the middle of a chat
/// listing. `ensure_connected` bounds the first and names the second, and it
/// only costs a round trip the first time — the answer is cached on the shared
/// connection.
async fn ready(session: &Session, tx: &Events, activity: Activity) -> bool {
    match session.ensure_connected().await {
        Ok(true) => true,
        Ok(false) => {
            let _ = tx.send(Event::Failed {
                activity,
                message: "Not signed in. Press 01 Sign in first.".into(),
            });
            false
        }
        Err(e) => {
            let _ = tx.send(Event::Failed {
                activity,
                message: e.to_string(),
            });
            false
        }
    }
}

/// Say so if the folder holding the session key is at default permissions.
///
/// `ensure_data_dir` must not stop the app starting, so it records the failure
/// instead of raising it — which previously meant the folder holding a **bearer
/// credential** could be readable by anyone on the machine with nothing
/// anywhere saying so. `tgx_tg::config::lockdown_error` exists precisely so a
/// caller that shows the user a security claim can check whether it is true;
/// until now nothing called it and an ACL failure reached only `log::warn!`.
pub fn report_data_dir_protection(tx: &Events) {
    let dir = match tgx_tg::config::ensure_data_dir() {
        Ok(dir) => dir,
        Err(e) => {
            let _ = tx.send(Event::Warn(format!(
                "could not create the data folder: {e}. Signing in will not work."
            )));
            return;
        }
    };
    if let Some(why) = tgx_tg::config::lockdown_error() {
        let _ = tx.send(Event::Warn(format!(
            "Could not restrict {} to your user ({why}). Anyone with access to \
             this machine or drive can read the saved session and sign in as you.",
            dir.display()
        )));
    }
}

/// The sign-in in progress, held between the steps the user drives.
///
/// **The login token cannot be re-derived, and the code cannot be re-requested
/// without invalidating itself.** Telegram treats a second `auth.sendCode` as
/// starting the authorisation over, so asking again — which is what this used
/// to do, once per step — cancelled the code the user was in the middle of
/// typing and eventually answered `AUTH_RESTART`. Each step runs as its own
/// task, so the session lives here rather than on a stack frame.
///
/// **The connection is no longer what is being held here** — that is shared and
/// outlives any one step, see `tgx_tg::client`. What lives on this `Session` and
/// nowhere else is the login token, and after a two-factor prompt the password
/// token: both are half-finished credentials, so this is cleared the moment the
/// sign-in ends, either way.
static PENDING: std::sync::OnceLock<tokio::sync::Mutex<Option<Session>>> =
    std::sync::OnceLock::new();

fn pending() -> &'static tokio::sync::Mutex<Option<Session>> {
    PENDING.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// Drop any half-finished sign-in.
pub async fn forget_pending_login() {
    *pending().lock().await = None;
}

/// Ask Telegram to send a login code.
pub async fn request_code(settings: Settings, tx: Events) {
    let mut session = match Session::connect(&settings).await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(Event::LoginFailed(e.to_string()));
            return;
        }
    };
    match session.request_code(&settings.phone).await {
        Ok(LoginStep::Ready) => match session.me().await {
            Ok(name) => {
                let _ = tx.send(Event::SignedIn(name));
            }
            Err(e) => {
                let _ = tx.send(Event::LoginFailed(e.to_string()));
            }
        },
        Ok(_) => {
            // Held for the next step. The code Telegram just sent is only
            // usable with the token on this session.
            *pending().lock().await = Some(session);
            let _ = tx.send(Event::LoginStage(crate::login::Stage::Code));
        }
        Err(e) => {
            let _ = tx.send(Event::LoginFailed(e.to_string()));
        }
    }
}

/// Submit the code, or the two-factor password.
///
/// Continues the session [`request_code`] left in [`PENDING`]. It does **not**
/// re-request the code: that starts the authorisation over and invalidates the
/// one the user just typed.
pub async fn finish_login(secret: String, is_code: bool, tx: Events) {
    let mut held = pending().lock().await;
    let Some(session) = held.as_mut() else {
        // Nothing to continue: the dialog was reopened, or the app restarted
        // between the code arriving and it being typed.
        let _ = tx.send(Event::LoginFailed(
            "That sign-in expired. Send a new code.".into(),
        ));
        let _ = tx.send(Event::LoginStage(crate::login::Stage::Phone));
        return;
    };
    if is_code {
        match session.sign_in(&secret).await {
            Ok(LoginStep::Ready) => {}
            Ok(LoginStep::NeedPassword) => {
                // Kept: the password token `sign_in` stashed lives on this
                // session and there is no way to obtain another.
                let _ = tx.send(Event::LoginStage(crate::login::Stage::Password));
                return;
            }
            Ok(LoginStep::NeedCode) => {
                let _ = tx.send(Event::LoginFailed("Telegram wanted another code.".into()));
                return;
            }
            // Kept as well. A mistyped code does not spend the token, so the
            // user retypes rather than starting the whole sign-in again.
            Err(e) => {
                let _ = tx.send(Event::LoginFailed(e.to_string()));
                return;
            }
        }
    } else if let Err(e) = session.check_password(&secret).await {
        let _ = tx.send(Event::LoginFailed(e.to_string()));
        return;
    }
    match session.me().await {
        Ok(name) => {
            // Signed in. The connection stays — it is shared, and every later
            // action is about to want it — but the login and password tokens
            // this `Session` is carrying are spent, and a spent credential
            // left in memory for the rest of the run is a credential left in
            // memory for the rest of the run.
            *held = None;
            let _ = tx.send(Event::SignedIn(name));
        }
        Err(e) => {
            let _ = tx.send(Event::LoginFailed(e.to_string()));
        }
    }
}

/// Load the chat list.
pub async fn refresh_chats(settings: Settings, tx: Events) {
    let _ = tx.send(Event::Status("Loading chats…".into()));
    let session = match Session::connect(&settings).await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(Event::Failed {
                activity: Activity::Chats,
                message: e.to_string(),
            });
            return;
        }
    };
    if !ready(&session, &tx, Activity::Chats).await {
        return;
    }
    match dialogs::list_chats(&session.client).await {
        Ok(chats) => {
            let _ = tx.send(Event::Chats(chats));
        }
        Err(e) => {
            let _ = tx.send(Event::Failed {
                activity: Activity::Chats,
                message: e.to_string(),
            });
        }
    }
}

/// Ask Telegram how many messages each chat holds.
///
/// **One request per chat**, which is why this is a button rather than
/// something that happens when the list loads, and why it can sit in a
/// two-minute rate-limit wait. Both facts are the user's to know before they
/// press it, and both are why it has to be stoppable.
pub async fn count_chats(settings: Settings, chat_ids: Vec<i64>, cancel: Cancel, tx: Events) {
    let session = match Session::connect(&settings).await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(Event::Failed {
                activity: Activity::Count,
                message: e.to_string(),
            });
            // Still ends the count, or the button reads "Stop counting" over a
            // run that never began.
            let _ = tx.send(Event::CountFinished {
                counted: 0,
                failed: 0,
            });
            return;
        }
    };
    if !ready(&session, &tx, Activity::Count).await {
        // Still ends the count, for the same reason as above: the button would
        // otherwise read "Stop counting" over a run that never began.
        let _ = tx.send(Event::CountFinished {
            counted: 0,
            failed: 0,
        });
        return;
    }

    let total = chat_ids.len();
    let mut done = 0usize;
    let _ = tx.send(Event::CountProgress { done, total });

    let tx_counted = tx.clone();
    let mut on = move |chat_id: i64, count: Option<i64>| {
        done += 1;
        // **`None` is not zero.** A chat Telegram would not count keeps its
        // blank row; writing 0 there would make it read as an empty chat and
        // sort among the empty ones.
        let _ = tx_counted.send(Event::Counted {
            chat_id,
            total: count,
        });
        let _ = tx_counted.send(Event::CountProgress { done, total });
    };

    let tx_wait = tx.clone();
    let mut waiting = move |seconds: u64| {
        let _ = tx_wait.send(Event::FloodWait(seconds));
    };

    let (counted, failed) =
        dialogs::count_all(&session.client, &chat_ids, &cancel, &mut on, &mut waiting).await;

    if failed > 0 {
        // A chat with no count paints blank and sorts last, which looks exactly
        // like a chat with no messages. Say how many, or a part-counted list
        // reads as a complete one.
        let _ = tx.send(Event::Warn(format!(
            "{counted} chats counted, {failed} could not be — those rows stay \
             blank and sort last."
        )));
    }
    let _ = tx.send(Event::CountFinished { counted, failed });
}

/// What discovering one chat's topics decided.
enum TopicsResolution {
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
async fn resolve_topics<F, Fut>(
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
    if result.media_failed > 0 {
        let _ = tx.send(Event::Warn(format!(
            "{title}: {} files could not be fetched — see missing_media.txt",
            result.media_failed
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

/// Open a folder in the system file manager.
///
/// A tool, not a step — it is live from the first frame, which is exactly why
/// it must not carry a step number.
///
/// Returns what went wrong, if anything. **The result used to be discarded**,
/// and it was covering a real bug: the path was built with `system32()` and
/// `explorer.exe` is not in System32 — it sits directly in `%SystemRoot%` — so
/// every press spawned nothing and reported nothing.
pub fn open_folder(dir: &std::path::Path) -> Result<(), String> {
    if !dir.exists() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    // Absolute, for the same reason icacls is: CreateProcess searches the
    // calling process's own directory before PATH, and this app is built to be
    // copied into arbitrary folders. A planted explorer.exe beside it would run
    // with the user's rights.
    #[cfg(windows)]
    let spawned = std::process::Command::new(tgx_tg::config::system_root("explorer.exe"))
        .arg(dir)
        .spawn();
    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let spawned = std::process::Command::new("xdg-open").arg(dir).spawn();
    spawned
        .map(|_| ())
        .map_err(|e| format!("opening {}: {e}", dir.display()))
}

/// Open the destination folder.
pub fn open_output_folder(dir: &str) -> Result<(), String> {
    open_folder(std::path::Path::new(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn signing_in_without_credentials_reports_rather_than_panics() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        sign_in(Settings::default(), tx).await;
        let mut events = Vec::new();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        // A status first, then a failure naming where to get credentials.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Failed { message: m, .. }
                                   if m.contains("my.telegram.org"))),
            "got {events:?}"
        );
    }

    #[tokio::test]
    async fn a_count_that_cannot_connect_still_ends_the_count() {
        // Otherwise the button is left reading "Stop counting" over a run that
        // never began, and the only control there is has become a lie.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        count_chats(Settings::default(), vec![1, 2], Cancel::new(), tx).await;
        let mut events = Vec::new();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::CountFinished { .. })),
            "got {events:?}"
        );
    }

    #[tokio::test]
    async fn an_export_that_cannot_connect_still_ends_the_queue() {
        // Same failure as above, on the control that matters more: without the
        // Finished, `exporting` stays set and Start is disabled for the rest of
        // the session.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        export(Settings::default(), Vec::new(), Cancel::new(), tx).await;
        let mut events = Vec::new();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        assert!(
            events.iter().any(|e| matches!(e, Event::Finished { .. })),
            "got {events:?}"
        );
    }

    /// The bug item 11 named: `export` used to catch *any* `list_topics`
    /// error, `Transient` included, and fall back to one folder. This drives
    /// `resolve_topics` with a canned `Transient` on every call — no network
    /// needed, since the whole point is that the answer must not depend on
    /// what a real flood wait happens to look like — and checks it retries
    /// exactly once before giving up on the *chat*, never on the split.
    #[tokio::test]
    async fn a_rate_limit_during_topic_discovery_skips_the_chat_rather_than_collapsing_it() {
        use std::time::Duration;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = Cancel::new();
        let mut calls = 0u32;
        let outcome = resolve_topics("Some Forum", 42, &cancel, &tx, || {
            calls += 1;
            async {
                Result::<Vec<dialogs::Topic>, EnrichError>::Err(EnrichError::Transient(
                    Duration::from_millis(1),
                ))
            }
        })
        .await;

        assert!(matches!(outcome, TopicsResolution::RateLimited));
        assert_eq!(calls, 2, "one retry, not zero and not more");

        let mut events = Vec::new();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        // The one text that must never appear here: it is what told the user
        // their forum had been quietly exported as a single folder.
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::Warn(m) if m.contains("one folder"))),
            "got {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::ChatFailed { chat_id: 42, .. })),
            "got {events:?}"
        );
    }

    /// A genuine refusal is not a rate limit and still has no shape to
    /// preserve — this is the one case that legitimately degrades to a single
    /// folder, and it must keep doing so.
    #[tokio::test]
    async fn a_real_refusal_still_degrades_to_one_folder() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = Cancel::new();
        let outcome = resolve_topics("Some Forum", 42, &cancel, &tx, || async {
            Result::<Vec<dialogs::Topic>, EnrichError>::Err(EnrichError::Refused(
                "CHAT_ADMIN_REQUIRED".into(),
            ))
        })
        .await;

        assert!(matches!(outcome, TopicsResolution::Unsplit));
        let mut events = Vec::new();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Warn(m) if m.contains("one folder"))),
            "got {events:?}"
        );
    }

    /// A cancel during the retry's wait must end the wait, not be reported as
    /// either a completed split or a chat failure — `export`'s loop is what
    /// turns this into "Stopped".
    #[tokio::test]
    async fn a_cancel_during_the_retry_wait_is_reported_as_neither_split_nor_failed() {
        use std::time::Duration;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = Cancel::new();
        cancel.cancel();
        let outcome = resolve_topics("Some Forum", 42, &cancel, &tx, || async {
            Result::<Vec<dialogs::Topic>, EnrichError>::Err(EnrichError::Transient(
                Duration::from_secs(30),
            ))
        })
        .await;

        assert!(matches!(outcome, TopicsResolution::Cancelled));
        let mut events = Vec::new();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        assert!(
            !events.iter().any(|e| matches!(e, Event::ChatFailed { .. })),
            "got {events:?}"
        );
    }

    #[test]
    fn opening_a_missing_output_folder_creates_it_first() {
        let dir = std::env::temp_dir().join(format!("tgx-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Do not actually spawn a file manager in the test; just prove the
        // directory is created, which is the part that would otherwise fail
        // silently on a first run.
        if !dir.exists() {
            std::fs::create_dir_all(&dir).unwrap();
        }
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
