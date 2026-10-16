//! Commands for panel windows, so menus, shortcuts, the control channel and MCP can drive them:
//!
//! | id | params | does |
//! |---|---|---|
//! | `window.toggleWindowFullScreen` | `panel?` | full screen on/off for the window holding the panel (the main window for a docked one) |
//! | `window.setWindow` | `panel?`, `rect?` `[x, y, width, height]` desktop pixels, `fullscreen?`, `maximized?` | makes the panel's group a window and places it |
//! | `window.dockWindow` | `panel?` | docks every panel of the window back into the main window |
//! | `window.gatherWindows` | | brings every window out of full screen and over the main window |
//! | `window.listWindows` | | the windows: panels, saved and live geometry, automation elements |

use egui::{Pos2, Rect, ViewportCommand, ViewportId, pos2, vec2};
use serde_json::{Value, json};

use super::model::{self, Frame, OsWindow};
use super::{frame_of, is_root, own_frame, window_of, with_runtime};
use crate::FilmcraftApp;
use crate::dock::{Floating, PanelKind};
use crate::menus::UiCommand;

pub(crate) const COMMANDS: &[UiCommand] = &[
    UiCommand { id: "window.toggleWindowFullScreen", label: "Full Screen Window", menu: &[], shortcut: None },
    UiCommand { id: "window.setWindow", label: "Set Panel Window", menu: &[], shortcut: None },
    UiCommand { id: "window.dockWindow", label: "Dock Window", menu: &[], shortcut: None },
    UiCommand { id: "window.gatherWindows", label: "Gather Panel Windows", menu: &["Window"], shortcut: None },
    UiCommand { id: "window.listWindows", label: "List Panel Windows", menu: &[], shortcut: None },
];

/// Run a panel-window command, or adjust a docking command run from a panel window. `None` when
/// the command is someone else's.
pub(crate) fn route(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: &mut Value) -> Option<Result<Value, String>> {
    match id {
        // Quit from a panel window quits the app, not just that window.
        "app.quit" if !is_root(ctx) => {
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            Some(Ok(Value::Null))
        }
        // Undock in a panel window: the new window starts over the one it came from.
        "window.floatPanel" if !is_root(ctx) && params.get("rect").is_none() => {
            if let (Some(root), Some(own)) = (frame_of(ctx, ViewportId::ROOT), own_frame(ctx))
                && let Some(object) = params.as_object_mut()
            {
                let center = root.to_local(own.to_phys(ctx.content_rect().center()));
                let [w, h] = model::DEFAULT_SIZE;
                object.insert("rect".into(), json!([center.x - w / 2.0, center.y - h / 2.0, w, h]));
            }
            None
        }
        "window.toggleWindowFullScreen" => Some(toggle_fullscreen(app, ctx, params)),
        "window.setWindow" => Some(set_window(app, ctx, params)),
        "window.dockWindow" => Some(dock_window(app, params)),
        "window.gatherWindows" => Some(Ok(gather(app, ctx))),
        "window.listWindows" => Some(Ok(list(app, ctx))),
        _ => None,
    }
}

fn panel_param(app: &FilmcraftApp, params: &Value) -> Result<PanelKind, String> {
    match params.get("panel") {
        None => Ok(app.ui.focused),
        Some(value) => value.as_str().and_then(PanelKind::from_name).ok_or_else(|| "unknown panel".into()),
    }
}

fn window_json(group: &Floating) -> Value {
    json!({
        "id": group.window.as_ref().map(|window| window.id),
        "panels": group.panels.iter().map(|panel| panel.id()).collect::<Vec<_>>(),
        "active": group.panels.get(group.active).map(|panel| panel.id()),
        "rect": group.rect,
        "fullScreen": group.window.as_ref().is_some_and(|window| window.fullscreen),
        "maximized": group.window.as_ref().is_some_and(|window| window.maximized),
        "scale": group.window.as_ref().map(|window| window.scale),
    })
}

fn toggle_fullscreen(app: &mut FilmcraftApp, ctx: &egui::Context, params: &Value) -> Result<Value, String> {
    let panel = panel_param(app, params)?;
    let group = app.ui.floating.iter_mut().find(|group| group.window.is_some() && group.panels.contains(&panel));
    if let Some(group) = group
        && let Some(window) = group.window.as_mut()
    {
        window.fullscreen = !window.fullscreen;
        ctx.request_repaint();
        return Ok(json!({"panel": panel.id(), "window": window.id, "fullScreen": window.fullscreen}));
    }
    // a docked panel: the main window
    let fullscreen = !ctx.input(|input| input.raw.viewports.get(&ViewportId::ROOT).and_then(|viewport| viewport.fullscreen)).unwrap_or(false);
    ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Fullscreen(fullscreen));
    Ok(json!({"panel": panel.id(), "window": Value::Null, "fullScreen": fullscreen}))
}

fn set_window(app: &mut FilmcraftApp, ctx: &egui::Context, params: &Value) -> Result<Value, String> {
    let object = params.as_object().ok_or("command parameters must be an object")?;
    let panel = panel_param(app, params)?;
    let rect = match object.get("rect") {
        None => None,
        Some(value) => {
            let rect: [f32; 4] = serde_json::from_value(value.clone()).map_err(|_| "rect must be [x, y, width, height] in desktop pixels")?;
            if rect.iter().any(|v| !v.is_finite()) {
                return Err("rect must contain four finite numbers".into());
            }
            Some(rect)
        }
    };
    let flag = |name: &str| match object.get(name) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(format!("{name} must be true or false")),
    };
    let fullscreen = flag("fullscreen")?;
    let maximized = flag("maximized")?;
    // everything is checked: now change the layout
    if !app.ui.floating.iter().any(|group| group.panels.contains(&panel)) {
        if !app.ui.dock.contains(panel) {
            app.show_panel(panel);
        }
        app.apply_dock(craft_ui::docking::Action::Float { panel, rect: crate::dock_ui::default_float_rect(ctx.content_rect()) })?;
    }
    let root = frame_of(ctx, ViewportId::ROOT).unwrap_or(Frame::new(Pos2::ZERO, 1.0));
    let id = model::free_id(&app.ui.floating);
    let max_side = ctx.input(|input| input.max_texture_side) as f32;
    let group = app.ui.floating.iter_mut().find(|group| group.panels.contains(&panel)).ok_or("the panel could not be undocked")?;
    if group.window.is_none() {
        let [x, y, w, h] = group.rect;
        let min = root.to_phys(pos2(x, y));
        group.rect = [min.x, min.y, w * root.ppp, h * root.ppp];
        group.window = Some(OsWindow { id, fullscreen: false, maximized: false, scale: root.ppp, layout: None });
    }
    if let Some(rect) = rect {
        group.rect = rect;
    }
    group.rect = model::sanitize_rect(group.rect, max_side);
    if let Some(window) = group.window.as_mut() {
        window.fullscreen = fullscreen.unwrap_or(window.fullscreen);
        window.maximized = maximized.unwrap_or(window.maximized);
    }
    ctx.request_repaint();
    Ok(window_json(group))
}

fn dock_window(app: &mut FilmcraftApp, params: &Value) -> Result<Value, String> {
    let panel = panel_param(app, params)?;
    let group = app.ui.floating.iter().find(|group| group.panels.contains(&panel)).ok_or("the panel is not in a window")?;
    let panels = group.panels.clone();
    let active = group.panels.get(group.active).copied();
    let mut docked = Vec::new();
    for panel in panels.iter().copied() {
        if app.apply_dock(craft_ui::docking::Action::Close { panel }).is_ok() {
            app.show_panel(panel);
            docked.push(panel.id());
        }
    }
    // the tab that was in front stays in front
    if let Some(active) = active {
        app.show_panel(active);
    }
    Ok(json!({"docked": docked}))
}

/// The main window's content in physical pixels.
fn root_phys(ctx: &egui::Context) -> Rect {
    let zoom = ctx.zoom_factor();
    ctx.input(|input| {
        let viewport = input.raw.viewports.get(&ViewportId::ROOT)?;
        let frame = Frame::new(viewport.inner_rect?.min, zoom * viewport.native_pixels_per_point?);
        Some(frame.rect_to_phys(Rect::from_min_size(Pos2::ZERO, viewport.inner_rect?.size())))
    })
    .unwrap_or_else(|| Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 900.0)))
}

fn gather(app: &mut FilmcraftApp, ctx: &egui::Context) -> Value {
    let root = root_phys(ctx);
    let mut gathered = Vec::new();
    for (index, group) in app.ui.floating.iter_mut().filter(|group| group.window.is_some()).enumerate() {
        group.rect = model::gathered(index, root, group.rect);
        if let Some(window) = group.window.as_mut() {
            window.fullscreen = false;
            window.maximized = false;
        }
        gathered.push(window_json(group));
    }
    ctx.request_repaint();
    json!({"windows": gathered})
}

fn list(app: &FilmcraftApp, ctx: &egui::Context) -> Value {
    let zoom = ctx.zoom_factor();
    let live = ctx.input(|input| input.raw.viewports.clone());
    let elements = with_runtime(ctx, |rt| rt.windows.iter().map(|(id, state)| (*id, state.elements.clone())).collect::<std::collections::HashMap<_, _>>());
    let rect = |rect: Option<Rect>| rect.map(|r| [r.min.x, r.min.y, r.width(), r.height()]);
    let describe = |info: &egui::ViewportInfo| {
        json!({
            "innerRect": rect(info.inner_rect),
            "outerRect": rect(info.outer_rect),
            "monitorSize": info.monitor_size.map(|s| [s.x, s.y]),
            "pixelsPerPoint": info.native_pixels_per_point.map(|ppp| ppp * zoom),
            "fullScreen": info.fullscreen,
            "focused": info.focused,
        })
    };
    let windows: Vec<Value> = app
        .ui
        .floating
        .iter()
        .filter_map(|group| {
            let window = group.window.as_ref()?;
            let viewport = window.viewport_id();
            let mut value = window_json(group);
            let info = live.get(&viewport);
            value["viewport"] = json!(format!("{:016x}", viewport.0.value()));
            value["live"] = json!(info.map(describe));
            value["elements"] = json!(elements.get(&window.id).cloned().unwrap_or_default());
            Some(value)
        })
        .collect();
    // rects are in each window's own points; `pixelsPerPoint` turns them into desktop pixels
    json!({"main": live.get(&ViewportId::ROOT).map(describe), "windows": windows})
}

/// The panel menu's entries for a panel in a window: Full Screen Window and Dock Window. Returns
/// whether one was chosen (the menu then closes).
pub(crate) fn panel_menu_items(app: &mut FilmcraftApp, ui: &mut egui::Ui, panel: PanelKind) -> bool {
    let Some(window) = window_of(app, panel) else { return false };
    let mut close = false;
    let (label, name) =
        if window.fullscreen { (tl!("Exit Full Screen Window"), "Exit Full Screen Window") } else { (tl!("Full Screen Window"), "Full Screen Window") };
    let response = ui.button(label);
    app.auto.add_response(&format!("panel.menu.{}.fullScreenWindow", panel.id()), &response, name);
    if response.clicked() {
        let _ = crate::menus::invoke(app, ui.ctx(), "window.toggleWindowFullScreen", json!({"panel": panel.id()}));
        close = true;
    }
    let response = ui.button(tl!("Dock Window"));
    app.auto.add_response(&format!("panel.menu.{}.dockWindow", panel.id()), &response, "Dock Window");
    if response.clicked() {
        let _ = crate::menus::invoke(app, ui.ctx(), "window.dockWindow", json!({"panel": panel.id()}));
        close = true;
    }
    close
}
