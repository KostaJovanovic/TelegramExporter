//! Portable paths and persisted settings.
//!
//! **All state lives beside the executable**, never in AppData and never in the
//! registry — with one exception: a binary running out of `target/` climbs back
//! to the workspace root, because `cargo clean` would otherwise delete the
//! session key and every export with it. `TelegramExporterData/` holds the session — which is a *bearer
//! credential*: anyone who can read it can act as the account — so the folder
//! is ACL-restricted to the current user on creation, and the app says so in
//! the log when that fails rather than leaving you to assume it worked.
//!
//! The protection does not exist on FAT32/exFAT at all, so a plain USB stick
//! leaves it readable by anyone holding the stick. Portability was a deliberate
//! choice; this is its cost and it is stated rather than hidden.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// The directory the app keeps its state in: beside the exe when built, the
/// repo root when run from source.
///
/// The second half of that sentence is not a nicety. "Beside the exe" during
/// development means `target\release\`, and `cargo clean` deletes `target` —
/// which would take the session key and every export with it. A build artefact
/// directory is the one place state must never live, so a binary running out
/// of one climbs back to the workspace root instead.
pub fn app_dir() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    match workspace_above_target(&exe_dir) {
        Some(root) if root.join("Cargo.toml").is_file() => root,
        _ => exe_dir,
    }
}

/// The directory holding `target/`, if `dir` is inside one.
///
/// Covers `target/debug`, `target/release` and the cross-compilation shape
/// `target/<triple>/release`. Takes the *last* `target` component rather than
/// the first, so a project that happens to live under a folder called `target`
/// is not mistaken for its own build directory.
fn workspace_above_target(dir: &std::path::Path) -> Option<PathBuf> {
    let mut found = None;
    let mut prefix = PathBuf::new();
    for part in dir.components() {
        prefix.push(part);
        if part.as_os_str() == "target" {
            found = prefix.parent().map(PathBuf::from);
        }
    }
    found
}

pub fn data_dir() -> PathBuf {
    app_dir().join("TelegramExporterData")
}

pub fn settings_file() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn session_file() -> PathBuf {
    data_dir().join("session")
}

/// The seven media categories Desktop offers.
pub const MEDIA_KINDS: [&str; 7] = [
    "photos",
    "video_files",
    "voice_messages",
    "video_messages",
    "stickers",
    "animations",
    "files",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub api_id: i64,
    pub api_hash: String,
    pub phone: String,

    pub output_dir: String,

    // Output formats — Telegram Desktop offers both; so do we.
    pub export_html: bool,
    pub export_json: bool,

    // Media
    pub media_kinds: Vec<String>,
    /// 0 == unlimited.
    pub size_limit_mb: i64,
    pub download_media: bool,

    /// Save the image attached to a link preview. Telegram Desktop does not,
    /// and doing so shifts every later `photo_N` in the folder, so it stays off
    /// by default — turning it on is a deliberate step away from Desktop's
    /// output.
    pub link_previews: bool,


    // Extra requests that recover what Telegram sends only when asked. None of
    // these exist in Desktop's format; each is separately switchable because
    // each costs traffic, and each degrades to nothing on failure.
    pub full_reactions: bool,
    pub chat_metadata: bool,
    pub invite_links: bool,
    pub refresh_polls: bool,
    pub scheduled_messages: bool,
    pub member_roster: bool,
    /// A public channel can have millions of members and Telegram stops serving
    /// the listing long before that, so the roster is capped by default.
    /// 0 == no cap.
    pub member_limit: i64,

    /// Forum supergroups → one folder per topic.
    pub split_topics: bool,

    // Performance
    //
    // **There is no `chat_concurrency`.** The Python original offers "Chats at
    // once, 1 to 8"; this engine exports one chat at a time, so the field was
    // carried over, clamped on load, offered nowhere and read by nothing. A
    // setting that does nothing is the same lie as a control that does
    // nothing, and it survived precisely because a struct field that is
    // written and never read is not dead code. An old `settings.json` naming
    // it still loads: unknown keys are dropped per field, which is what that
    // tolerance is for.
    pub download_concurrency: usize,

    /// Messages per `messages*.html` page.
    pub page_size: usize,

    // Chat list presentation
    pub sort_mode: String,
    pub group_by_type: bool,
    /// Category buckets the user has folded away.
    ///
    /// Remembered for the same reason the sort is: someone who folds Bots away
    /// has said something about how they want the list to look, and making them
    /// say it again on every launch is the interface forgetting on purpose. An
    /// unknown name here is simply a category that no longer exists and is
    /// ignored, so a file written by another build still opens.
    pub folded_categories: Vec<String>,

    /// "dark" or "light". Anything else falls back to the default, so an edited
    /// settings file cannot leave the app unreadable.
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_id: 0,
            api_hash: String::new(),
            phone: String::new(),
            output_dir: app_dir().join("Exports").to_string_lossy().into_owned(),
            export_html: true,
            export_json: true,
            media_kinds: MEDIA_KINDS.iter().map(|s| s.to_string()).collect(),
            size_limit_mb: 20,
            download_media: true,
            link_previews: false,
            full_reactions: true,
            chat_metadata: true,
            invite_links: true,
            refresh_polls: true,
            scheduled_messages: true,
            member_roster: true,
            member_limit: 10_000,
            split_topics: true,
            download_concurrency: 5,
            page_size: 1000,
            sort_mode: "recent".into(),
            group_by_type: true,
            folded_categories: Vec::new(),
            theme: "dark".into(),
        }
    }
}

impl Settings {
    pub fn size_limit_bytes(&self) -> Option<i64> {
        if self.size_limit_mb > 0 {
            // Saturating, because this number comes out of `settings.json` and
            // is therefore untrusted: `9223372036854775807` would wrap to a
            // *negative* limit, and every file in the chat would compare as
            // "larger than the limit" and be skipped. A limit too big to
            // represent is the same intent as no limit at all, so it clamps to
            // the largest one instead of inverting the test.
            Some(self.size_limit_mb.saturating_mul(1024 * 1024))
        } else {
            None
        }
    }

    /// Load, falling back **per field** rather than per file.
    ///
    /// An unknown key is dropped, which keeps a settings.json written by a
    /// newer build loadable. A wrong *type* is dropped too: `"size_limit_mb":
    /// "20"` makes the byte calculation meaningless, and a half-written or
    /// hand-edited file would otherwise stop the app starting. Nothing is
    /// coerced — a guess would be worse than a default.
    ///
    /// **Each field is accepted only if the whole struct still deserialises
    /// with it in place**, and that is the part [`same_shape`] cannot do on its
    /// own. It compares JSON *kinds*, so `-1` and `1000` are both "a number"
    /// and both pass — but `page_size` is a `usize`, so `-1` fails serde, and
    /// this used to be one `from_value` over the merged map with
    /// `.unwrap_or(defaults)` behind it. One out-of-range number therefore
    /// discarded the entire file, including `api_id` and `api_hash`, and the
    /// app came up asking for credentials the user had already given it. The
    /// same held for `20.5` in an `i64` field and for an integer too large to
    /// represent.
    ///
    /// The cost is one deserialisation of a small struct per key present in the
    /// file — about 25 on a full one, which is immaterial next to the file read
    /// that preceded it.
    pub fn load_from_str(raw: &str) -> Self {
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw) else {
            return Self::default();
        };
        let defaults = Self::default();
        let Ok(Value::Object(mut base)) = serde_json::to_value(&defaults) else {
            return defaults;
        };

        for (key, value) in map {
            // Unknown key: drop it.
            let Some(slot) = base.get(&key) else { continue };
            // Wrong type: drop it, keeping the default.
            if !same_shape(slot, &value) {
                continue;
            }
            let mut candidate = base.clone();
            candidate.insert(key, value);
            if serde_json::from_value::<Self>(Value::Object(candidate.clone())).is_ok() {
                base = candidate;
            }
        }
        // `base` is now known to deserialise — every step preserved that — so
        // the fallback here is unreachable rather than load-bearing.
        let mut out: Self = serde_json::from_value(Value::Object(base)).unwrap_or(defaults);
        if out.theme != "dark" && out.theme != "light" {
            out.theme = "dark".into();
        }
        out.page_size = out.page_size.max(1);
        out.download_concurrency = out.download_concurrency.max(1);
        // Zero means unlimited for both of these, and a negative is neither a
        // limit nor unlimited: `size_limit_bytes` tests `> 0`, so -1 silently
        // becomes "no limit", while `member_limit` is compared as a cap and a
        // negative one caps at nothing. Clamping makes the two agree, and makes
        // the doc comments on the fields true.
        out.size_limit_mb = out.size_limit_mb.max(0);
        out.member_limit = out.member_limit.max(0);
        out
    }

    pub fn load() -> Self {
        match std::fs::read_to_string(settings_file()) {
            Ok(raw) => Self::load_from_str(&raw),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        ensure_data_dir()?;
        let body = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into());
        write_atomically(&settings_file(), body.as_bytes())
    }
}

/// Write via a sibling temporary file and one rename.
///
/// A plain `fs::write` truncates first and fills after, so an interruption —
/// power, a full disk, the process killed — leaves a *valid file containing
/// half a document*. A half-written `settings.json` does not parse, and
/// [`Settings::load_from_str`] then falls back to defaults for the whole file:
/// the same credential loss the per-field validation just closed, arriving by
/// another route. The window is small and this file is written on every settings
/// change, which is precisely the shape of a race that eventually happens.
///
/// The temporary sits in the *same directory* because that is what makes the
/// rename atomic — NTFS and every POSIX filesystem guarantee it within a volume
/// and nothing guarantees it across one. It also keeps the replacement inside
/// the ACL-restricted folder, rather than staging a bearer credential in a
/// world-readable `%TEMP%`.
fn write_atomically(target: &std::path::Path, body: &[u8]) -> std::io::Result<()> {
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, body)?;
    // Windows' rename replaces an existing target; so does POSIX's.
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Do these two values have the same JSON shape?
///
/// Booleans are checked before numbers deliberately: in JSON they are distinct,
/// but a caller reading `true` as `1` is exactly the coercion this refuses.
fn same_shape(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Bool(_), Value::Bool(_)) => true,
        (Value::Bool(_), _) | (_, Value::Bool(_)) => false,
        (Value::Number(_), Value::Number(_)) => true,
        (Value::String(_), Value::String(_)) => true,
        (Value::Array(_), Value::Array(items)) => {
            // Every settings array is a list of strings.
            items.iter().all(Value::is_string)
        }
        (Value::Object(_), Value::Object(_)) => true,
        (Value::Null, Value::Null) => true,
        _ => false,
    }
}

/// Create the data directory and restrict it to the current user.
///
/// **The restriction runs once per process, not once per call — once
/// *successfully*.** This is on the path of every action the window takes —
/// connect, list chats, list topics, export — and the Windows implementation
/// shells out to `icacls`, so calling it per action meant spawning a process on
/// every click. The permissions do not change between calls; re-asserting them
/// thousands of times only costs.
///
/// But the flag used to be set whether or not it worked, so "tried once" and
/// "succeeded once" were the same state. A transient failure — the folder open
/// in another process, a momentarily unavailable domain controller for the
/// grantee lookup — left the bearer credential at default permissions for the
/// life of the process, and `actions::ready`'s deliberate re-call did nothing.
/// The Python original got this right (`app/config.py:81-86`) and the port lost
/// it. A retry costs one process spawn on a path that is already failing.
pub fn ensure_data_dir() -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static RESTRICTED: AtomicBool = AtomicBool::new(false);

    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    if !RESTRICTED.load(Ordering::Relaxed) && restrict_to_current_user(&dir) {
        RESTRICTED.store(true, Ordering::Relaxed);
    }
    Ok(dir)
}

/// Why the last [`ensure_data_dir`] could not restrict the folder, or `None`.
///
/// **A silent failure leaves the session key at default permissions while the
/// README says otherwise.** Logging it is not enough; a caller that shows the
/// user a security claim needs to be able to check whether it is true.
static LOCKDOWN_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn lockdown_error() -> Option<String> {
    LOCKDOWN_ERROR.lock().ok().and_then(|e| e.clone())
}

fn set_lockdown_error(e: Option<String>) {
    if let Ok(mut slot) = LOCKDOWN_ERROR.lock() {
        *slot = e;
    }
}

/// Absolute path to a Windows system binary.
///
/// **`CreateProcess` resolves a bare name by searching the calling process's
/// own directory first.** This app is built to run from a USB stick with its
/// data folder alongside it, so a planted `icacls.exe` next to the exe would
/// run at startup with the user's rights. Never invoke a system tool by bare
/// name from a portable binary.
#[cfg(windows)]
pub fn system32(exe: &str) -> PathBuf {
    windows_dir().join("System32").join(exe)
}

/// Absolute path to a Windows binary that lives in `%SystemRoot%` itself.
///
/// **`explorer.exe` is not in `System32`.** It sits directly in `C:\Windows`,
/// so `system32("explorer.exe")` built a path to a file that does not exist and
/// the Open-folder button did nothing at all — silently, because the spawn
/// result was discarded. Keeping the absolute-path property while fixing the
/// directory is what this second helper is for; the answer to "is a bare name
/// acceptable here" is still no.
#[cfg(windows)]
pub fn system_root(exe: &str) -> PathBuf {
    windows_dir().join(exe)
}

#[cfg(windows)]
fn windows_dir() -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
}

/// How long `icacls` gets before it is killed.
///
/// It runs on the first line of `main` in both binaries, so an `icacls` that
/// never returns is an app that never draws. `.output()` waits forever, and
/// there are real ways to get there: a network path in `SystemRoot`, a hung
/// filter driver, a domain controller the grantee lookup cannot reach. The
/// Python original passed `timeout=15` for exactly this reason and the port
/// dropped it.
#[cfg(windows)]
const LOCKDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// `true` when the folder is now restricted.
#[cfg(windows)]
fn restrict_to_current_user(dir: &std::path::Path) -> bool {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;

    // icacls is the documented way to do this without pulling in a Win32 ACL
    // crate. A failure is recorded rather than fatal: on FAT32/exFAT there are
    // no ACLs at all, and the export must still run — but the user is told,
    // since the folder holds a bearer credential.
    //
    // CREATE_NO_WINDOW, because the app is a GUI binary and a console child
    // gets a console window of its own. Without this the user sees a black
    // cmd box flash on screen — which, in an app that is about to ask for
    // their phone number and a login code, looks exactly like malware.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let Some(grantee) = grantee() else {
        // Better to say the folder is unprotected than to grant to a guessed
        // name: a literal "%USERNAME%" is not a principal, and icacls would
        // either fail obscurely or name something nobody intended.
        set_lockdown_error(Some(
            "USERNAME is not set, so there is no user to grant access to".into(),
        ));
        return false;
    };

    // `/t`: apply to what is already in the folder, not only to the folder. A
    // `dist\` copied from another machine, or restored from a backup, carries
    // inherited ACEs on `session` itself — and `session` is the bearer
    // credential. Changing the container's ACL leaves those untouched, so the
    // one file that matters kept the permissions it arrived with.
    //
    // **Deliberately not `/c`.** It reads like the right companion to `/t` —
    // carry on past a file you could not touch — but measured on this machine,
    // `icacls <missing path> /t /c` exits **0** while printing "Failed
    // processing 1 files", and without `/c` the same command exits 3. So `/c`
    // would convert every partial failure into a reported success, which is
    // precisely the state `lockdown_error` exists to prevent. Detecting the
    // failure matters more than finishing the remaining two or three files, and
    // parsing "Failed processing" out of the text is not an option: icacls is
    // localised.
    let spawned = std::process::Command::new(system32("icacls.exe"))
        .arg(dir)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{grantee}:(OI)(CI)F"))
        .arg("/t")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();

    let failure = match spawned {
        Err(e) => Some(e.to_string()),
        Ok(child) => wait_bounded(child),
    };
    if let Some(why) = &failure {
        log::warn!("could not restrict {} to your user: {why}", dir.display());
    }
    set_lockdown_error(failure.clone());
    failure.is_none()
}

/// Wait for `icacls`, killing it at [`LOCKDOWN_TIMEOUT`]. `None` on success.
#[cfg(windows)]
fn wait_bounded(mut child: std::process::Child) -> Option<String> {
    use std::io::Read;

    // The pipes are drained on their own threads rather than after the wait.
    // `/t` prints a line per file processed, and a child that fills its pipe
    // blocks writing while we sit in try_wait — a deadlock that would look
    // exactly like the hang the timeout is here to bound.
    fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<String> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).trim().to_string()
        })
    }
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());

    let deadline = std::time::Instant::now() + LOCKDOWN_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Err(e) => return Some(e.to_string()),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Some(format!(
                        "icacls did not finish within {}s and was stopped",
                        LOCKDOWN_TIMEOUT.as_secs()
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    };
    if status.success() {
        return None;
    }
    // stderr *or* stdout, as the original had it: icacls writes most of its
    // failure text ("Access is denied.", "The system cannot find the path
    // specified.") to stdout, so reading only stderr degraded every real
    // explanation to "icacls exited exit code: 1".
    let err = err.join().unwrap_or_default();
    let out = out.join().unwrap_or_default();
    let said = if !err.is_empty() { err } else { out };
    Some(if said.is_empty() {
        format!("icacls exited {status}")
    } else {
        said
    })
}

/// The principal to grant, or `None` when Windows has not told us who we are.
#[cfg(windows)]
fn grantee() -> Option<String> {
    let user = std::env::var("USERNAME").ok().filter(|u| !u.is_empty())?;
    match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => Some(format!("{d}\\{user}")),
        _ => Some(user),
    }
}

#[cfg(unix)]
fn restrict_to_current_user(dir: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let failure = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .err()
        .map(|e| e.to_string());
    if let Some(why) = &failure {
        log::warn!("could not restrict {}: {why}", dir.display());
    }
    let ok = failure.is_none();
    set_lockdown_error(failure);
    ok
}

/// Neither Windows nor Unix: nothing was attempted, and saying so is the point.
///
/// This used to fall through silently, leaving `lockdown_error()` `None` — which
/// every caller reads as "the folder is restricted". A GUI claiming a protection
/// nobody attempted is worse than one admitting it cannot.
#[cfg(not(any(windows, unix)))]
fn restrict_to_current_user(dir: &std::path::Path) -> bool {
    let _ = dir;
    set_lockdown_error(Some(
        "this platform has no file permissions this app knows how to set, \
         so the session key is readable by anything that can read the folder"
            .into(),
    ));
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn an_absurd_size_limit_does_not_wrap_into_a_negative_one() {
        // `settings.json` is untrusted input. Multiplying by 1 MiB overflowed,
        // and a negative limit compares *every* file as too large — an export
        // that silently downloads nothing while reporting a 20 MB limit.
        let s = Settings {
            size_limit_mb: i64::MAX,
            ..Settings::default()
        };
        let limit = s.size_limit_bytes().expect("a positive limit");
        assert!(limit > 0, "the limit wrapped: {limit}");
        assert_eq!(limit, i64::MAX);

        // And the ordinary case still means what it says.
        let s = Settings {
            size_limit_mb: 20,
            ..Settings::default()
        };
        assert_eq!(s.size_limit_bytes(), Some(20 * 1024 * 1024));
        // Zero is "no limit", not "skip everything".
        let s = Settings {
            size_limit_mb: 0,
            ..Settings::default()
        };
        assert_eq!(s.size_limit_bytes(), None);
    }

    #[test]
    fn a_binary_run_from_target_keeps_its_state_in_the_repo() {
        // cargo clean deletes target. If the session key and the exports live
        // under it, a routine clean logs you out and takes the exports too.
        assert_eq!(
            workspace_above_target(Path::new("C:/proj/telegram_rust/target/release")),
            Some(PathBuf::from("C:/proj/telegram_rust"))
        );
        assert_eq!(
            workspace_above_target(Path::new("C:/proj/telegram_rust/target/debug")),
            Some(PathBuf::from("C:/proj/telegram_rust"))
        );
        // Cross-compiled: target/<triple>/release.
        assert_eq!(
            workspace_above_target(Path::new("/p/app/target/x86_64-pc-windows-msvc/release")),
            Some(PathBuf::from("/p/app"))
        );
    }

    #[test]
    fn the_real_state_directory_is_never_inside_target() {
        // Not a unit test of the helper: this one runs from
        // target/debug/deps/, so it checks the answer app_dir() actually
        // gives on this machine, through current_exe and the Cargo.toml probe.
        let dir = data_dir();
        assert!(
            !dir.components().any(|c| c.as_os_str() == "target"),
            "state would be destroyed by cargo clean: {}",
            dir.display()
        );
        assert!(
            dir.parent().is_some_and(|p| p.join("Cargo.toml").is_file()),
            "expected the workspace root, got {}",
            dir.display()
        );
    }

    #[cfg(windows)]
    #[test]
    fn system_tools_are_invoked_by_absolute_path() {
        // CreateProcess searches the calling process's own directory first, and
        // this app is designed to run from a stick with its data folder beside
        // it. A bare "icacls" would run a planted icacls.exe with the user's
        // rights, at startup, before anything else happens.
        let p = system32("icacls.exe");
        assert!(p.is_absolute(), "{}", p.display());
        assert!(p.ends_with("System32/icacls.exe") || p.ends_with(r"System32\icacls.exe"));
        assert!(
            p.is_file(),
            "{} does not exist on this machine",
            p.display()
        );
    }

    #[cfg(windows)]
    #[test]
    fn explorer_is_found_where_it_actually_lives() {
        // The mirror of the test above, and the one nobody wrote.
        // `explorer.exe` is **not** in System32 — it sits directly in
        // C:\Windows — so `system32("explorer.exe")` named a file that does not
        // exist, and the Open-folder button did nothing at all. Silently: the
        // spawn result was discarded.
        let p = system_root("explorer.exe");
        assert!(p.is_absolute(), "{}", p.display());
        assert!(
            !p.to_string_lossy()
                .to_ascii_lowercase()
                .contains("system32"),
            "{} is the path that did not work",
            p.display()
        );
        assert!(
            p.is_file(),
            "{} does not exist on this machine",
            p.display()
        );
        // And the wrong one really is wrong, so this test can fail.
        assert!(!system32("explorer.exe").is_file());
    }

    #[cfg(windows)]
    #[test]
    fn the_grantee_is_a_real_name_or_nothing() {
        // The failure mode being excluded is granting to a literal
        // "%USERNAME%": not a principal, so icacls either fails obscurely or
        // names something nobody intended.
        let g = grantee().expect("USERNAME is set when tests run");
        assert!(!g.contains('%'), "{g}");
        assert!(!g.is_empty());
        assert!(!g.ends_with('\\'), "a domain with no user: {g}");
    }

    #[cfg(windows)]
    #[test]
    fn a_failed_lockdown_says_why_and_does_not_claim_success() {
        // Three things at once, because they are one behaviour:
        //
        //  - the call reports whether it worked, so `ensure_data_dir` can tell
        //    "tried once" from "succeeded once" and retry the first;
        //  - the reason is recorded, not swallowed;
        //  - the reason is the one icacls gave, which means reading both pipes.
        //    Measured: a missing path puts "The system cannot find the path
        //    specified." on stderr and the summary on stdout, and other
        //    failures put the explanation on stdout. Reading only stderr
        //    degraded some of them to "icacls exited exit code: 1".
        let saved = lockdown_error();
        // Two levels missing, not one: icacls treats the final component as a
        // name pattern inside its parent, so a missing leaf under a directory
        // that *does* exist matches nothing, reports "Successfully processed 0
        // files" and exits 0. A missing parent is what actually fails.
        let missing = data_dir().join("no-such-folder-b7f3c1").join("deeper");
        assert!(
            !restrict_to_current_user(&missing),
            "icacls cannot have succeeded on a path that does not exist"
        );
        let why = lockdown_error().expect("a failure must be recorded, not swallowed");
        assert!(
            !why.is_empty() && !why.starts_with("icacls exited"),
            "the reason icacls gave was thrown away: {why:?}"
        );
        // And a real folder still succeeds, so the retry does not become a
        // spawn on every action.
        let dir = ensure_data_dir().expect("the data directory");
        assert!(dir.is_dir());
        set_lockdown_error(saved);
    }

    #[test]
    fn a_shipped_binary_stays_beside_itself() {
        // The portable design is the point everywhere else: an exe on a stick
        // keeps its data on the stick. Only a build directory is special.
        assert_eq!(
            workspace_above_target(Path::new("D:/TelegramExporter")),
            None
        );
        assert_eq!(workspace_above_target(Path::new("/usr/local/bin")), None);
    }

    #[test]
    fn the_last_target_wins() {
        // A project that happens to live under a folder called `target` is not
        // its own build directory.
        assert_eq!(
            workspace_above_target(Path::new("/target/myapp/target/release")),
            Some(PathBuf::from("/target/myapp"))
        );
    }

    #[test]
    fn an_unknown_key_is_dropped_not_fatal() {
        // A settings.json written by a newer build must still load.
        let s = Settings::load_from_str(r#"{"something_from_2027": 5, "page_size": 500}"#);
        assert_eq!(s.page_size, 500);
    }

    #[test]
    fn a_wrong_type_falls_back_to_the_default_rather_than_coercing() {
        // "20" is not 20. Coercing it would be a guess; the default is not.
        let s = Settings::load_from_str(r#"{"size_limit_mb": "20"}"#);
        assert_eq!(s.size_limit_mb, Settings::default().size_limit_mb);
    }

    #[test]
    fn a_bool_is_not_a_number_and_a_number_is_not_a_bool() {
        let s = Settings::load_from_str(r#"{"export_html": 1, "size_limit_mb": true}"#);
        assert!(s.export_html, "1 must not be read as true");
        assert_eq!(s.size_limit_mb, 20, "true must not be read as 1");
    }

    #[test]
    fn one_bad_field_does_not_lose_the_others() {
        // The whole point of per-field validation: a half-written file still
        // starts the app.
        let s = Settings::load_from_str(
            r#"{"size_limit_mb": "nonsense", "page_size": 250, "theme": "light"}"#,
        );
        assert_eq!(s.page_size, 250);
        assert_eq!(s.theme, "light");
        assert_eq!(s.size_limit_mb, 20);
    }

    #[test]
    fn a_number_serde_cannot_take_loses_only_that_field() {
        // The three shapes `same_shape` waves through because it compares JSON
        // *kinds*: a negative in a usize, a fraction in an i64, and an integer
        // too large to represent. Each one used to sink the whole file --
        // including the credentials, so the app restarted asking for an api_id
        // the user had already entered.
        for bad in [
            r#"{"api_id": 12345, "api_hash": "cafebabe", "page_size": -1}"#,
            r#"{"api_id": 12345, "api_hash": "cafebabe", "size_limit_mb": 20.5}"#,
            r#"{"api_id": 12345, "api_hash": "cafebabe", "api_id_typo": 0,
                "download_concurrency": -4}"#,
            r#"{"api_id": 99999999999999999999999, "api_hash": "cafebabe"}"#,
        ] {
            let s = Settings::load_from_str(bad);
            assert_eq!(s.api_hash, "cafebabe", "api_hash lost by: {bad}");
        }
        // And the bad field itself falls back rather than being coerced.
        let s = Settings::load_from_str(r#"{"page_size": -1}"#);
        assert_eq!(s.page_size, Settings::default().page_size);
        // An api_id too large is dropped; the rest of the file is not.
        let s = Settings::load_from_str(r#"{"api_id": 99999999999999999999999, "phone": "+381"}"#);
        assert_eq!(s.api_id, 0);
        assert_eq!(s.phone, "+381");
    }

    #[test]
    fn a_settings_write_replaces_the_file_rather_than_truncating_it() {
        // Not a test of `save()` -- that writes the real TelegramExporterData,
        // which holds this machine's actual credentials. The property under
        // test is the one `save()` delegates: the target is never observed
        // half-written, and an existing file is replaced.
        let dir = std::env::temp_dir().join(format!("tgx-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("settings.json");
        std::fs::write(&target, b"{\"page_size\": 1}").unwrap();

        write_atomically(&target, b"{\"page_size\": 2}").unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            r#"{"page_size": 2}"#
        );
        // No debris left beside it.
        assert!(!dir.join("settings.json.tmp").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_negative_limit_does_not_mean_unlimited() {
        // `size_limit_bytes` tests `> 0`, so -1 read as "no limit" -- the
        // opposite of what someone typing a minus sign meant, and the opposite
        // direction to the i64::MAX overflow the saturating multiply guards.
        let s = Settings::load_from_str(r#"{"size_limit_mb": -1, "member_limit": -50}"#);
        assert_eq!(s.size_limit_mb, 0);
        assert_eq!(s.member_limit, 0);
    }

    #[test]
    fn malformed_json_gives_defaults_rather_than_panicking() {
        assert_eq!(Settings::load_from_str("{not json"), Settings::default());
        assert_eq!(Settings::load_from_str("[]"), Settings::default());
        assert_eq!(Settings::load_from_str(""), Settings::default());
    }

    #[test]
    fn an_unreadable_theme_cannot_be_persisted() {
        // An edited settings file must not leave the app unreadable.
        let s = Settings::load_from_str(r#"{"theme": "chartreuse"}"#);
        assert_eq!(s.theme, "dark");
        assert_eq!(
            Settings::load_from_str(r#"{"theme":"light"}"#).theme,
            "light"
        );
    }

    #[test]
    fn a_media_kinds_list_must_be_all_strings() {
        let s = Settings::load_from_str(r#"{"media_kinds": ["photos", 5]}"#);
        assert_eq!(s.media_kinds, Settings::default().media_kinds);
        let ok = Settings::load_from_str(r#"{"media_kinds": ["photos"]}"#);
        assert_eq!(ok.media_kinds, vec!["photos"]);
    }

    #[test]
    fn zero_size_limit_means_unlimited() {
        let unlimited = Settings {
            size_limit_mb: 0,
            ..Settings::default()
        };
        assert_eq!(unlimited.size_limit_bytes(), None);
        let capped = Settings {
            size_limit_mb: 20,
            ..Settings::default()
        };
        assert_eq!(capped.size_limit_bytes(), Some(20 * 1024 * 1024));
    }

    #[test]
    fn counts_that_would_divide_by_zero_are_floored_at_one() {
        let s = Settings::load_from_str(
            r#"{"page_size": 0, "chat_concurrency": 0, "download_concurrency": 0}"#,
        );
        assert_eq!(s.page_size, 1);
        assert_eq!(s.download_concurrency, 1);
    }

    #[test]
    fn settings_round_trip_through_json() {
        let s = Settings {
            page_size: 42,
            theme: "light".into(),
            ..Settings::default()
        };
        let raw = serde_json::to_string(&s).unwrap();
        assert_eq!(Settings::load_from_str(&raw), s);
    }

    #[test]
    fn state_lives_beside_the_executable() {
        // Never AppData, never the registry.
        assert!(data_dir().ends_with("TelegramExporterData"));
        assert!(session_file().starts_with(data_dir()));
        assert!(settings_file().starts_with(data_dir()));
    }
}
