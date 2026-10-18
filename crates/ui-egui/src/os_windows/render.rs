//! Drawing the panel windows: one egui immediate viewport per window, holding its own dock tree
//! (splits, gutters, tab groups) drawn exactly as the main window draws its dock.

use std::panic::{AssertUnwindSafe, catch_unwind};

use craft_ui::docking::DropTarget;
use egui::{Rect, ViewportClass, ViewportId, pos2, vec2};
use serde_json::json;

use super::model::{self, Frame, OsWindow};
use super::sync::{self, InfoLite};
use super::{Runtime, Sample, WindowRt, native, own_frame, paint_preview, with_runtime};
use crate::FilmcraftApp;
use crate::dock::{self, DockAction, Floating, PanelKind};

/// What one window's pass hands back to the main window's dock pass.
#[derive(Default)]
struct PassOut {
    actions: Vec<DockAction>,
    targets: Vec<DropTarget<PanelKind>>,
    /// The window was closed: dock its panels.
    close: bool,
}

/// The builder a window is created with. It must stay the same for the window's life (egui turns
/// any change into commands), so it is kept and only the title follows the active tab.
fn builder(group: &Floating, window: &OsWindow, activate: bool) -> egui::ViewportBuilder {
    let scale = model::sane_scale(window.scale);
    let [x, y, w, h] = group.rect;
    egui::ViewportBuilder::default()
        .with_title(title(group))
        .with_position(pos2(x / scale, y / scale))
        .with_inner_size(vec2(w / scale, h / scale))
        .with_min_inner_size(vec2(160.0, 100.0))
        .with_drag_and_drop(true)
        .with_active(activate)
}

/// Remove a temp value and return it.
fn take<T: 'static + Clone + Send + Sync>(data: &mut egui::util::IdTypeMap, id: egui::Id) -> Option<T> {
    let value = data.get_temp::<T>(id);
    data.remove::<T>(id);
    value
}

/// "Project, Source — FilmCraft": the panel on show in each of the window's groups.
fn title(group: &Floating) -> String {
    fn shown(node: &dock::DockNode, out: &mut Vec<String>) {
        match node {
            dock::DockNode::Tabs { panels, active } => {
                out.extend(panels.get(*active).or(panels.first()).map(|panel| crate::i18n::t(panel.title()).to_string()))
            }
            dock::DockNode::Split { a, b, .. } => {
                shown(a, out);
                shown(b, out);
            }
        }
    }
    let mut names = Vec::new();
    shown(&model::tree_of(group), &mut names);
    format!("{} — FilmCraft", names.join(", "))
}

impl FilmcraftApp {
    /// Show every floating group that is an OS window. Their tab actions join the main window's,
    /// and their drop targets (in main-window points) join the panel drag's.
    pub(crate) fn show_panel_windows(
        &mut self,
        ctx: &egui::Context,
        seqs: &dock::SeqTabs,
        actions: &mut Vec<DockAction>,
        targets: &mut Vec<DropTarget<PanelKind>>,
    ) {
        if !native(ctx) {
            return;
        }
        let ids: Vec<u32> = self.ui.floating.iter().filter_map(|group| group.window.as_ref().map(|window| window.id)).collect();
        if ids.is_empty() {
            return;
        }
        let root = own_frame(ctx);
        let live: Vec<ViewportId> = ctx.input(|input| input.raw.viewports.keys().copied().collect());
        let mut rt = with_runtime(ctx, std::mem::take);
        let mut close = Vec::new();
        for id in ids {
            let Some(group) = self.ui.floating.iter().find(|group| group.window.as_ref().is_some_and(|window| window.id == id)).cloned() else { continue };
            let Some(window) = group.window.clone() else { continue };
            let viewport = window.viewport_id();
            let state = rt.windows.entry(id).or_default();
            if state.builder.is_none() || !live.contains(&viewport) {
                // (re)created: from the layout as it is now
                state.builder = Some(builder(&group, &window, state.activate));
                state.sync = sync::SyncState::default();
                state.activate = false;
            }
            let created = state.builder.clone().unwrap_or_default().with_title(title(&group));
            // eframe skips the callback when it cannot create the window, and egui then panics:
            // dock the panels back rather than lose them
            let shown = catch_unwind(AssertUnwindSafe(|| {
                ctx.show_viewport_immediate(viewport, created, |ui, class| {
                    let mut out = PassOut::default();
                    // a panel that panics here docks back, and the error window says what happened
                    let guarded = catch_unwind(AssertUnwindSafe(|| self.window_pass(ui, class, id, &mut rt, seqs, root, &mut out)));
                    if guarded.is_err() {
                        self.ui_error = Some(crate::crash::take_last().unwrap_or_else(|| "unknown error".into()));
                        self.playback.playing = false;
                        out.close = true;
                    }
                    out
                })
            }));
            match shown {
                Ok(out) => {
                    actions.extend(out.actions);
                    targets.extend(out.targets);
                    if out.close {
                        close.push(id);
                    }
                }
                Err(_) => {
                    let panel = group.panels.get(group.active).map(|panel| crate::i18n::t(panel.title()).to_string()).unwrap_or_default();
                    self.ui.status = tlf!("Could not open a window for {panel}; it was docked again", panel);
                    close.push(id);
                }
            }
        }
        with_runtime(ctx, |slot| *slot = rt);
        for id in close {
            if let Some(panel) = self
                .ui
                .floating
                .iter()
                .find(|group| group.window.as_ref().is_some_and(|window| window.id == id))
                .and_then(|group| group.panels.first().copied())
            {
                let _ = crate::menus::invoke(self, ctx, "window.dockWindow", json!({"panel": panel.id()}));
            }
        }
    }

    fn window_pass(
        &mut self,
        ui: &mut egui::Ui,
        class: ViewportClass,
        id: u32,
        rt: &mut Runtime,
        seqs: &dock::SeqTabs,
        root: Option<Frame>,
        out: &mut PassOut,
    ) {
        let ctx = ui.ctx().clone();
        let viewport = ctx.viewport_id();
        let Some(index) = self.ui.floating.iter().position(|group| group.window.as_ref().is_some_and(|window| window.id == id)) else { return };
        let Some(group) = self.ui.floating.get(index).cloned() else { return };
        let Some(window) = group.window.clone() else { return };
        let mut state: WindowRt = rt.windows.remove(&id).unwrap_or_default();
        let embedded = class == ViewportClass::EmbeddedWindow;
        if ctx.input(|input| input.viewport().close_requested()) {
            out.close = true;
        }
        if !embedded {
            let step = sync::step(group.rect, &window, &mut state.sync, &InfoLite::read(&ctx));
            if step.waiting {
                ctx.request_repaint();
            }
            for command in step.commands {
                ctx.send_viewport_cmd(command);
            }
            if let Some(slot) = self.ui.floating.get_mut(index) {
                if let Some(rect) = step.rect {
                    slot.rect = rect;
                }
                if let Some(saved) = slot.window.as_mut() {
                    saved.fullscreen = step.fullscreen.unwrap_or(saved.fullscreen);
                    saved.maximized = step.maximized.unwrap_or(saved.maximized);
                    saved.scale = step.scale.unwrap_or(saved.scale);
                }
            }
        }

        // keys pressed in this window: the same shortcuts as the main window
        let main_press = self.value_press.take();
        self.value_press = state.press.take();
        self.begin_press(&ctx);
        self.handle_shortcuts(&ctx);
        let focused = ctx.input(|input| input.viewport().focused.unwrap_or(false));
        if focused && !state.was_focused && !group.panels.contains(&self.ui.focused) {
            self.ui.focused = group.panels.get(group.active).copied().unwrap_or(self.ui.focused);
        }
        state.was_focused = focused;
        if ctx.input(|input| !input.raw.hovered_files.is_empty() || !input.raw.dropped_files.is_empty()) {
            self.handle_drops(&ctx);
        }

        let t = self.tokens;
        let full = ui.max_rect();
        t.fill(ui.painter(), full, 0.0, t.app_bg);
        let rect = full.shrink(1.0);
        let first_element = self.auto.elements.len();
        // the window's own dock tree: splits, gutters and tab groups as in the main window
        let mut tree = model::tree_of(&group);
        let mut groups = Vec::new();
        let mut resized = Vec::new();
        dock::layout_with_actions(ui, &mut tree, rect, &t, &format!("w{id}"), &mut groups, &mut self.auto, &mut resized);
        if !resized.is_empty() {
            for action in resized {
                crate::dock_shared::apply_tree(&mut tree, action);
            }
            if let Some(saved) = self.ui.floating.iter_mut().filter_map(|slot| slot.window.as_mut()).find(|saved| saved.id == id) {
                saved.layout = Some(tree);
            }
        }
        let mut tabs = Vec::new();
        for laid in &groups {
            let chrome = dock::draw_group_chrome_details(ui, laid, seqs, &t, &mut self.auto);
            tabs.push(chrome.tabs);
            for action in chrome.actions {
                match action {
                    DockAction::PanelMenu(panel, pos) => {
                        state.menu = Some((panel, pos));
                        state.menu_opened = ctx.cumulative_frame_nr();
                    }
                    DockAction::BeginDrag(panel) => {
                        rt.drag_owner = Some((panel, viewport));
                        out.actions.push(action);
                    }
                    other => out.actions.push(other),
                }
            }
        }
        for laid in &groups {
            if let Some(active) = laid.panels.get(laid.active).copied() {
                self.panel_body(ui, active, laid.content);
            }
        }
        for laid in &groups {
            dock::paint_group_focus(ui, laid, self.ui.focused, &t);
        }

        // this window's panel menu, under the key the panel menu reads (the main window's own
        // menu, if one is open, is put back afterwards)
        let menu_key = egui::Id::new("panel-menu");
        let opened_key = egui::Id::new("panel-menu-opened");
        let main_menu = ctx.data_mut(|data| (take::<(PanelKind, egui::Pos2)>(data, menu_key), take::<u64>(data, opened_key)));
        if let Some(menu) = state.menu.take() {
            ctx.data_mut(|data| {
                data.insert_temp(menu_key, menu);
                data.insert_temp(opened_key, state.menu_opened);
            });
        }
        crate::panels::panel_menu_popup(self, ui);
        state.menu = ctx.data_mut(|data| {
            let menu = take::<(PanelKind, egui::Pos2)>(data, menu_key);
            data.remove::<u64>(opened_key);
            if let Some(main) = main_menu.0 {
                data.insert_temp(menu_key, main);
            }
            if let Some(opened) = main_menu.1 {
                data.insert_temp(opened_key, opened);
            }
            menu
        });

        // a panel drag in progress: its preview, and this window's pointer for it
        let frame = own_frame(&ctx);
        let dragging = ctx.data(|data| data.get_temp::<PanelKind>(crate::dock_ui::drag_id())).is_some();
        if dragging && let (Some(preview), Some(frame)) = (rt.preview, frame) {
            paint_preview(&ctx, &preview, frame, &t);
            if rt.drag_owner.is_some_and(|(_, owner)| owner == viewport) {
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            }
        }
        let sample = ctx.input(|input| Sample {
            phys: input.pointer.latest_pos().zip(frame).map(|(pos, frame)| frame.to_phys(pos)),
            down: input.pointer.primary_down(),
            released: input.pointer.primary_released(),
            escape: input.key_pressed(egui::Key::Escape),
            command: input.modifiers.command,
        });
        rt.samples.insert(viewport, sample);

        // Escape leaves full screen when nothing in the window wants it
        let carrying = crate::panels::dragged_project_item(ui).is_some() || crate::panels::dragged_effect(ui).is_some();
        let widget_focused = ctx.memory(|memory| memory.focused().is_some());
        let menu_open = state.menu.is_some() || ctx.any_popup_open();
        if !embedded
            && model::escape_exits_fullscreen(window.fullscreen, sample.escape, widget_focused, dragging || carrying, menu_open)
            && let Some(saved) = self.ui.floating.iter_mut().filter_map(|slot| slot.window.as_mut()).find(|saved| saved.id == id)
        {
            saved.fullscreen = false;
        }

        self.cancel_value_drag(&ctx);
        state.press = self.value_press.take();
        self.value_press = main_press;

        // this window's automation elements stay out of the main window's list (their rects are
        // in this window's points); `window.listWindows` reports them
        let first_element = first_element.min(self.auto.elements.len());
        state.elements = self.auto.elements.split_off(first_element);

        // every group of the window is a drop target (splits allowed), in main-window points
        if let Some((root, frame)) = root.zip(frame) {
            let to_root = |rect: Rect| root.rect_to_local(frame.rect_to_phys(rect));
            for (laid, tabs) in groups.iter().zip(tabs) {
                let Some(mut target) = crate::dock_ui::target(laid, tabs, true) else { continue };
                target.rect = to_root(target.rect);
                target.tab_strip = target.tab_strip.map(to_root);
                for (_, tab) in &mut target.tabs {
                    if tab.is_finite() && tab.is_positive() {
                        *tab = to_root(*tab);
                    }
                }
                out.targets.push(target);
            }
        }
        rt.windows.insert(id, state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builder_places_the_window_in_its_own_points() {
        let group = Floating {
            panels: vec![PanelKind::Program],
            active: 0,
            rect: [3840.0, 200.0, 2560.0, 1440.0],
            window: Some(OsWindow { id: 1, fullscreen: true, maximized: false, scale: 2.0, layout: None }),
        };
        let b = builder(&group, &group.window.clone().unwrap_or_default(), false);
        assert_eq!(b.position, Some(pos2(1920.0, 100.0)));
        assert_eq!(b.inner_size, Some(vec2(1280.0, 720.0)));
        assert_eq!(b.title.as_deref(), Some("Program — FilmCraft"));
        // never created full screen: the window goes full screen once it is on its monitor
        assert_eq!(b.fullscreen, None);
    }

    #[test]
    fn a_split_window_is_titled_after_every_panel_on_show() {
        let layout = dock::DockNode::Split {
            vertical: true,
            size: dock::SplitSize::Ratio(0.5),
            a: Box::new(dock::DockNode::Tabs { panels: vec![PanelKind::Project, PanelKind::Info], active: 0 }),
            b: Box::new(dock::DockNode::Tabs { panels: vec![PanelKind::Source], active: 0 }),
        };
        let group = Floating {
            panels: vec![PanelKind::Project, PanelKind::Info, PanelKind::Source],
            active: 2,
            rect: [0.0, 0.0, 800.0, 600.0],
            window: Some(OsWindow { id: 1, layout: Some(layout), ..Default::default() }),
        };
        assert_eq!(title(&group), "Project, Source — FilmCraft");
    }

    #[test]
    fn a_window_survives_a_hostile_scale() {
        let group =
            Floating { panels: vec![], active: 7, rect: [1.0, 2.0, 300.0, 200.0], window: Some(OsWindow { id: 1, scale: f32::NAN, ..Default::default() }) };
        let b = builder(&group, &group.window.clone().unwrap_or_default(), true);
        assert!(b.position.is_some_and(|p| p.is_finite()));
        assert_eq!(b.title.as_deref(), Some(" — FilmCraft"));
    }
}
