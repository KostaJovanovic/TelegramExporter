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
use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::Mutex;

/// How long establishing the connection may take before it is called a failure.
///
/// **There was no timeout at all, at any layer.** `NetStream::connect` in
/// `grammers-mtsender` is a bare `TcpStream::connect`, and nothing in this
/// crate wrapped it. An address that is *filtered* rather than refused — which
/// is what Telegram looks like on a network that blocks it — therefore sat in
/// Windows' own SYN retry for about twenty-one seconds, per attempt, with the
/// sign-in button reading "Working…" throughout and no way for the user to
/// tell a slow link from a blocked one.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// How long one authorisation step may take.
///
/// Three times [`CONNECT_TIMEOUT`] because an authorisation step can *contain*
/// a connection: `auth.sendCode` answers `PHONE_MIGRATE` when the account does
/// not live on the datacentre we reached, and grammers handles that by dropping
/// that connection, generating a fresh authorisation key against the home one —
/// a full Diffie-Hellman exchange — and sending the code again.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(45);

/// Fail a step rather than let it hang, and say which step it was.
///
/// Returns the future's own output untouched, so a caller that has to tell
/// `PasswordRequired` from a real error still can. Only the *waiting* is this
/// function's business.
async fn within<F: Future>(what: &str, limit: Duration, fut: F) -> Result<F::Output> {
    match tokio::time::timeout(limit, fut).await {
        Ok(v) => Ok(v),
        Err(_) => {
            log::warn!("{what}: no answer within {}s", limit.as_secs());
            Err(anyhow!(
                "{what}: gave up after {}s — Telegram did not answer. The \
                 connection may be blocked rather than merely slow.",
                limit.as_secs()
            ))
        }
    }
}

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

/// The live connection to Telegram, shared by every action.
struct Connection {
    client: Client,
    /// The task driving the connection pool's I/O.
    ///
    /// **A `Client` is only a handle.** `SenderPool` hands back three things —
    /// a handle, a runner and an update channel — and the runner is the half
    /// that owns the sockets. Taking the handle and dropping the runner
    /// compiles, connects, and then fails every single request with
    /// `dropped (cancelled)`: the requests go into a channel whose receiver no
    /// longer exists. Held here so it lives exactly as long as the client that
    /// depends on it, and [`Drop`] below closes it when the last user goes.
    runner: tokio::task::JoinHandle<()>,
    /// The session store the pool writes authorisation keys and datacentre
    /// addresses into. Held because dropping it closes the database underneath
    /// the runner.
    session: Arc<SqliteSession>,
    /// Which credentials this was built with. A different `api_id` is a
    /// different application as far as Telegram is concerned, so a connection
    /// made with one cannot be handed to the other.
    api_id: i64,
    /// `None` until something has asked.
    ///
    /// Cached because the answer costs a round trip and the sign-in path asked
    /// for it twice: once from the "Sign in" button's probe, and again inside
    /// [`Session::request_code`] on a second connection, to answer a question
    /// whose answer had just been used to open the dialog.
    authorized: Mutex<Option<bool>>,
}

impl Connection {
    async fn open(settings: &Settings) -> Result<Self> {
        ensure_data_dir().context("creating the data directory")?;
        let path = session_file();
        let session = Arc::new(
            SqliteSession::open(&path)
                .await
                .map_err(|e| anyhow!("opening {}: {e}", path.display()))?,
        );
        // Checked, not `as i32`. The api id comes from `settings.json`, and a
        // silent truncation would hand Telegram a *different, valid-looking*
        // id — which fails as an authorisation error at the far end of a
        // connection, nowhere near the typo that caused it.
        let api_id = i32::try_from(settings.api_id).map_err(|_| {
            anyhow!(
                "api_id {} is not a Telegram api id — check settings.json",
                settings.api_id
            )
        })?;
        let pool = SenderPool::new(session.clone(), api_id);
        let client = Client::new(pool.handle);
        // `pool.updates` is deliberately dropped. Nothing here reacts to live
        // updates — an export reads history — and the runner sends them with
        // `let _ = tx.send(..)`, so a dropped receiver makes each send a
        // no-op. Holding the receiver without draining it would instead grow
        // an unbounded queue for the length of an export.
        let runner = tokio::spawn(pool.runner.run());
        log::info!("connection pool started for api_id {}", settings.api_id);
        Ok(Self {
            client,
            runner,
            session,
            api_id: settings.api_id,
            authorized: Mutex::new(None),
        })
    }
}

impl Drop for Connection {
    /// Stop driving the pool when the last holder goes.
    ///
    /// Without this the runner outlives everything that depended on it for as
    /// long as the runtime does — a socket and a task nothing will ever close.
    fn drop(&mut self) {
        self.runner.abort();
    }
}

/// The one connection, for the life of the process.
///
/// **Every action used to build its own.** Each new one paid a TCP connect, an
/// `InvokeWithLayer(InitConnection(help.getConfig))` and a write of every
/// datacentre address that answer carries, before its first useful request —
/// twice over a single sign-in, and again for each of refresh, count and
/// export. None of it bought anything: the pool multiplexes requests over one
/// connection per datacentre, which is what a pool is for, and opening a second
/// `SqliteSession` on the same file meant two writers contending for the store
/// the first one was still using.
static SHARED: OnceLock<Mutex<Option<Arc<Connection>>>> = OnceLock::new();

fn shared() -> &'static Mutex<Option<Arc<Connection>>> {
    SHARED.get_or_init(|| Mutex::new(None))
}

/// Close the shared connection; the next [`Session::connect`] builds a fresh one.
///
/// Nothing in the app needs this during a run — the point of the connection is
/// that it persists — but a signed-out or revoked session has to be able to
/// start over without restarting the process.
pub async fn disconnect() {
    if shared().lock().await.take().is_some() {
        log::info!("shared connection closed");
    }
}

pub struct Session {
    pub client: Client,
    conn: Arc<Connection>,
    api_hash: String,
    token: Option<grammers_client::client::LoginToken>,
    /// Held only between `sign_in` answering `NeedPassword` and
    /// `check_password` consuming it. Taken, not cloned, so a completed or
    /// abandoned exchange does not leave a live credential in memory.
    password: Option<grammers_client::client::PasswordToken>,
}

impl Session {
    /// Take a handle on the connection, opening it if this is the first caller.
    ///
    /// **No I/O happens here, and none ever did.** The pool connects on demand,
    /// when the first request to a datacentre is made, so the cost this call
    /// looks like it pays is really paid by whatever asks first — which is why
    /// [`Session::ensure_connected`] exists and why it is the thing with a
    /// timeout on it.
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
        // The lock is held across `open` on purpose: two actions starting at
        // once would otherwise each build a connection and one would be thrown
        // away, having already done the handshake.
        let mut held = shared().lock().await;
        let usable = held
            .as_ref()
            .is_some_and(|c| c.api_id == settings.api_id && !c.runner.is_finished());
        if !usable {
            *held = Some(Arc::new(Connection::open(settings).await?));
        }
        let conn = held.as_ref().expect("just set").clone();
        drop(held);
        Ok(Self {
            client: conn.client.clone(),
            conn,
            api_hash: settings.api_hash.clone(),
            token: None,
            password: None,
        })
    }

    /// Ask Telegram whether this account is signed in, and remember the answer.
    ///
    /// This is the call that actually establishes the socket, so it is the one
    /// carrying [`CONNECT_TIMEOUT`].
    pub async fn is_authorized(&self) -> Result<bool> {
        let answer = within(
            "connecting to Telegram",
            CONNECT_TIMEOUT,
            self.client.is_authorized(),
        )
        .await?
        .map_err(|e| anyhow!("checking authorisation: {e}"))?;
        self.remember_authorized(answer).await;
        Ok(answer)
    }

    /// The same question, asked over the wire at most once per connection.
    ///
    /// Every action calls this before its real work, so a blocked network fails
    /// in [`CONNECT_TIMEOUT`] with a sentence about it rather than in whatever
    /// the operating system's TCP retry happens to be, and so an unauthorised
    /// account is told it is unauthorised instead of being handed a wire error
    /// from the middle of a chat listing.
    pub async fn ensure_connected(&self) -> Result<bool> {
        // Bound in a `let` first: the temporary guard of an `if let` scrutinee
        // lives for the whole `if let`, and `is_authorized` takes the same
        // lock. That would deadlock on the cold path only — the shape of bug
        // that passes every warm test.
        let known = *self.conn.authorized.lock().await;
        match known {
            Some(known) => Ok(known),
            None => self.is_authorized().await,
        }
    }

    /// Ask again rather than trust the cache.
    ///
    /// For the "Sign in" button, whose entire job is to re-check the stored
    /// session — a cached answer there would make the button do nothing.
    pub async fn refresh_authorization(&self) -> Result<bool> {
        *self.conn.authorized.lock().await = None;
        self.is_authorized().await
    }

    async fn remember_authorized(&self, yes: bool) {
        *self.conn.authorized.lock().await = Some(yes);
    }

    /// Ask Telegram to send a login code.
    ///
    /// **Calling this twice invalidates the first code.** Telegram treats a
    /// second `auth.sendCode` as starting over, so the code already in the
    /// user's hand stops working. Request once per attempt and keep the
    /// `Session` alive until the code is submitted.
    pub async fn request_code(&mut self, phone: &str) -> Result<LoginStep> {
        if self.ensure_connected().await? {
            return Ok(LoginStep::Ready);
        }
        let sent = within(
            "requesting a login code",
            AUTH_TIMEOUT,
            self.client.request_login_code(phone, &self.api_hash),
        )
        .await?;
        let token = match sent {
            Ok(t) => t,
            // AUTH_RESTART means "an earlier authorisation was left half
            // finished; begin again" — and beginning again is this same call.
            // Retried once, because a user who is told to restart, restarts by
            // pressing the same button, and there is nothing else for them to
            // do differently.
            Err(e) if is_auth_restart(&e) => {
                log::warn!("Telegram asked to restart the login; retrying once");
                within(
                    "requesting a login code",
                    AUTH_TIMEOUT,
                    self.client.request_login_code(phone, &self.api_hash),
                )
                .await?
                .map_err(|e| anyhow!("requesting a login code: {e}"))?
            }
            Err(e) => return Err(anyhow!("requesting a login code: {e}")),
        };
        self.token = Some(token);
        Ok(LoginStep::NeedCode)
    }

    /// Complete sign-in with the code that arrived.
    pub async fn sign_in(&mut self, code: &str) -> Result<LoginStep> {
        let outcome = {
            let token = self
                .token
                .as_ref()
                .ok_or_else(|| anyhow!("no login code was requested"))?;
            within("signing in", AUTH_TIMEOUT, self.client.sign_in(token, code)).await?
        };
        match outcome {
            Ok(_) => {
                self.remember_authorized(true).await;
                Ok(LoginStep::Ready)
            }
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
    ///
    /// **It hands it back in `InvalidPassword`, not `PasswordRequired`.** The
    /// two are distinct variants: `PasswordRequired` is Telegram *asking* for a
    /// password and only ever comes out of `sign_in`, while `InvalidPassword`
    /// is the answer to one that was wrong. Matching only the first meant the
    /// wrong-password path fell to the generic arm, the token was dropped by
    /// the `take()` above, and the next attempt failed with "two-factor sign-in
    /// is not pending" — so one typo cost the whole exchange, including a fresh
    /// SMS code. This is why the two variants are matched together and the
    /// message is the same for both: from here they mean the same thing.
    pub async fn check_password(&mut self, password: &str) -> Result<LoginStep> {
        let token = self
            .password
            .take()
            .ok_or_else(|| anyhow!("two-factor sign-in is not pending"))?;
        let outcome = within(
            "checking the password",
            AUTH_TIMEOUT,
            self.client.check_password(token, password),
        )
        .await?;
        match outcome {
            Ok(_) => {
                self.remember_authorized(true).await;
                Ok(LoginStep::Ready)
            }
            Err(e) => match Self::token_to_retry(e) {
                Ok(token) => {
                    self.password = Some(token);
                    Err(anyhow!("that password was not accepted"))
                }
                Err(e) => Err(anyhow!("checking the password: {e}")),
            },
        }
    }

    /// Is a two-factor step waiting for a password?
    pub fn awaiting_password(&self) -> bool {
        self.password.is_some()
    }

    /// Which sign-in errors hand a token back for another attempt.
    ///
    /// Split out from [`Self::check_password`] because it is the only part of
    /// that function a test can reach without a signed-in account:
    /// `PasswordToken::new` is public, so both token-bearing variants can be
    /// built here even though the request that produces them cannot.
    // `SignInError` is a large enum and clippy would rather it were boxed. It
    // is grammers' type, not ours, and boxing it here would only move the size
    // one level down while making both call sites read worse.
    #[allow(clippy::result_large_err)]
    fn token_to_retry(
        e: grammers_client::SignInError,
    ) -> Result<grammers_client::client::PasswordToken, grammers_client::SignInError> {
        use grammers_client::SignInError as E;
        match e {
            E::InvalidPassword(t) | E::PasswordRequired(t) => Ok(t),
            other => Err(other),
        }
    }

    /// The signed-in account's own display name.
    pub async fn me(&self) -> Result<String> {
        let me = within("fetching your account", AUTH_TIMEOUT, self.client.get_me())
            .await?
            .map_err(|e| anyhow!("fetching your account: {e}"))?;
        Ok(me.full_name())
    }

    pub fn session(&self) -> Arc<SqliteSession> {
        self.conn.session.clone()
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
    /// Reachable by a public `t.me/name` link, which is the whole of Desktop's
    /// `public_*` / `private_*` distinction in `result.json`'s `type`.
    ///
    /// This field is why that header is now ever written as `private_*`: the
    /// export hardcoded `export_type(true)`, so a private supergroup — which is
    /// what an invite-link-only group is — claimed to be public in every export
    /// this program had ever produced.
    pub public: bool,
    /// `None` means *not counted*, which is **not** the same as zero. It paints
    /// blank, sorts last, and every place that sums has to tell the two apart.
    pub message_count: Option<i64>,
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

// **`Session` has no `Drop`.** It used to abort the runner, because it owned
// it; now it holds an `Arc<Connection>` shared with every other action, and
// closing the socket when one action finished would tear the connection out
// from under the others. `Connection::drop` does the aborting, when the last
// holder — including [`SHARED`] — is gone.
//
// What dropping a `Session` still does, and is still relied on, is discard the
// login and password tokens it is carrying: an abandoned sign-in leaves no
// half-finished credential in memory.

/// Turn a grammers error into our vocabulary. Re-exported so callers never
/// reach for the raw type.
pub fn classify_error(e: &grammers_client::InvocationError) -> EnrichError {
    classify(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `PasswordToken` with nothing real in it.
    ///
    /// `PasswordToken::new` is public, which is the only reason the two-factor
    /// retry path is testable at all: the *request* that produces this needs a
    /// signed-in account, but the token it carries does not.
    fn a_token() -> grammers_client::client::PasswordToken {
        use grammers_tl_types as tl;
        grammers_client::client::PasswordToken::new(tl::types::account::Password {
            has_recovery: false,
            has_secure_values: false,
            has_password: true,
            current_algo: None,
            srp_b: None,
            srp_id: None,
            hint: Some("the usual".into()),
            email_unconfirmed_pattern: None,
            new_algo: tl::enums::PasswordKdfAlgo::Unknown,
            new_secure_algo: tl::enums::SecurePasswordKdfAlgo::Unknown,
            secure_random: vec![],
            pending_reset_date: None,
            login_email_pattern: None,
        })
    }

    #[test]
    fn a_wrong_two_factor_password_hands_the_token_back() {
        // grammers answers a *wrong* password with `InvalidPassword(token)`;
        // `PasswordRequired(token)` is Telegram *asking* and only ever comes out
        // of `sign_in`. Matching only the second meant the wrong-password path
        // fell to the generic arm, the token was already consumed by `take()`,
        // and the next attempt failed with "two-factor sign-in is not pending"
        // — one typo costing the whole exchange, including a fresh SMS code.
        use grammers_client::SignInError as E;
        assert!(
            Session::token_to_retry(E::InvalidPassword(a_token())).is_ok(),
            "a wrong password must leave the user able to try again"
        );
        assert!(Session::token_to_retry(E::PasswordRequired(a_token())).is_ok());
        // And an error with no token in it is still an error.
        assert!(Session::token_to_retry(E::InvalidCode).is_err());
        assert!(Session::token_to_retry(E::SignUpRequired).is_err());
    }

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
            public: false,
            message_count: None,
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
    async fn a_step_that_never_answers_is_given_up_on_rather_than_waited_out() {
        // The failure this exists for: with no timeout anywhere, a filtered
        // address sat in the OS's SYN retry and the button read "Working…" for
        // twenty-one seconds with nothing to say why.
        let err = within(
            "connecting to Telegram",
            Duration::from_millis(10),
            std::future::pending::<()>(),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("gave up"), "got {err}");
        assert!(err.contains("connecting to Telegram"), "got {err}");
    }

    #[tokio::test]
    async fn a_step_that_answers_in_time_is_handed_back_untouched() {
        // Including its own error. `sign_in` has to tell `PasswordRequired`
        // from a real failure, so `within` must wrap the waiting and nothing
        // else — flattening the result here would make two-factor sign-in
        // indistinguishable from a wrong code.
        let inner: std::result::Result<i32, &str> = Err("password required");
        let got = within("signing in", Duration::from_secs(5), async move { inner })
            .await
            .unwrap();
        assert_eq!(got, Err("password required"));
    }

    #[test]
    fn an_authorisation_step_is_allowed_longer_than_a_connection() {
        // `auth.sendCode` can contain a whole second connection: PHONE_MIGRATE
        // means dropping the datacentre we reached and generating a fresh
        // authorisation key against the home one before the code is sent.
        assert!(AUTH_TIMEOUT > CONNECT_TIMEOUT);
        // And the connect cap has to beat Windows' own SYN retry, or a blocked
        // address is still diagnosed by the operating system rather than by us
        // and the timeout buys nothing.
        assert!(CONNECT_TIMEOUT < Duration::from_secs(21));
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
