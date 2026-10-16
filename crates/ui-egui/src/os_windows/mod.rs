//! Panel windows: floating panel groups as real OS windows, so a panel can sit on any monitor
//! (the Program monitor full screen on a second display, the Project panel on a third).
//!
//! On the desktop every floating group is its own OS window, drawn as an egui immediate viewport
//! inside the main window's pass (the app state is not `Send`, and this keeps the whole frame
//! under the UI panic guard). Where viewports are embedded (tests, the web) floating groups stay
//! areas inside the main window, exactly as before.
//!
//! Upstream code calls in at a handful of places: `begin_frame` and `show_panel_windows` from
//! `dock_area`, `drag_input` and `publish_preview` from the panel drag, `carry_over` after each
//! docking transaction, `route` from `menus::invoke`, and `drag_ghost` / `panel_menu_items` from
//! the panel menu. Usage and limits: `docs/multi-monitor.md`.

mod commands;
pub mod model;
mod plugin;
mod render;
pub mod sync;
mod tree;

use std::collections::HashMap;

use egui::{Color32, Pos2, Rect, Stroke, StrokeKind, ViewportId, pos2, vec2};

use crate::FilmcraftApp;
use crate::dock::{Floating, PanelKind};

pub(crate) use commands::{COMMANDS, panel_menu_items, route};
use model::Frame;
pub use model::OsWindow;
pub(crate) use model::{carry_over, floated};
pub(crate) use tree::dock_into_window;

/// Whether this context opens real OS windows (eframe on the desktop) rather than embedding them.
pub(crate) fn native(ctx: &egui::Context) -> bool {
    !ctx.embed_viewports()
}

/// Whether a floating group is drawn as its own OS window.
pub(crate) fn is_separate(ctx: &egui::Context, group: &Floating) -> bool {
    group.window.is_some() && native(ctx)
}

/// Whether this pass is the main window's.
pub(crate) fn is_root(ctx: &egui::Context) -> bool {
    ctx.viewport_id() == ViewportId::ROOT
}

/// Whether the mouse press in progress began in this pass's window. Drags of clips, effects and
/// templates belong to that window: only it may draw their ghost or drop them.
pub(crate) fn owns_press(ctx: &egui::Context) -> bool {
    let owner = ctx.with_plugin::<plugin::OsWindowsPlugin, _>(|plugin| plugin.press_owner).flatten();
    owner.unwrap_or(ViewportId::ROOT) == ctx.viewport_id()
}

/// A window's frame on the desktop, from the viewport list of the current pass.
fn frame_of(ctx: &egui::Context, id: ViewportId) -> Option<Frame> {
    let zoom = ctx.zoom_factor();
    ctx.input(|input| {
        let viewport = input.raw.viewports.get(&id)?;
        Some(Frame::new(viewport.inner_rect?.min, zoom * viewport.native_pixels_per_point?))
    })
}

/// The current pass's own window frame.
fn own_frame(ctx: &egui::Context) -> Option<Frame> {
    ctx.input(|input| input.viewport().inner_rect.map(|rect| Frame::new(rect.min, input.pixels_per_point)))
}

/// The OS cursor in physical desktop pixels (the host hook; None in tests and on the web).
fn os_cursor(app: &FilmcraftApp) -> Option<Pos2> {
    app.hooks.cursor_screen_position.as_ref().and_then(|cursor| cursor()).map(|(x, y)| pos2(x as f32, y as f32))
}

/// One window's pointer, as sampled at the end of its pass (for a panel drag it started).
#[derive(Clone, Copy, Debug, Default)]
struct Sample {
    phys: Option<Pos2>,
    down: bool,
    released: bool,
    escape: bool,
    command: bool,
}

/// The panel drag's drop preview, published by the main window for the panel windows to draw
/// (a preview over a window is hidden behind it in the main window). Physical pixels.
#[derive(Clone, Copy, Debug)]
struct Preview {
    panel: PanelKind,
    pos: Pos2,
    rect: Option<Rect>,
    insertion: Option<Rect>,
}

/// What each panel window keeps between frames.
#[derive(Clone, Default)]
struct WindowRt {
    builder: Option<egui::ViewportBuilder>,
    sync: sync::SyncState,
    /// Opened by a user action this session: it takes focus when it appears.
    activate: bool,
    press: Option<(f64, filmcraft_engine::UndoMark)>,
    was_focused: bool,
    menu: Option<(PanelKind, Pos2)>,
    menu_opened: u64,
    elements: Vec<crate::automation::Element>,
}

/// Per-frame state shared by the main window and the panel windows (egui temp data, so the app
/// struct stays as upstream has it).
#[derive(Clone, Default)]
struct Runtime {
    /// The panel being dragged by its tab, and the window the drag began in.
    drag_owner: Option<(PanelKind, ViewportId)>,
    samples: HashMap<ViewportId, Sample>,
    preview: Option<Preview>,
    windows: HashMap<u32, WindowRt>,
}

fn runtime_id() -> egui::Id {
    egui::Id::new("filmcraft-panel-windows")
}

fn with_runtime<R>(ctx: &egui::Context, f: impl FnOnce(&mut Runtime) -> R) -> R {
    ctx.data_mut(|data| f(data.get_temp_mut_or_default::<Runtime>(runtime_id())))
}

/// Start of the main window's dock pass: turn new floating groups into windows, make saved
/// geometry safe, and route requests aimed at panels that live in a window.
pub(crate) fn begin_frame(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if !native(ctx) {
        return;
    }
    if ctx.with_plugin::<plugin::OsWindowsPlugin, _>(|_| ()).is_none() {
        ctx.add_plugin(plugin::OsWindowsPlugin::default());
    }
    let made = match own_frame(ctx) {
        Some(root) => model::promote(&mut app.ui.floating, root),
        None => Vec::new(),
    };
    model::dedupe_ids(&mut app.ui.floating);
    model::reconcile_all(&mut app.ui.floating);
    let max_side = ctx.input(|input| input.max_texture_side) as f32;
    for group in app.ui.floating.iter_mut().filter(|group| group.window.is_some()) {
        group.rect = model::sanitize_rect(group.rect, max_side);
        if let Some(window) = group.window.as_mut() {
            window.scale = model::sane_scale(window.scale);
        }
    }
    // Bring a panel's window back when it was minimized and the panel is asked for (Window menu).
    if let Some(panel) = app.pending_panel_raise
        && let Some(window) = window_of(app, panel)
    {
        app.pending_panel_raise = None;
        let id = window.viewport_id();
        if ctx.input(|input| input.raw.viewports.get(&id).and_then(|viewport| viewport.minimized)).unwrap_or(false) {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        }
    }
    // Maximize Frame on a panel in a window maximizes the window (drawing it in the main window
    // too would show the panel twice).
    if let Some(panel) = app.ui.keys.maximized
        && let Some(group) = app.ui.floating.iter_mut().find(|group| group.window.is_some() && group.panels.contains(&panel))
        && let Some(window) = group.window.as_mut()
    {
        app.ui.keys.maximized = None;
        window.maximized = !window.maximized;
    }
    let dragging = ctx.data(|data| data.get_temp::<PanelKind>(crate::dock_ui::drag_id())).is_some();
    let live: Vec<u32> = app.ui.floating.iter().filter_map(|group| group.window.as_ref().map(|window| window.id)).collect();
    with_runtime(ctx, |rt| {
        if !dragging {
            rt.drag_owner = None;
            rt.preview = None;
        }
        rt.windows.retain(|id, _| live.contains(id));
        for id in made {
            rt.windows.entry(id).or_default().activate = true;
        }
    });
}

/// The window a panel is in, if any.
fn window_of(app: &FilmcraftApp, panel: PanelKind) -> Option<OsWindow> {
    app.ui.floating.iter().find(|group| group.panels.contains(&panel)).and_then(|group| group.window.clone())
}

/// The pointer state that drives a panel drag, in main-window points. A drag that began in a
/// panel window reads that window's pointer: the main window sees no events while another window
/// holds the mouse. On the desktop the OS cursor is used, so a drop beyond any window works.
pub(crate) struct DragInput {
    pub pos: Option<Pos2>,
    pub canceled: bool,
    pub released: bool,
    pub force_float: bool,
}

pub(crate) fn drag_input(app: &FilmcraftApp, ctx: &egui::Context, panel: PanelKind) -> DragInput {
    let own = ctx.input(|input| DragInput {
        pos: None,
        canceled: input.key_pressed(egui::Key::Escape) || !input.focused || (!input.pointer.primary_down() && !input.pointer.primary_released()),
        released: input.pointer.primary_released(),
        force_float: input.modifiers.command,
    });
    let own = DragInput { pos: ctx.pointer_latest_pos(), ..own };
    if !native(ctx) {
        return own;
    }
    let root = own_frame(ctx);
    let cursor = os_cursor(app).and_then(|phys| root.map(|root| root.to_local(phys)));
    let owner =
        with_runtime(ctx, |rt| rt.drag_owner.filter(|(dragged, id)| *dragged == panel && *id != ViewportId::ROOT).map(|(_, id)| rt.samples.get(&id).copied()));
    match owner {
        // the window it began in has gone
        Some(None) => DragInput { pos: None, canceled: true, released: false, force_float: false },
        // The window holding the mouse keeps getting its position beyond its edges, so that is
        // used first (synthetic control-channel drags included); the OS cursor only fills a gap.
        Some(Some(sample)) => DragInput {
            pos: sample.phys.zip(root).map(|(phys, root)| root.to_local(phys)).or(cursor),
            canceled: sample.escape || (!sample.down && !sample.released),
            released: sample.released,
            force_float: sample.command,
        },
        None => DragInput { pos: own.pos.or(cursor), ..own },
    }
}

/// Hand the panel drag's preview to the panel windows (main-window points in, desktop pixels out).
pub(crate) fn publish_preview(ctx: &egui::Context, panel: PanelKind, pos: Pos2, rect: Option<Rect>, insertion: Option<Rect>) {
    if !native(ctx) {
        return;
    }
    let Some(root) = own_frame(ctx) else { return };
    let preview = Preview { panel, pos: root.to_phys(pos), rect: rect.map(|r| root.rect_to_phys(r)), insertion: insertion.map(|r| root.rect_to_phys(r)) };
    with_runtime(ctx, |rt| rt.preview = Some(preview));
}

/// Draw the panel drag's preview and ghost where they fall in this window.
fn paint_preview(ctx: &egui::Context, preview: &Preview, frame: Frame, t: &crate::theme::Tokens) {
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("dock-drop")));
    if let Some(rect) = preview.rect.map(|r| frame.rect_to_local(r)) {
        painter.rect_filled(rect.shrink(2.0), t.radius, t.focus.gamma_multiply(0.2));
        painter.rect_stroke(rect.shrink(2.0), t.radius, Stroke::new(2.0, t.focus), StrokeKind::Inside);
    }
    if let Some(mark) = preview.insertion.map(|r| frame.rect_to_local(r)) {
        painter.rect_filled(mark, 0.0, t.focus);
    }
    let pos = frame.to_local(preview.pos);
    let title = painter.layout_no_wrap(crate::i18n::t(preview.panel.title()).to_string(), crate::theme::Tokens::ui(12.0), Color32::WHITE);
    let ghost = Rect::from_min_size(pos + vec2(12.0, 10.0), title.size() + vec2(16.0, 8.0));
    painter.rect_filled(ghost, 4.0, t.focus.gamma_multiply(0.85));
    painter.galley(ghost.min + vec2(8.0, 4.0), title, Color32::WHITE);
}

/// The panel menu's drag ghost, run only by the window the press began in, which also hands a
/// clip, effect or template drag on to the window under the cursor so it can be dropped there.
pub(crate) fn drag_ghost(app: &mut FilmcraftApp, ui: &egui::Ui) {
    let ctx = ui.ctx();
    if !owns_press(ctx) {
        return;
    }
    if native(ctx) {
        let carrying = crate::panels::dragged_project_item(ui).is_some()
            || crate::panels::dragged_effect(ui).is_some()
            || crate::panels::dragged_template(ui).is_some()
            || crate::panels::dragged_bin(ui).is_some();
        let next = if carrying { forward_target(app, ctx) } else { None };
        if next.is_some() {
            ctx.request_repaint();
        }
        ctx.with_plugin::<plugin::OsWindowsPlugin, _>(|plugin| plugin.set_forward(next));
    }
    crate::panels::drag_ghost(app, ui);
}

/// The other window under the cursor during a drag, and the cursor in its points.
fn forward_target(app: &FilmcraftApp, ctx: &egui::Context) -> Option<plugin::Forward> {
    let me = ctx.viewport_id();
    let cursor = own_frame(ctx).zip(ctx.pointer_latest_pos()).map(|(frame, pos)| frame.to_phys(pos)).or_else(|| os_cursor(app))?;
    let zoom = ctx.zoom_factor();
    let windows: Vec<(ViewportId, Frame, Rect)> = ctx.input(|input| {
        input
            .raw
            .viewports
            .iter()
            .filter(|(id, viewport)| **id != me && viewport.minimized != Some(true))
            .filter_map(|(id, viewport)| {
                let inner = viewport.inner_rect?;
                let frame = Frame::new(inner.min, zoom * viewport.native_pixels_per_point?);
                Some((*id, frame, frame.rect_to_phys(Rect::from_min_size(Pos2::ZERO, inner.size()))))
            })
            .collect()
    });
    // panel windows sit in front of the main window
    let (target, frame, _) = windows.into_iter().filter(|(_, _, rect)| rect.contains(cursor)).min_by_key(|(id, _, _)| *id == ViewportId::ROOT)?;
    let (released, modifiers) = ctx.input(|input| (!input.pointer.any_down(), input.modifiers));
    Some(plugin::Forward { target, pos: frame.to_local(cursor), released, modifiers, release_sent: false })
}
