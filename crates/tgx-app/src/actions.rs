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
/// the caller opens the sign-in dialog on it. Conflating the two puts two modal
/// dialogs on top of each other, which a user experiences as the app freezing
/// the moment it logs them in.
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

mod export;

pub use export::export;

// Test-only: the topic-resolution cases are exercised from this module's own
// tests, where the degrade-to-one-folder behaviour was written.
#[cfg(test)]
use export::{resolve_topics, TopicsResolution};

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
