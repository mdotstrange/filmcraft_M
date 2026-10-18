//! The Uberector theme (docs/uberector-theme.md), driven headless through the control channel:
//! the preference paints the graph paper and recolours the chrome, the View ▸ Appearance item
//! toggles it with a checkmark, the light and dark families both carry it, and the focus outline
//! stays on top of the paper.

use std::sync::mpsc::{Sender, channel};

use egui::Color32;
use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use filmcraft_ui_egui::theme::ThemeKind;
use filmcraft_ui_egui::theme_uberector::PINK;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
}

impl Driver {
    fn demo() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx };
        d.frames(4);
        d
    }
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }
    fn call(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method} {params}");
    }
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params.clone());
        assert_eq!(r["ok"], true, "{method} {params}: {r}");
        r["result"].clone()
    }
    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }
    fn menu(&mut self, id: &str) -> Value {
        let r = self.ok("ui.menu.invoke", json!({"id": id}));
        self.frames(2);
        r
    }
    fn set_uberector(&mut self, on: bool) {
        self.exec("prefs.set", json!({"key": "appearance.uberectorTheme", "value": on}));
        self.frames(3);
    }
    fn menu_item(&mut self, id: &str) -> Value {
        let items = self.ok("ui.menu.list", json!({}));
        items.as_array().and_then(|a| a.iter().find(|i| i["id"] == id).cloned()).unwrap_or_else(|| panic!("no menu item {id}: {items}"))
    }
    /// The rects of the last frame filled with texture `tex` (the paper tile).
    fn papered(&self, tex: egui::TextureId) -> Vec<egui::Rect> {
        fn walk(shape: &egui::Shape, tex: egui::TextureId, out: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(r) if r.brush.as_ref().is_some_and(|b| b.fill_texture_id == tex) => out.push(r.rect),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, tex, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for s in &self.harness.output().shapes {
            walk(&s.shape, tex, &mut out);
        }
        out
    }
}

#[test]
fn uberector_theme_paints_graph_paper_and_toggles_from_the_menu() {
    let mut d = Driver::demo();
    let stock = d.harness.state().tokens;
    assert!(stock.paper.is_none() && stock.accent != PINK, "stock theme first");
    assert_eq!(d.menu_item("view.theme.uberector")["checked"], json!(false));

    d.set_uberector(true);
    let t = d.harness.state().tokens;
    let tex = t.paper.and_then(|p| p.tex).expect("the paper tile is built");
    assert_eq!((t.accent, t.focus, t.playhead), (PINK, PINK, PINK), "pink stands while the highlight is the default");
    assert_eq!(t.app_bg, Color32::from_rgb(0x2b, 0x30, 0x67));
    let papers = d.papered(tex);
    assert!(papers.len() >= 3, "window, gutters and panel bodies carry the paper: {papers:?}");
    assert!(papers.iter().any(|r| r.width() >= 1590.0 && r.height() >= 970.0), "the whole window is papered: {papers:?}");
    let item = d.menu_item("view.theme.uberector");
    assert_eq!((item["checked"].clone(), item["label"].clone()), (json!(true), json!("Uberector Theme")));
    assert_eq!(d.ok("ui.inspect", json!({}))["ui"]["dark"], json!(true));

    // the menu item flips it back; the paper and the pink go
    d.menu("view.theme.uberector");
    assert!(!d.harness.state().session.prefs.appearance.uberector_theme);
    let t = d.harness.state().tokens;
    assert!(t.paper.is_none());
    assert_eq!(t.accent, stock.accent);
    assert_eq!(d.menu_item("view.theme.uberector")["checked"], json!(false));
    assert!(d.papered(tex).is_empty(), "no shape still uses the dropped tile");
}

#[test]
fn uberector_follows_the_light_and_dark_modes() {
    let mut d = Driver::demo();
    d.set_uberector(true);
    for (theme, kind, app_bg, dark) in [
        ("light", ThemeKind::Light, Color32::from_rgb(0xf0, 0xf2, 0xfa), false),
        ("medium", ThemeKind::Medium, Color32::from_rgb(0x2b, 0x30, 0x67), true),
        ("dark", ThemeKind::Dark, Color32::from_rgb(0x2b, 0x30, 0x67), true),
    ] {
        d.ok("ui.set", json!({"theme": theme}));
        d.frames(3);
        let t = d.harness.state().tokens;
        assert_eq!((t.kind, t.app_bg, t.accent), (kind, app_bg, PINK), "{theme}");
        let tex = t.paper.and_then(|p| p.tex).unwrap_or_else(|| panic!("{theme}: paper tile"));
        assert!(!d.papered(tex).is_empty(), "{theme}: the paper is painted");
        assert_eq!(d.ok("ui.inspect", json!({}))["ui"]["dark"], json!(dark), "{theme}");
        assert!(d.harness.state().session.prefs.appearance.uberector_theme, "{theme}: the switch survives a family change");
    }
    // a highlight of the user's own wins over the pink
    d.exec("prefs.set", json!({"key": "appearance.highlightColor", "value": "#ff0000"}));
    d.frames(3);
    assert_eq!(d.harness.state().tokens.accent, Color32::from_rgb(255, 0, 0));
    assert!(d.harness.state().tokens.paper.is_some(), "the paper stays");
}

/// #1075 with the paper on: the overlay is painted with each base fill, so no later child
/// background covers the focus outline.
#[test]
fn focus_outline_survives_the_paper_overlay() {
    fn rectangles<'a>(shape: &'a egui::Shape, clip: egui::Rect, out: &mut Vec<(&'a egui::epaint::RectShape, egui::Rect)>) {
        match shape {
            egui::Shape::Rect(rect) => out.push((rect, clip)),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| rectangles(shape, clip, out)),
            _ => {}
        }
    }
    let mut d = Driver::demo();
    d.set_uberector(true);
    for theme in ["dark", "light"] {
        d.ok("ui.set", json!({"theme": theme}));
        for panel in ["Program", "Timeline", "Project", "EffectControls", "EssentialSound", "Text"] {
            d.ok("ui.panel.show", json!({"panel": panel}));
            d.ok("ui.set", json!({"focused": panel}));
            d.frames(3);
            let mut rects = Vec::new();
            for shape in &d.harness.output().shapes {
                rectangles(&shape.shape, shape.clip_rect, &mut rects);
            }
            let focus = d.harness.state().tokens.focus;
            assert_eq!(focus, PINK);
            let outlines: Vec<_> = rects
                .iter()
                .enumerate()
                .filter(|(_, (r, _))| r.stroke.color == focus && r.stroke.width == 1.0 && r.rect.width() > 100.0 && r.rect.height() > 80.0)
                .collect();
            assert_eq!(outlines.len(), 1, "{theme} {panel}: one focus outline for the whole parent group");
            let (at, (outline, _)) = outlines[0];
            let bounds = outline.rect;
            let strips = [
                egui::Rect::from_min_max(bounds.min, egui::pos2(bounds.max.x, bounds.min.y + 1.0)),
                egui::Rect::from_min_max(egui::pos2(bounds.min.x, bounds.max.y - 1.0), bounds.max),
                egui::Rect::from_min_max(bounds.min, egui::pos2(bounds.min.x + 1.0, bounds.max.y)),
                egui::Rect::from_min_max(egui::pos2(bounds.max.x - 1.0, bounds.min.y), bounds.max),
            ];
            for (child, clip) in &rects[at + 1..] {
                if child.fill.a() != 0 {
                    let painted = child.rect.intersect(*clip);
                    assert!(
                        strips.iter().all(|edge| !edge.intersect(painted).is_positive()),
                        "{theme} {panel}: a later child background {painted:?} covers the focus outline {bounds:?}"
                    );
                }
            }
        }
    }
}
