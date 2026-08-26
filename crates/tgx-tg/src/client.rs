//! Connecting, signing in, and listing chats.
//!
//! **You sign in as *yourself*, not as a bot** — bots cannot read chat history,
//! so there is no bot path here at all.
//!
//! The session file is a **bearer credential**: anyone who can read it can act
//! as the account. It lives in `TelegramExporterData/` beside the executable,
//! ACL-restricted on creation — see [`crate::config`].

use crate::config::{ensure_data_dir, session_file, Settings};
use crate::error::{classify, EnrichError};
use anyhow::{anyhow, Context, Result};
use grammers_client::{Client, SenderPool};
use grammers_session::storages::SqliteSession;
use std::sync::Arc;

/// Where a sign-in got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginStep {
    /// Already authorised; nothing to do.
    Ready,
    /// A code was sent; call [`Session::sign_in`] with it.
    NeedCode,
    /// The account has two-factor auth; call [`Session::check_password`].
    NeedPassword,
}

pub struct Session {
    pub client: Client,
    /// The task driving the connection pool's I/O.
    ///
    /// **A `Client` is only a handle.** `SenderPool` hands back three things —
    /// a handle, a runner and an update channel — and the runner is the half
    /// that owns the sockets. Taking the handle and dropping the runner
    /// compiles, connects, and then fails every single request with
    /// `dropped (cancelled)`: the requests go into a channel whose receiver no
    /// longer exists. Held here so it lives exactly as long as the client that
    /// depends on it, and no longer — each action makes its own `Session`, so
    /// a detached runner per action would be a leak.
    runner: tokio::task::JoinHandle<()>,
    session: Arc<SqliteSession>,
    api_hash: String,
    token: Option<grammers_client::client::LoginToken>,
    /// Held only between `sign_in` answering `NeedPassword` and
    /// `check_password` consuming it. Taken, not cloned, so a completed or
    /// abandoned exchange does not leave a live credential in memory.
    password: Option<grammers_client::client::PasswordToken>,
}

impl Session {
    /// Connect, creating the session file if it is not there yet.
    ///
    /// Answering "connected but not signed in" is a *success*, not an error:
    /// the caller opens the sign-in dialog on it. Conflating the two is what
    /// produced two stacked modal dialogs in the Python original, which the
    /// user experienced as the app freezing the moment it logged them in.
    pub async fn connect(settings: &Settings) -> Result<Self> {
        if settings.api_id == 0 || settings.api_hash.is_empty() {
            return Err(anyhow!(
                "no API credentials yet — get an api_id and api_hash from my.telegram.org"
            ));
        }
        ensure_data_dir().context("creating the data directory")?;
        let path = session_file();
        let session = Arc::new(
            SqliteSession::open(&path)
                .await
                .map_err(|e| anyhow!("opening {}: {e}", path.display()))?,
        );
        let pool = SenderPool::new(session.clone(), settings.api_id as i32);
        let client = Client::new(pool.handle);
        // `pool.updates` is deliberately dropped. Nothing here reacts to live
        // updates — an export reads history — and the runner sends them with
        // `let _ = tx.send(..)`, so a dropped receiver makes each send a
        // no-op. Holding the receiver without draining it would instead grow
        // an unbounded queue for the length of an export.
        let runner = tokio::spawn(pool.runner.run());
        Ok(Self {
            client,
            runner,
            session,
            api_hash: settings.api_hash.clone(),
            token: None,
            password: None,
        })
    }

    pub async fn is_authorized(&self) -> Result<bool> {
        self.client
            .is_authorized()
            .await
            .map_err(|e| anyhow!("checking authorisation: {e}"))
    }

    /// Ask Telegram to send a login code.
    ///
    /// **Calling this twice invalidates the first code.** Telegram treats a
    /// second `auth.sendCode` as starting over, so the code already in the
    /// user's hand stops working. Request once per attempt and keep the
    /// `Session` alive until the code is submitted.
    pub async fn request_code(&mut self, phone: &str) -> Result<LoginStep> {
        if self.is_authorized().await? {
            return Ok(LoginStep::Ready);
        }
        let token = match self.client.request_login_code(phone, &self.api_hash).await {
            Ok(t) => t,
            // AUTH_RESTART means "an earlier authorisation was left half
            // finished; begin again" — and beginning again is this same call.
            // Retried once, because a user who is told to restart, restarts by
            // pressing the same button, and there is nothing else for them to
            // do differently.
            Err(e) if is_auth_restart(&e) => {
                log::warn!("Telegram asked to restart the login; retrying once");
                self.client
                    .request_login_code(phone, &self.api_hash)
                    .await
                    .map_err(|e| anyhow!("requesting a login code: {e}"))?
            }
            Err(e) => return Err(anyhow!("requesting a login code: {e}")),
        };
        self.token = Some(token);
        Ok(LoginStep::NeedCode)
    }

    /// Complete sign-in with the code that arrived.
    pub async fn sign_in(&mut self, code: &str) -> Result<LoginStep> {
        let token = self
            .token
            .as_ref()
            .ok_or_else(|| anyhow!("no login code was requested"))?;
        match self.client.sign_in(token, code).await {
            Ok(_) => Ok(LoginStep::Ready),
            Err(grammers_client::SignInError::PasswordRequired(token)) => {
                // The error *carries* the token check_password needs. Dropping
                // it here is what made two-factor sign-in impossible to
                // complete — the second step had nothing to present.
                self.password = Some(token);
                Ok(LoginStep::NeedPassword)
            }
            Err(e) => Err(anyhow!("signing in: {e}")),
        }
    }

    /// Second factor.
    ///
    /// Consumes the token `sign_in` stashed. On a wrong password grammers hands
    /// the token back, so the user can try again rather than restarting the
    /// whole sign-in.
    pub async fn check_password(&mut self, password: &str) -> Result<LoginStep> {
        let token = self
            .password
            .take()
            .ok_or_else(|| anyhow!("two-factor sign-in is not pending"))?;
        match self.client.check_password(token, password).await {
            Ok(_) => Ok(LoginStep::Ready),
            Err(grammers_client::SignInError::PasswordRequired(token)) => {
                self.password = Some(token);
                Err(anyhow!("that password was not accepted"))
            }
            Err(e) => Err(anyhow!("checking the password: {e}")),
        }
    }

    /// Is a two-factor step waiting for a password?
    pub fn awaiting_password(&self) -> bool {
        self.password.is_some()
    }

    /// The signed-in account's own display name.
    pub async fn me(&self) -> Result<String> {
        let me = self
            .client
            .get_me()
            .await
            .map_err(|e| anyhow!("fetching your account: {e}"))?;
        Ok(me.full_name())
    }

    pub fn session(&self) -> Arc<SqliteSession> {
        self.session.clone()
    }
}

/// One row of the chat list.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatInfo {
    pub id: i64,
    pub title: String,
    /// One of the five buckets the interface groups by.
    pub kind: ChatKind,
    /// Unix seconds of the last activity, for the default sort.
    pub last_activity: i64,
    /// A forum supergroup — its topics become separate folders.
    pub is_forum: bool,
    /// `None` means *not counted*, which is **not** the same as zero. It paints
    /// blank, sorts last, and every place that sums has to tell the two apart.
    pub message_count: Option<i64>,
    pub access_hash: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatKind {
    Channel,
    Supergroup,
    Group,
    Private,
    Bot,
}

impl ChatKind {
    pub fn label(self) -> &'static str {
        match self {
            ChatKind::Channel => "Channel",
            ChatKind::Supergroup => "Supergroup",
            ChatKind::Group => "Group",
            ChatKind::Private => "Private chat",
            ChatKind::Bot => "Bot",
        }
    }

    /// Desktop's `type` field in `result.json`.
    pub fn export_type(self, public: bool) -> &'static str {
        match self {
            ChatKind::Channel if public => "public_channel",
            ChatKind::Channel => "private_channel",
            ChatKind::Supergroup if public => "public_supergroup",
            ChatKind::Supergroup => "private_supergroup",
            ChatKind::Group => "private_group",
            ChatKind::Private | ChatKind::Bot => "personal_chat",
        }
    }
}

/// Is this Telegram telling us the authorisation flow must start over?
///
/// Matched on the name rather than the code: Telegram sends it as a 500, which
/// is otherwise "server had a problem" and not something to retry blindly.
pub fn is_auth_restart(e: &grammers_client::InvocationError) -> bool {
    e.to_string().contains("AUTH_RESTART")
}

impl Drop for Session {
    /// Stop driving the pool when the session goes.
    ///
    /// The runner would otherwise outlive its `Session` for as long as the
    /// runtime does, and the app builds one per action — connect, list chats,
    /// list topics, export — so a detached runner each time is a socket and a
    /// task that nothing will ever close.
    fn drop(&mut self) {
        self.runner.abort();
    }
}

/// Turn a grammers error into our vocabulary. Re-exported so callers never
/// reach for the raw type.
pub fn classify_error(e: &grammers_client::InvocationError) -> EnrichError {
    classify(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_count_is_not_a_count_of_zero() {
        // The list's count column is optional — it costs one request per chat —
        // so a chat can legitimately have no number. It paints blank and sorts
        // last, which looks exactly like a chat with no messages.
        let uncounted = ChatInfo {
            id: 1,
            title: "a".into(),
            kind: ChatKind::Private,
            last_activity: 0,
            is_forum: false,
            message_count: None,
            access_hash: 0,
        };
        let empty = ChatInfo {
            message_count: Some(0),
            ..uncounted.clone()
        };
        assert_ne!(uncounted.message_count, empty.message_count);
        assert!(uncounted.message_count.is_none());
        assert_eq!(empty.message_count, Some(0));
    }

    #[test]
    fn export_type_distinguishes_public_from_private() {
        assert_eq!(ChatKind::Supergroup.export_type(true), "public_supergroup");
        assert_eq!(
            ChatKind::Supergroup.export_type(false),
            "private_supergroup"
        );
        assert_eq!(ChatKind::Channel.export_type(true), "public_channel");
        assert_eq!(ChatKind::Private.export_type(false), "personal_chat");
        // The reference export's own header reads public_supergroup.
        assert_eq!(ChatKind::Supergroup.export_type(true), "public_supergroup");
    }

    #[test]
    fn groups_and_supergroups_are_separate_buckets() {
        // They differ in what an export of them contains: only supergroups can
        // be forums, and only they carry history a new member can read.
        assert_ne!(ChatKind::Group, ChatKind::Supergroup);
        assert_ne!(ChatKind::Group.label(), ChatKind::Supergroup.label());
    }

    #[tokio::test]
    async fn connecting_without_credentials_says_where_to_get_them() {
        let s = Settings::default();
        let err = match Session::connect(&s).await {
            Ok(_) => panic!("connected with no credentials"),
            Err(e) => e.to_string(),
        };
        // A message that names a screen has to name a screen that exists.
        assert!(err.contains("my.telegram.org"), "got {err}");
    }
}
