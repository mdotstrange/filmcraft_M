//! Keeping a panel window and its saved state in step. The OS owns the window: whatever the user
//! does to it (move, resize, snap, maximize, full screen) is read back into the layout. FilmCraft
//! changes it only when the layout itself changed (a command, a workspace switch, Gather), and
//! then in an order that lands it on the right monitor: out of full screen first, then position
//! and size, then into full screen on whatever monitor the window now is on.

use egui::{Rect, ViewportCommand, pos2, vec2};

use super::model::{OsWindow, sane_scale};

/// Frames to let the OS apply a request before the window is read back again.
const SETTLE_FRAMES: u16 = 20;
/// A difference smaller than this (physical pixels) is rounding, not a move.
const SLACK_PX: f32 = 1.0;

/// The parts of egui's `ViewportInfo` the sync needs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InfoLite {
    pub inner: Option<Rect>,
    pub outer: Option<Rect>,
    pub fullscreen: bool,
    pub maximized: bool,
    pub minimized: bool,
    pub ppp: f32,
}

impl InfoLite {
    pub fn read(ctx: &egui::Context) -> Self {
        ctx.input(|input| {
            let viewport = input.viewport();
            Self {
                inner: viewport.inner_rect,
                outer: viewport.outer_rect,
                fullscreen: viewport.fullscreen.unwrap_or(false),
                maximized: viewport.maximized.unwrap_or(false),
                minimized: viewport.minimized.unwrap_or(false),
                ppp: input.pixels_per_point,
            }
        })
    }
}

/// What the sync last told the window, or saw it agree to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SyncState {
    rect: Option<[f32; 4]>,
    fullscreen: Option<bool>,
    maximized: Option<bool>,
    settle: u16,
}

/// One frame's result: commands for the window, and state to write back into the layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    pub commands: Vec<ViewportCommand>,
    pub rect: Option<[f32; 4]>,
    pub fullscreen: Option<bool>,
    pub maximized: Option<bool>,
    pub scale: Option<f32>,
    /// Still waiting for the OS: keep repainting so the next check comes.
    pub waiting: bool,
}

pub fn step(rect: [f32; 4], window: &OsWindow, state: &mut SyncState, info: &InfoLite) -> Step {
    let mut out = Step::default();
    // a window that is still being created has no position yet
    let (Some(inner), Some(outer)) = (info.inner, info.outer) else {
        out.waiting = true;
        return out;
    };
    let ppp = sane_scale(info.ppp);
    if (ppp - window.scale).abs() > 1e-3 {
        out.scale = Some(ppp);
    }
    if state.settle > 0 {
        state.settle -= 1;
        out.waiting = true;
        return out;
    }
    // The OS (or the user, or Ctrl+` in that window) changed full screen or maximize since the
    // layout last asked for it: the layout follows.
    let mut fullscreen = window.fullscreen;
    if state.fullscreen == Some(window.fullscreen) && info.fullscreen != window.fullscreen {
        fullscreen = info.fullscreen;
        out.fullscreen = Some(fullscreen);
        state.fullscreen = Some(fullscreen);
    }
    let mut maximized = window.maximized;
    if !info.fullscreen && state.maximized == Some(window.maximized) && info.maximized != window.maximized {
        maximized = info.maximized;
        out.maximized = Some(maximized);
        state.maximized = Some(maximized);
    }
    // position and size, only while the window is a plain window
    if !info.fullscreen && !info.maximized && !info.minimized {
        let now = [outer.min.x * ppp, outer.min.y * ppp, inner.width() * ppp, inner.height() * ppp];
        match state.rect {
            // first sight: the window was just created from the layout's rect
            None => state.rect = Some(rect),
            // the layout did not change: follow the window
            Some(applied) if applied == rect => {
                if differs(now, rect) {
                    out.rect = Some(now);
                    state.rect = Some(now);
                }
            }
            // the layout moved it
            Some(_) => {
                out.commands.push(ViewportCommand::OuterPosition(pos2(rect[0] / ppp, rect[1] / ppp)));
                out.commands.push(ViewportCommand::InnerSize(vec2(rect[2] / ppp, rect[3] / ppp)));
                state.rect = Some(rect);
                state.settle = SETTLE_FRAMES;
                out.waiting = true;
                return out;
            }
        }
    }
    if fullscreen != info.fullscreen {
        out.commands.push(ViewportCommand::Fullscreen(fullscreen));
        state.fullscreen = Some(fullscreen);
        state.settle = SETTLE_FRAMES;
        out.waiting = true;
        return out;
    }
    state.fullscreen = Some(info.fullscreen);
    if !info.fullscreen && maximized != info.maximized {
        out.commands.push(ViewportCommand::Maximized(maximized));
        state.maximized = Some(maximized);
        state.settle = SETTLE_FRAMES;
        out.waiting = true;
        return out;
    }
    state.maximized = Some(info.maximized);
    out
}

fn differs(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b.iter()).any(|(a, b)| !a.is_finite() || !b.is_finite() || (a - b).abs() > SLACK_PX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECT: [f32; 4] = [3840.0, 0.0, 1280.0, 720.0];

    fn window(fullscreen: bool) -> OsWindow {
        OsWindow { id: 1, fullscreen, maximized: false, scale: 1.0, layout: None }
    }

    fn info(rect: [f32; 4], fullscreen: bool) -> InfoLite {
        let r = Rect::from_min_size(pos2(rect[0], rect[1]), vec2(rect[2], rect[3]));
        InfoLite { inner: Some(r), outer: Some(r), fullscreen, maximized: false, minimized: false, ppp: 1.0 }
    }

    fn settle(state: &mut SyncState) {
        state.settle = 0;
    }

    #[test]
    fn nothing_happens_until_the_window_exists() {
        let mut state = SyncState::default();
        let out = step(RECT, &window(true), &mut state, &InfoLite::default());
        assert!(out.commands.is_empty() && out.waiting);
        assert_eq!(state, SyncState::default());
    }

    #[test]
    fn a_restored_full_screen_window_goes_full_screen_once_created() {
        let mut state = SyncState::default();
        let out = step(RECT, &window(true), &mut state, &info(RECT, false));
        assert_eq!(out.commands, vec![ViewportCommand::Fullscreen(true)]);
        // while the OS gets there, nothing is sent again
        assert!(step(RECT, &window(true), &mut state, &info(RECT, false)).commands.is_empty());
        settle(&mut state);
        let out = step(RECT, &window(true), &mut state, &info([0.0, 0.0, 3840.0, 2160.0], true));
        assert!(out.commands.is_empty() && out.fullscreen.is_none() && out.rect.is_none());
    }

    #[test]
    fn the_layout_follows_a_window_the_user_moved() {
        let mut state = SyncState::default();
        step(RECT, &window(false), &mut state, &info(RECT, false));
        let moved = [100.0, 200.0, 640.0, 480.0];
        let out = step(RECT, &window(false), &mut state, &info(moved, false));
        assert_eq!(out.rect, Some(moved));
        assert!(out.commands.is_empty());
    }

    #[test]
    fn the_layout_follows_full_screen_changed_in_the_window() {
        let mut state = SyncState::default();
        step(RECT, &window(false), &mut state, &info(RECT, false));
        let out = step(RECT, &window(false), &mut state, &info(RECT, true));
        assert_eq!(out.fullscreen, Some(true));
        assert!(out.commands.is_empty());
    }

    #[test]
    fn no_read_back_while_full_screen_maximized_or_minimized() {
        for (fullscreen, maximized, minimized) in [(true, false, false), (false, true, false), (false, false, true)] {
            let w = OsWindow { id: 1, fullscreen, maximized, scale: 1.0, layout: None };
            let mut state = SyncState { rect: Some(RECT), fullscreen: Some(fullscreen), maximized: Some(maximized), settle: 0 };
            let mut i = info([0.0, 0.0, 3840.0, 2160.0], fullscreen);
            i.maximized = maximized;
            i.minimized = minimized;
            assert!(step(RECT, &w, &mut state, &i).rect.is_none());
        }
    }

    #[test]
    fn a_layout_move_sends_position_and_size() {
        let mut state = SyncState::default();
        step(RECT, &window(false), &mut state, &info(RECT, false));
        let target = [-1920.0, 0.0, 800.0, 600.0];
        let out = step(target, &window(false), &mut state, &info(RECT, false));
        assert_eq!(out.commands, vec![ViewportCommand::OuterPosition(pos2(-1920.0, 0.0)), ViewportCommand::InnerSize(vec2(800.0, 600.0))]);
    }

    #[test]
    fn going_full_screen_on_another_monitor_moves_first() {
        let mut state = SyncState::default();
        step(RECT, &window(false), &mut state, &info(RECT, false));
        let right = [7680.0, 0.0, 1280.0, 720.0];
        let out = step(right, &window(true), &mut state, &info(RECT, false));
        assert!(matches!(out.commands.first(), Some(ViewportCommand::OuterPosition(_))));
        assert!(!out.commands.contains(&ViewportCommand::Fullscreen(true)));
        settle(&mut state);
        let out = step(right, &window(true), &mut state, &info(right, false));
        assert_eq!(out.commands, vec![ViewportCommand::Fullscreen(true)]);
    }

    #[test]
    fn leaving_full_screen_comes_before_a_layout_move() {
        let mut state = SyncState { rect: Some(RECT), fullscreen: Some(true), maximized: Some(false), settle: 0 };
        let gathered = [100.0, 100.0, 640.0, 480.0];
        let out = step(gathered, &window(false), &mut state, &info([0.0, 0.0, 3840.0, 2160.0], true));
        assert_eq!(out.commands, vec![ViewportCommand::Fullscreen(false)]);
        settle(&mut state);
        let out = step(gathered, &window(false), &mut state, &info(RECT, false));
        assert!(matches!(out.commands.first(), Some(ViewportCommand::OuterPosition(_))));
    }

    #[test]
    fn a_hostile_scale_never_divides_by_zero() {
        let mut state = SyncState { rect: Some(RECT), ..Default::default() };
        let mut i = info(RECT, false);
        i.ppp = 0.0;
        let out = step([1.0, 2.0, 300.0, 200.0], &window(false), &mut state, &i);
        for command in out.commands {
            match command {
                ViewportCommand::OuterPosition(p) => assert!(p.is_finite()),
                ViewportCommand::InnerSize(s) => assert!(s.is_finite()),
                _ => {}
            }
        }
    }
}
