//! The Uberector theme: the navy / pink palette and the graph-paper background of Uberector (the
//! maintainer's production-planning app) on whichever light or dark theme is shown. Switched by
//! Settings ▸ Appearance ▸ "Uberector Theme" (`appearance.uberectorTheme`) and View ▸ Appearance
//! ▸ Uberector Theme; Appearance Mode (Auto / Dark / Light) keeps choosing the family.
//!
//! Everything lives here, with one-line hooks elsewhere, so upstream merges stay small: the stock
//! themes are untouched while the preference is off ([`Tokens::fill`] is then a plain
//! `rect_filled`). See docs/uberector-theme.md.

use egui::{Color32, ColorImage, CornerRadius, Rect, Stroke, TextureHandle, TextureId, TextureOptions, pos2};
use filmcraft_engine::settings::{self, AppearancePrefs, DEFAULT_HIGHLIGHT};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::theme::{ThemeKind, Tokens};

/// Graph paper behind blank surfaces: one-pixel lines every `minor_step` points and a heavier
/// one every `major_step` (five cells), drawn from one repeating tile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paper {
    pub minor: Color32,
    pub major: Color32,
    pub minor_step: f32,
    pub major_step: f32,
    /// Size in points of the tile built for the current pixel scale (about `major_step`).
    pub tile: f32,
    /// The repeating tile; `None` until built (the lines are then drawn one by one).
    pub tex: Option<TextureId>,
}

/// The live tile texture, owned by the app; rebuilt when the pixel scale or the light / dark
/// family changes. [`Tokens`] carries only its id.
pub struct PaperTile {
    pub handle: TextureHandle,
    pub ppp: f32,
    pub light: bool,
}

/// Uberector's accent (`--accent-primary`).
pub const PINK: Color32 = Color32::from_rgb(0xf6, 0x91, 0xdf);
/// The grid lines' colour (`--accent-secondary`); only their opacity differs per family.
const GRID: [u8; 3] = [54, 111, 180];
const MINOR_STEP: f32 = 12.0;
const MAJOR_STEP: f32 = 60.0;
/// The line fallback draws at most this many lines.
const MAX_LINES: f32 = 2048.0;

const fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn paper(minor_alpha: u8, major_alpha: u8) -> Option<Paper> {
    let [r, g, b] = GRID;
    Some(Paper {
        minor: Color32::from_rgba_unmultiplied(r, g, b, minor_alpha),
        major: Color32::from_rgba_unmultiplied(r, g, b, major_alpha),
        minor_step: MINOR_STEP,
        major_step: MAJOR_STEP,
        tile: MAJOR_STEP,
        tex: None,
    })
}

/// The Uberector palette on `kind`: navy for the dark kinds, pale blue-white for Light. The
/// kind is kept, so the brand wordmark, Settings radios and the highlight rules follow it.
pub fn tokens(kind: ThemeKind) -> Tokens {
    match kind {
        ThemeKind::Light => light(kind),
        ThemeKind::Dark | ThemeKind::Medium => dark(kind),
    }
}

/// `globals.css` `[data-theme="dark"]`; alpha blends baked against the surface they sit on.
fn dark(kind: ThemeKind) -> Tokens {
    let base = rgb(0x2b3067);
    let surface = rgb(0x232856);
    let elevated = rgb(0x1e2348);
    let text = rgb(0xedf2f4);
    let text2 = rgb(0xa8b0c8);
    let muted = rgb(0x6b7394);
    let hover = rgb(0x3c4173);
    let pressed = rgb(0x444979);
    let border = rgb(0x44486f);
    let border_strong = rgb(0x5a5e80);
    Tokens {
        kind,
        app_bg: base,
        header_bg: surface,
        panel_bg: base,
        tab_bg: surface,
        tab_text: text2,
        tab_text_active: text,
        focus: PINK,
        accent: PINK,
        accent_hover: rgb(0xfdd4f6),
        text,
        text_dim: text2,
        text_faint: muted,
        icon: text2,
        icon_active: text,
        hover,
        pressed,
        field_bg: surface,
        field_border: border,
        separator: hover,
        row_alt: rgb(0x31366c),
        row_selected: rgb(0x2f4682),
        hot_text: PINK,
        pill_active_bg: pressed,
        clip_bar_bg: rgb(0x3a4678),
        control_surface: surface,
        control_well: elevated,
        control_border: border_strong,
        control_handle_bg: base,
        control_handle: text,
        control_handle_dim: text2,
        plot_bg: elevated,
        plot_grid: rgb(0x212e57),
        plot_axis: rgb(0x253866),
        plot_handle: text,
        plot_handle_dim: text2,
        meter_bg: rgb(0x141735),
        meter_readout_bg: elevated,
        meter_clip_off: border,
        panner_bg: elevated,
        panner_border: border_strong,
        panner_speaker: text2,
        fader_knob: muted,
        eq_track: hover,
        eq_tick: muted,
        eq_knob: text2,
        eq_knob_active: text,
        eq_node: text,
        eq_node_off: muted,
        crossover_handle: Color32::WHITE,
        keyframe_plot_bg: elevated,
        keyframe_plot_grid: rgb(0x212e57),
        keyframe_plot_axis: rgb(0x253866),
        keyframe_handle: text,
        switch_fill: text2,
        switch_knob: elevated,
        switch_border: muted,
        slider_disabled_track: hover,
        slider_disabled_knob: muted,
        button_hover: hover,
        button_border: border,
        caption_track_bg: rgb(0x302a60),
        scroll_thumb: rgb(0x4b4f7e),
        scroll_thumb_hover: rgb(0x60648d),
        vertical_scroll_thumb: rgb(0x4b4f7e),
        disabled_clip_bg: hover,
        tl_bg: base,
        tl_track_bg: surface,
        tl_track_bg_alt: rgb(0x262b5c),
        tl_header_bg: surface,
        tl_ruler_bg: surface,
        tl_ruler_tick: text2,
        tl_ruler_text: text2,
        playhead: PINK,
        track_target: rgb(0xd57ec1),
        in_out_shade: rgb(0x3f4580),
        clip_selected_border: text,
        monitor_bg: elevated,
        timecode: PINK,
        danger: rgb(0xef4444),
        radius: 0.0,
        radius_sm: 6.0,
        gap: 4.0,
        tab_h: 32.0,
        popup_bg: elevated,
        paper: paper(20, 36),
        ..Tokens::for_kind(kind)
    }
}

/// `globals.css` `[data-theme="light"]`. The accent stays pink; the thin and small uses
/// (playhead, timecode, hot text, track targeting) take the darker hover pink, as `#f691df` is
/// below 2:1 on white.
fn light(kind: ThemeKind) -> Tokens {
    let base = rgb(0xf0f2fa);
    let surface = Color32::WHITE;
    let elevated = rgb(0xe8eaf5);
    let text = rgb(0x2b3067);
    let text2 = rgb(0x64748b);
    let muted = rgb(0x94a3b8);
    let hover = rgb(0xe4e6f1);
    let pressed = rgb(0xdcdfeb);
    let border = rgb(0xdfe0e8);
    let border_strong = rgb(0xcacbd9);
    let pink_dark = rgb(0xe070c5);
    Tokens {
        kind,
        app_bg: base,
        header_bg: surface,
        panel_bg: base,
        tab_bg: surface,
        tab_text: text2,
        tab_text_active: text,
        focus: PINK,
        accent: PINK,
        accent_hover: pink_dark,
        text,
        text_dim: text2,
        text_faint: muted,
        icon: text2,
        icon_active: text,
        hover,
        pressed,
        field_bg: surface,
        field_border: border,
        separator: rgb(0xe0e2ee),
        row_alt: elevated,
        row_selected: rgb(0xcddbec),
        hot_text: pink_dark,
        pill_active_bg: pressed,
        clip_bar_bg: rgb(0xcddbec),
        control_surface: surface,
        control_well: elevated,
        control_border: border_strong,
        control_handle_bg: base,
        control_handle: text,
        control_handle_dim: text2,
        plot_bg: surface,
        plot_grid: rgb(0xe7eef6),
        plot_axis: rgb(0xd3dfee),
        plot_handle: text,
        plot_handle_dim: text2,
        meter_bg: elevated,
        meter_readout_bg: surface,
        meter_clip_off: border_strong,
        panner_bg: elevated,
        panner_border: border_strong,
        panner_speaker: text2,
        fader_knob: muted,
        eq_track: rgb(0xe0e2ee),
        eq_tick: border_strong,
        eq_knob: text2,
        eq_knob_active: text,
        eq_node: text,
        eq_node_off: muted,
        crossover_handle: text,
        keyframe_plot_bg: surface,
        keyframe_plot_grid: rgb(0xe7eef6),
        keyframe_plot_axis: rgb(0xd3dfee),
        keyframe_handle: text,
        switch_fill: text2,
        switch_knob: surface,
        switch_border: muted,
        slider_disabled_track: rgb(0xe0e2ee),
        slider_disabled_knob: border_strong,
        button_hover: hover,
        button_border: border,
        caption_track_bg: rgb(0xece6f7),
        scroll_thumb: rgb(0xccced4),
        scroll_thumb_hover: rgb(0xb4b5bb),
        vertical_scroll_thumb: rgb(0xccced4),
        disabled_clip_bg: pressed,
        tl_bg: base,
        tl_track_bg: surface,
        tl_track_bg_alt: rgb(0xf7f8fc),
        tl_header_bg: surface,
        tl_ruler_bg: surface,
        tl_ruler_tick: text2,
        tl_ruler_text: text,
        playhead: pink_dark,
        track_target: pink_dark,
        in_out_shade: pressed,
        clip_selected_border: text,
        monitor_bg: elevated,
        timecode: pink_dark,
        danger: rgb(0xef4444),
        radius: 0.0,
        radius_sm: 6.0,
        gap: 4.0,
        tab_h: 32.0,
        popup_bg: surface,
        paper: paper(31, 56),
        ..Tokens::for_kind(kind)
    }
}

/// The highlight Settings ▸ Appearance layers on the theme: none while the Uberector theme is on
/// and the highlight is still the default, so the palette's pink stands; a highlight of the
/// user's own wins on any theme.
pub fn highlight(a: &AppearancePrefs) -> Option<[u8; 3]> {
    if a.uberector_theme && a.highlight_color.trim().eq_ignore_ascii_case(DEFAULT_HIGHLIGHT) { None } else { settings::parse_hex(&a.highlight_color) }
}

/// The tokens the app shows for `kind` under `a`: the Uberector palette when it is on, else the
/// stock theme; the highlight and contrast on top; the paper tile built for this context.
pub fn resolve(ctx: &egui::Context, a: &AppearancePrefs, kind: ThemeKind, tile: &mut Option<PaperTile>) -> Tokens {
    let base = if a.uberector_theme { tokens(kind) } else { Tokens::for_kind(kind) };
    let mut t = base.with_appearance(highlight(a), a.accessible_contrast);
    if t.paper.is_some() {
        attach(ctx, &mut t, tile);
    } else {
        *tile = None;
    }
    t
}

/// Once per frame: rebuild the tile when the pixel scale (UI zoom, another monitor) or the
/// family changed under it. A no-op (one compare) otherwise.
pub fn refresh(ctx: &egui::Context, t: &mut Tokens, tile: &mut Option<PaperTile>) {
    let Some(paper) = t.paper else {
        *tile = None;
        return;
    };
    let ppp = ctx.pixels_per_point();
    let light = t.kind == ThemeKind::Light;
    let stale = match tile {
        Some(p) => (p.ppp - ppp).abs() > f32::EPSILON || p.light != light || paper.tex != Some(p.handle.id()),
        None => true,
    };
    if stale {
        attach(ctx, t, tile);
    }
}

fn attach(ctx: &egui::Context, t: &mut Tokens, tile: &mut Option<PaperTile>) {
    let Some(mut paper) = t.paper else {
        *tile = None;
        return;
    };
    let ppp = ctx.pixels_per_point();
    let (image, size) = tile_image(&paper, ppp);
    let handle = ctx.load_texture("uberector-paper", image, TextureOptions::NEAREST_REPEAT);
    paper.tex = Some(handle.id());
    paper.tile = size;
    t.paper = Some(paper);
    *tile = Some(PaperTile { handle, ppp, light: t.kind == ThemeKind::Light });
}

/// One tile of the paper at pixel scale `ppp`: the minor cell rounded to whole pixels, five
/// cells a side, lines `round(ppp)` pixels wide, the major line along the top and left edges.
/// Returns the image and the tile's size in points.
pub fn tile_image(paper: &Paper, ppp: f32) -> (ColorImage, f32) {
    let ppp = if ppp.is_finite() { ppp.clamp(0.5, 4.0) } else { 1.0 };
    let minor_px = ((paper.minor_step * ppp).round() as usize).clamp(1, 64);
    let cells = ((paper.major_step / paper.minor_step).round() as usize).clamp(1, 16);
    let side = minor_px * cells;
    let thick = (ppp.round() as usize).clamp(1, 4);
    let mut img = ColorImage::filled([side, side], Color32::TRANSPARENT);
    for y in 0..side {
        for x in 0..side {
            let c = if x < thick || y < thick {
                paper.major
            } else if x % minor_px < thick || y % minor_px < thick {
                paper.minor
            } else {
                continue;
            };
            if let Some(px) = img.pixels.get_mut(y * side + x) {
                *px = c;
            }
        }
    }
    (img, side as f32 / ppp)
}

/// The tokens a Settings ▸ Appearance theme card previews for `kind` with the draft's
/// Uberector switch, highlight and contrast. The live tile is reused when the card shows the
/// family the app shows; otherwise the preview draws its lines one by one.
pub fn look(live: &Tokens, on: bool, kind: ThemeKind, highlight: Option<[u8; 3]>, contrast: bool) -> Tokens {
    if !on {
        return Tokens::for_kind(kind).with_appearance(highlight, contrast);
    }
    let highlight = if highlight == settings::parse_hex(DEFAULT_HIGHLIGHT) { None } else { highlight };
    let mut t = tokens(kind).with_appearance(highlight, contrast);
    if let (Some(mut paper), Some(live_paper)) = (t.paper, live.paper)
        && (live.kind == ThemeKind::Light) == (kind == ThemeKind::Light)
    {
        paper.tex = live_paper.tex;
        paper.tile = live_paper.tile;
        t.paper = Some(paper);
    }
    t
}

/// View ▸ Appearance ▸ Uberector Theme (`view.theme.uberector`): flip the preference and apply it.
pub fn toggle(app: &mut FilmcraftApp, ctx: &egui::Context) -> Result<Value, String> {
    let on = !app.session.prefs.appearance.uberector_theme;
    app.session.execute("prefs.set", json!({"key": "appearance.uberectorTheme", "value": on})).map_err(|e| e.to_string())?;
    app.apply_prefs(ctx);
    Ok(json!({"uberectorTheme": on}))
}

/// The menu item's checkmark.
pub fn checked(app: &FilmcraftApp, id: &str) -> Option<bool> {
    (id == "view.theme.uberector").then_some(app.session.prefs.appearance.uberector_theme)
}

impl Tokens {
    /// Fill `rect` with `base`, and with the graph paper over it on the Uberector theme. The
    /// grid is aligned to the screen, not to `rect`, so it runs on across the window, the
    /// gutters and every panel. A plain `rect_filled` on the stock themes.
    pub fn fill(&self, p: &egui::Painter, rect: Rect, corner_radius: impl Into<CornerRadius>, base: Color32) {
        let cr = corner_radius.into();
        p.rect_filled(rect, cr, base);
        let Some(paper) = self.paper else { return };
        if !rect.is_positive() || !rect.is_finite() {
            return;
        }
        match paper.tex {
            Some(tex) => {
                // snapped to device pixels first, so the tessellator's own rounding of the rect
                // cannot shift the grid's phase against the texture
                let ppp = p.ctx().pixels_per_point();
                let snap = |v: f32| if ppp > 0.0 && ppp.is_finite() { (v * ppp).round() / ppp } else { v };
                let r = Rect::from_min_max(pos2(snap(rect.min.x), snap(rect.min.y)), pos2(snap(rect.max.x), snap(rect.max.y)));
                let tile = if paper.tile.is_finite() && paper.tile >= 1.0 { paper.tile } else { paper.major_step.max(1.0) };
                let uv = Rect::from_min_max(pos2(r.min.x / tile, r.min.y / tile), pos2(r.max.x / tile, r.max.y / tile));
                p.add(egui::epaint::RectShape::filled(r, cr, Color32::WHITE).with_texture(tex, uv));
            }
            None => lines(p, rect, &paper),
        }
    }
}

/// The paper without a tile (previews before the tile exists): one line segment per grid line,
/// clipped to `rect`, skipped when there would be more than [`MAX_LINES`].
fn lines(p: &egui::Painter, rect: Rect, paper: &Paper) {
    let minor = paper.minor_step;
    if !(minor.is_finite() && minor >= 1.0) || (rect.width() + rect.height()) / minor > MAX_LINES {
        return;
    }
    let cells = ((paper.major_step / minor).round() as i64).max(1);
    let p = p.with_clip_rect(rect.intersect(p.clip_rect()));
    let first = |lo: f32| (lo / minor).ceil() as i64;
    let last = |hi: f32| (hi / minor).floor() as i64;
    let colour = |i: i64| if i.rem_euclid(cells) == 0 { paper.major } else { paper.minor };
    for i in first(rect.min.x)..=last(rect.max.x) {
        let x = i as f32 * minor;
        p.line_segment([pos2(x, rect.min.y), pos2(x, rect.max.y)], Stroke::new(1.0, colour(i)));
    }
    for j in first(rect.min.y)..=last(rect.max.y) {
        let y = j as f32 * minor;
        p.line_segment([pos2(rect.min.x, y), pos2(rect.max.x, y)], Stroke::new(1.0, colour(j)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Shape;

    const KINDS: [ThemeKind; 3] = [ThemeKind::Dark, ThemeKind::Medium, ThemeKind::Light];

    fn prefs(on: bool, highlight: &str) -> AppearancePrefs {
        AppearancePrefs { uberector_theme: on, highlight_color: highlight.into(), ..AppearancePrefs::default() }
    }

    /// Every shape of one frame, flattened.
    fn shapes(ctx: &egui::Context, draw: impl FnMut(&mut egui::Ui)) -> Vec<Shape> {
        let mut out = ctx.run_ui(egui::RawInput::default(), draw);
        out.textures_delta.clear();
        fn flatten(s: Shape, into: &mut Vec<Shape>) {
            match s {
                Shape::Vec(v) => v.into_iter().for_each(|s| flatten(s, into)),
                s => into.push(s),
            }
        }
        let mut all = Vec::new();
        out.shapes.into_iter().for_each(|cs| flatten(cs.shape, &mut all));
        all
    }

    fn luminance(c: Color32) -> u16 {
        u16::from(c.r()) + u16::from(c.g()) + u16::from(c.b())
    }

    #[test]
    fn stock_themes_are_untouched_while_the_preference_is_off() {
        let ctx = egui::Context::default();
        for kind in KINDS {
            let stock = Tokens::for_kind(kind);
            assert!(stock.paper.is_none(), "{kind:?}");
            assert_eq!(stock.popup_bg, stock.panel_bg, "{kind:?}");
            let mut tile = None;
            let shown = resolve(&ctx, &AppearancePrefs::default(), kind, &mut tile);
            assert_eq!(shown, stock.with_appearance(settings::parse_hex(DEFAULT_HIGHLIGHT), false), "{kind:?}");
            assert!(tile.is_none());
        }
    }

    #[test]
    fn uberector_palette_per_kind() {
        let dark = tokens(ThemeKind::Dark);
        let medium = tokens(ThemeKind::Medium);
        let light = tokens(ThemeKind::Light);
        assert_eq!(Tokens { kind: ThemeKind::Dark, ..medium }, dark, "Dark and Medium share the navy palette");
        assert_eq!(dark.app_bg, rgb(0x2b3067));
        assert_eq!(light.app_bg, rgb(0xf0f2fa));
        assert!(luminance(light.app_bg) > luminance(dark.app_bg));
        assert!(luminance(light.text) < luminance(dark.text));
        for t in [dark, medium, light] {
            assert_eq!(t.accent, PINK);
            assert_eq!(t.focus, PINK);
            let paper = t.paper.expect("paper");
            assert_eq!((paper.minor_step, paper.major_step), (12.0, 60.0));
            assert!(paper.tex.is_none());
            // the stock render colours and the kind survive
            assert_eq!(t.render_green, Tokens::for_kind(t.kind).render_green);
        }
        let (d, l) = (dark.paper.expect("paper"), light.paper.expect("paper"));
        assert!(l.minor.a() > d.minor.a() && l.major.a() > d.major.a(), "light paper lines are stronger");
        assert!(d.major.a() > d.minor.a() && l.major.a() > l.minor.a());
    }

    #[test]
    fn default_highlight_leaves_pink_but_a_custom_one_wins() {
        let ctx = egui::Context::default();
        assert_eq!(highlight(&prefs(true, DEFAULT_HIGHLIGHT)), None);
        assert_eq!(highlight(&prefs(true, " #2F6BDF ")), None);
        assert_eq!(highlight(&prefs(false, DEFAULT_HIGHLIGHT)), Some([0x2f, 0x6b, 0xdf]));
        assert_eq!(highlight(&prefs(true, "#ff0000")), Some([255, 0, 0]));
        let mut tile = None;
        let t = resolve(&ctx, &prefs(true, DEFAULT_HIGHLIGHT), ThemeKind::Dark, &mut tile);
        assert_eq!(t.accent, PINK);
        assert_eq!(t.playhead, PINK);
        let t = resolve(&ctx, &prefs(true, "#ff0000"), ThemeKind::Dark, &mut tile);
        assert_eq!(t.accent, Color32::from_rgb(255, 0, 0));
        assert_ne!(t.playhead, PINK, "the custom highlight recolours the playhead");
        let _ = shapes(&ctx, |_| {});
    }

    #[test]
    fn tile_is_built_at_the_pixel_scale() {
        let paper = tokens(ThemeKind::Dark).paper.expect("paper");
        let (img, size) = tile_image(&paper, 1.0);
        assert_eq!((img.size, size), ([60, 60], 60.0));
        let at = |x: usize, y: usize| img.pixels.get(y * 60 + x).copied();
        assert_eq!(at(0, 0), Some(paper.major));
        assert_eq!(at(0, 30), Some(paper.major));
        assert_eq!(at(30, 0), Some(paper.major));
        assert_eq!(at(12, 5), Some(paper.minor));
        assert_eq!(at(5, 24), Some(paper.minor));
        assert_eq!(at(5, 5), Some(Color32::TRANSPARENT));
        assert_eq!(at(7, 19), Some(Color32::TRANSPARENT));
        let (img, size) = tile_image(&paper, 2.0);
        assert_eq!((img.size, size), ([120, 120], 60.0));
        let at = |x: usize, y: usize| img.pixels.get(y * 120 + x).copied();
        assert_eq!(at(1, 0), Some(paper.major), "two-pixel lines at 2x");
        assert_eq!(at(0, 1), Some(paper.major));
        assert_eq!(at(25, 3), Some(paper.minor));
        assert_eq!(at(2, 2), Some(Color32::TRANSPARENT));
        // hostile scales are clamped, never panic
        for ppp in [f32::NAN, f32::INFINITY, -3.0, 0.0, 1e9] {
            let (img, size) = tile_image(&paper, ppp);
            assert!(img.size[0] >= 5 && img.size[0] <= 256 && size.is_finite() && size > 0.0, "{ppp}");
        }
    }

    #[test]
    fn fill_is_one_plain_rect_without_paper_and_a_textured_rect_with_it() {
        let ctx = egui::Context::default();
        let rect = Rect::from_min_max(pos2(30.0, 60.0), pos2(150.0, 180.0));
        let rects = |all: &[Shape]| all.iter().filter_map(|s| if let Shape::Rect(r) = s { Some(r.clone()) } else { None }).collect::<Vec<_>>();

        let stock = Tokens::for_kind(ThemeKind::Dark);
        let all = shapes(&ctx, |ui| stock.fill(ui.painter(), rect, 0.0, stock.panel_bg));
        let r = rects(&all);
        assert_eq!(r.len(), 1);
        assert!(r.first().is_some_and(|r| r.brush.is_none() && r.fill == stock.panel_bg));

        let mut t = tokens(ThemeKind::Dark);
        let tex = TextureId::User(7);
        if let Some(p) = t.paper.as_mut() {
            p.tex = Some(tex);
            p.tile = 60.0;
        }
        let all = shapes(&ctx, |ui| t.fill(ui.painter(), rect, 4.0, t.panel_bg));
        let r = rects(&all);
        assert_eq!(r.len(), 2, "{all:?}");
        assert!(r.first().is_some_and(|r| r.brush.is_none() && r.fill == t.panel_bg));
        let textured = r.get(1).expect("paper rect");
        let brush = textured.brush.as_ref().expect("textured");
        assert_eq!(brush.fill_texture_id, tex);
        assert_eq!(textured.rect, rect);
        assert_eq!(brush.uv, Rect::from_min_max(pos2(0.5, 1.0), pos2(2.5, 3.0)), "uv is the screen rect in tiles");
        assert_eq!(textured.fill, Color32::WHITE);

        if let Some(p) = t.paper.as_mut() {
            p.tex = None;
        }
        let all = shapes(&ctx, |ui| t.fill(ui.painter(), rect, 0.0, t.panel_bg));
        assert_eq!(rects(&all).len(), 1);
        let lines = all.iter().filter(|s| matches!(s, Shape::LineSegment { .. })).count();
        // x = 36..=144 step 12 → 10 lines, y = 60..=180 → 11 lines
        assert_eq!(lines, 21);

        let empty = Rect::from_min_max(pos2(10.0, 10.0), pos2(10.0, 10.0));
        let all = shapes(&ctx, |ui| t.fill(ui.painter(), empty, 0.0, t.panel_bg));
        assert!(all.iter().all(|s| !matches!(s, Shape::LineSegment { .. })), "nothing drawn for an empty rect");
    }

    #[test]
    fn pixel_scale_change_rebuilds_the_tile() {
        let ctx = egui::Context::default();
        let mut tile = None;
        let mut t = resolve(&ctx, &prefs(true, DEFAULT_HIGHLIGHT), ThemeKind::Dark, &mut tile);
        let first = t.paper.and_then(|p| p.tex).expect("tile built");
        assert!(tile.as_ref().is_some_and(|p| p.ppp == 1.0 && !p.light));
        refresh(&ctx, &mut t, &mut tile);
        assert_eq!(t.paper.and_then(|p| p.tex), Some(first), "unchanged scale keeps the tile");
        ctx.set_pixels_per_point(2.0);
        let _ = shapes(&ctx, |_| {});
        refresh(&ctx, &mut t, &mut tile);
        let second = t.paper.and_then(|p| p.tex).expect("tile rebuilt");
        assert_ne!(first, second);
        assert!(tile.as_ref().is_some_and(|p| p.ppp == 2.0));
        let _ = shapes(&ctx, |_| {});
        // off again: the tile is dropped
        let t = resolve(&ctx, &prefs(false, DEFAULT_HIGHLIGHT), ThemeKind::Dark, &mut tile);
        assert!(t.paper.is_none() && tile.is_none());
    }

    #[test]
    fn preview_look_follows_the_draft_switch() {
        let ctx = egui::Context::default();
        let mut tile = None;
        let live = resolve(&ctx, &prefs(true, DEFAULT_HIGHLIGHT), ThemeKind::Dark, &mut tile);
        let highlight = settings::parse_hex(DEFAULT_HIGHLIGHT);
        let off = look(&live, false, ThemeKind::Light, highlight, false);
        assert_eq!(off, Tokens::for_kind(ThemeKind::Light).with_appearance(highlight, false));
        let same_family = look(&live, true, ThemeKind::Medium, highlight, false);
        assert_eq!(same_family.accent, PINK);
        assert_eq!(same_family.paper.and_then(|p| p.tex), live.paper.and_then(|p| p.tex), "reuses the live tile");
        let other_family = look(&live, true, ThemeKind::Light, highlight, false);
        assert!(other_family.paper.is_some_and(|p| p.tex.is_none()), "another family draws its own lines");
        let custom = look(&live, true, ThemeKind::Dark, Some([255, 0, 0]), false);
        assert_eq!(custom.accent, Color32::from_rgb(255, 0, 0));
        let _ = shapes(&ctx, |_| {});
    }

    #[test]
    fn apply_prefs_rethemes_when_the_preference_changes() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let _ = shapes(&ctx, |ui| app.apply_prefs(ui.ctx()));
        assert!(app.tokens.paper.is_none());
        app.session.execute("prefs.set", json!({"key": "appearance.uberectorTheme", "value": true})).expect("set");
        let _ = shapes(&ctx, |ui| app.apply_prefs(ui.ctx()));
        assert!(app.tokens.paper.is_some_and(|p| p.tex.is_some()));
        assert_eq!(app.tokens.accent, PINK);
        assert_eq!(app.tokens.kind, ThemeKind::Dark);
        app.session.execute("prefs.set", json!({"key": "appearance.uberectorTheme", "value": false})).expect("set");
        let _ = shapes(&ctx, |ui| app.apply_prefs(ui.ctx()));
        assert!(app.tokens.paper.is_none());
        assert_eq!(app.tokens, Tokens::for_kind(ThemeKind::Dark).with_appearance(settings::parse_hex(DEFAULT_HIGHLIGHT), false));
    }
}
