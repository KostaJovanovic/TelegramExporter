//! The sign-in dialog, painted, and the submit that advances it.
//!
//! **One dialog, ever.** The state is a single `Option<LoginDialog>` on the
//! shell — see `crate::login` for why two of these once made the app look
//! frozen the moment it logged you in.

use super::*;
use crate::login::Field;
use eframe::egui::{Sense, Ui};
use tgx_ui::components::{eyebrow, rule, uppercase};
use tgx_ui::tokens::type_scale;

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
        let modal = egui::Modal::new(egui::Id::new("login")).show(ctx, |ui| {
            ui.set_width(420.0);
            let dialog = self.login.as_ref().expect("checked above");
            let stage = dialog.stage;
            let busy = dialog.busy;
            let action = stage.action();

            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.label(eyebrow(stage.title(), &p));
            });
            ui.add_space(16.0);
            rule(ui, &p);

            // Everything that can grow — the hint, the numbered steps, an error
            // of unknown length — goes inside the scrolling body. The action row
            // sits outside it, so Cancel and the action button are reachable at
            // any window height.
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
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                cancel = ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new("Cancel")
                                .font(tgx_ui::fonts::sans(type_scale::SMALL))
                                .color(p.muted),
                        )
                        .sense(Sense::click()),
                    )
                    .clicked();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(20.0);
                    submit = ui
                        .add(
                            egui::Label::new(
                                egui::RichText::new(if busy { "Working…" } else { action })
                                    .font(tgx_ui::fonts::sans(type_scale::SMALL))
                                    .color(if busy { p.muted } else { p.fg }),
                            )
                            .sense(Sense::click()),
                        )
                        .clicked();
                });
            });
            ui.add_space(16.0);

            if cancel {
                // Abandoning the dialog abandons the half-finished credential
                // with it, rather than leaving a live login token sitting
                // around for the rest of the run. The connection is shared and
                // stays up; the token is what must not.
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

        ui.add_space(14.0);
        ui.horizontal_wrapped(|ui| {
            ui.add_space(20.0);
            ui.label(
                egui::RichText::new(stage.hint())
                    .font(tgx_ui::fonts::sans(type_scale::TINY))
                    .color(p.muted),
            );
        });

        // Numbered, because these are steps taken on another site in order, and
        // a paragraph of prose describing a five-step form is how someone ends
        // up on the wrong page deciding what "platform" to pick.
        let steps = stage.steps();
        if !steps.is_empty() {
            ui.add_space(10.0);
            for (i, step) in steps.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(20.0);
                    ui.label(
                        egui::RichText::new(format!("{}", i + 1))
                            .font(tgx_ui::fonts::mono(type_scale::TINY))
                            .color(p.rule),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(*step)
                            .font(tgx_ui::fonts::sans(type_scale::TINY))
                            .color(p.muted),
                    );
                });
                ui.add_space(6.0);
            }
        }

        if let Some(link) = stage.link() {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                if ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(link.label)
                                .font(tgx_ui::fonts::sans(type_scale::TINY))
                                .color(p.accent),
                        )
                        .sense(Sense::click()),
                    )
                    .clicked()
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(link.url));
                }
            });
        }

        for field in stage.fields() {
            let field = *field;
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.label(
                    egui::RichText::new(uppercase(field.label()))
                        .font(tgx_ui::fonts::sans(type_scale::MICRO))
                        .color(p.muted),
                );
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                let width = ui.available_width() - 20.0;
                let Some(dialog) = self.login.as_mut() else {
                    return;
                };
                // `Field::masked` is the only source of truth for which fields
                // are secret. A separate bool here let the two disagree, which
                // is exactly how a credential ends up rendered in plain text.
                ui.add_sized(
                    [width, 26.0],
                    egui::TextEdit::singleline(dialog.value_mut(field))
                        .password(field.masked())
                        .hint_text(field.placeholder())
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
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(20.0);
                if ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(&err)
                                .font(tgx_ui::fonts::sans(type_scale::TINY))
                                .color(p.accent),
                        )
                        .sense(Sense::click()),
                    )
                    .clicked()
                {
                    ui.ctx().copy_text(err.clone());
                    if let Some(d) = self.login.as_mut() {
                        d.copied = true;
                    }
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.label(
                    egui::RichText::new(if copied {
                        "Copied to clipboard"
                    } else {
                        "Click the message to copy it"
                    })
                    .font(tgx_ui::fonts::sans(type_scale::MICRO))
                    .color(p.muted),
                );
            });
        }
        ui.add_space(14.0);
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
