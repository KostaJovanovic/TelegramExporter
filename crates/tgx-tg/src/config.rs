//! Portable paths and persisted settings.
//!
//! **All state lives beside the executable**, never in AppData and never in the
//! registry. `TelegramExporterData/` holds the session — which is a *bearer
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
pub fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
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
    pub chat_concurrency: usize,
    pub download_concurrency: usize,

    /// Messages per `messages*.html` page.
    pub page_size: usize,

    // Chat list presentation
    pub sort_mode: String,
    pub group_by_type: bool,

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
            chat_concurrency: 1,
            download_concurrency: 5,
            page_size: 1000,
            sort_mode: "recent".into(),
            group_by_type: true,
            theme: "dark".into(),
        }
    }
}

impl Settings {
    pub fn size_limit_bytes(&self) -> Option<i64> {
        if self.size_limit_mb > 0 {
            Some(self.size_limit_mb * 1024 * 1024)
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
            base.insert(key, value);
        }
        let mut out: Self = serde_json::from_value(Value::Object(base)).unwrap_or(defaults);
        if out.theme != "dark" && out.theme != "light" {
            out.theme = "dark".into();
        }
        out.page_size = out.page_size.max(1);
        out.chat_concurrency = out.chat_concurrency.max(1);
        out.download_concurrency = out.download_concurrency.max(1);
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
        std::fs::write(settings_file(), body)
    }
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
pub fn ensure_data_dir() -> std::io::Result<PathBuf> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    restrict_to_current_user(&dir);
    Ok(dir)
}

#[cfg(windows)]
fn restrict_to_current_user(dir: &std::path::Path) {
    // icacls is the documented way to do this without pulling in a Win32 ACL
    // crate. A failure is logged rather than fatal: on FAT32/exFAT there are no
    // ACLs at all, and the export must still run — but the user is told, since
    // the folder holds a bearer credential.
    let out = std::process::Command::new("icacls")
        .arg(dir)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{}:(OI)(CI)F", whoami()))
        .output();
    match out {
        Ok(o) if o.status.success() => {}
        Ok(o) => log::warn!(
            "could not restrict {} to your user: {}",
            dir.display(),
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log::warn!("could not restrict {}: {e}", dir.display()),
    }
}

#[cfg(windows)]
fn whoami() -> String {
    std::env::var("USERDOMAIN")
        .ok()
        .zip(std::env::var("USERNAME").ok())
        .map(|(d, u)| format!("{d}\\{u}"))
        .or_else(|| std::env::var("USERNAME").ok())
        .unwrap_or_else(|| "%USERNAME%".into())
}

#[cfg(not(windows))]
fn restrict_to_current_user(dir: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(s.chat_concurrency, 1);
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
