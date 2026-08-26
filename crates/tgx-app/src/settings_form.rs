//! The editable settings, and the rules for reading a number out of a field.
//!
//! `settings_panel(&self)` used to take no `Context`, which meant it *could
//! not* be interactive whatever it painted: six of roughly twenty-four fields
//! were printed as labels and the only editable fields anywhere in the app were
//! the three in the sign-in dialog.
//!
//! **Nothing here coerces.** `Settings::load_from_str` already refuses to read
//! `"20"` as `20` — *"a guess would be worse than a default"* — and a text
//! field is the same problem arriving by a different route. A field holding
//! nonsense keeps the stored value rather than becoming zero, and a field
//! holding a number outside its range is clamped **and written back**, so what
//! is on screen is what is stored.

use gpui::{AppContext, Entity, Window};
use gpui_component::input::InputState;
use tgx_tg::config::Settings;

/// Messages per `messages*.html` page. Desktop's own default is 1,000.
pub const PAGE_RANGE: (i64, i64) = (50, 20_000);
/// Parallel media downloads. 4-6 is a good balance; above that Telegram starts
/// rate-limiting the account rather than going faster.
pub const DOWNLOAD_RANGE: (i64, i64) = (1, 16);
/// Size limit in MB. `0` is unlimited, which is why the floor is zero and not
/// one — see [`Settings::size_limit_bytes`].
pub const SIZE_RANGE: (i64, i64) = (0, 20_000);
/// Member roster cap. `0` is no cap. A public channel can have millions of
/// members and Telegram stops serving the listing long before that.
pub const MEMBER_RANGE: (i64, i64) = (0, 1_000_000);

/// Read a number out of a text field.
///
/// Returns `current` for anything that is not a whole number, including an
/// empty field: a half-typed value must not be read as a decision. Everything
/// else is clamped into range, and the caller writes the result back so the
/// field cannot disagree with what was stored.
pub fn number(raw: &str, range: (i64, i64), current: i64) -> i64 {
    let trimmed = raw.trim();
    match trimmed.parse::<i64>() {
        Ok(n) => n.clamp(range.0, range.1),
        Err(_) => current,
    }
}

/// The text fields the settings panel owns.
///
/// Checkboxes are not here: a tick is a `bool` on [`Settings`] and needs no
/// buffer between the click and the value, so giving it one would only create
/// a second place for the answer to live.
pub struct SettingsForm {
    pub output_dir: Entity<InputState>,
    pub page_size: Entity<InputState>,
    pub size_limit: Entity<InputState>,
    pub downloads: Entity<InputState>,
    pub member_limit: Entity<InputState>,
}

impl SettingsForm {
    pub fn new(settings: &Settings, window: &mut Window, cx: &mut gpui::App) -> Self {
        Self {
            output_dir: field(window, cx, &settings.output_dir, "Where exports go"),
            page_size: field(window, cx, &settings.page_size.to_string(), "1000"),
            size_limit: field(window, cx, &settings.size_limit_mb.to_string(), "0"),
            downloads: field(window, cx, &settings.download_concurrency.to_string(), "5"),
            member_limit: field(window, cx, &settings.member_limit.to_string(), "0"),
        }
    }

    /// Read every field back into `settings`.
    ///
    /// **The destination keeps its stored value when the field is blank.** An
    /// export into `""` resolves to the process's working directory, which for
    /// a portable exe is wherever the shell happened to be — a folder the user
    /// never chose and will not think to look in.
    pub fn collect(&self, settings: &mut Settings, cx: &gpui::App) {
        let dir = self.output_dir.read(cx).value().as_ref().trim().to_string();
        if !dir.is_empty() {
            settings.output_dir = dir;
        }
        settings.page_size = number(
            self.page_size.read(cx).value().as_ref(),
            PAGE_RANGE,
            settings.page_size as i64,
        ) as usize;
        settings.size_limit_mb = number(
            self.size_limit.read(cx).value().as_ref(),
            SIZE_RANGE,
            settings.size_limit_mb,
        );
        settings.download_concurrency = number(
            self.downloads.read(cx).value().as_ref(),
            DOWNLOAD_RANGE,
            settings.download_concurrency as i64,
        ) as usize;
        settings.member_limit = number(
            self.member_limit.read(cx).value().as_ref(),
            MEMBER_RANGE,
            settings.member_limit,
        );
    }

    /// Write the stored values back into the fields.
    ///
    /// Run after [`collect`](Self::collect), so a clamped or rejected entry is
    /// *visible*. Without it someone types `0` into Messages per page, the
    /// setting becomes 50, and the field goes on reading `0` — the interface
    /// asserting something about the export that is not true.
    pub fn sync(&self, settings: &Settings, window: &mut Window, cx: &mut gpui::App) {
        set(&self.output_dir, &settings.output_dir, window, cx);
        set(&self.page_size, &settings.page_size.to_string(), window, cx);
        set(
            &self.size_limit,
            &settings.size_limit_mb.to_string(),
            window,
            cx,
        );
        set(
            &self.downloads,
            &settings.download_concurrency.to_string(),
            window,
            cx,
        );
        set(
            &self.member_limit,
            &settings.member_limit.to_string(),
            window,
            cx,
        );
    }
}

fn field(
    window: &mut Window,
    cx: &mut gpui::App,
    value: &str,
    placeholder: &'static str,
) -> Entity<InputState> {
    let value = value.to_string();
    cx.new(|cx| {
        let mut state = InputState::new(window, cx).placeholder(placeholder);
        if !value.is_empty() {
            state.set_value(value, window, cx);
        }
        state
    })
}

/// Set a field, but only when it actually differs.
///
/// Writing an identical value still moves the caret to the end, which turns
/// typing in the middle of a path into a fight with the field.
fn set(state: &Entity<InputState>, value: &str, window: &mut Window, cx: &mut gpui::App) {
    state.update(cx, |state, cx| {
        if state.value().as_ref() != value {
            state.set_value(value.to_string(), window, cx);
        }
    });
}

/// A path shown in a field that is too narrow for it.
///
/// **Shows the start, not the end.** A field scrolled to its caret renders
/// `C:\Users\Kosta\Projekti\telegram_rust\Exports` as `…\telegram_rust\Exports`
/// at best and as a drive letter sliced in half at worst, which reads as a
/// typo. The whole path goes in the tooltip, and both have to be redone on
/// every change — not once at construction — because Browse writes back
/// afterwards.
pub fn elided_start(path: &str, max_chars: usize) -> String {
    let count = path.chars().count();
    if count <= max_chars {
        return path.to_string();
    }
    let kept: String = path.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonsense_keeps_the_stored_value_rather_than_becoming_zero() {
        // The same refusal `Settings::load_from_str` makes: a guess would be
        // worse than what is already there.
        assert_eq!(number("twenty", PAGE_RANGE, 1000), 1000);
        assert_eq!(number("", PAGE_RANGE, 1000), 1000);
        assert_eq!(number("  ", PAGE_RANGE, 1000), 1000);
        assert_eq!(number("12.5", PAGE_RANGE, 1000), 1000);
    }

    #[test]
    fn a_number_out_of_range_is_clamped_not_rejected() {
        // Rejecting it would leave the field and the setting disagreeing with
        // no way for the user to tell which won.
        assert_eq!(number("0", PAGE_RANGE, 1000), 50);
        assert_eq!(number("999999", PAGE_RANGE, 1000), 20_000);
        assert_eq!(number("-4", DOWNLOAD_RANGE, 5), 1);
        assert_eq!(number("500", PAGE_RANGE, 1000), 500);
    }

    #[test]
    fn zero_is_a_legitimate_size_limit_and_member_cap() {
        // 0 means unlimited in both, so the floor is zero — clamping it up to
        // one would quietly cap every download at a megabyte.
        assert_eq!(number("0", SIZE_RANGE, 20), 0);
        assert_eq!(number("0", MEMBER_RANGE, 10_000), 0);
    }

    #[test]
    fn a_path_is_elided_at_its_end_so_its_start_stays_readable() {
        // A drive letter sliced in half reads as a typo.
        let path = r"C:\Users\Kosta\Projekti\telegram_rust\Exports";
        let short = elided_start(path, 20);
        assert!(short.starts_with(r"C:\Users"), "got {short}");
        assert!(short.ends_with('…'));
        assert_eq!(short.chars().count(), 20);
        // A path that fits is untouched, ellipsis included.
        assert_eq!(elided_start(path, 200), path);
    }
}
