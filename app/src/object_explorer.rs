//! Live, daemon-backed object tabs. Paths are opaque daemon names, never code.
//! Accessors are copied verbatim; this module does not generate R/Python syntax.

use std::collections::{BTreeSet, HashMap, HashSet};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, Variable, VariableMatch, VariableMatches, Variables};

use crate::explorer::Explorer;
use crate::look::{self, Palette, icon, icon_button, pane};
use crate::workbench::Workbench;

type Path = Vec<String>;

#[derive(Default)]
pub(crate) struct ObjectExplorers {
    tabs: Vec<ObjectTab>,
    pub(crate) shown: Option<u64>,
    next_id: u64,
}

impl ObjectExplorers {
    pub(crate) fn is_shown(&self) -> bool {
        self.shown.is_some()
    }
}

#[derive(Default)]
struct Listing {
    nodes: Vec<Variable>,
    listed_len: usize,
    more: u64,
    loaded: bool,
}

struct ObjectTab {
    id: u64,
    session: String,
    language: String,
    root: Path,
    variable: Variable,
    query: Entity<InputState>,
    listings: HashMap<Path, Listing>,
    expanded: HashSet<Path>,
    pending: HashSet<Path>,
    errors: HashMap<Path, String>,
    matches: Option<Vec<VariableMatch>>,
    visible: HashSet<Path>,
    matched: HashSet<Path>,
    complete: bool,
    searching: bool,
    search_error: Option<String>,
    busy: bool,
    revision: u64,
    search_revision: u64,
    scroll: ScrollHandle,
    _subscription: Subscription,
}

/// An old daemon's missing accessor must not produce a copy action.
fn copy_accessor(variable: &Variable) -> Option<&str> {
    variable.accessor.as_deref()
}

/// Only ancestors inside this browser's root are expanded, not matching leaves.
fn match_expansions(root: &[String], paths: impl IntoIterator<Item = Path>) -> BTreeSet<Path> {
    let mut expanded = BTreeSet::new();
    for path in paths {
        if !path.starts_with(root) {
            continue;
        }
        for depth in root.len()..path.len() {
            expanded.insert(path[..depth].to_vec());
        }
    }
    expanded
}

/// Supplement capped listings with exact search metadata. Rebuilding the
/// supplement removes old-query nodes without disturbing loaded siblings.
fn merge_match_nodes(
    listings: &mut HashMap<Path, Listing>,
    root: &[String],
    matches: Option<&[VariableMatch]>,
) -> HashSet<Path> {
    for listing in listings.values_mut() {
        listing.nodes.truncate(listing.listed_len);
    }
    let mut visible = HashSet::new();
    let Some(matches) = matches else {
        return visible;
    };
    for hit in matches {
        if !hit.path.starts_with(root) {
            continue;
        }
        for depth in root.len()..=hit.path.len() {
            visible.insert(hit.path[..depth].to_vec());
        }
        // Unknown intermediate nodes have no invented type, value or accessor.
        for depth in root.len() + 1..=hit.path.len() {
            let parent = hit.path[..depth - 1].to_vec();
            let name = &hit.path[depth - 1];
            let listing = listings.entry(parent).or_default();
            let leaf = depth == hit.path.len();
            let existing = listing.nodes.iter().position(|node| node.name == *name);
            if let Some(index) = existing {
                if index < listing.listed_len {
                    continue;
                }
                if !leaf {
                    listing.nodes[index].children = true;
                    continue;
                }
            }
            let node = Variable {
                name: name.clone(),
                kind: String::new(),
                class: if leaf {
                    hit.class.clone()
                } else {
                    String::new()
                },
                size: None,
                children: !leaf || hit.children,
                table: leaf && hit.table,
                summary: if leaf {
                    hit.summary.clone()
                } else {
                    String::new()
                },
                accessor: leaf.then(|| hit.accessor.clone()),
            };
            if let Some(index) = existing {
                listing.nodes[index] = node;
            } else {
                listing.nodes.push(node);
            }
        }
    }
    visible
}

impl ObjectTab {
    fn merge_matches(&mut self) {
        self.visible = merge_match_nodes(&mut self.listings, &self.root, self.matches.as_deref());
        self.matched = self
            .matches
            .iter()
            .flatten()
            .map(|hit| hit.path.clone())
            .collect();
    }
}

impl Workbench {
    /// Append this to each existing Variables row. Neither action executes code.
    pub(crate) fn variable_object_actions(
        &self,
        variable: &Variable,
        path: &[String],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let mut actions = h_flex().flex_none().gap(px(2.));
        if let Some(accessor) = copy_accessor(variable) {
            let code = accessor.to_string();
            actions = actions.child(
                icon_button(
                    SharedString::from(format!("copy-accessor-{path:?}")),
                    "edit.copy",
                    &p,
                    false,
                )
                .tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Copy access code").build(w, cx)
                })
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.stop_propagation();
                    cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                })),
            );
        }
        if variable.children {
            let path = path.to_vec();
            let variable = variable.clone();
            actions = actions.child(
                icon_button(
                    SharedString::from(format!("open-object-{path:?}")),
                    "search",
                    &p,
                    false,
                )
                .tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Open in Object Explorer")
                        .build(w, cx)
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.open_object_explorer(path.clone(), variable.clone(), window, cx);
                })),
            );
        }
        actions.into_any_element()
    }

    pub(crate) fn open_object_explorer(
        &mut self,
        root: Path,
        variable: Variable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !variable.children || root.is_empty() {
            return;
        }
        let Some(session) = self.vars.session.clone() else {
            return;
        };
        if self.daemon().is_none() {
            return;
        }
        if let Some(id) = self
            .objects
            .tabs
            .iter()
            .find(|tab| tab.session == session && tab.root == root)
            .map(|tab| tab.id)
        {
            self.show_object_explorer(id, cx);
            return;
        }
        let language = self
            .active_kernel()
            .and_then(|kernel| self.kernels.iter().find(|spec| spec.name == kernel))
            .map_or_else(
                || "python".to_string(),
                |spec| spec.language.to_ascii_lowercase(),
            );
        let id = self.objects.next_id;
        self.objects.next_id += 1;
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search names and values"));
        let subscription = cx.subscribe(&query, move |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.search_object_explorer(id, cx);
            }
        });
        self.objects.tabs.push(ObjectTab {
            id,
            session,
            language,
            root: root.clone(),
            variable,
            query,
            listings: HashMap::new(),
            expanded: HashSet::from([root.clone()]),
            pending: HashSet::new(),
            errors: HashMap::new(),
            matches: None,
            visible: HashSet::new(),
            complete: true,
            searching: false,
            busy: false,
            matched: HashSet::new(),
            search_error: None,
            revision: 0,
            search_revision: 0,
            scroll: ScrollHandle::new(),
            _subscription: subscription,
        });
        self.show_object_explorer(id, cx);
        self.reload_object_explorer(id, cx);
    }

    fn show_object_explorer(&mut self, id: u64, cx: &mut Context<Self>) {
        if !self.objects.tabs.iter().any(|tab| tab.id == id) {
            return;
        }
        self.reveal_middle();
        self.objects.shown = Some(id);
        self.explorer_shown = false;
        self.diff_shown = false;
        self.media_shown = None;
        if self.panel_collapsed(crate::panes::Panel::Editor) {
            self.middle_vertical = crate::panes::Vertical::Split;
        }
        cx.notify();
    }

    pub(crate) fn hide_object_explorer(&mut self) {
        self.objects.shown = None;
    }

    pub(crate) fn clear_object_explorers(&mut self) {
        // Preserve monotonically increasing IDs so outstanding replies cannot
        // land in a new project's tab even before its connection changes.
        self.objects.tabs.clear();
        self.objects.shown = None;
    }

    pub(crate) fn close_shown_object_explorer(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.objects.shown {
            self.close_object_explorer(id, cx);
        }
    }

    fn close_object_explorer(&mut self, id: u64, cx: &mut Context<Self>) {
        let index = self.objects.tabs.iter().position(|tab| tab.id == id);
        self.objects.tabs.retain(|tab| tab.id != id);
        if self.objects.shown == Some(id) {
            self.objects.shown = index.and_then(|index| {
                self.objects
                    .tabs
                    .get(index.min(self.objects.tabs.len().saturating_sub(1)))
                    .map(|tab| tab.id)
            });
            if self.objects.shown.is_none() && self.open.is_none() {
                self.explorer_shown = self.explorer.is_some();
            }
        }
        cx.notify();
    }

    /// Call for every non-quiet run's start/end, including background sessions.
    pub(crate) fn object_explorers_run(
        &mut self,
        session: &str,
        busy: bool,
        cx: &mut Context<Self>,
    ) {
        for tab in &mut self.objects.tabs {
            if tab.session == session {
                tab.busy = busy;
                if busy {
                    tab.revision += 1;
                    tab.search_revision += 1;
                    tab.pending.clear();
                    tab.searching = false;
                }
            }
        }
        if !busy {
            self.refresh_object_explorers(session, cx);
        }
        cx.notify();
    }

    pub(crate) fn refresh_object_explorers(&mut self, session: &str, cx: &mut Context<Self>) {
        let ids: Vec<_> = self
            .objects
            .tabs
            .iter()
            .filter(|tab| tab.session == session)
            .map(|tab| tab.id)
            .collect();
        for id in ids {
            self.reload_object_explorer(id, cx);
        }
    }

    fn reload_object_explorer(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(tab) = self.objects.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        if tab.busy {
            return;
        }
        tab.revision += 1;
        tab.search_revision += 1;
        tab.pending.clear();
        tab.errors.clear();
        tab.listings.clear();
        let mut paths = BTreeSet::from([tab.root.clone(), tab.root[..tab.root.len() - 1].to_vec()]);
        paths.extend(tab.expanded.iter().cloned());
        self.load_object_paths(id, paths.into_iter().collect(), cx);
        self.search_object_explorer(id, cx);
    }

    fn load_object_paths(&mut self, id: u64, paths: Vec<Path>, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else { return };
        let Some(tab) = self.objects.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        if tab.busy {
            return;
        }
        let paths: Vec<_> = paths
            .into_iter()
            .filter(|path| tab.pending.insert(path.clone()))
            .collect();
        if paths.is_empty() {
            return;
        }
        let session = tab.session.clone();
        let revision = tab.revision;
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            // One quiet kernel request at a time for this batch.
            for path in paths {
                let valid = this
                    .update(cx, |this, _| {
                        this.generation == generation
                            && this
                                .objects
                                .tabs
                                .iter()
                                .any(|tab| tab.id == id && tab.revision == revision)
                    })
                    .unwrap_or(false);
                if !valid {
                    break;
                }
                let result = daemon
                    .call_in_order::<Variables>(Call::ListVariables {
                        session: session.clone(),
                        path: path.clone(),
                    })
                    .await;
                this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    let Some(tab) = this
                        .objects
                        .tabs
                        .iter_mut()
                        .find(|tab| tab.id == id && tab.revision == revision)
                    else {
                        return;
                    };
                    tab.pending.remove(&path);
                    match result {
                        Ok(listed) => {
                            tab.errors.remove(&path);
                            if path == tab.root[..tab.root.len() - 1]
                                && let Some(root) = listed
                                    .variables
                                    .iter()
                                    .find(|node| node.name == tab.variable.name)
                            {
                                tab.variable = root.clone();
                            }
                            let listed_len = listed.variables.len();
                            tab.listings.insert(
                                path,
                                Listing {
                                    nodes: listed.variables,
                                    listed_len,
                                    more: listed.more,
                                    loaded: true,
                                },
                            );
                            tab.merge_matches();
                        }
                        Err(error) => {
                            tab.errors
                                .insert(path, format!("Could not list object: {error:#}"));
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
    }

    fn toggle_object_node(&mut self, id: u64, path: Path, cx: &mut Context<Self>) {
        let Some(tab) = self.objects.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        if tab.expanded.remove(&path) {
            cx.notify();
            return;
        }
        tab.expanded.insert(path.clone());
        if !tab
            .listings
            .get(&path)
            .is_some_and(|listing| listing.loaded)
        {
            self.load_object_paths(id, vec![path], cx);
        }
        cx.notify();
    }

    fn search_object_explorer(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else { return };
        let Some(tab) = self.objects.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        tab.search_revision += 1;
        let search_revision = tab.search_revision;
        let query = tab.query.read(cx).value().trim().to_string();
        tab.matches = None;
        tab.merge_matches();
        tab.search_error = None;
        tab.complete = true;
        tab.searching = !query.is_empty() && !tab.busy;
        if !tab.searching {
            cx.notify();
            return;
        }
        let session = tab.session.clone();
        let root = tab.root.clone();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(200))
                .await;
            let valid = this
                .update(cx, |this, _| {
                    this.generation == generation
                        && this
                            .objects
                            .tabs
                            .iter()
                            .any(|tab| tab.id == id && tab.search_revision == search_revision)
                })
                .unwrap_or(false);
            if !valid {
                return;
            }
            let result = daemon
                .call_in_order::<VariableMatches>(Call::FindVariables {
                    session,
                    query,
                    root,
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                let Some(tab) = this
                    .objects
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == id && tab.search_revision == search_revision)
                else {
                    return;
                };
                tab.searching = false;
                match result {
                    Ok(found) => {
                        let expanded = match_expansions(
                            &tab.root,
                            found.matches.iter().map(|hit| hit.path.clone()),
                        );
                        tab.expanded.extend(expanded.iter().cloned());
                        tab.complete = found.complete;
                        tab.matches = Some(found.matches);
                        tab.merge_matches();
                        let paths = expanded
                            .into_iter()
                            .filter(|path| {
                                !tab.listings.get(path).is_some_and(|listing| listing.loaded)
                            })
                            .collect();
                        this.load_object_paths(id, paths, cx);
                    }
                    Err(error) => {
                        tab.search_error = Some(format!("Could not search object: {error:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn object_node_table(
        &mut self,
        id: u64,
        path: Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.daemon() else { return };
        let Some(tab) = self.objects.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let session = tab.session.clone();
        let language = tab.language.clone();
        let project = self.project.clone();
        self.reveal_middle();
        self.explorer =
            Some(cx.new(|cx| Explorer::new(daemon, session, language, path, project, window, cx)));
        if let Some(explorer) = &self.explorer {
            cx.subscribe_in(
                explorer,
                window,
                |this, _, _: &crate::explorer::FocusChanged, window, cx| {
                    this.report_focus(window, cx);
                },
            )
            .detach();
        }
        self.hide_object_explorer();
        self.explorer_shown = true;
        self.diff_shown = false;
        cx.notify();
    }

    pub(crate) fn object_tab_elements(
        &self,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let p = *Palette::get(cx);
        self.objects
            .tabs
            .iter()
            .map(|tab| {
                let id = tab.id;
                let tip = format!("Object Explorer: {}", tab.root.join(" / "));
                let picked = expanded && self.objects.shown == Some(id);
                let lead = icon("search").size(px(14.)).into_any_element();
                let (lead, trailing) = if expanded {
                    look::place_close(
                        lead,
                        look::tab_close(
                            SharedString::from(format!("object-close-{id}")),
                            picked,
                            &p,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_object_explorer(id, cx);
                        })),
                        picked,
                    )
                } else {
                    (lead, None)
                };
                Self::tab(
                    SharedString::from(format!("object-tab-{id}")),
                    Some(lead),
                    if expanded {
                        tab.variable.name.clone()
                    } else {
                        String::new()
                    },
                    picked,
                    &p,
                )
                .flex_none()
                .when(!expanded, |d| d.px(px(5.)).gap_0())
                .tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                })
                .when_some(trailing, |d, close| d.pr(px(3.)).child(close))
                .on_click(cx.listener(move |this, _, _, cx| this.show_object_explorer(id, cx)))
                .into_any_element()
            })
            .collect()
    }

    pub(crate) fn render_object_explorer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tab = self
            .objects
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == self.objects.shown)?;
        let p = *Palette::get(cx);
        let id = tab.id;
        let mut rows = Vec::new();
        self.object_node_rows(tab, &tab.root, &tab.variable, &mut rows, cx);
        let heading = h_flex()
            .h(px(27.))
            .border_b_1()
            .border_color(p.line)
            .child(div().w(px(220.)).flex_none().px(px(12.)).child("Name"))
            .child(div().w(px(120.)).flex_none().px(px(8.)).child("Type"))
            .child(div().flex_1().px(px(8.)).child("Value"));
        Some(pane(&p).flex_1().h_full()
            .child(self.render_tabs(cx))
            .child(h_flex().flex_none().gap(px(6.)).p(px(8.))
                .child(div().flex_1().min_w_0().child(Input::new(&tab.query).small()))
                .child(icon_button("object-refresh", "refresh", &p, false)
                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Refresh object").build(w, cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.reload_object_explorer(id, cx)))))
            .when(tab.busy, |d| d.child(div().px(px(12.)).text_color(p.muted).child("Kernel is running. The object will refresh when it finishes.")))
            .when(tab.searching, |d| d.child(div().px(px(12.)).text_color(p.muted).child("Searching names and values…")))
            .when_some(tab.search_error.clone(), |d, error| d.child(div().px(px(12.)).text_color(p.err).child(error)))
            .when(!tab.complete, |d| d.child(div().px(px(12.)).text_color(p.warn).child("Only part of this object was searched (depth, node, or time limit reached).")))
            .when(tab.matches.as_ref().is_some_and(Vec::is_empty), |d| d.child(div().px(px(12.)).text_color(p.muted).child("No matching names or values.")))
            .child(div().id("object-tree").flex_1().min_h_0().overflow_scroll()
                .track_scroll(&tab.scroll).child(v_flex().w_full().min_w(px(480.)).text_size(px(look::SMALL))
                    .child(heading).children(rows)))
            .into_any_element())
    }

    fn object_node_rows(
        &self,
        tab: &ObjectTab,
        path: &[String],
        variable: &Variable,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let p = *Palette::get(cx);
        let id = tab.id;
        let open = tab.expanded.contains(path);
        let toggle = path.to_vec();
        let table = path.to_vec();
        let depth = path.len() - tab.root.len();
        let mut actions = h_flex().flex_none();
        if let Some(code) = copy_accessor(variable) {
            let code = code.to_string();
            actions = actions.child(
                icon_button(
                    SharedString::from(format!("object-copy-{path:?}")),
                    "edit.copy",
                    &p,
                    false,
                )
                .tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Copy access code").build(w, cx)
                })
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.stop_propagation();
                    cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                })),
            );
        }
        if variable.table {
            actions = actions.child(
                icon_button(
                    SharedString::from(format!("object-table-{path:?}")),
                    "table",
                    &p,
                    false,
                )
                .tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Open in Data Explorer").build(w, cx)
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.object_node_table(id, table.clone(), window, cx);
                })),
            );
        }
        rows.push(
            h_flex()
                .id(SharedString::from(format!("object-node-{path:?}")))
                .w_full()
                .h(px(27.))
                .border_b_1()
                .border_color(p.ink(4.))
                .hover(move |d| d.bg(p.hover))
                .when(tab.matched.contains(path), |d| d.bg(p.accent_soft))
                .child(
                    h_flex()
                        .id(SharedString::from(format!("object-toggle-{path:?}")))
                        .w(px(220.))
                        .flex_none()
                        .pl(px(8. + 14. * depth as f32))
                        .pr(px(8.))
                        .overflow_hidden()
                        .when(variable.children, |d| {
                            d.cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_object_node(id, toggle.clone(), cx)
                                }))
                        })
                        .child(div().w(px(18.)).flex_none().when(variable.children, |d| {
                            d.child(
                                icon(if open {
                                    "chevron.down"
                                } else {
                                    "chevron.right"
                                })
                                .size(px(13.)),
                            )
                        }))
                        .child(
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(variable.name.clone()),
                        ),
                )
                .child(
                    div()
                        .w(px(120.))
                        .flex_none()
                        .px(px(8.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_color(p.muted)
                        .child(variable.class.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(px(8.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_color(p.muted)
                        .child(variable.summary.clone()),
                )
                .child(actions)
                .into_any_element(),
        );
        if let Some(error) = tab.errors.get(path) {
            rows.push(
                div()
                    .px(px(12.))
                    .py(px(4.))
                    .text_color(p.err)
                    .child(error.clone())
                    .into_any_element(),
            );
        }
        if !open {
            return;
        }
        if tab.pending.contains(path) {
            rows.push(
                div()
                    .px(px(12.))
                    .text_color(p.muted)
                    .child("Loading…")
                    .into_any_element(),
            );
        }
        if let Some(listing) = tab.listings.get(path) {
            for node in &listing.nodes {
                let mut child = path.to_vec();
                child.push(node.name.clone());
                if tab.matches.is_some() && !tab.visible.contains(&child) {
                    continue;
                }
                self.object_node_rows(tab, &child, node, rows, cx);
            }
            if listing.more > 0 {
                rows.push(
                    div()
                        .px(px(12.))
                        .text_color(p.muted)
                        .child(format!(
                            "{} additional children are not listed. Search to find them.",
                            listing.more
                        ))
                        .into_any_element(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BTreeSet, HashMap, Listing, Path, Variable, VariableMatch, copy_accessor, match_expansions,
        merge_match_nodes,
    };

    fn variable(accessor: Option<&str>) -> Variable {
        Variable {
            name: "x".into(),
            kind: "data".into(),
            class: "list".into(),
            size: None,
            children: true,
            table: false,
            summary: String::new(),
            accessor: accessor.map(str::to_string),
        }
    }

    fn path(parts: &[&str]) -> Path {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn copy_visibility_uses_only_daemon_accessor() {
        assert_eq!(copy_accessor(&variable(None)), None);
        assert_eq!(
            copy_accessor(&variable(Some(
                "LayerData(seu, assay = \"RNA\", layer = \"counts\")"
            ))),
            Some("LayerData(seu, assay = \"RNA\", layer = \"counts\")")
        );
        assert_eq!(
            copy_accessor(&variable(Some("adata.obsm[\"X_pca\"][:, 0]"))),
            Some("adata.obsm[\"X_pca\"][:, 0]")
        );
        assert_eq!(copy_accessor(&variable(Some(""))), Some(""));
    }

    #[test]
    fn expands_all_matching_ancestors_inside_root_only() {
        let root = path(&["seu", "RNA"]);
        let matches = vec![
            path(&["seu", "RNA", "layers", "counts"]),
            path(&["seu", "RNA", "layers", "data"]),
            path(&["other", "RNA", "x"]),
            root.clone(),
        ];
        assert_eq!(
            match_expansions(&root, matches),
            BTreeSet::from([root, path(&["seu", "RNA", "layers"]),])
        );
    }

    #[test]
    fn root_match_and_empty_results_do_not_expand_leaves() {
        let root = path(&["adata"]);
        assert!(match_expansions(&root, vec![root.clone()]).is_empty());
        assert!(match_expansions(&root, Vec::new()).is_empty());
        assert_eq!(
            match_expansions(&root, vec![path(&["adata", "obs"])]),
            BTreeSet::from([root])
        );
    }

    #[test]
    fn search_supplements_capped_listings_and_clear_removes_only_supplements() {
        let root = path(&["adata"]);
        let known = variable(Some("adata.x"));
        let mut listings = HashMap::from([(
            root.clone(),
            Listing {
                nodes: vec![known.clone()],
                listed_len: 1,
                more: 17,
                loaded: true,
            },
        )]);
        let hits = vec![VariableMatch {
            path: path(&["adata", "obs"]),
            name: "obs".into(),
            class: "DataFrame".into(),
            summary: "12 × 3".into(),
            accessor: "adata.obs".into(),
            children: true,
            table: true,
        }];
        let visible = merge_match_nodes(&mut listings, &root, Some(&hits));
        assert!(visible.contains(&path(&["adata", "obs"])));
        let added = &listings[&root].nodes[1];
        assert!(added.children && added.table);
        assert_eq!(copy_accessor(added), Some("adata.obs"));
        assert!(merge_match_nodes(&mut listings, &root, None).is_empty());
        assert_eq!(listings[&root].nodes, vec![known]);
        assert_eq!(listings[&root].more, 17);
    }

    #[test]
    fn matching_parent_keeps_its_metadata_after_descendant_creates_the_path() {
        let root = path(&["seu"]);
        let mut listings = HashMap::new();
        let hits = vec![
            VariableMatch {
                path: path(&["seu", "meta.data", "clusters"]),
                name: "clusters".into(),
                class: "factor".into(),
                summary: "8 levels".into(),
                accessor: "seu@meta.data$clusters".into(),
                children: false,
                table: false,
            },
            VariableMatch {
                path: path(&["seu", "meta.data"]),
                name: "meta.data".into(),
                class: "data.frame".into(),
                summary: "24 × 2".into(),
                accessor: "seu@meta.data".into(),
                children: true,
                table: true,
            },
        ];
        merge_match_nodes(&mut listings, &root, Some(&hits));
        let parent = &listings[&root].nodes[0];
        assert_eq!(parent.class, "data.frame");
        assert!(parent.children && parent.table);
        assert_eq!(parent.accessor.as_deref(), Some("seu@meta.data"));
    }
}
