//! The folder chooser for servers, backed by the server's daemon. This Mac
//! uses the system's folder window instead (`choose_local_folder`).

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Enter, Input, InputEvent, InputState, MoveToEnd, MoveToStart};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, DirEntry, EntryKind, FileWritten, PingReply};

use crate::daemon::Daemon;
use crate::files::{join_path, parent_of};
use crate::look::{self, Palette, icon, icon_button, picture};

pub(crate) struct FolderDialogDone(pub Option<String>);

/// A shortcut in the chooser's sidebar, below Home and `/`.
pub(crate) struct Place {
    pub label: SharedString,
    pub path: String,
}

struct Entry {
    name: String,
    dir: bool,
    git: bool,
    sandhill: bool,
}

pub(crate) struct FolderDialog {
    daemon: Rc<Daemon>,
    title: SharedString,
    confirm: SharedString,
    /// Filters the list, or takes a path starting with `/` or `~`.
    field: Entity<InputState>,
    /// The name of a folder being made, while it is typed.
    naming: Option<Entity<InputState>>,
    home: Option<String>,
    places: Vec<Place>,
    path: Option<String>,
    entries: Option<Result<Vec<Entry>, String>>,
    problem: Option<String>,
    hidden: bool,
    /// The picked folder's name in the folder shown.
    selected: Option<String>,
    /// The folder the last single click entered: a double click opens it.
    entered: Option<String>,
    generation: u64,
    list: ScrollHandle,
    _subscription: Subscription,
}

impl EventEmitter<FolderDialogDone> for FolderDialog {}

impl FolderDialog {
    pub(crate) fn new(
        daemon: Rc<Daemon>,
        title: SharedString,
        confirm: SharedString,
        start: Option<String>,
        places: Vec<Place>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Filter, or type a path such as ~/data")
        });
        field.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe(&field, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                // Return then enters the first match.
                this.selected = this.shown(cx).first().map(|e| e.name.clone());
                this.list.scroll_to_item(0);
                cx.notify();
            }
        });
        let ping = daemon.call::<PingReply>(Call::Ping);
        cx.spawn_in(window, async move |this, cx| {
            let ping = ping.await;
            this.update_in(cx, |this, window, cx| {
                match ping {
                    Ok(ping) => {
                        this.home = Some(ping.home.clone());
                        // Do not replace a path the user entered while Ping was pending.
                        if this.path.is_none() {
                            // A start folder that does not exist yet (plots/
                            // before the first save) opens its nearest parent.
                            this.go_to_folder(start.unwrap_or(ping.home), true, None, window, cx);
                        }
                    }
                    Err(err) => this.entries = Some(Err(format!("{err:#}"))),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            daemon,
            title,
            confirm,
            field,
            naming: None,
            home: None,
            places,
            path: None,
            entries: None,
            problem: None,
            hidden: false,
            selected: None,
            entered: None,
            generation: 0,
            list: ScrollHandle::new(),
            _subscription: subscription,
        }
    }

    /// The text in the field when it filters rather than names a path.
    fn filter(&self, cx: &App) -> Option<String> {
        let text = self.field.read(cx).value().trim().to_lowercase();
        (!text.is_empty() && !text.starts_with('/') && !text.starts_with('~')).then_some(text)
    }

    /// The listed entries after the hidden and filter settings, folders first.
    fn shown(&self, cx: &App) -> Vec<&Entry> {
        let Some(Ok(entries)) = &self.entries else {
            return Vec::new();
        };
        let filter = self.filter(cx);
        entries
            .iter()
            .filter(|e| self.hidden || !e.name.starts_with('.'))
            .filter(|e| {
                filter
                    .as_ref()
                    .is_none_or(|f| e.name.to_lowercase().contains(f))
            })
            .collect()
    }

    fn shown_folders(&self, cx: &App) -> Vec<String> {
        self.shown(cx)
            .into_iter()
            .filter(|e| e.dir)
            .map(|e| e.name.clone())
            .collect()
    }

    /// The folder Open would use: the picked one, else the one shown.
    fn target(&self, cx: &App) -> Option<String> {
        let path = self.path.as_ref()?;
        if !matches!(self.entries, Some(Ok(_))) {
            return None;
        }
        Some(match &self.selected {
            Some(name) if self.shown_folders(cx).contains(name) => join_path(path, name),
            _ => path.clone(),
        })
    }

    fn go_to_folder(
        &mut self,
        typed: String,
        climb: bool,
        select: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let typed = typed.trim();
        let path = if let Some(rest) = typed.strip_prefix('~')
            && (rest.is_empty() || rest.starts_with('/'))
        {
            let Some(home) = &self.home else {
                self.problem = Some("The home folder is not available yet.".into());
                cx.notify();
                return;
            };
            format!("{home}{rest}")
        } else if typed.starts_with('/') {
            typed.to_string()
        } else {
            join_path(self.path.as_deref().unwrap_or("/"), typed)
        };
        let path = path.trim_end_matches('/');
        let path = if path.is_empty() { "/" } else { path }.to_string();
        self.path = Some(path.clone());
        self.entries = None;
        self.problem = None;
        self.selected = select;
        self.entered = None;
        self.naming = None;
        self.generation += 1;
        let generation = self.generation;
        self.field.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        let listed = self.daemon.call::<Vec<DirEntry>>(Call::ListDir {
            path,
            sizes: false,
            marks: true,
        });
        cx.spawn_in(window, async move |this, cx| {
            let listed = listed.await;
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                if climb
                    && listed.is_err()
                    && let Some(path) = this.path.clone().filter(|path| path != "/")
                {
                    this.go_to_folder(parent_of(&path), true, None, window, cx);
                    return;
                }
                this.entries = Some(
                    listed
                        .map(|entries| {
                            entries
                                .into_iter()
                                .map(|e| Entry {
                                    dir: e.kind == EntryKind::Dir,
                                    name: e.name,
                                    git: e.git,
                                    sandhill: e.sandhill,
                                })
                                .collect()
                        })
                        .map_err(|err| format!("{err:#}")),
                );
                if let Some(name) = &this.selected
                    && let Some(ix) = this.shown_folders(cx).iter().position(|n| n == name)
                {
                    this.list.scroll_to_item(ix);
                } else {
                    this.list.scroll_to_item(0);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn enter(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = &self.path {
            self.go_to_folder(join_path(path, &name), false, None, window, cx);
        }
    }

    fn go_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.path.clone().filter(|path| path != "/") {
            let child = path.rsplit('/').next().map(str::to_string);
            self.go_to_folder(parent_of(&path), false, child, window, cx);
        }
    }

    fn move_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        let folders = self.shown_folders(cx);
        if folders.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|name| folders.iter().position(|n| n == name));
        let next = match (at, down) {
            (None, true) => 0,
            (None, false) => folders.len() - 1,
            (Some(ix), true) => (ix + 1).min(folders.len() - 1),
            (Some(ix), false) => ix.saturating_sub(1),
        };
        self.selected = Some(folders[next].clone());
        self.list
            .scroll_to_item(next + usize::from(self.naming.is_some()));
        cx.notify();
    }

    fn start_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.path.is_none() || !matches!(self.entries, Some(Ok(_))) {
            return;
        }
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value("untitled folder", window, cx);
            input.select_all(window, cx);
            input
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.naming = Some(input);
        self.problem = None;
        self.list.scroll_to_item(0);
        cx.notify();
    }

    fn make_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(path), Some(naming)) = (self.path.clone(), &self.naming) else {
            return;
        };
        let name = naming.read(cx).value().trim().to_string();
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            self.problem = Some("A folder name can't be empty or contain /.".into());
            cx.notify();
            return;
        }
        let made = self.daemon.call::<FileWritten>(Call::MakeDir {
            path: join_path(&path, &name),
        });
        cx.spawn_in(window, async move |this, cx| {
            let made = made.await;
            this.update_in(cx, |this, window, cx| {
                if this.path.as_ref() != Some(&path) {
                    return;
                }
                match made {
                    Ok(_) => {
                        if name.starts_with('.') {
                            this.hidden = true;
                        }
                        this.go_to_folder(path, false, Some(name), window, cx);
                    }
                    Err(err) => {
                        this.problem = Some(format!("{err:#}"));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn cancel_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.naming = None;
        self.problem = None;
        self.field.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        if let Some(target) = self.target(cx) {
            cx.emit(FolderDialogDone(Some(target)));
        }
    }

    /// Escape and ↑/↓, which the text fields pass on as keys.
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        let handled = match keys.key.as_str() {
            "escape" if self.naming.is_some() => {
                self.cancel_naming(window, cx);
                true
            }
            "escape" => {
                if self.field.read(cx).value().is_empty() {
                    cx.emit(FolderDialogDone(None));
                } else {
                    self.field
                        .update(cx, |input, cx| input.set_value("", window, cx));
                    self.selected = None;
                    cx.notify();
                }
                true
            }
            "up" | "down" if self.naming.is_none() && !keys.modifiers.modified() => {
                self.move_selection(keys.key == "down", cx);
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }

    /// Return enters the picked folder (or goes to a typed path); ⌘Return opens.
    fn press_enter(&mut self, action: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.naming.is_some() {
            self.make_folder(window, cx);
        } else if action.secondary {
            self.finish(cx);
        } else {
            let text = self.field.read(cx).value().trim().to_string();
            if text.starts_with('/') || text.starts_with('~') {
                self.go_to_folder(text, false, None, window, cx);
            } else if let Some(name) = self
                .selected
                .clone()
                .filter(|name| self.shown_folders(cx).contains(name))
            {
                self.enter(name, window, cx);
            }
        }
    }
}

/// The clickable segments of `path` (from `~` when under `home`), with an
/// ellipsis segment standing for what is left out of a long path.
fn crumbs(path: &str, home: Option<&str>) -> Vec<(String, String)> {
    const KEEP: usize = 4;
    let (first, rest) = match home {
        Some(home) if path == home => (("~".to_string(), home.to_string()), ""),
        Some(home) if home != "/" && path.starts_with(&format!("{home}/")) => {
            (("~".to_string(), home.to_string()), &path[home.len() + 1..])
        }
        _ => (
            ("/".to_string(), "/".to_string()),
            path.trim_start_matches('/'),
        ),
    };
    let mut at = first.1.clone();
    let mut all = Vec::new();
    for name in rest.split('/').filter(|n| !n.is_empty()) {
        at = join_path(&at, name);
        all.push((name.to_string(), at.clone()));
    }
    let mut crumbs = vec![first];
    if all.len() > KEEP {
        let cut = all.len() - KEEP;
        crumbs.push(("…".to_string(), all[cut - 1].1.clone()));
        crumbs.extend(all.drain(cut..));
    } else {
        crumbs.extend(all);
    }
    crumbs
}

/// `path` with the home folder written `~`.
fn tilde(path: &str, home: Option<&str>) -> String {
    match home {
        Some(home) if path == home => "~".into(),
        Some(home) if home != "/" && path.starts_with(&format!("{home}/")) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.into(),
    }
}

impl Render for FolderDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let hover = p.ink(7.);
        let picked = p.ink(12.);
        let path = self.path.clone().unwrap_or_default();
        let home = self.home.as_deref();
        let mut rows: Vec<AnyElement> = Vec::new();
        if let Some(naming) = &self.naming {
            rows.push(
                h_flex()
                    .h(px(28.))
                    .px(px(8.))
                    .gap(px(6.))
                    .flex_none()
                    .child(picture(&look::folder_icon("")))
                    .child(div().flex_1().child(Input::new(naming).small()))
                    .into_any_element(),
            );
        }
        let note = |text: String, color: Hsla| {
            div()
                .p(px(8.))
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        match &self.entries {
            None => rows.push(note("Loading…".into(), p.muted)),
            Some(Err(err)) => rows.push(note(err.clone(), p.err)),
            Some(Ok(_)) => {
                let shown = self.shown(cx);
                if shown.is_empty() && self.naming.is_none() {
                    rows.push(note(
                        if self.filter(cx).is_some() {
                            "Nothing here matches".into()
                        } else {
                            "This folder is empty".into()
                        },
                        p.muted,
                    ));
                }
                for (ix, entry) in shown.into_iter().enumerate() {
                    let name = entry.name.clone();
                    let row = h_flex()
                        .id(("folder-dialog-entry", ix))
                        .h(px(28.))
                        .px(px(8.))
                        .gap(px(6.))
                        .flex_none()
                        .rounded(px(look::RADIUS))
                        .child(picture(&if entry.sandhill {
                            "folders/project".to_string()
                        } else if entry.dir {
                            look::folder_icon(&entry.name)
                        } else {
                            look::file_icon(&entry.name)
                        }))
                        .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                        .when(entry.git, |d| {
                            d.child(
                                div()
                                    .id(("folder-dialog-git", ix))
                                    .text_color(p.muted)
                                    .child(icon("git").size(px(13.)))
                                    .tooltip(|w, cx| Tooltip::new("Git repository").build(w, cx)),
                            )
                        });
                    rows.push(if entry.dir {
                        let selected = self.selected.as_ref() == Some(&name);
                        row.cursor_pointer()
                            .when(selected, |d| d.bg(picked))
                            .when(!selected, |d| d.hover(move |d| d.bg(hover)))
                            .when(entry.sandhill, |d| {
                                d.tooltip(|w, cx| Tooltip::new("Sandhill project").build(w, cx))
                            })
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                // The second click of a double click lands on
                                // the folder just entered; the list opens it.
                                if event.click_count() > 1 {
                                    return;
                                }
                                this.enter(name.clone(), window, cx);
                                this.entered = this.path.clone();
                            }))
                            .into_any_element()
                    } else {
                        row.opacity(0.45).into_any_element()
                    });
                }
            }
        }
        let mut sidebar = v_flex().w(px(150.)).flex_none().gap(px(1.));
        let home_place = self
            .home
            .clone()
            .map(|home| ("Home".into(), home, "welcome"));
        let fixed =
            home_place
                .into_iter()
                .chain(std::iter::once(("/".into(), "/".to_string(), "disk")));
        let place_row = |ix: usize, label: SharedString, to: String, mark: AnyElement| {
            let current = to == path;
            let tip: SharedString = to.clone().into();
            h_flex()
                .id(("folder-dialog-place", ix))
                .h(px(26.))
                .px(px(8.))
                .gap(px(6.))
                .rounded(px(look::RADIUS))
                .cursor_pointer()
                .when(current, |d| d.bg(picked))
                .when(!current, |d| d.hover(move |d| d.bg(hover)))
                .child(mark)
                .child(div().flex_1().min_w_0().truncate().child(label))
                .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.go_to_folder(to.clone(), false, None, window, cx)
                }))
        };
        for (ix, (label, to, mark)) in fixed.enumerate() {
            sidebar = sidebar.child(place_row(
                ix,
                label,
                to,
                div()
                    .text_color(p.muted)
                    .child(icon(mark).size(px(14.)))
                    .into_any_element(),
            ));
        }
        if !self.places.is_empty() {
            sidebar = sidebar.child(div().h(px(1.)).my(px(4.)).mx(px(8.)).bg(p.line));
        }
        for (ix, place) in self.places.iter().enumerate() {
            let name = place.path.rsplit('/').next().unwrap_or(&place.path);
            sidebar = sidebar.child(place_row(
                ix + 2,
                place.label.clone(),
                place.path.clone(),
                picture(&look::folder_icon(name)).into_any_element(),
            ));
        }
        let mut trail = h_flex().flex_1().min_w_0().overflow_hidden().gap(px(2.));
        if self.path.is_some() {
            for (ix, (label, to)) in crumbs(&path, home).into_iter().enumerate() {
                if ix > 0 {
                    trail = trail.child(
                        div()
                            .text_color(p.faint)
                            .child(icon("chevron.right").size(px(12.))),
                    );
                }
                let last = to == path;
                trail = trail.child(
                    div()
                        .id(("folder-dialog-crumb", ix))
                        .px(px(5.))
                        .py(px(2.))
                        .flex_none()
                        .rounded(px(look::RADIUS))
                        .cursor_pointer()
                        .text_color(if last { p.text } else { p.muted })
                        .when(last, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .hover(move |d| d.bg(hover))
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.go_to_folder(to.clone(), false, None, window, cx)
                        })),
                );
            }
        }
        let target = self.target(cx);
        let hidden = self.hidden;
        let card = v_flex()
            .id("folder-dialog-card")
            .occlude()
            .w(px(720.))
            .max_w(relative(0.9))
            .max_h(relative(0.9))
            .overflow_y_scroll()
            .gap(px(12.))
            .p(px(20.))
            .rounded(px(look::RADIUS))
            .bg(p.panel)
            .border_1()
            .border_color(p.glass_edge)
            .shadow_lg()
            .text_color(p.text)
            .text_size(px(look::SMALL))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .capture_key_down(cx.listener(Self::key_down))
            .capture_action(cx.listener(Self::press_enter))
            // ⌘↑ and ⌘↓ move through folders unless a name is being typed.
            .capture_action(cx.listener(|this, _: &MoveToStart, window, cx| {
                if this.naming.is_none() {
                    cx.stop_propagation();
                    this.go_up(window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveToEnd, window, cx| {
                if this.naming.is_none() {
                    cx.stop_propagation();
                    if let Some(name) = this.selected.clone() {
                        this.enter(name, window, cx);
                    }
                }
            }))
            .capture_action(cx.listener(|this, _: &crate::NewFolder, window, cx| {
                cx.stop_propagation();
                this.start_naming(window, cx);
            }))
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.title.clone()),
            )
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(
                        icon_button("folder-dialog-up", "arrow.up", &p, false)
                            .tooltip(|w, cx| Tooltip::new("Enclosing folder  ⌘↑").build(w, cx))
                            .on_click(cx.listener(|this, _, window, cx| this.go_up(window, cx))),
                    )
                    .child(trail)
                    .child(
                        icon_button("folder-dialog-new", "folder.new", &p, false)
                            .tooltip(|w, cx| Tooltip::new("New Folder  ⌥⌘N").build(w, cx))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.start_naming(window, cx)),
                            ),
                    )
                    .child(
                        icon_button(
                            "folder-dialog-hidden",
                            if hidden {
                                "context.hide"
                            } else {
                                "context.show"
                            },
                            &p,
                            false,
                        )
                        .when(hidden, |d| d.bg(picked).text_color(p.text))
                        .tooltip(move |w, cx| {
                            Tooltip::new(if hidden {
                                "Hide names starting with a dot"
                            } else {
                                "Show names starting with a dot"
                            })
                            .build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.hidden = !this.hidden;
                            cx.notify();
                        })),
                    ),
            )
            .child(Input::new(&self.field).small())
            .child(
                h_flex().items_start().gap(px(8.)).child(sidebar).child(
                    v_flex()
                        .id("folder-dialog-list")
                        .flex_1()
                        .min_w_0()
                        .h(px(300.))
                        .p(px(4.))
                        .overflow_y_scroll()
                        .track_scroll(&self.list)
                        .bg(p.bg)
                        .border_1()
                        .border_color(p.line)
                        .rounded(px(look::RADIUS))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                if event.click_count > 1
                                    && this.entered.is_some()
                                    && this.entered == this.path
                                {
                                    this.finish(cx);
                                }
                            }),
                        )
                        .children(rows),
                ),
            )
            .when_some(self.problem.clone(), |d, problem| {
                d.child(div().text_color(p.err).child(problem))
            })
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(6.))
                            .text_color(p.muted)
                            .font_family(look::MONO)
                            .when_some(target.clone(), |d, target| {
                                let name = target.rsplit('/').next().unwrap_or("").to_string();
                                d.child(picture(&look::folder_icon(&name)))
                                    .child(div().min_w_0().truncate().child(tilde(&target, home)))
                            }),
                    )
                    .child(
                        Button::new("folder-dialog-cancel")
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(FolderDialogDone(None)))),
                    )
                    .child(
                        Button::new("folder-dialog-confirm")
                            .small()
                            .primary()
                            .label(self.confirm.clone())
                            .disabled(target.is_none())
                            .tooltip("⌘↩")
                            .on_click(cx.listener(|this, _, _, cx| this.finish(cx))),
                    ),
            );
        deferred(
            div()
                .id("folder-dialog-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(p.shadow.opacity(0.35))
                .child(card)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| cx.emit(FolderDialogDone(None))),
                ),
        )
        .with_priority(4)
    }
}

/// Shows the system's folder window and hands back the chosen folder.
pub(crate) fn choose_local_folder(
    prompt: &str,
    cx: &mut App,
) -> impl std::future::Future<Output = Option<String>> + use<> {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some(prompt.to_string().into()),
    });
    async move {
        let paths = picked.await.ok()?.ok()??;
        paths
            .into_iter()
            .next()
            .map(|path| path.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{crumbs, tilde};

    fn labels(path: &str, home: Option<&str>) -> Vec<String> {
        crumbs(path, home).into_iter().map(|(l, _)| l).collect()
    }

    #[test]
    fn crumbs_start_at_home_and_shorten_long_paths() {
        let home = Some("/home/huan");
        assert_eq!(labels("/home/huan", home), ["~"]);
        assert_eq!(labels("/home/huan/data/rna", home), ["~", "data", "rna"]);
        // A sibling whose name starts like home is not under it.
        assert_eq!(labels("/home/huanx", home), ["/", "home", "huanx"]);
        assert_eq!(labels("/", home), ["/"]);
        let long = crumbs("/a/b/c/d/e/f", None);
        assert_eq!(
            long,
            [
                ("/".into(), "/".into()),
                ("…".into(), "/a/b".into()),
                ("c".into(), "/a/b/c".into()),
                ("d".into(), "/a/b/c/d".into()),
                ("e".into(), "/a/b/c/d/e".into()),
                ("f".into(), "/a/b/c/d/e/f".into()),
            ]
        );
    }

    #[test]
    fn tilde_shortens_only_paths_under_home() {
        let home = Some("/home/huan");
        assert_eq!(tilde("/home/huan", home), "~");
        assert_eq!(tilde("/home/huan/data", home), "~/data");
        assert_eq!(tilde("/home/huanx", home), "/home/huanx");
        assert_eq!(tilde("/srv", None), "/srv");
    }
}
