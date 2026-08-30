//! The sign-in dialog: credentials → phone → code → 2FA.
//!
//! **One dialog, ever.** Entering the credentials calls connect, which answers
//! *reached Telegram, not signed in yet* — and that is a success. Unguarded,
//! it opened a second modal on top of the first; both then survived to the end
//! of the login and each showed its own modal "Signed in" box. Modal windows
//! can end up ordered behind one another, where they are unreachable but still
//! hold every click in the application — which the user sees as the app
//! freezing the moment it logs them in.
//!
//! So: the stage is a single value, the dialog is a single field on the shell,
//! and **success just closes it** — the status bar already reads
//! `Signed in: <name>`.

/// Where the sign-in has got to. There is exactly one of these at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stage {
    /// No `api_id`/`api_hash` yet. **This is the first page of sign-in, not a
    /// Settings screen** — there is no Settings screen, and telling a new user
    /// to look for one sent them hunting for something that does not exist.
    #[default]
    Credentials,
    Phone,
    Code,
    Password,
}

impl Stage {
    pub fn title(self) -> &'static str {
        match self {
            Stage::Credentials => "API credentials",
            Stage::Phone => "Phone number",
            Stage::Code => "Login code",
            Stage::Password => "Two-factor password",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Stage::Credentials => {
                "Telegram requires your own credentials for any non-official \
                 client. They are free, they take about a minute, and you only \
                 do this once."
            }
            Stage::Phone => "With the country code, e.g. +381…",
            Stage::Code => "Telegram sent a code to your other devices.",
            Stage::Password => "This account has two-factor authentication.",
        }
    }

    /// The numbered instructions shown under the hint.
    ///
    /// The old hint was one sentence — "get them from my.telegram.org → API
    /// development tools" — which is accurate and still leaves you on a page
    /// asking for an app title, a short name, a URL and a platform, with no
    /// indication that three of the four do not matter. Someone doing this for
    /// the first time has no way to know that, and guessing wrong looks like a
    /// permanent decision about their account.
    pub fn steps(self) -> &'static [&'static str] {
        match self {
            Stage::Credentials => &[
                "Open my.telegram.org and sign in with the phone number of the \
                 account you want to export.",
                "The confirmation code arrives in Telegram itself, not by SMS.",
                "Choose API development tools.",
                "Fill in any app title and short name. Platform and URL do not \
                 matter, and nothing here is shown to anyone.",
                "Copy api_id and api_hash from the App configuration box.",
            ],
            // The one thing worth saying here is the thing Telegram itself
            // warns about, because this app looks exactly like what that
            // warning is about: a program asking for your login code.
            Stage::Code => &[
                "The code is in your Telegram app, under Telegram Service \
                 Notifications.",
                "Never give this code to anyone who asks you for it — not by \
                 message, not on a call. This app runs on your machine and \
                 sends it only to Telegram.",
            ],
            Stage::Password => &[
                "This is your Telegram cloud password, the one you set under \
                 Settings → Privacy and Security → Two-Step Verification.",
                "It is not your login code and not your device PIN.",
            ],
            Stage::Phone => &[],
        }
    }

    /// A page this stage can open in the browser.
    pub fn link(self) -> Option<Link> {
        match self {
            Stage::Credentials => Some(Link {
                label: "Open my.telegram.org",
                // The apps page directly, rather than the front page: it is
                // where the credentials are, and it redirects to the sign-in
                // first when you are not logged in, so it works either way.
                url: "https://my.telegram.org/apps",
            }),
            _ => None,
        }
    }

    /// Which fields this stage shows.
    pub fn fields(self) -> &'static [Field] {
        match self {
            Stage::Credentials => &[Field::ApiId, Field::ApiHash],
            Stage::Phone => &[Field::Phone],
            Stage::Code => &[Field::Code],
            Stage::Password => &[Field::Password],
        }
    }

    pub fn action(self) -> &'static str {
        match self {
            Stage::Credentials => "Continue",
            Stage::Phone => "Send code",
            Stage::Code => "Sign in",
            Stage::Password => "Sign in",
        }
    }
}

/// A page the dialog can open in the system browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub label: &'static str,
    pub url: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    ApiId,
    ApiHash,
    Phone,
    Code,
    Password,
}

impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Field::ApiId => "api_id",
            Field::ApiHash => "api_hash",
            Field::Phone => "Phone",
            Field::Code => "Code",
            Field::Password => "Password",
        }
    }

    /// Is this a secret that should not be shown on screen?
    pub fn masked(self) -> bool {
        matches!(self, Field::Password | Field::ApiHash)
    }

    /// The greyed-out example shown in an empty field.
    pub fn placeholder(self) -> &'static str {
        match self {
            Field::ApiId => "12345678",
            Field::ApiHash => "abcdef…",
            Field::Phone => "+381…",
            Field::Code => "12345",
            // A password has no example anyone should be shown.
            Field::Password => "",
        }
    }
}

/// The dialog's state. Held as `Option<LoginDialog>` on the shell, so "is one
/// open?" and "which one?" cannot disagree.
///
/// The five buffers are plain `String`s. Under GPUI each was an
/// `Entity<InputState>` that could only be built with a live `Window`, so
/// nothing about this dialog was reachable from a test.
#[derive(Debug, Default, Clone)]
pub struct LoginDialog {
    pub stage: Stage,
    pub api_id: String,
    pub api_hash: String,
    pub phone: String,
    pub code: String,
    pub password: String,
    /// Shown under the fields. Cleared on every submit, so a stale failure
    /// cannot sit under a fresh attempt.
    pub error: Option<String>,
    /// Whether the last error was copied. Reset with the error, so the
    /// confirmation never sits under a message it does not belong to.
    pub copied: bool,
    pub busy: bool,
}

impl LoginDialog {
    pub fn new(stage: Stage, api_id: &str, api_hash: &str, phone: &str) -> Self {
        Self {
            stage,
            api_id: api_id.to_string(),
            api_hash: api_hash.to_string(),
            phone: phone.to_string(),
            ..Default::default()
        }
    }

    pub fn value(&self, field: Field) -> &str {
        match field {
            Field::ApiId => &self.api_id,
            Field::ApiHash => &self.api_hash,
            Field::Phone => &self.phone,
            Field::Code => &self.code,
            Field::Password => &self.password,
        }
    }

    pub fn value_mut(&mut self, field: Field) -> &mut String {
        match field {
            Field::ApiId => &mut self.api_id,
            Field::ApiHash => &mut self.api_hash,
            Field::Phone => &mut self.phone,
            Field::Code => &mut self.code,
            Field::Password => &mut self.password,
        }
    }

    /// Clear whatever the last attempt said, and its copy confirmation with it.
    pub fn clear_error(&mut self) {
        self.error = None;
        self.copied = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stage_asks_for_something() {
        for stage in [
            Stage::Credentials,
            Stage::Phone,
            Stage::Code,
            Stage::Password,
        ] {
            assert!(!stage.fields().is_empty(), "{stage:?} asks for nothing");
            assert!(!stage.title().is_empty());
            assert!(!stage.action().is_empty());
        }
    }

    #[test]
    fn no_stage_names_a_screen_that_does_not_exist() {
        // The only instruction a first run ever got was "…and enter them in
        // Settings", and there is no Settings anywhere in this app.
        for stage in [
            Stage::Credentials,
            Stage::Phone,
            Stage::Code,
            Stage::Password,
        ] {
            assert!(
                !stage.hint().contains("Settings"),
                "{stage:?} sends the user hunting: {}",
                stage.hint()
            );
        }
    }

    #[test]
    fn the_credentials_stage_says_where_to_get_them() {
        let link = Stage::Credentials
            .link()
            .expect("no link to my.telegram.org");
        assert!(link.url.starts_with("https://"), "{}", link.url);
        assert!(link.url.contains("my.telegram.org"), "{}", link.url);
        assert!(
            Stage::Credentials
                .steps()
                .iter()
                .any(|s| s.contains("API development tools")),
            "the steps never name the page the credentials are on"
        );
        assert!(
            Stage::Credentials
                .steps()
                .iter()
                .any(|s| s.contains("api_id") && s.contains("api_hash")),
            "the steps never say which two values to copy"
        );
    }

    #[test]
    fn the_credentials_steps_say_which_fields_do_not_matter() {
        // The form asks for an app title, a short name, a URL and a platform.
        // Three of those are irrelevant and there is nothing on the page that
        // says so, which turns a one-minute task into a decision someone
        // believes they are making permanently about their account.
        let joined = Stage::Credentials.steps().join(" ");
        assert!(
            joined.contains("Platform") || joined.contains("platform"),
            "{joined}"
        );
        assert!(joined.contains("do not matter"), "{joined}");
    }

    #[test]
    fn the_code_stage_repeats_telegrams_own_warning() {
        // This app is shaped exactly like the thing Telegram warns about: a
        // program asking for your login code. Saying so is the difference
        // between a user who is careful here and one who is careful nowhere.
        let joined = Stage::Code.steps().join(" ");
        assert!(
            joined.contains("Never give this code to anyone"),
            "{joined}"
        );
    }

    #[test]
    fn no_step_is_empty_and_only_the_first_stage_links_out() {
        for stage in [
            Stage::Credentials,
            Stage::Phone,
            Stage::Code,
            Stage::Password,
        ] {
            for step in stage.steps() {
                assert!(!step.trim().is_empty(), "{stage:?} has a blank step");
            }
            if let Some(link) = stage.link() {
                assert_eq!(stage, Stage::Credentials, "{stage:?} should not link out");
                assert!(!link.label.trim().is_empty());
            }
        }
    }

    #[test]
    fn secrets_are_masked_and_ordinary_fields_are_not() {
        assert!(Field::Password.masked());
        assert!(Field::ApiHash.masked(), "the api_hash is a credential too");
        assert!(!Field::Phone.masked());
        assert!(!Field::Code.masked());
        assert!(!Field::ApiId.masked(), "the api_id is not secret");
    }

    #[test]
    fn no_masked_field_offers_an_example_of_itself() {
        // A placeholder is drawn in the clear whatever the field's mask says,
        // so a realistic-looking one under `Password` is a credential shape on
        // screen. The `api_hash` keeps an elided example because its *format*
        // is what people get wrong, and `abcdef…` is not one.
        assert_eq!(Field::Password.placeholder(), "");
        assert!(!Field::ApiHash.placeholder().contains(char::is_numeric));
    }

    #[test]
    fn a_dialog_carries_the_values_it_was_opened_with() {
        // Reopening at the phone stage must not lose credentials already
        // entered, or the second attempt asks for everything again.
        let d = LoginDialog::new(Stage::Phone, "12345678", "deadbeef", "+381600000");
        assert_eq!(d.value(Field::ApiId), "12345678");
        assert_eq!(d.value(Field::ApiHash), "deadbeef");
        assert_eq!(d.value(Field::Phone), "+381600000");
        assert_eq!(d.value(Field::Code), "");
        assert!(d.error.is_none());
        assert!(!d.busy);
    }

    #[test]
    fn every_field_is_reachable_for_reading_and_for_typing() {
        // The two accessors must agree about which buffer is which, or a
        // keystroke lands in a field the submit does not read.
        let mut d = LoginDialog::default();
        for field in [
            Field::ApiId,
            Field::ApiHash,
            Field::Phone,
            Field::Code,
            Field::Password,
        ] {
            d.value_mut(field).push_str(field.label());
            assert_eq!(d.value(field), field.label());
        }
    }

    #[test]
    fn clearing_the_error_clears_the_copy_confirmation_with_it() {
        // "Copied" sitting under a message that has since been replaced is a
        // claim that has quietly become false.
        let mut d = LoginDialog::default();
        d.error = Some("PHONE_CODE_INVALID".into());
        d.copied = true;
        d.clear_error();
        assert!(d.error.is_none());
        assert!(!d.copied);
    }

    #[test]
    fn each_stage_shows_only_its_own_fields() {
        assert_eq!(Stage::Phone.fields(), &[Field::Phone]);
        assert_eq!(Stage::Code.fields(), &[Field::Code]);
        assert_eq!(Stage::Password.fields(), &[Field::Password]);
        assert_eq!(Stage::Credentials.fields(), &[Field::ApiId, Field::ApiHash]);
    }

    #[test]
    fn the_stages_are_distinct() {
        // "Is a dialog open?" and "which one?" cannot disagree, because the
        // stage is one value rather than a set of booleans.
        let all = [
            Stage::Credentials,
            Stage::Phone,
            Stage::Code,
            Stage::Password,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
                assert_ne!(a.title(), b.title());
            }
        }
    }
}
