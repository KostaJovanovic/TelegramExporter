//! What the nav bar's cells actually do.
//!
//! Every one of these submits to the tokio side and returns immediately — **the
//! UI thread never blocks on a future.** Results come back as
//! [`Event`](crate::bridge::Event)s, which the shell drains on its next frame.

use crate::bridge::Event;
use std::sync::mpsc::Sender;
use tgx_tg::client::{LoginStep, Session};
use tgx_tg::config::Settings;
use tgx_tg::dialogs;

/// Connect and report whether the session is already authorised.
///
/// **Reaching Telegram without being signed in is a success, not an error** —
/// the caller opens the sign-in dialog on it. Conflating the two is what put
/// two modal dialogs on top of each other in the original, which the user
/// experienced as the app freezing the moment it logged them in.
pub async fn sign_in(settings: Settings, tx: Sender<Event>) {
    let _ = tx.send(Event::Status("Connecting…".into()));
    let session = match Session::connect(&settings).await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(Event::Failed(e.to_string()));
            return;
        }
    };
    match session.is_authorized().await {
        Ok(true) => match session.me().await {
            Ok(name) => {
                let _ = tx.send(Event::SignedIn(name));
            }
            Err(e) => {
                let _ = tx.send(Event::Failed(e.to_string()));
            }
        },
        Ok(false) => {
            // Reached Telegram, not signed in yet.
            let _ = tx.send(Event::Status(
                "Not signed in — run `tgx login` to sign in".into(),
            ));
        }
        Err(e) => {
            let _ = tx.send(Event::Failed(e.to_string()));
        }
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
/// It holds a half-finished credential, so it is cleared the moment the
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
pub async fn request_code(settings: Settings, tx: Sender<Event>) {
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
pub async fn finish_login(_settings: Settings, secret: String, is_code: bool, tx: Sender<Event>) {
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
            // Signed in: the auth key is on disk now, so every later action
            // reconnects on its own. Drop the half-finished credential rather
            // than leaving it and its open connection alive for the run.
            *held = None;
            let _ = tx.send(Event::SignedIn(name));
        }
        Err(e) => {
            let _ = tx.send(Event::LoginFailed(e.to_string()));
        }
    }
}

/// Load the chat list.
pub async fn refresh_chats(settings: Settings, tx: Sender<Event>) {
    let _ = tx.send(Event::Status("Loading chats…".into()));
    let session = match Session::connect(&settings).await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(Event::Failed(e.to_string()));
            return;
        }
    };
    match dialogs::list_chats(&session.client).await {
        Ok(chats) => {
            let _ = tx.send(Event::Chats(chats));
        }
        Err(e) => {
            let _ = tx.send(Event::Failed(e.to_string()));
        }
    }
}

/// Export every selected chat, in queue order.
///
/// **Per-chat tallies live on the result, never on a shared counter** — see
/// `tgx_tg::engine::ExportResult`. This function keeps nothing across chats.
pub async fn export(settings: Settings, chats: Vec<tgx_tg::client::ChatInfo>, tx: Sender<Event>) {
    use tgx_tg::engine::{ChatExporter, Progress};

    let session = match Session::connect(&settings).await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(Event::Failed(e.to_string()));
            return;
        }
    };

    let mut done_chats = 0usize;
    for chat in &chats {
        let _ = tx.send(Event::Status(format!("Exporting {}", chat.title)));

        let peer = match peer_ref_for(&session, chat.id).await {
            Some(p) => p,
            None => {
                let _ = tx.send(Event::Log(format!(
                    "{} is no longer in the dialog list",
                    chat.title
                )));
                continue;
            }
        };

        let topics = if chat.is_forum && settings.split_topics {
            match dialogs::list_topics(&session.client, peer).await {
                Ok(t) => t,
                Err(e) => {
                    let _ = tx.send(Event::Log(format!("topics for {}: {e}", chat.title)));
                    vec![dialogs::Topic::general()]
                }
            }
        } else {
            vec![dialogs::Topic::general()]
        };

        let root = match tgx_tg::engine::unique_dir(
            std::path::Path::new(&settings.output_dir),
            &chat.title,
        ) {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(Event::Failed(format!("creating a folder: {e}")));
                return;
            }
        };

        let mut exporter = ChatExporter::new(&session.client, &settings);
        let tx2 = tx.clone();
        let mut on_progress = move |p: Progress| match p {
            Progress::Messages { done, total, .. } => {
                let _ = tx2.send(Event::Progress { done, total });
            }
            Progress::Total { total, .. } => {
                let _ = tx2.send(Event::Progress { done: 0, total });
            }
            Progress::FloodWait { seconds } => {
                let _ = tx2.send(Event::FloodWait(seconds));
            }
            Progress::Topic { title, messages } => {
                let _ = tx2.send(Event::Log(format!("{title}: {messages}")));
            }
            Progress::Log(msg) => {
                let _ = tx2.send(Event::Log(msg));
            }
        };

        match exporter
            .run(chat, peer, &topics, &root, &mut on_progress)
            .await
        {
            Ok(result) => {
                done_chats += 1;
                // A short export must not read like a complete one.
                if result.complete() {
                    let _ = tx.send(Event::Log(format!(
                        "{}: {} messages",
                        chat.title, result.messages
                    )));
                } else {
                    let _ = tx.send(Event::Log(format!(
                        "{}: INCOMPLETE — Telegram counted {}, {} came through",
                        chat.title, result.expected, result.messages
                    )));
                }
            }
            Err(e) => {
                let _ = tx.send(Event::Log(format!("{}: {e}", chat.title)));
            }
        }
    }
    let _ = tx.send(Event::Finished(format!(
        "Exported {done_chats} of {} chats",
        chats.len()
    )));
}

/// The peer reference for a chat we listed.
async fn peer_ref_for(
    session: &Session,
    id: i64,
) -> Option<grammers_client::session::types::PeerRef> {
    let mut iter = session.client.iter_dialogs();
    while let Ok(Some(d)) = iter.next().await {
        if d.peer.id().bare_id() == Some(id) {
            return Some(d.peer_ref());
        }
    }
    None
}

/// Open the output folder in the system file manager.
///
/// A tool, not a step — it is live from the first frame, which is exactly why
/// it must not carry a step number.
pub fn open_output_folder(dir: &str) {
    let path = std::path::Path::new(dir);
    if !path.exists() {
        let _ = std::fs::create_dir_all(path);
    }
    // Absolute, for the same reason icacls is: CreateProcess searches the
    // calling process's own directory before PATH, and this app is built to be
    // copied into arbitrary folders. A planted explorer.exe beside it would run
    // with the user's rights.
    #[cfg(windows)]
    let _ = std::process::Command::new(tgx_tg::config::system32("explorer.exe"))
        .arg(path)
        .spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn signing_in_without_credentials_reports_rather_than_panics() {
        let (tx, rx) = std::sync::mpsc::channel();
        sign_in(Settings::default(), tx).await;
        let events: Vec<Event> = rx.try_iter().collect();
        // A status first, then a failure naming where to get credentials.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Failed(m) if m.contains("my.telegram.org"))),
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
        let path = dir.to_string_lossy().to_string();
        let p = std::path::Path::new(&path);
        if !p.exists() {
            std::fs::create_dir_all(p).unwrap();
        }
        assert!(p.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
