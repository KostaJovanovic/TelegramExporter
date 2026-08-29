//! The frame around the panels: the nav bar and the status bar.
//!
//! The panels themselves are the sibling modules -- chats, run, settings,
//! signin.

use super::*;

impl Shell {
    // -- painting ----------------------------------------------------------
    // (see the panel modules beside this one)

    pub(super) fn nav_bar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.palette;
        // The bar numbers the steps and only the steps. Stop and Open output
        // folder are tools: unnumbered, and pushed right where the sequence
        // has ended.
        let busy = self.exporting || self.counting;
        let steps = [
            // Disabled while the dialog is up: a second click would spawn a
            // second `Session::connect` behind the one already running, and
            // `open_login` would raise the existing dialog while the extra
            // connection carried on in the background.
            NavCell::step(1, "Sign in").enabled(!busy && self.login.is_none()),
            NavCell::step(2, "Refresh chats").enabled(self.signed_in && !busy),
            NavCell::step(3, "Start export")
                .enabled(self.signed_in && !self.selected.is_empty() && !busy),
        ];
        let tools = [
            NavCell::tool("Stop").enabled(busy),
            NavCell::tool("Open output folder"),
        ];

        let mut bar = div()
            .flex()
            .items_center()
            .w_full()
            .flex_none()
            .h(metrics::NAV_HEIGHT)
            .bg(p.bg);

        for (i, cell) in steps.iter().enumerate() {
            let live = cell.enabled;
            let mut slot = div().id(("step", i)).px(px(16.0)).child(cell.render(p));
            if live {
                slot = slot
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        match i {
                            0 => this.start_sign_in(window, cx),
                            1 => this.start_refresh(),
                            2 => this.start_export(window, cx),
                            _ => {}
                        }
                        cx.notify();
                    }));
            }
            bar = bar.child(slot).child(vrule(p));
        }

        bar = bar.child(div().flex_1());

        for (i, cell) in tools.iter().enumerate() {
            let live = cell.enabled;
            let mut slot = div().id(("tool", i)).px(px(16.0)).child(cell.render(p));
            if live {
                slot = slot
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        match i {
                            0 => this.stop(),
                            // Reported, not discarded. A silent spawn failure
                            // is what let `explorer.exe` be looked for in
                            // System32 — where it is not — for as long as
                            // nobody happened to watch the button do nothing.
                            1 => {
                                if let Err(e) =
                                    crate::actions::open_output_folder(&this.settings.output_dir)
                                {
                                    this.journal.warn(e);
                                    this.log_copied = false;
                                }
                            }
                            _ => {}
                        }
                        cx.notify();
                    }));
            }
            bar = bar.child(vrule(p)).child(slot);
        }

        // **The chip names the theme it switches *to***, like the site's, so it
        // reads as an action rather than as a label for where you already are.
        bar.child(vrule(p)).child(
            div()
                .id("theme")
                .px(px(16.0))
                .cursor_pointer()
                .child(tgx_ui::components::caps(
                    other_theme(&self.settings.theme),
                    tgx_ui::tokens::type_scale::MICRO,
                    p.muted,
                ))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_theme(window, cx))),
        )
    }

    /// Swap the appearance.
    ///
    /// **A token swap, not a rebuild.** Every colour in this window comes from
    /// the palette, so re-reading it repaints the lot; the borrowed components
    /// need one extra call because they paint from `gpui-component`'s own
    /// global rather than from ours — see [`crate::theme`].
    pub(super) fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.theme = other_theme(&self.settings.theme).into();
        self.palette = Palette::named(&self.settings.theme);
        crate::theme::apply(&self.palette, cx);
        self.commit_settings(window, cx);
    }

    pub(super) fn status_bar(&self) -> gpui::Div {
        let p = &self.palette;
        // Counting reports in chats and an export in messages, and the export
        // owns the line whenever it is running.
        let right = if self.exporting {
            self.queue
                .fraction()
                .map(|f| format!("{:.0}%", f * 100.0))
                .unwrap_or_default()
        } else {
            match self.count_progress {
                Some((done, total)) => format!("{done} of {total}"),
                None => String::new(),
            }
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .w_full()
            .px(px(16.0))
            .py(px(10.0))
            .child(
                div()
                    .flex_1()
                    // A long status must push nothing off the bar; a flex child
                    // is allowed to shrink below its content only with this.
                    .min_w_0()
                    // **Not letterspaced, deliberately.** `eyebrow` goes
                    // through `components::tracked`, which lays out one `div`
                    // per character with `flex_none` children — its own doc
                    // says never to put a string of unknown length through it,
                    // and this line carries chat titles and raw RPC errors. It
                    // would overflow the bar rather than truncate, and rebuild
                    // a div per character on every progress event.
                    .truncate()
                    .text_size(tgx_ui::tokens::type_scale::MICRO)
                    .text_color(p.muted)
                    .child(tgx_ui::components::uppercase(self.status.clone())),
            )
            // The right-hand figure is ours and short, so it can be tracked.
            .child(eyebrow(right, p))
    }
}
