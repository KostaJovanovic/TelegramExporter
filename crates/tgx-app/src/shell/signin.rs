//! The sign-in card.
//!
//! **One dialog, ever** — see [`crate::login`] for why. What is fixed here is
//! the shape it is painted in: the card was a fixed 420px wide with no height
//! cap, so on a short window it overflowed both edges with its action button
//! off screen and no way to reach it. A modal you cannot dismiss and cannot
//! scroll is the same failure as the two stacked modals it replaced.

use super::Shell;
use crate::login::Field;
use gpui::prelude::*;
use gpui::{div, px, relative, Context, Div, SharedString};
use gpui_component::input::Input;
use gpui_component::scroll::Scrollbar;
use tgx_ui::components::{eyebrow, rule, uppercase};
use tgx_ui::tokens::type_scale;

impl Shell {
    pub(super) fn login_panel(&self, cx: &mut Context<Self>) -> Option<gpui::Stateful<Div>> {
        let dialog = self.login.as_ref()?;
        let p = &self.palette;

        // Everything that can grow — the hint, the numbered steps, an error of
        // unknown length — goes inside the scrolling body. The header and the
        // action row sit outside it, so Cancel and the action button are
        // reachable at any window height.
        let mut body = div()
            .id("login-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&dialog.scroll)
            .child(
                div()
                    .px(px(20.0))
                    .pt(px(14.0))
                    .text_size(type_scale::TINY)
                    .text_color(p.muted)
                    .child(SharedString::from(dialog.stage.hint())),
            );

        // Numbered, because these are steps taken on another site in order,
        // and a paragraph of prose describing a five-step form is how someone
        // ends up on the wrong page deciding what "platform" to pick.
        let steps = dialog.stage.steps();
        if !steps.is_empty() {
            let mut list = div()
                .px(px(20.0))
                .pt(px(10.0))
                .flex()
                .flex_col()
                .gap(px(6.0));
            for (i, step) in steps.iter().enumerate() {
                list = list.child(
                    div()
                        .flex()
                        .w_full()
                        .gap(px(8.0))
                        .text_size(type_scale::TINY)
                        .text_color(p.muted)
                        .child(
                            div()
                                .w(px(14.0))
                                .flex_none()
                                .text_color(p.rule)
                                .child(SharedString::from(format!("{}", i + 1))),
                        )
                        // `flex_1` to take the rest of the row and `min_w_0` to
                        // be *allowed* to. A flex child's automatic minimum
                        // size is its content, so without this a long line
                        // pushes the row wider than the card instead of
                        // wrapping inside it — the text runs off the panel.
                        .child(div().flex_1().min_w_0().child(SharedString::from(*step))),
                );
            }
            body = body.child(list);
        }

        if let Some(link) = dialog.stage.link() {
            body = body.child(
                div().px(px(20.0)).pt(px(12.0)).child(
                    div()
                        .id("login-link")
                        .text_size(type_scale::TINY)
                        .text_color(p.accent)
                        .cursor_pointer()
                        .child(SharedString::from(link.label))
                        .on_click(cx.listener(move |_, _, _, cx| cx.open_url(link.url))),
                ),
            );
        }

        for field in dialog.stage.fields() {
            body = body.child(
                div()
                    .px(px(20.0))
                    .pt(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(type_scale::MICRO)
                            .text_color(p.muted)
                            .child(uppercase(field.label())),
                    )
                    .child(Input::new(dialog.state_for(*field))),
            );
        }

        if let Some(err) = &dialog.error {
            // **Click to copy.** A Telegram RPC error is the one string in this
            // app a user genuinely needs to hand to someone else — it names the
            // constructor that failed — and it arrives in a modal with nothing
            // selectable in it. Retyping `AUTH_RESTART caused by auth.sendCode`
            // from a screenshot is how the useful half gets lost.
            let copyable = err.to_string();
            let copied = dialog.copied;
            body = body.child(
                div()
                    .px(px(20.0))
                    .pt(px(10.0))
                    .pb(px(4.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .id("login-error")
                            .w_full()
                            .min_w_0()
                            .text_size(type_scale::TINY)
                            .text_color(p.accent)
                            .cursor_pointer()
                            .child(err.clone())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    copyable.clone(),
                                ));
                                if let Some(d) = this.login.as_mut() {
                                    d.copied = true;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_size(type_scale::MICRO)
                            .text_color(p.muted)
                            .child(SharedString::from(if copied {
                                "Copied to clipboard"
                            } else {
                                "Click the message to copy it"
                            })),
                    ),
            );
        }

        let action = dialog.stage.action();
        let busy = dialog.busy;
        let card = div()
            .flex()
            .flex_col()
            .w(px(420.0))
            // A cap, not a height: the card is as tall as its content until the
            // window is too short for it, and then it scrolls instead of
            // growing past both edges.
            .max_h(relative(0.9))
            .bg(p.bg)
            .border_1()
            .border_color(p.hairline)
            .child(
                div()
                    .flex_none()
                    .px(px(20.0))
                    .py(px(16.0))
                    .child(eyebrow(dialog.stage.title(), p)),
            )
            .child(rule(p))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(body)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .child(Scrollbar::vertical(&dialog.scroll)),
                    ),
            )
            .child(rule(p))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .justify_between()
                    .px(px(20.0))
                    .py(px(16.0))
                    .child(
                        div()
                            .id("login-cancel")
                            .text_size(type_scale::SMALL)
                            .text_color(p.muted)
                            .cursor_pointer()
                            .child(SharedString::from("Cancel"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                // Abandoning the dialog abandons the
                                // half-finished credential with it, rather than
                                // leaving a live login token sitting around for
                                // the rest of the run. The connection is shared
                                // and stays up; the token is what must not.
                                this.bridge.spawn(crate::actions::forget_pending_login());
                                this.login = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("login-go")
                            .text_size(type_scale::SMALL)
                            .text_color(if busy { p.muted } else { p.fg })
                            .cursor_pointer()
                            .child(SharedString::from(if busy { "Working…" } else { action }))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.submit_login(cx);
                                cx.notify();
                            })),
                    ),
            );

        Some(
            div()
                // **`id` and `occlude` are what make this modal.** A plain
                // `div` with no id takes part in no hit testing at all, so a
                // scrim built without them dims the window and stops nothing:
                // clicks land on the nav bar, on chat rows and on settings
                // behind it, and "Refresh chats" fires in the middle of a
                // sign-in. Trading two stacked modals for one that blocks
                // nothing is not a fix.
                .id("login-scrim")
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                // A scrim, not a second window. A real second window is what
                // could end up ordered behind the first while still holding
                // every click in the application.
                .bg(gpui::hsla(0.0, 0.0, 0.0, 0.55))
                .child(card),
        )
    }

    /// Advance the sign-in by one step.
    pub(super) fn submit_login(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.login.as_ref() else {
            return;
        };
        if dialog.busy {
            return;
        }
        let stage = dialog.stage;
        let values: Vec<(Field, String)> = stage
            .fields()
            .iter()
            .map(|f| (*f, dialog.value(*f, cx)))
            .collect();
        let get = |want: Field| -> String {
            values
                .iter()
                .find(|(f, _)| *f == want)
                .map(|(_, v)| v.trim().to_string())
                .unwrap_or_default()
        };

        // A stale failure must not sit under a fresh attempt.
        if let Some(d) = self.login.as_mut() {
            d.error = None;
            d.copied = false;
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
