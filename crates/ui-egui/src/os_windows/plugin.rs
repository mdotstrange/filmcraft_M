//! Pointer plumbing between windows. While a mouse button is held, the OS sends every pointer
//! event to the window the press began in, so a clip dragged from the Project panel on one
//! monitor never reaches the Timeline on another. This egui plugin sees each window's input
//! before its pass: it records which window owns the press, and replays the drag (pointer
//! position, modifiers, the release) into the window under the cursor.

use egui::{Event, Modifiers, Pos2, RawInput, ViewportId};

/// A drag handed on to another window: the cursor in that window's points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Forward {
    pub target: ViewportId,
    pub pos: Pos2,
    pub released: bool,
    pub modifiers: Modifiers,
    pub release_sent: bool,
}

#[derive(Default)]
pub(crate) struct OsWindowsPlugin {
    /// The window the last mouse press began in.
    pub press_owner: Option<ViewportId>,
    forward: Option<Forward>,
    /// A window a forwarded pointer has just left: it gets `PointerGone` once.
    gone: Option<ViewportId>,
}

impl OsWindowsPlugin {
    pub(crate) fn set_forward(&mut self, next: Option<Forward>) {
        let old = self.forward.take();
        let old_target = old.map(|forward| forward.target);
        if old_target.is_some() && old_target != next.map(|forward| forward.target) {
            self.gone = old_target;
        }
        self.forward = match (next, old) {
            // the release goes out once, however long the drag stays over the window
            (Some(mut next), Some(old)) if next.target == old.target => {
                next.release_sent |= old.release_sent;
                Some(next)
            }
            (next, _) => next,
        };
    }

    fn feed(&mut self, input: &mut RawInput) {
        let id = input.viewport_id;
        if input.events.iter().any(|event| matches!(event, Event::PointerButton { pressed: true, .. })) {
            self.press_owner = Some(id);
        }
        if self.gone == Some(id) {
            input.events.push(Event::PointerGone);
            self.gone = None;
        }
        let Some(forward) = self.forward.as_mut().filter(|forward| forward.target == id) else { return };
        input.events.push(Event::ModifiersChanged(forward.modifiers));
        input.events.push(Event::PointerMoved(forward.pos));
        if forward.released && !forward.release_sent {
            input.events.push(Event::PointerButton { pos: forward.pos, button: egui::PointerButton::Primary, pressed: false, modifiers: forward.modifiers });
            forward.release_sent = true;
        }
    }
}

impl egui::Plugin for OsWindowsPlugin {
    fn debug_name(&self) -> &'static str {
        "filmcraft-panel-windows"
    }

    fn input_hook(&mut self, _ctx: &egui::Context, input: &mut RawInput) {
        self.feed(input);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: ViewportId, events: Vec<Event>) -> RawInput {
        RawInput { viewport_id: id, events, ..Default::default() }
    }

    fn child() -> ViewportId {
        ViewportId::from_hash_of("child")
    }

    fn forward(target: ViewportId, released: bool) -> Forward {
        Forward { target, pos: Pos2::new(10.0, 20.0), released, modifiers: Modifiers::COMMAND, release_sent: false }
    }

    #[test]
    fn a_press_marks_its_window_as_the_owner() {
        let mut plugin = OsWindowsPlugin::default();
        let press = Event::PointerButton { pos: Pos2::ZERO, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE };
        plugin.feed(&mut input(child(), vec![press]));
        assert_eq!(plugin.press_owner, Some(child()));
        // a release (forwarded or real) does not move the ownership
        let release = Event::PointerButton { pos: Pos2::ZERO, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE };
        plugin.feed(&mut input(ViewportId::ROOT, vec![release]));
        assert_eq!(plugin.press_owner, Some(child()));
    }

    #[test]
    fn a_forwarded_drag_reaches_only_its_target_and_releases_once() {
        let mut plugin = OsWindowsPlugin::default();
        plugin.set_forward(Some(forward(ViewportId::ROOT, false)));
        let mut other = input(child(), vec![]);
        plugin.feed(&mut other);
        assert!(other.events.is_empty());
        let mut root = input(ViewportId::ROOT, vec![]);
        plugin.feed(&mut root);
        assert!(root.events.contains(&Event::PointerMoved(Pos2::new(10.0, 20.0))));
        assert!(root.events.contains(&Event::ModifiersChanged(Modifiers::COMMAND)));
        plugin.set_forward(Some(forward(ViewportId::ROOT, true)));
        let releases = |events: &[Event]| events.iter().filter(|e| matches!(e, Event::PointerButton { pressed: false, .. })).count();
        let mut root = input(ViewportId::ROOT, vec![]);
        plugin.feed(&mut root);
        assert_eq!(releases(&root.events), 1);
        plugin.set_forward(Some(forward(ViewportId::ROOT, true)));
        let mut root = input(ViewportId::ROOT, vec![]);
        plugin.feed(&mut root);
        assert_eq!(releases(&root.events), 0);
    }

    #[test]
    fn leaving_a_window_tells_it_the_pointer_is_gone() {
        let mut plugin = OsWindowsPlugin::default();
        plugin.set_forward(Some(forward(ViewportId::ROOT, false)));
        plugin.set_forward(None);
        let mut root = input(ViewportId::ROOT, vec![]);
        plugin.feed(&mut root);
        assert_eq!(root.events, vec![Event::PointerGone]);
        let mut root = input(ViewportId::ROOT, vec![]);
        plugin.feed(&mut root);
        assert!(root.events.is_empty());
    }
}
