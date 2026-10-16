//! Panel windows: the commands, saved workspaces and docking transactions keep each floating
//! group's OS window (identity, desktop rect, full screen). Tests run with embedded viewports, so
//! windows stay areas in the main window here; the model is what is checked.

use craft_ui::docking::{Action, Placement, Zone};
use egui::vec2;
use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::{
    FilmcraftApp,
    dock::{self, DockNode, Layout, PanelKind, SplitSize},
    menus,
};
use serde_json::{Value, json};

fn layout() -> Layout {
    Layout {
        root: DockNode::Split {
            vertical: false,
            size: SplitSize::FixedA(400.0),
            a: Box::new(DockNode::Tabs { panels: vec![PanelKind::Project, PanelKind::Info], active: 0 }),
            b: Box::new(DockNode::Tabs { panels: vec![PanelKind::Source, PanelKind::Program, PanelKind::Timeline], active: 1 }),
        },
        floating: vec![],
    }
}

fn app() -> (FilmcraftApp, egui::Context) {
    let mut app = FilmcraftApp::new(Session::default());
    app.ui.dock = layout().root;
    app.ui.floating.clear();
    (app, egui::Context::default())
}

fn window(app: &FilmcraftApp, panel: PanelKind) -> Option<filmcraft_ui_egui::os_windows::OsWindow> {
    app.ui.floating.iter().find(|group| group.panels.contains(&panel)).and_then(|group| group.window.clone())
}

const RIGHT: [f32; 4] = [3840.0, 0.0, 1280.0, 720.0];

#[test]
fn set_window_places_a_docked_panel_in_its_own_full_screen_window() {
    let (mut app, ctx) = app();
    let out = menus::invoke(&mut app, &ctx, "window.setWindow", json!({"panel": "Program", "rect": RIGHT, "fullscreen": true})).unwrap();
    assert_eq!(out["panels"], json!(["Program"]));
    assert_eq!(out["fullScreen"], json!(true));
    assert!(!app.ui.dock.contains(PanelKind::Program));
    let group = app.ui.floating.iter().find(|group| group.panels == [PanelKind::Program]).unwrap();
    assert_eq!(group.rect, RIGHT);
    let w = group.window.clone().unwrap();
    assert!(w.fullscreen && !w.maximized && w.id > 0);

    let out = menus::invoke(&mut app, &ctx, "window.toggleWindowFullScreen", json!({"panel": "Program"})).unwrap();
    assert_eq!(out["fullScreen"], json!(false));
    menus::invoke(&mut app, &ctx, "window.toggleWindowFullScreen", json!({"panel": "Program"})).unwrap();
    assert!(window(&app, PanelKind::Program).unwrap().fullscreen);

    let listed = menus::invoke(&mut app, &ctx, "window.listWindows", json!({})).unwrap();
    assert_eq!(listed["windows"].as_array().map(Vec::len), Some(1));
    assert_eq!(listed["windows"][0]["id"], json!(w.id));
    assert_eq!(listed["windows"][0]["rect"], json!(RIGHT));
}

#[test]
fn docking_transactions_keep_the_window() {
    let (mut app, ctx) = app();
    menus::invoke(&mut app, &ctx, "window.setWindow", json!({"panel": "Program", "rect": RIGHT})).unwrap();
    let id = window(&app, PanelKind::Program).unwrap().id;
    // another panel dropped onto the window's tab strip joins it
    let tab = Action::Move { panel: PanelKind::Source, anchor: PanelKind::Program, placement: Placement::Tab { before: None } };
    app.apply_dock(tab).unwrap();
    assert_eq!(window(&app, PanelKind::Source).map(|w| w.id), Some(id));
    for action in [Action::Activate { panel: PanelKind::Source }, Action::Activate { panel: PanelKind::Program }, Action::Close { panel: PanelKind::Source }] {
        app.apply_dock(action).unwrap();
        assert_eq!(window(&app, PanelKind::Program).map(|w| w.id), Some(id));
    }
    // undocking a panel makes a new group, which is not the old window
    app.apply_dock(Action::Move { panel: PanelKind::Info, anchor: PanelKind::Program, placement: Placement::Tab { before: None } }).unwrap();
    menus::invoke(&mut app, &ctx, "window.floatPanel", json!({"panel": "Info", "rect": [10, 10, 300, 200]})).unwrap();
    assert_eq!(window(&app, PanelKind::Program).map(|w| w.id), Some(id));
    assert_eq!(window(&app, PanelKind::Info), None);
    // the same through the shared Layout API
    let mut layout = Layout { root: app.ui.dock.clone(), floating: app.ui.floating.clone() };
    layout.apply(Action::Activate { panel: PanelKind::Program }).unwrap();
    assert_eq!(layout.floating.iter().find(|g| g.panels.contains(&PanelKind::Program)).and_then(|g| g.window.as_ref()).map(|w| w.id), Some(id));
}

/// The tab group (panels) holding `panel` in a dock tree.
fn group_of(tree: &DockNode, panel: PanelKind) -> Option<Vec<PanelKind>> {
    match tree {
        DockNode::Tabs { panels, .. } => panels.contains(&panel).then(|| panels.clone()),
        DockNode::Split { a, b, .. } => group_of(a, panel).or_else(|| group_of(b, panel)),
    }
}

fn window_tree(app: &FilmcraftApp, panel: PanelKind) -> DockNode {
    let group = app.ui.floating.iter().find(|group| group.panels.contains(&panel)).unwrap();
    group.window.as_ref().and_then(|w| w.layout.clone()).unwrap_or(DockNode::Tabs { panels: group.panels.clone(), active: group.active })
}

#[test]
fn panels_split_and_tab_inside_one_window() {
    let (mut app, ctx) = app();
    menus::invoke(&mut app, &ctx, "window.setWindow", json!({"panel": "Project", "rect": [-1920, 0, 1920, 1160]})).unwrap();
    let id = window(&app, PanelKind::Project).unwrap().id;
    // Source below Project, in the same window (a drop on the lower part of the Project group)
    app.apply_dock(Action::Move { panel: PanelKind::Source, anchor: PanelKind::Project, placement: Placement::Split(Zone::Bottom) }).unwrap();
    assert!(!app.ui.dock.contains(PanelKind::Source));
    assert_eq!(window(&app, PanelKind::Source).map(|w| w.id), Some(id));
    let tree = window_tree(&app, PanelKind::Project);
    assert!(matches!(tree, DockNode::Split { vertical: true, .. }), "{tree:?}");
    assert_eq!(group_of(&tree, PanelKind::Source), Some(vec![PanelKind::Source]));
    // Info as a tab beside Source, by the dockPanel command
    menus::invoke(&mut app, &ctx, "window.dockPanel", json!({"panel": "Info", "anchor": "Source", "zone": "center"})).unwrap();
    assert_eq!(group_of(&window_tree(&app, PanelKind::Project), PanelKind::Info), Some(vec![PanelKind::Source, PanelKind::Info]));
    // and from there to the right of Project, inside the window
    menus::invoke(&mut app, &ctx, "window.dockPanel", json!({"panel": "Info", "anchor": "Project", "zone": "right"})).unwrap();
    let tree = window_tree(&app, PanelKind::Project);
    assert_eq!(group_of(&tree, PanelKind::Info), Some(vec![PanelKind::Info]));
    assert_eq!(group_of(&tree, PanelKind::Source), Some(vec![PanelKind::Source]));
    // clicking a tab and closing a panel go through the shared model; the tree follows
    app.apply_dock(Action::Activate { panel: PanelKind::Source }).unwrap();
    app.apply_dock(Action::Close { panel: PanelKind::Info }).unwrap();
    let tree = window_tree(&app, PanelKind::Project);
    assert_eq!(group_of(&tree, PanelKind::Info), None);
    assert!(group_of(&tree, PanelKind::Source).is_some() && group_of(&tree, PanelKind::Project).is_some());
    // Source docked back into the main window leaves the window's tree
    app.apply_dock(Action::Move { panel: PanelKind::Source, anchor: PanelKind::Program, placement: Placement::Split(Zone::Left) }).unwrap();
    assert!(app.ui.dock.contains(PanelKind::Source));
    assert_eq!(window_tree(&app, PanelKind::Project), DockNode::Tabs { panels: vec![PanelKind::Project], active: 0 });
    assert_eq!(window(&app, PanelKind::Project).map(|w| w.id), Some(id));
    // a panel cannot dock beside itself, and nothing changes
    let before = app.ui.floating.clone();
    assert!(app.apply_dock(Action::Move { panel: PanelKind::Project, anchor: PanelKind::Project, placement: Placement::Split(Zone::Left) }).is_err());
    assert_eq!(app.ui.floating, before);
}

#[test]
fn a_split_window_is_saved_with_the_workspace() {
    let dir = std::env::temp_dir().join(format!("filmcraft-os-windows-split-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut session = Session::default();
    session.prefs_path = Some(dir.join("preferences.json"));
    let mut app = FilmcraftApp::new(session);
    let ctx = egui::Context::default();
    app.ui.dock = layout().root;
    app.ui.floating.clear();
    menus::invoke(&mut app, &ctx, "window.setWindow", json!({"panel": "Project", "rect": [-1920, 0, 1920, 1160]})).unwrap();
    menus::invoke(&mut app, &ctx, "window.dockPanel", json!({"panel": "Source", "anchor": "Project", "zone": "bottom"})).unwrap();
    let saved = app.ui.floating.clone();
    menus::invoke(&mut app, &ctx, "window.workspace.saveAs", json!({"name": "Left Monitor"})).unwrap();
    app.set_workspace("Editing");
    app.set_workspace("Left Monitor");
    assert_eq!(app.ui.floating, saved);
    assert!(matches!(window_tree(&app, PanelKind::Project), DockNode::Split { .. }));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dock_window_returns_every_panel_and_gather_resets_windows() {
    let (mut app, ctx) = app();
    menus::invoke(&mut app, &ctx, "window.setWindow", json!({"panel": "Program", "rect": RIGHT, "fullscreen": true})).unwrap();
    menus::invoke(&mut app, &ctx, "window.setWindow", json!({"panel": "Project", "rect": [-1920, 0, 960, 1080], "maximized": true})).unwrap();
    let gathered = menus::invoke(&mut app, &ctx, "window.gatherWindows", json!({})).unwrap();
    assert_eq!(gathered["windows"].as_array().map(Vec::len), Some(2));
    for group in &app.ui.floating {
        let w = group.window.clone().unwrap();
        assert!(!w.fullscreen && !w.maximized);
        assert!(group.rect[0] >= 0.0 && group.rect[1] >= 0.0, "{:?}", group.rect);
    }
    let docked = menus::invoke(&mut app, &ctx, "window.dockWindow", json!({"panel": "Program"})).unwrap();
    assert_eq!(docked["docked"], json!(["Program"]));
    menus::invoke(&mut app, &ctx, "window.dockWindow", json!({"panel": "Project"})).unwrap();
    assert!(app.ui.floating.is_empty());
    assert!(app.ui.dock.contains(PanelKind::Program) && app.ui.dock.contains(PanelKind::Project));
    assert!(menus::invoke(&mut app, &ctx, "window.dockWindow", json!({"panel": "Program"})).is_err());
}

#[test]
fn hostile_parameters_are_refused_without_changing_the_layout() {
    let (mut app, ctx) = app();
    menus::invoke(&mut app, &ctx, "window.setWindow", json!({"panel": "Program", "rect": RIGHT})).unwrap();
    let before = (app.ui.dock.clone(), app.ui.floating.clone());
    for params in [
        json!({"panel": 5}),
        json!({"panel": "NoSuchPanel"}),
        json!({"panel": "Program", "rect": [1, 2, 3]}),
        json!({"panel": "Program", "rect": [f64::NAN, 0, 100, 100]}),
        json!({"panel": "Program", "rect": [1e100, 0, 100, 100]}),
        json!({"panel": "Program", "rect": "big"}),
        json!({"panel": "Program", "fullscreen": "yes"}),
        json!({"panel": "Program", "maximized": 1}),
        json!([1, 2]),
    ] {
        assert!(menus::invoke(&mut app, &ctx, "window.setWindow", params.clone()).is_err(), "{params}");
        assert_eq!((app.ui.dock.clone(), app.ui.floating.clone()), before, "{params}");
    }
    for id in ["window.toggleWindowFullScreen", "window.dockWindow"] {
        assert!(menus::invoke(&mut app, &ctx, id, json!({"panel": 7})).is_err());
    }
    // out-of-range numbers are clamped, never stored as given
    menus::invoke(&mut app, &ctx, "window.setWindow", json!({"panel": "Program", "rect": [1e9, -1e9, 0, 1e9]})).unwrap();
    let rect = app.ui.floating.iter().find(|g| g.panels.contains(&PanelKind::Program)).unwrap().rect;
    assert!(rect.iter().all(|v| v.is_finite() && v.abs() <= 100_000.0), "{rect:?}");
    assert!(rect[2] >= 100.0 && rect[3] <= 16_384.0, "{rect:?}");
}

#[test]
fn workspaces_save_and_restore_windows() {
    let dir = std::env::temp_dir().join(format!("filmcraft-os-windows-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut session = Session::default();
    session.prefs_path = Some(dir.join("preferences.json"));
    let mut app = FilmcraftApp::new(session);
    let ctx = egui::Context::default();
    app.ui.dock = layout().root;
    app.ui.floating.clear();
    menus::invoke(&mut app, &ctx, "window.setWindow", json!({"panel": "Program", "rect": RIGHT, "fullscreen": true})).unwrap();
    menus::invoke(&mut app, &ctx, "window.setWindow", json!({"panel": "Project", "rect": [-1920, 0, 960, 1080]})).unwrap();
    let saved = app.ui.floating.clone();
    menus::invoke(&mut app, &ctx, "window.workspace.saveAs", json!({"name": "Three Monitors"})).unwrap();
    app.set_workspace("Editing");
    assert!(app.ui.floating.iter().all(|g| g.window.is_none()));
    app.set_workspace("Three Monitors");
    assert_eq!(app.ui.floating, saved);
    // and from the file, after a restart
    let prefs = dock::WorkspacePrefs::load(&dir.join(dock::WORKSPACES_FILE));
    assert_eq!(dock::saved_docking(&prefs, "Three Monitors").floating, saved);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn saved_files_without_windows_keep_their_format_and_bad_windows_stay_isolated() {
    let (mut app, ctx) = app();
    menus::invoke(&mut app, &ctx, "window.floatPanel", json!({"panel": "Info", "rect": [45, 65, 420, 320]})).unwrap();
    let value = serde_json::to_value(&app.ui.floating).unwrap();
    assert!(value[0].get("window").is_none(), "{value}");
    let root = DockNode::Tabs { panels: vec![PanelKind::Project, PanelKind::Timeline], active: 0 };
    let good =
        json!({"name": "Good", "layout": root, "floating": [{"panels": ["Program"], "active": 0, "rect": RIGHT, "window": {"id": 2, "fullscreen": true}}]});
    let bad =
        json!({"name": "Bad", "layout": root, "floating": [{"panels": ["Program"], "active": 0, "rect": RIGHT, "window": {"id": "two", "fullscreen": "yes"}}]});
    let prefs: dock::WorkspacePrefs = serde_json::from_value(json!({"saved": [good, bad], "current": "Good"})).unwrap();
    let loaded = dock::saved_docking(&prefs, "Good");
    let w = loaded.floating.first().and_then(|g| g.window.clone()).unwrap();
    assert!(w.fullscreen && w.id == 2 && !w.maximized);
    // the unreadable workspace falls back without touching the other one
    let fallback = dock::saved_docking(&prefs, "Bad");
    assert!(fallback.floating.iter().all(|g| g.window.is_none()));
}

#[test]
fn embedded_viewports_draw_windows_inside_the_main_window() {
    let (mut app, ctx) = app();
    menus::invoke(&mut app, &ctx, "window.setWindow", json!({"panel": "Info", "rect": [100, 100, 500, 300], "fullscreen": true})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1200.0, 760.0)).build_eframe(move |_| app);
    for _ in 0..3 {
        h.step();
    }
    assert!(h.state().auto.find("panel.Info").is_some());
    let menu: Value = menus::invoke(h.state_mut(), &ctx, "window.listWindows", json!({})).unwrap();
    assert_eq!(menu["windows"][0]["panels"], json!(["Info"]));
}
