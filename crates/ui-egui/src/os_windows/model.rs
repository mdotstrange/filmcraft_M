//! The pure part of panel windows: which floating group is which OS window, and the geometry
//! that moves between them. Nothing here touches egui state, so all of it is unit tested.

use std::cmp::Reverse;

use egui::{Pos2, Rect, ViewportId, pos2};
use serde::{Deserialize, Serialize};

use crate::dock::{Floating, PanelKind};

/// A floating group shown as its own OS window. The group's `rect` then holds the window's outer
/// position and inner size in physical desktop pixels, so it reopens on the same monitor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OsWindow {
    /// Stable identity: the window survives tab changes, and its egui viewport derives from it.
    pub id: u32,
    pub fullscreen: bool,
    pub maximized: bool,
    /// The window's pixels per point when last seen (monitor scale × UI zoom); 0 = unknown.
    pub scale: f32,
}

impl OsWindow {
    pub fn viewport_id(&self) -> ViewportId {
        ViewportId::from_hash_of(("filmcraft-panel-window", self.id))
    }
}

/// Smallest window side, in physical pixels.
pub const MIN_SIDE_PX: f32 = 100.0;
/// Positions beyond this many pixels from the origin are clamped (a hostile or stale file).
pub const COORD_LIMIT_PX: f32 = 100_000.0;
/// Size of a window placed without one, in points.
pub const DEFAULT_SIZE: [f32; 2] = [420.0, 320.0];

/// A usable pixels-per-point: anything non-finite or outside 0.05–16 counts as 1.
pub fn sane_scale(scale: f32) -> f32 {
    if scale.is_finite() && (0.05..=16.0).contains(&scale) { scale } else { 1.0 }
}

/// Clamp a window rect in physical pixels so egui and the GPU surface can always take it.
/// `max_side` is the largest texture side the renderer allows.
pub fn sanitize_rect(rect: [f32; 4], max_side: f32) -> [f32; 4] {
    let max_side = if max_side.is_finite() && max_side >= 256.0 { max_side.min(16_384.0) } else { 8_192.0 };
    let clamp = |value: f32, low: f32, high: f32, fallback: f32| if value.is_finite() { value.clamp(low, high) } else { fallback };
    [
        clamp(rect[0], -COORD_LIMIT_PX, COORD_LIMIT_PX, 0.0),
        clamp(rect[1], -COORD_LIMIT_PX, COORD_LIMIT_PX, 0.0),
        clamp(rect[2], MIN_SIDE_PX, max_side, DEFAULT_SIZE[0]),
        clamp(rect[3], MIN_SIDE_PX, max_side, DEFAULT_SIZE[1]),
    ]
}

/// Where a window's content sits on the desktop: its inner top-left in its own points, and its
/// pixels per point. Converts between that window's points and physical desktop pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub min: Pos2,
    pub ppp: f32,
}

impl Frame {
    pub fn new(min: Pos2, ppp: f32) -> Self {
        Self { min, ppp: sane_scale(ppp) }
    }
    pub fn to_phys(&self, local: Pos2) -> Pos2 {
        pos2((self.min.x + local.x) * self.ppp, (self.min.y + local.y) * self.ppp)
    }
    pub fn to_local(&self, phys: Pos2) -> Pos2 {
        pos2(phys.x / self.ppp - self.min.x, phys.y / self.ppp - self.min.y)
    }
    pub fn rect_to_phys(&self, rect: Rect) -> Rect {
        Rect::from_min_max(self.to_phys(rect.min), self.to_phys(rect.max))
    }
    pub fn rect_to_local(&self, rect: Rect) -> Rect {
        Rect::from_min_max(self.to_local(rect.min), self.to_local(rect.max))
    }
}

/// The panel an action floats into a new group: that group must become a new window, not
/// inherit the window its panel came from.
pub fn floated(action: &craft_ui::docking::Action<PanelKind>) -> Option<PanelKind> {
    match action {
        craft_ui::docking::Action::Float { panel, .. } => Some(*panel),
        _ => None,
    }
}

/// Give the groups of a docking transaction's result back the windows they had before it. The
/// shared docking model knows nothing of OS windows, so every transaction drops them. A new group
/// takes the window of an old one that shares a panel with it: the one with an identical rect
/// first (groups that were not moved keep their rect), then the one sharing the most panels.
/// The group holding `fresh` (just floated) gets none.
pub fn carry_over(before: &[Floating], after: &mut [Floating], fresh: Option<PanelKind>) {
    let limit = craft_ui::docking::MAX_PANELS;
    let mut pairs = Vec::new();
    for (b, old) in before.iter().enumerate().take(limit) {
        let Some(window) = old.window else { continue };
        for (a, new) in after.iter().enumerate().take(limit) {
            if new.window.is_some() || fresh.is_some_and(|panel| new.panels.contains(&panel)) {
                continue;
            }
            let shared = new.panels.iter().filter(|panel| old.panels.contains(panel)).count();
            if shared > 0 {
                pairs.push((new.rect != old.rect, Reverse(shared), b, a, window));
            }
        }
    }
    pairs.sort_by_key(|(moved, shared, b, a, _)| (*moved, *shared, *b, *a));
    let mut taken_before = vec![false; before.len()];
    let mut taken_after = vec![false; after.len()];
    for (_, _, b, a, window) in pairs {
        let (Some(old), Some(new)) = (taken_before.get(b).copied(), taken_after.get(a).copied()) else { continue };
        if old || new {
            continue;
        }
        if let (Some(old), Some(new), Some(group)) = (taken_before.get_mut(b), taken_after.get_mut(a), after.get_mut(a)) {
            *old = true;
            *new = true;
            group.window = Some(window);
        }
    }
}

/// Turn every in-window floating group into an OS window: its rect (main-window points) becomes
/// desktop pixels through the main window's frame. Returns the new windows' ids.
pub fn promote(groups: &mut [Floating], root: Frame) -> Vec<u32> {
    let mut made = Vec::new();
    for index in 0..groups.len() {
        if groups.get(index).is_none_or(|group| group.window.is_some()) {
            continue;
        }
        let id = free_id(groups);
        let Some(group) = groups.get_mut(index) else { continue };
        let [x, y, w, h] = group.rect;
        let min = root.to_phys(pos2(x, y));
        group.rect = [min.x, min.y, w.max(160.0) * root.ppp, h.max(100.0) * root.ppp];
        group.window = Some(OsWindow { id, fullscreen: false, maximized: false, scale: root.ppp });
        made.push(id);
    }
    made
}

/// Give windows that share an id (only a hand-edited file can do that) fresh ones.
pub fn dedupe_ids(groups: &mut [Floating]) {
    let mut seen = Vec::new();
    for index in 0..groups.len() {
        let Some(id) = groups.get(index).and_then(|group| group.window).map(|window| window.id) else { continue };
        if !seen.contains(&id) {
            seen.push(id);
            continue;
        }
        let fresh = free_id(groups);
        if let Some(window) = groups.get_mut(index).and_then(|group| group.window.as_mut()) {
            window.id = fresh;
            seen.push(fresh);
        }
    }
}

/// The smallest window id no group uses.
pub fn free_id(groups: &[Floating]) -> u32 {
    (1..).find(|id| !groups.iter().any(|group| group.window.is_some_and(|window| window.id == *id))).unwrap_or(u32::MAX)
}

/// Whether Escape should leave a fullscreen panel window: only when nothing else wants it (a
/// focused field, a drag, an open menu or popup).
pub fn escape_exits_fullscreen(fullscreen: bool, escape: bool, widget_focused: bool, dragging: bool, menu_open: bool) -> bool {
    fullscreen && escape && !widget_focused && !dragging && !menu_open
}

/// Cascade positions for Gather Panel Windows: windows step down and right from the main
/// window's top-left, each no larger than the main window.
pub fn gathered(index: usize, root_phys: Rect, rect: [f32; 4]) -> [f32; 4] {
    let step = 32.0 * (index.min(16) as f32 + 1.0);
    let width = rect[2].min(root_phys.width() * 0.8).max(MIN_SIDE_PX);
    let height = rect[3].min(root_phys.height() * 0.8).max(MIN_SIDE_PX);
    [root_phys.min.x + step, root_phys.min.y + step, width, height]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(panels: &[PanelKind], rect: [f32; 4], window: Option<u32>) -> Floating {
        Floating { panels: panels.to_vec(), active: 0, rect, window: window.map(|id| OsWindow { id, fullscreen: id % 2 == 0, maximized: false, scale: 1.5 }) }
    }

    fn ids(groups: &[Floating]) -> Vec<Option<u32>> {
        groups.iter().map(|g| g.window.map(|w| w.id)).collect()
    }

    const R1: [f32; 4] = [10.0, 20.0, 400.0, 300.0];
    const R2: [f32; 4] = [900.0, 20.0, 400.0, 300.0];

    #[test]
    fn a_tab_change_keeps_the_window() {
        let before = vec![group(&[PanelKind::Program, PanelKind::Source], R1, Some(4))];
        let mut after = vec![group(&[PanelKind::Program, PanelKind::Source], R1, None)];
        after[0].active = 1;
        carry_over(&before, &mut after, None);
        assert_eq!(after[0].window, before[0].window);
    }

    #[test]
    fn closing_or_reordering_tabs_keeps_the_window() {
        let before = vec![group(&[PanelKind::Program, PanelKind::Source, PanelKind::Info], R1, Some(3))];
        let mut after = vec![group(&[PanelKind::Info, PanelKind::Program], R1, None)];
        carry_over(&before, &mut after, None);
        assert_eq!(ids(&after), vec![Some(3)]);
    }

    #[test]
    fn tearing_a_tab_out_leaves_the_source_window_and_makes_a_new_one() {
        let before = vec![group(&[PanelKind::Program, PanelKind::Source], R1, Some(1))];
        let mut after = vec![group(&[PanelKind::Program], R1, None), group(&[PanelKind::Source], [50.0, 60.0, 420.0, 320.0], None)];
        carry_over(&before, &mut after, Some(PanelKind::Source));
        assert_eq!(ids(&after), vec![Some(1), None]);
    }

    #[test]
    fn floating_a_windows_only_tab_gives_a_new_window() {
        let before = vec![group(&[PanelKind::Program], R1, Some(1))];
        let mut after = vec![group(&[PanelKind::Program], [5.0, 5.0, 420.0, 320.0], None)];
        carry_over(&before, &mut after, Some(PanelKind::Program));
        assert_eq!(ids(&after), vec![None]);
    }

    #[test]
    fn merging_two_windows_keeps_the_target() {
        let before = vec![group(&[PanelKind::Program], R1, Some(1)), group(&[PanelKind::Source], R2, Some(2))];
        // Program dropped into Source's window: the result keeps Source's rect
        let mut after = vec![group(&[PanelKind::Source, PanelKind::Program], R2, None)];
        carry_over(&before, &mut after, None);
        assert_eq!(ids(&after), vec![Some(2)]);
    }

    #[test]
    fn moving_a_window_keeps_it() {
        let before = vec![group(&[PanelKind::Program], R1, Some(7))];
        let mut after = vec![group(&[PanelKind::Program], R2, None)];
        carry_over(&before, &mut after, None);
        assert_eq!(ids(&after), vec![Some(7)]);
    }

    #[test]
    fn docking_the_last_panel_drops_the_window() {
        let before = vec![group(&[PanelKind::Program], R1, Some(1))];
        let mut after: Vec<Floating> = vec![];
        carry_over(&before, &mut after, None);
        assert!(after.is_empty());
    }

    #[test]
    fn each_window_goes_to_one_group() {
        let before = vec![group(&[PanelKind::Program, PanelKind::Source], R1, Some(1))];
        let mut after = vec![group(&[PanelKind::Program], R2, None), group(&[PanelKind::Source], R2, None)];
        carry_over(&before, &mut after, None);
        assert_eq!(after.iter().filter(|g| g.window.is_some()).count(), 1);
    }

    #[test]
    fn many_groups_stay_bounded() {
        let before: Vec<Floating> = (0..200).map(|i| group(&[PanelKind::Info], [i as f32, 0.0, 100.0, 100.0], Some(i))).collect();
        let mut after: Vec<Floating> = (0..200).map(|i| group(&[PanelKind::Info], [i as f32, 0.0, 100.0, 100.0], None)).collect();
        carry_over(&before, &mut after, None);
        assert!(after.iter().take(craft_ui::docking::MAX_PANELS).all(|g| g.window.is_some()));
    }

    #[test]
    fn promote_moves_rects_to_desktop_pixels() {
        let mut groups = vec![group(&[PanelKind::Program], [100.0, 50.0, 400.0, 300.0], None), group(&[PanelKind::Source], R1, Some(1))];
        let made = promote(&mut groups, Frame::new(pos2(-1920.0, 10.0), 2.0));
        assert_eq!(made, vec![2]);
        assert_eq!(groups[0].rect, [-3640.0, 120.0, 800.0, 600.0]);
        assert_eq!(groups[0].window.map(|w| (w.id, w.scale)), Some((2, 2.0)));
        assert_eq!(groups[1].rect, R1);
    }

    #[test]
    fn duplicate_ids_are_renumbered() {
        let mut groups = vec![group(&[PanelKind::Program], R1, Some(1)), group(&[PanelKind::Source], R2, Some(1)), group(&[PanelKind::Info], R2, Some(2))];
        dedupe_ids(&mut groups);
        assert_eq!(ids(&groups), vec![Some(1), Some(3), Some(2)]);
    }

    #[test]
    fn hostile_rects_and_scales_are_made_safe() {
        for rect in [[f32::NAN; 4], [1e30, -1e30, 1e30, -5.0], [0.0, 0.0, 0.0, 0.0], [f32::INFINITY, 0.0, f32::NEG_INFINITY, 1.0]] {
            for max_side in [0.0, f32::NAN, 1e9, 4096.0] {
                let [x, y, w, h] = sanitize_rect(rect, max_side);
                assert!(x.abs() <= COORD_LIMIT_PX && y.abs() <= COORD_LIMIT_PX, "{rect:?}");
                assert!((MIN_SIDE_PX..=16_384.0).contains(&w) && (MIN_SIDE_PX..=16_384.0).contains(&h), "{rect:?}");
            }
        }
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY, 1e9] {
            assert_eq!(sane_scale(scale), 1.0);
            let frame = Frame::new(pos2(0.0, 0.0), scale);
            assert!(frame.to_local(pos2(10.0, 10.0)).is_finite());
        }
    }

    #[test]
    fn coordinates_round_trip_between_monitors_of_different_scale() {
        let left = Frame::new(pos2(-1280.0, 0.0), 1.5);
        let main = Frame::new(pos2(0.0, 0.0), 1.0);
        let phys = left.to_phys(pos2(100.0, 40.0));
        assert_eq!(phys, pos2(-1770.0, 60.0));
        assert_eq!(main.to_local(phys), pos2(-1770.0, 60.0));
        assert_eq!(left.to_local(phys), pos2(100.0, 40.0));
    }

    #[test]
    fn escape_only_leaves_fullscreen_when_nothing_else_wants_it() {
        assert!(escape_exits_fullscreen(true, true, false, false, false));
        assert!(!escape_exits_fullscreen(false, true, false, false, false));
        assert!(!escape_exits_fullscreen(true, false, false, false, false));
        assert!(!escape_exits_fullscreen(true, true, true, false, false));
        assert!(!escape_exits_fullscreen(true, true, false, true, false));
        assert!(!escape_exits_fullscreen(true, true, false, false, true));
    }

    #[test]
    fn gathered_windows_cascade_inside_the_main_window() {
        let root = Rect::from_min_size(pos2(100.0, 100.0), egui::vec2(1600.0, 900.0));
        let a = gathered(0, root, [5000.0, 5000.0, 4000.0, 3000.0]);
        let b = gathered(1, root, [5000.0, 5000.0, 400.0, 300.0]);
        assert_eq!(a, [132.0, 132.0, 1280.0, 720.0]);
        assert_eq!(b, [164.0, 164.0, 400.0, 300.0]);
    }
}
