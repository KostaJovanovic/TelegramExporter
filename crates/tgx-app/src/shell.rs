//! The window: nav bar, chat list, settings, queue, log.

use crate::login::{Field, LoginDialog, Stage};
use gpui::prelude::*;
use gpui::{div, px, Context, Div, SharedString, Window};
use gpui_component::input::Input;
use tgx_tg::client::ChatInfo;
use tgx_tg::config::Settings;
use tgx_ui::components::{
    count_text, eyebrow, rule, selection_label, vrule, EmptyState, ListState, NavCell,
};
use tgx_ui::tokens::{metrics, type_scale, Palette};

pub struct Shell {
    /// The tokio side. The UI thread never blocks on it; it drains events.
    bridge: crate::bridge::Bridge,
    palette: Palette,
    settings: Settings,
    signed_in: bool,
    /// Tracked separately from `chats.is_empty()`: the list is empty both
    /// before a sign-in and after one that found nothing, and those two need
    /// opposite instructions.
    loaded: bool,
    exporting: bool,
    chats: Vec<ChatInfo>,
    /// Ticks are held **by chat id**, so they survive re-sorting, regrouping
    /// and filtering.
    selected: std::collections::HashSet<i64>,
    filter: String,
    status: SharedString,
    log: Vec<SharedString>,
    progress: Option<(usize, i64)>,
    /// **One dialog, ever.** An `Option`, not a flag plus a stage, so
    /// "is one open?" and "which one?" cannot disagree.
    login: Option<LoginDialog>,
}

impl Shell {
    /// Build the shell.
    ///
    /// The window is taken to match GPUI's constructor shape; nothing here
    /// needs one, because the login dialog starts closed. [`Self::headless`]
    /// is the same state without it, so the interaction rules below can be
    /// tested without opening a window.
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self::headless()
    }

    fn headless() -> Self {
        let settings = Settings::load();
        let palette = Palette::named(&settings.theme);
        let bridge = crate::bridge::Bridge::new()
            .expect("a tokio runtime is required for any Telegram work");
        Self {
            bridge,
            palette,
            settings,
            signed_in: false,
            loaded: false,
            exporting: false,
            chats: Vec::new(),
            selected: Default::default(),
            filter: String::new(),
            status: "Not signed in".into(),
            log: Vec::new(),
            progress: None,
            login: None,
        }
    }

    /// Open the sign-in dialog, or **raise the existing one**.
    ///
    /// Making a second dialog is what put two modals on top of each
    /// other, which the user experienced as the app freezing the moment
    /// it logged them in.
    fn open_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.login.is_some() {
            return;
        }
        let stage = if self.settings.api_id == 0 || self.settings.api_hash.is_empty() {
            Stage::Credentials
        } else {
            Stage::Phone
        };
        let api_id = if self.settings.api_id == 0 {
            String::new()
        } else {
            self.settings.api_id.to_string()
        };
        let hash = self.settings.api_hash.clone();
        let phone = self.settings.phone.clone();
        self.login = Some(LoginDialog::new(stage, window, cx, &api_id, &hash, &phone));
    }

    /// The rows a filter leaves visible.
    fn visible(&self) -> Vec<&ChatInfo> {
        if self.filter.is_empty() {
            return self.chats.iter().collect();
        }
        let needle = self.filter.to_lowercase();
        self.chats
            .iter()
            .filter(|c| c.title.to_lowercase().contains(&needle))
            .collect()
    }

    fn list_state(&self) -> ListState {
        ListState::decide(
            self.signed_in,
            self.loaded,
            self.chats.len(),
            self.visible().len(),
        )
    }

    /// **All / None / Invert act on the *visible* rows**, so over an empty list
    /// every one is a no-op — and a button that is enabled and does nothing
    /// teaches that the interface is unreliable.
    fn selection_actions_enabled(&self) -> bool {
        !self.visible().is_empty()
    }

    fn selection_total(&self) -> (i64, bool) {
        let mut total = 0i64;
        let mut any_uncounted = false;
        for c in self.chats.iter().filter(|c| self.selected.contains(&c.id)) {
            match c.message_count {
                Some(n) => total += n,
                // A missing count is not a count of zero.
                None => any_uncounted = true,
            }
        }
        (total, any_uncounted)
    }

    fn login_panel(&self, cx: &mut Context<Self>) -> Option<Div> {
        let dialog = self.login.as_ref()?;
        let p = &self.palette;

        let mut card = div()
            .w(px(420.0))
            .bg(p.bg)
            .border_1()
            .border_color(p.hairline)
            .child(
                div()
                    .px(px(20.0))
                    .py(px(16.0))
                    .child(eyebrow(dialog.stage.title(), p)),
            )
            .child(rule(p))
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
                        .child(div().child(SharedString::from(*step))),
                );
            }
            card = card.child(list);
        }

        if let Some(link) = dialog.stage.link() {
            card = card.child(
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
            card = card.child(
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
                            .child(tgx_ui::components::uppercase(field.label())),
                    )
                    .child(Input::new(dialog.state_for(*field))),
            );
        }

        if let Some(err) = &dialog.error {
            card = card.child(
                div()
                    .px(px(20.0))
                    .pt(px(10.0))
                    .text_size(type_scale::TINY)
                    .text_color(p.accent)
                    .child(err.clone()),
            );
        }

        let action = dialog.stage.action();
        let busy = dialog.busy;
        card = card.child(
            div()
                .flex()
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
                        .child(SharedString::from(if busy {
                            "Working\u{2026}"
                        } else {
                            action
                        }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.submit_login(cx);
                            cx.notify();
                        })),
                ),
        );

        Some(
            div()
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
    fn submit_login(&mut self, cx: &mut Context<Self>) {
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
        }

        match stage {
            Stage::Credentials => {
                let (id, hash) = (get(Field::ApiId), get(Field::ApiHash));
                match id.parse::<i64>() {
                    Ok(n) if n > 0 && !hash.is_empty() => {
                        self.settings.api_id = n;
                        self.settings.api_hash = hash;
                        let _ = self.settings.save();
                        if let Some(d) = self.login.as_mut() {
                            d.stage = Stage::Phone;
                        }
                    }
                    _ => {
                        if let Some(d) = self.login.as_mut() {
                            d.error = Some(
                                "An api_id is a number, and an api_hash cannot be blank.".into(),
                            );
                        }
                    }
                }
            }
            Stage::Phone => {
                let phone = get(Field::Phone);
                if phone.is_empty() {
                    if let Some(d) = self.login.as_mut() {
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
            Stage::Code | Stage::Password => {
                let is_code = stage == Stage::Code;
                let secret = get(if is_code {
                    Field::Code
                } else {
                    Field::Password
                });
                if secret.is_empty() {
                    if let Some(d) = self.login.as_mut() {
                        d.error = Some("This cannot be blank.".into());
                    }
                    return;
                }
                if let Some(d) = self.login.as_mut() {
                    d.busy = true;
                }
                let tx = self.bridge.sender();
                let settings = self.settings.clone();
                self.bridge.spawn(async move {
                    crate::actions::finish_login(settings, secret, is_code, tx).await
                });
            }
        }
    }

    // -- painting ----------------------------------------------------------

    fn nav_bar(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        // The bar numbers the steps and only the steps. Stop and Open output
        // folder are tools: unnumbered, and pushed right where the sequence
        // has ended.
        let steps = [
            NavCell::step(1, "Sign in").enabled(!self.exporting),
            NavCell::step(2, "Refresh chats").enabled(self.signed_in && !self.exporting),
            NavCell::step(3, "Start export")
                .enabled(self.signed_in && !self.selected.is_empty() && !self.exporting),
        ];
        let tools = [
            NavCell::tool("Stop").enabled(self.exporting),
            NavCell::tool("Open output folder"),
        ];

        let mut bar = div()
            .flex()
            .items_center()
            .w_full()
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
                            2 => this.start_export(),
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
                            1 => crate::actions::open_output_folder(&this.settings.output_dir),
                            _ => {}
                        }
                        cx.notify();
                    }));
            }
            bar = bar.child(vrule(p)).child(slot);
        }
        bar
    }

    // -- actions -----------------------------------------------------------

    fn start_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Probe the session on disk first: if it is already good, the
        // dialog never opens and the status bar just says who you are.
        let tx = self.bridge.sender();
        let settings = self.settings.clone();
        self.bridge
            .spawn(async move { crate::actions::sign_in(settings, tx).await });
        if !self.signed_in {
            self.open_login(window, cx);
        }
    }

    fn start_refresh(&mut self) {
        let tx = self.bridge.sender();
        let settings = self.settings.clone();
        self.bridge
            .spawn(async move { crate::actions::refresh_chats(settings, tx).await });
    }

    fn start_export(&mut self) {
        // An export is the longer job and it claims the progress bar here.
        // While `exporting` is set, no other handler may write to it.
        self.exporting = true;
        self.progress = Some((0, 0));
        self.status = "Exporting…".into();
        let queue: Vec<ChatInfo> = self
            .chats
            .iter()
            .filter(|c| self.selected.contains(&c.id))
            .cloned()
            .collect();
        let tx = self.bridge.sender();
        let settings = self.settings.clone();
        self.bridge
            .spawn(async move { crate::actions::export(settings, queue, tx).await });
    }

    fn stop(&mut self) {
        // A cancelled export writes no count at all.
        self.exporting = false;
        self.progress = None;
        self.status = "Stopped".into();
    }

    fn chat_row(&self, chat: &ChatInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &self.palette;
        let ticked = self.selected.contains(&chat.id);
        let id = chat.id;
        // Clicking anywhere on a row ticks it.
        let mut row = div()
            .id(("chat", id as usize))
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(16.0))
            .py(px(10.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.selected.remove(&id) {
                    this.selected.insert(id);
                }
                cx.notify();
            }));

        // The tick box: a hairline square, filled when ticked. Square corners.
        row = row.child(
            div()
                .w(px(12.0))
                .h(px(12.0))
                .border_1()
                .border_color(p.hairline)
                .when(ticked, |d| d.bg(p.fg)),
        );

        // A forum is marked by a painted dot, never by a suffix on the stored
        // title — presentation in the string is what the filter then searches.
        if chat.is_forum {
            row = row.child(div().w(px(6.0)).h(px(6.0)).bg(p.accent));
        }

        row.child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .child(
                    div()
                        .text_size(type_scale::SMALL)
                        .text_color(p.fg)
                        .child(SharedString::from(chat.title.clone())),
                )
                .child(
                    div()
                        .text_size(type_scale::TINY)
                        .text_color(p.muted)
                        .child(SharedString::from(chat.kind.label())),
                ),
        )
        .child(
            div()
                .text_size(type_scale::TINY)
                .text_color(p.muted)
                .child(count_text(chat.message_count)),
        )
    }

    fn chat_panel(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let mut panel = div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .child(div().px(px(16.0)).py(px(12.0)).child(eyebrow("Chats", p)))
            .child(rule(p));

        match self.list_state().empty_state(&self.filter) {
            Some(empty) => {
                panel = panel.child(div().flex_1().child(empty.render(p, true)));
            }
            None => {
                let mut list = div().flex().flex_col().flex_1();
                let rows: Vec<ChatInfo> = self.visible().into_iter().cloned().collect();
                for chat in &rows {
                    list = list
                        .child(self.chat_row(chat, cx))
                        .child(div().h(px(1.0)).w_full().bg(p.rule));
                }
                panel = panel.child(list);
            }
        }

        // Nothing offers to do what it cannot: these act on the *visible*
        // rows, so over an empty list every one is a no-op. A button that is
        // enabled and does nothing teaches that the interface is unreliable.
        let live = self.selection_actions_enabled();
        let mut actions = div().flex().gap(px(16.0)).px(px(16.0)).py(px(8.0));
        for (i, label) in ["All", "None", "Invert", "Only forums"].iter().enumerate() {
            let mut cell = div()
                .id(("sel", i))
                .text_size(type_scale::TINY)
                .text_color(if live { p.fg } else { p.muted })
                .child(tgx_ui::components::uppercase(*label));
            if live {
                cell = cell
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let visible: Vec<i64> = this.visible().iter().map(|c| c.id).collect();
                        let forums: Vec<i64> = this
                            .visible()
                            .iter()
                            .filter(|c| c.is_forum)
                            .map(|c| c.id)
                            .collect();
                        match i {
                            0 => this.selected.extend(visible),
                            1 => {
                                for id in visible {
                                    this.selected.remove(&id);
                                }
                            }
                            2 => {
                                for id in visible {
                                    if !this.selected.remove(&id) {
                                        this.selected.insert(id);
                                    }
                                }
                            }
                            3 => this.selected.extend(forums),
                            _ => {}
                        }
                        cx.notify();
                    }));
            }
            actions = actions.child(cell);
        }

        let (total, any_uncounted) = self.selection_total();
        panel.child(rule(p)).child(actions).child(
            div()
                .px(px(16.0))
                .py(px(10.0))
                .text_size(type_scale::TINY)
                .text_color(p.muted)
                .child(SharedString::from(selection_label(
                    self.selected.len(),
                    total,
                    any_uncounted,
                ))),
        )
    }

    fn settings_panel(&self) -> Div {
        let p = &self.palette;
        let rows: Vec<(&str, String)> = vec![
            ("Folder", self.settings.output_dir.clone()),
            (
                "Formats",
                match (self.settings.export_html, self.settings.export_json) {
                    (true, true) => "HTML and JSON".into(),
                    (true, false) => "HTML".into(),
                    (false, true) => "JSON".into(),
                    (false, false) => "none".into(),
                },
            ),
            (
                "Split forum topics",
                if self.settings.split_topics {
                    "on"
                } else {
                    "off"
                }
                .into(),
            ),
            ("Messages per page", self.settings.page_size.to_string()),
            (
                "Size limit",
                match self.settings.size_limit_bytes() {
                    None => "unlimited".into(),
                    Some(_) => format!("{} MB", self.settings.size_limit_mb),
                },
            ),
            (
                "Parallel downloads",
                self.settings.download_concurrency.to_string(),
            ),
        ];

        let mut panel = div()
            .flex()
            .flex_col()
            .w(px(320.0))
            .h_full()
            .child(
                div()
                    .px(px(16.0))
                    .py(px(12.0))
                    .child(eyebrow("Settings", p)),
            )
            .child(rule(p));

        for (label, value) in rows {
            panel = panel
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .gap(px(12.0))
                        .px(px(16.0))
                        .py(px(10.0))
                        .child(
                            div()
                                .text_size(type_scale::SMALL)
                                .text_color(p.fg)
                                .child(SharedString::from(label)),
                        )
                        .child(
                            div()
                                .text_size(type_scale::TINY)
                                .text_color(p.muted)
                                .child(SharedString::from(value)),
                        ),
                )
                .child(div().h(px(1.0)).w_full().bg(p.rule));
        }
        panel
    }

    fn queue_panel(&self) -> Div {
        let p = &self.palette;
        let mut panel = div()
            .flex()
            .flex_col()
            .h(px(180.0))
            .child(div().px(px(16.0)).py(px(12.0)).child(eyebrow("Queue", p)))
            .child(rule(p));

        if self.log.is_empty() {
            panel = panel.child(
                div()
                    .flex_1()
                    .child(EmptyState::new("Nothing queued", None).render(p, false)),
            );
        } else {
            let mut lines = div().flex().flex_col().flex_1().px(px(16.0)).py(px(8.0));
            for line in self.log.iter().rev().take(6) {
                lines = lines.child(
                    div()
                        .text_size(type_scale::TINY)
                        .text_color(p.muted)
                        .child(line.clone()),
                );
            }
            panel = panel.child(lines);
        }
        panel
    }

    fn status_bar(&self) -> Div {
        let p = &self.palette;
        let progress = match self.progress {
            Some((done, total)) if total > 0 => {
                format!("{done} of {total}")
            }
            Some((done, _)) => format!("{done}"),
            None => String::new(),
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .px(px(16.0))
            .py(px(10.0))
            .child(
                div()
                    .text_size(type_scale::TINY)
                    .text_color(p.muted)
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .text_size(type_scale::TINY)
                    .text_color(p.muted)
                    .child(SharedString::from(progress)),
            )
    }
}

impl Shell {
    /// Take whatever the worker has said since the last frame.
    ///
    /// **`exporting` decides who may write to the progress bar.** An export is
    /// the longer job and it claims the bar; while it is set, a counting
    /// handler returns without touching it — otherwise a count finishing
    /// mid-export paints "Counted 12 of 12 chats" over "6,000 of 6,643".
    fn pump(&mut self) {
        use crate::bridge::Event;
        if self.bridge.is_stopped() {
            return;
        }
        for event in self.bridge.drain() {
            match event {
                Event::Status(s) => self.status = s.into(),
                Event::Log(s) => self.log.push(s.into()),
                Event::SignedIn(name) => {
                    self.signed_in = true;
                    // Success just closes the dialog: the status bar
                    // already reads "Signed in: <name>", and a modal box
                    // saying it again is exactly what could end up
                    // ordered behind the window.
                    self.login = None;
                    self.status = format!("Signed in: {name}").into();
                }
                Event::LoginStage(stage) => {
                    if let Some(d) = self.login.as_mut() {
                        d.busy = false;
                        d.stage = stage;
                    }
                }
                Event::LoginFailed(msg) => {
                    if let Some(d) = self.login.as_mut() {
                        d.busy = false;
                        d.error = Some(msg.into());
                    }
                }
                Event::Chats(chats) => {
                    self.loaded = true;
                    let n = chats.len();
                    self.chats = chats;
                    // Ticks are held by id, so a refresh keeps any selection
                    // whose chat is still there and silently drops the rest.
                    let live: std::collections::HashSet<i64> =
                        self.chats.iter().map(|c| c.id).collect();
                    self.selected.retain(|id| live.contains(id));
                    self.status = format!("{n} chats").into();
                }
                Event::Progress { done, total } => {
                    if self.exporting {
                        self.progress = Some((done, total));
                    }
                }
                Event::FloodWait(seconds) => {
                    self.status = format!("Rate limited, waiting {seconds}s").into();
                }
                Event::Finished(msg) => {
                    self.exporting = false;
                    self.progress = None;
                    self.status = msg.into();
                }
                Event::Failed(msg) => {
                    // A cancelled or failed export writes no count at all: a
                    // truncated run must not leave its own length behind as
                    // the size of the chat.
                    self.exporting = false;
                    self.progress = None;
                    self.status = format!("Failed: {msg}").into();
                }
            }
        }
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.pump();
        let p = self.palette;
        let body = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(p.bg)
            .text_color(p.fg)
            .child(self.nav_bar(_cx))
            .child(rule(&p))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .child(self.chat_panel(_cx))
                    .child(vrule(&p))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(self.settings_panel())
                            .child(rule(&p))
                            .child(self.queue_panel()),
                    ),
            )
            .child(rule(&p))
            .child(self.status_bar());

        match self.login_panel(_cx) {
            Some(dialog) => body.child(dialog),
            None => body,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tgx_tg::client::ChatKind;

    fn chat(id: i64, title: &str, count: Option<i64>) -> ChatInfo {
        ChatInfo {
            id,
            title: title.into(),
            kind: ChatKind::Supergroup,
            last_activity: 0,
            is_forum: false,
            message_count: count,
            access_hash: 0,
        }
    }

    fn shell_with(chats: Vec<ChatInfo>) -> Shell {
        let mut s = Shell::headless();
        s.signed_in = true;
        s.loaded = true;
        s.chats = chats;
        s
    }

    #[test]
    fn the_list_opens_on_not_signed_in() {
        // An empty list is the state this app opens on, and blank is not
        // neutral: it reads as broken.
        let s = Shell::headless();
        assert_eq!(s.list_state(), ListState::NotSignedIn);
        assert!(s.list_state().empty_state("").is_some());
    }

    #[test]
    fn signed_in_with_nothing_loaded_says_so() {
        let mut s = Shell::headless();
        s.signed_in = true;
        assert_eq!(s.list_state(), ListState::SignedInNothingLoaded);
    }

    #[test]
    fn a_filter_matching_nothing_is_not_an_empty_account() {
        let mut s = shell_with(vec![chat(1, "news", None)]);
        s.filter = "zzz".into();
        assert_eq!(s.list_state(), ListState::FilterMatchedNothing);
        s.filter.clear();
        assert_eq!(s.list_state(), ListState::Populated);
    }

    #[test]
    fn selection_actions_are_disabled_over_an_empty_list() {
        // Nothing offers to do what it cannot.
        let s = Shell::headless();
        assert!(!s.selection_actions_enabled());
        let populated = shell_with(vec![chat(1, "a", None)]);
        assert!(populated.selection_actions_enabled());
    }

    #[test]
    fn ticks_are_held_by_id_so_they_survive_filtering() {
        let mut s = shell_with(vec![chat(1, "news", None), chat(2, "other", None)]);
        s.filter = "news".into();
        s.selected.insert(1);
        // Filter to something else: the tick is still held.
        s.filter = "other".into();
        assert!(s.selected.contains(&1));
        s.filter.clear();
        assert_eq!(s.visible().len(), 2);
        assert!(s.selected.contains(&1));
    }

    #[test]
    fn the_footer_says_at_least_when_a_selected_chat_is_uncounted() {
        let mut s = shell_with(vec![chat(1, "a", Some(10)), chat(2, "b", None)]);
        s.selected.insert(1);
        let (total, any) = s.selection_total();
        assert_eq!((total, any), (10, false));
        s.selected.insert(2);
        let (total, any) = s.selection_total();
        assert_eq!((total, any), (10, true));
        assert!(selection_label(2, total, any).contains("at least"));
    }

    #[test]
    fn a_zero_count_is_summed_and_a_missing_one_is_not() {
        let mut s = shell_with(vec![chat(1, "empty", Some(0)), chat(2, "unknown", None)]);
        s.selected.insert(1);
        assert_eq!(s.selection_total(), (0, false), "0 is a count");
        s.selected.clear();
        s.selected.insert(2);
        assert_eq!(s.selection_total(), (0, true), "blank is not a count");
    }

    #[test]
    fn start_export_needs_a_signed_in_account_and_a_selection() {
        let mut s = Shell::headless();
        assert!(!(s.signed_in && !s.selected.is_empty()));
        s.signed_in = true;
        assert!(!(s.signed_in && !s.selected.is_empty()));
        s.selected.insert(1);
        assert!(s.signed_in && !s.selected.is_empty());
    }

    #[test]
    fn an_unreadable_theme_setting_still_opens_a_readable_window() {
        let p = Palette::named("chartreuse");
        assert_eq!(p, Palette::dark());
    }
}
