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

use gpui::{AppContext, Entity, SharedString, Window};
use gpui_component::input::InputState;

/// Where the sign-in has got to. There is exactly one of these at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// No `api_id`/`api_hash` yet. **This is the first page of sign-in, not a
    /// Settings screen** — there is no Settings screen, and telling a new user
    /// to look for one sent them hunting for something that does not exist.
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
                 client. Get them from my.telegram.org → API development tools."
            }
            Stage::Phone => "With the country code, e.g. +381…",
            Stage::Code => "Telegram sent a code to your other devices.",
            Stage::Password => "This account has two-factor authentication.",
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
}

/// The dialog's state. Held as `Option<LoginDialog>` on the shell, so "is one
/// open?" and "which one?" cannot disagree.
pub struct LoginDialog {
    pub stage: Stage,
    pub api_id: Entity<InputState>,
    pub api_hash: Entity<InputState>,
    pub phone: Entity<InputState>,
    pub code: Entity<InputState>,
    pub password: Entity<InputState>,
    /// Shown under the fields. Cleared on every submit, so a stale failure
    /// cannot sit under a fresh attempt.
    pub error: Option<SharedString>,
    pub busy: bool,
}

impl LoginDialog {
    pub fn new(
        stage: Stage,
        window: &mut Window,
        cx: &mut gpui::App,
        api_id: &str,
        api_hash: &str,
        phone: &str,
    ) -> Self {
        // `Field::masked` is the only source of truth for which fields are
        // secret. Passing a separate bool here let the two disagree, which
        // is exactly how a credential ends up rendered in plain text.
        fn field(
            window: &mut Window,
            cx: &mut gpui::App,
            which: Field,
            value: &str,
            placeholder: &'static str,
        ) -> Entity<InputState> {
            let value = value.to_string();
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                if which.masked() {
                    state = state.masked(true);
                }
                if !value.is_empty() {
                    state.set_value(value, window, cx);
                }
                state
            })
        }
        Self {
            stage,
            api_id: field(window, cx, Field::ApiId, api_id, "12345678"),
            api_hash: field(window, cx, Field::ApiHash, api_hash, "abcdef…"),
            phone: field(window, cx, Field::Phone, phone, "+381…"),
            code: field(window, cx, Field::Code, "", "12345"),
            password: field(window, cx, Field::Password, "", ""),
            error: None,
            busy: false,
        }
    }

    pub fn state_for(&self, field: Field) -> &Entity<InputState> {
        match field {
            Field::ApiId => &self.api_id,
            Field::ApiHash => &self.api_hash,
            Field::Phone => &self.phone,
            Field::Code => &self.code,
            Field::Password => &self.password,
        }
    }

    pub fn value(&self, field: Field, cx: &gpui::App) -> String {
        self.state_for(field).read(cx).value().to_string()
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
        assert!(Stage::Credentials.hint().contains("my.telegram.org"));
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
