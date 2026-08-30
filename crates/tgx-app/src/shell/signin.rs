//! The sign-in dialog, painted, and the submit that advances it.
//!
//! **One dialog, ever.** The state is a single `Option<LoginDialog>` on the
//! shell — see `crate::login` for why two of these once made the app look
//! frozen the moment it logged you in.

use super::*;
use crate::login::Field;
use eframe::egui::{Align, Layout, Ui};
use tgx_ui::components::{
    action, block, button, eyebrow, field, figure, meta, row, rule, text, title,
};
use tgx_ui::tokens::{space, window};

/// How wide the dialog is. Wide enough for an api_hash on one line.
const DIALOG_W: f32 = 420.0;

impl Shell {
    pub(super) fn login_panel(&mut self, ctx: &Context) {
        if self.login.is_none() {
            return;
        }
        let p = self.palette;

        // **A modal viewport-wide area, not a second window.** A real second
        // window is what could end up ordered behind the first while still
        // holding every click in the application. `egui::Modal` dims what is
        // behind it *and* blocks input to it — the GPUI version had to build
        // that from an `id`-carrying, `occlude`-ing scrim, because a plain
        // `div` with no id takes part in no hit testing at all and so dimmed
        // the window while stopping nothing.
        // The frame carries no margin of its own: the rules under the title and
        // over the action row run the full width of the dialog, as every rule in
        // this window does, and the gutter is put back on the content by `row`
        // and `block`.
        let modal = egui::Modal::new(egui::Id::new("login"))
            .frame(
                egui::Frame::NONE
                    .fill(p.bg)
                    .stroke(egui::Stroke::new(1.0_f32, p.hairline)),
            )
            .show(ctx, |ui| {
                ui.set_width(DIALOG_W);
                let dialog = self.login.as_ref().expect("checked above");
                let stage = dialog.stage;
                let busy = dialog.busy;
                let verb = stage.action();

                ui.add_space(space::STEP);
                row(ui, |ui| ui.label(title(stage.title(), &p)));
                ui.add_space(space::TIGHT);
                rule(ui, &p);

                // Everything that can grow — the hint, the numbered steps, an
                // error of unknown length — goes inside the scrolling body. The
                // action row sits outside it, so Cancel and the action button
                // are reachable at any window height.
                let max_body = (ctx.content_rect().height() * 0.6).max(120.0);
                egui::ScrollArea::vertical()
                    .id_salt("login-body")
                    .max_height(max_body)
                    .auto_shrink([false, true])
                    .show(ui, |ui| self.login_body(ui));

                rule(ui, &p);
                ui.add_space(16.0);
                let mut cancel = false;
                let mut submit = false;
                // A secondary box and a primary one, as the analyser's footer
                // has. This is the one screen in the window where the accent is
                // not on Start export, because while the dialog is up it is the
                // only thing that can be pressed at all.
                ui.add_space(space::STEP);
                row(ui, |ui| {
                    cancel = button(ui, "Cancel", true, false, &p).clicked();
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // **Busy is disabled, not merely greyed.** The label
                        // already said "Working…" while still taking clicks, so
                        // a second press queued a second request behind the one
                        // in flight; `submit_login` refuses it, but the control
                        // should not have offered.
                        let label = if busy { "Working…" } else { verb };
                        submit = button(ui, label, !busy, true, &p).clicked();
                    });
                });
                ui.add_space(16.0);

                if cancel {
                    // Abandoning the dialog abandons the half-finished
                    // credential with it, rather than leaving a live login token
                    // sitting around for the rest of the run. The connection is
                    // shared and stays up; the token is what must not.
                    self.bridge.spawn(crate::actions::forget_pending_login());
                    self.login = None;
                } else if submit {
                    self.submit_login();
                }
            });

        // Escape and a click outside both close it, by the same route Cancel
        // does — an abandoned dialog must not leave the token behind whichever
        // way it was abandoned.
        if modal.should_close() && self.login.is_some() {
            self.bridge.spawn(crate::actions::forget_pending_login());
            self.login = None;
        }
    }

    fn login_body(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let Some(dialog) = self.login.as_ref() else {
            return;
        };
        let stage = dialog.stage;

        ui.add_space(space::STEP);
        block(ui, |ui| ui.label(text(stage.hint(), &p)));

        // Numbered, because these are steps taken on another site in order, and
        // a paragraph of prose describing a five-step form is how someone ends
        // up on the wrong page deciding what "platform" to pick.
        //
        // **A grid, so the prose hangs off the number rather than under it.**
        // These were wrapping rows with the number as the first widget, which
        // means a step that wraps puts its second line under the digit; a
        // two-column grid is the shape the layout was drawn as.
        let steps = stage.steps();
        if !steps.is_empty() {
            ui.add_space(space::STEP);
            block(ui, |ui| {
                egui::Grid::new("login-steps")
                    .num_columns(2)
                    .spacing([space::TIGHT, space::TIGHT])
                    .show(ui, |ui| {
                        for (i, step) in steps.iter().enumerate() {
                            // The digit is `figure`, like every other number in
                            // the window. It was `p.rule` — the divider grey,
                            // which is not a text colour and on the light
                            // appearance is very nearly the page.
                            ui.label(figure(format!("{}", i + 1), &p));
                            ui.label(text(*step, &p));
                            ui.end_row();
                        }
                    });
            });
        }

        if let Some(link) = stage.link() {
            ui.add_space(space::STEP);
            row(ui, |ui| {
                // A link is the accent's one exception, and it is egui's own
                // convention rather than this design's — `hyperlink_color` is
                // set to the same value in `theme::install`.
                let label = egui::RichText::new(link.label)
                    .font(tgx_ui::fonts::sans(window::READING))
                    .color(p.accent);
                if action(ui, label, true).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(link.url));
                }
            });
        }

        // `kind`, not `field`: `components::field` is the text control, and this
        // loop is over the stage's `login::Field`s.
        for kind in stage.fields() {
            let kind = *kind;
            ui.add_space(space::STEP);
            block(ui, |ui| {
                ui.label(eyebrow(kind.label(), &p));
                ui.add_space(space::TIGHT);
                let Some(dialog) = self.login.as_mut() else {
                    return;
                };
                // `Field::masked` is the only source of truth for which fields
                // are secret. A separate bool here let the two disagree, which
                // is exactly how a credential ends up rendered in plain text.
                let width = ui.available_width();
                ui.add(
                    field(dialog.value_mut(kind), &p)
                        .password(kind.masked())
                        .hint_text(kind.placeholder())
                        .desired_width(width),
                );
            });
        }

        let Some(dialog) = self.login.as_ref() else {
            return;
        };
        if let Some(err) = dialog.error.clone() {
            // **Click to copy.** A Telegram RPC error is the one string in this
            // app a user genuinely needs to hand to someone else — it names the
            // constructor that failed. Retyping `AUTH_RESTART caused by
            // auth.sendCode` from a screenshot is how the useful half gets lost.
            let copied = dialog.copied;
            ui.add_space(space::STEP);
            block(ui, |ui| {
                // **Ink, not the accent**, for the same reason a log warning is:
                // this is the only red thing the dialog would contain apart from
                // the button that submits it, and two reds on one small screen
                // is one too many. The message is already the only line here in
                // the text colour.
                if action(ui, text(&err, &p), true).clicked() {
                    ui.ctx().copy_text(err.clone());
                    if let Some(d) = self.login.as_mut() {
                        d.copied = true;
                    }
                }
                ui.add_space(space::TIGHT);
                ui.label(meta(
                    if copied {
                        "Copied to clipboard"
                    } else {
                        "Click the message to copy it"
                    },
                    &p,
                ));
            });
        }
        ui.add_space(space::STEP);
    }

    /// Advance the sign-in by one step.
    pub(super) fn submit_login(&mut self) {
        let Some(dialog) = self.login.as_ref() else {
            return;
        };
        if dialog.busy {
            return;
        }
        let stage = dialog.stage;
        // Read out before anything below takes `&mut self.login`. Only the
        // current stage's fields are looked at, which is also what stops a
        // password typed at one step reaching the request made at another.
        let values: Vec<(Field, String)> = stage
            .fields()
            .iter()
            .map(|f| (*f, dialog.value(*f).trim().to_string()))
            .collect();
        let get = |want: Field| -> String {
            values
                .iter()
                .find(|(f, _)| *f == want)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };

        // A stale failure must not sit under a fresh attempt.
        if let Some(d) = self.login.as_mut() {
            d.clear_error();
        }

        match stage {
            crate::login::Stage::Credentials => {
                let (id, hash) = (get(Field::ApiId), get(Field::ApiHash));
                match id.parse::<i64>() {
                    Ok(n) if n > 0 && !hash.is_empty() => {
                        self.settings.api_id = n;
                        self.settings.api_hash = hash;
                        let _ = self.settings.save();
                        if let Some(d) = self.login.as_mut() {
                            d.stage = crate::login::Stage::Phone;
                        }
                    }
                    _ => {
                        if let Some(d) = self.login.as_mut() {
                            d.copied = false;
                            d.error = Some(
                                "An api_id is a number, and an api_hash cannot be blank.".into(),
                            );
                        }
                    }
                }
            }
            crate::login::Stage::Phone => {
                let phone = get(Field::Phone);
                if phone.is_empty() {
                    if let Some(d) = self.login.as_mut() {
                        d.copied = false;
                        d.error = Some("A phone number is needed, with its country code.".into());
                    }
                    return;
                }
                self.settings.phone = phone;
                let _ = self.settings.save();
                if let Some(d) = self.login.as_mut() {
                    d.busy = true;
                }
                let tx = self.bridge.sender();
                let settings = self.settings.clone();
                self.bridge
                    .spawn(async move { crate::actions::request_code(settings, tx).await });
            }
            crate::login::Stage::Code | crate::login::Stage::Password => {
                let is_code = stage == crate::login::Stage::Code;
                let secret = get(if is_code {
                    Field::Code
                } else {
                    Field::Password
                });
                if secret.is_empty() {
                    if let Some(d) = self.login.as_mut() {
                        d.copied = false;
                        d.error = Some("This cannot be blank.".into());
                    }
                    return;
                }
                if let Some(d) = self.login.as_mut() {
                    d.busy = true;
                }
                let tx = self.bridge.sender();
                self.bridge
                    .spawn(async move { crate::actions::finish_login(secret, is_code, tx).await });
            }
        }
    }
}
