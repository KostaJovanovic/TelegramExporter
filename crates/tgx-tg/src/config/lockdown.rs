//! Creating the data directory and restricting it to the current user.
//!
//! `TelegramExporterData/` holds a Telegram **session key, which is a bearer
//! credential**: anyone who can read it can act as the account. The ACL is the
//! only thing standing between that and every other account on the machine,
//! and it does not exist at all on FAT32 or exFAT.
//!
//! Three guards here are easy to drop and each was added for a reason:
//! `CREATE_NO_WINDOW`, a once-per-process flag, and an **absolute**
//! `System32\icacls.exe` path -- a portable exe invoking a bare name lets a
//! planted `icacls.exe` beside it run with the user's rights.

use super::*;

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
/// A retry costs one process spawn on a path that is already failing.
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

pub(crate) fn set_lockdown_error(e: Option<String>) {
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
pub(crate) fn windows_dir() -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
}

/// How long `icacls` gets before it is killed.
///
/// It runs on the first line of `main` in both binaries, so an `icacls` that
/// never returns is an app that never draws. `.output()` waits forever, and
/// there are real ways to get there: a network path in `SystemRoot`, a hung
/// filter driver, a domain controller the grantee lookup cannot reach. The
/// timeout is the only thing standing between any of those and a window that
/// never opens.
#[cfg(windows)]
const LOCKDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// `true` when the folder is now restricted.
#[cfg(windows)]
pub(crate) fn restrict_to_current_user(dir: &std::path::Path) -> bool {
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

    // **Two calls, and `/t` is not on the first one.**
    //
    // `(OI)` and `(CI)` are *inheritance* flags: they say what a container
    // hands down to the things inside it, and they mean nothing on a file.
    // Combining them with `/t` — which walks into every file — was measured to
    // leave `session` with an **empty DACL**: `/inheritance:r` stripped the
    // inherited ACE from the file, and the `(OI)(CI)F` grant did not apply to
    // it, so nothing replaced what was removed. The result is a bearer
    // credential nobody can open, including its owner, and SQLite reports it as
    // error 14 — `SQLITE_CANTOPEN` — from inside `Session::connect`.
    //
    // That is worse than the gap it was added to close: the folder went from
    // "possibly too permissive" to "unusable", and the app could not sign in at
    // all. The lockdown must never be able to lock the user out.
    //
    // So: set the container's ACL, then make the existing children inherit it.
    // `/inheritance:e` on a file *restores* the inherited ACE rather than
    // granting a new explicit one, which is why the second call needs no
    // grantee and cannot repeat the mistake.
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
    let run = |args: Vec<std::ffi::OsString>| -> Option<String> {
        let spawned = std::process::Command::new(system32("icacls.exe"))
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
        match spawned {
            Err(e) => Some(e.to_string()),
            Ok(child) => wait_bounded(child),
        }
    };

    // 1. The container itself, with the flags that make *future* files inherit.
    let mut failure = run(vec![
        dir.as_os_str().to_owned(),
        "/inheritance:r".into(),
        "/grant:r".into(),
        format!("{grantee}:(OI)(CI)F").into(),
    ]);

    // 2. What is already inside it — a `dist\` copied from another machine, or
    //    restored from a backup, carries whatever ACEs it arrived with, and
    //    `session` is the file that matters. Re-enabling inheritance points
    //    those at the ACL just set above.
    //
    //    Skipped when the first call failed, because the ACE the children would
    //    inherit is then not the one we wanted.
    if failure.is_none() {
        failure = run(vec![
            dir.as_os_str().to_owned(),
            "/inheritance:e".into(),
            "/t".into(),
        ]);
    }

    if let Some(why) = &failure {
        log::warn!("could not restrict {} to your user: {why}", dir.display());
    }
    set_lockdown_error(failure.clone());
    failure.is_none()
}

/// Wait for `icacls`, killing it at [`LOCKDOWN_TIMEOUT`]. `None` on success.
#[cfg(windows)]
pub(crate) fn wait_bounded(mut child: std::process::Child) -> Option<String> {
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
pub(crate) fn grantee() -> Option<String> {
    let user = std::env::var("USERNAME").ok().filter(|u| !u.is_empty())?;
    match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => Some(format!("{d}\\{user}")),
        _ => Some(user),
    }
}

#[cfg(unix)]
pub(crate) fn restrict_to_current_user(dir: &std::path::Path) -> bool {
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
pub(crate) fn restrict_to_current_user(dir: &std::path::Path) -> bool {
    let _ = dir;
    set_lockdown_error(Some(
        "this platform has no file permissions this app knows how to set, \
         so the session key is readable by anything that can read the folder"
            .into(),
    ));
    false
}
