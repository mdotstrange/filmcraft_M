//! Arranging panels inside a window: splits and tab groups, as in the main window's dock.
//!
//! The shared docking model keeps a window as one flat floating group (it rejects splits there),
//! so a drop into a window is done in two steps: the panel joins the window's group in the shared
//! model (unless it is already in it), then the window's own tree places it beside the drop's
//! anchor. Every other change (close, activate, a panel leaving) goes through the shared model as
//! before, and `model::reconcile` brings the tree back in step.

use craft_ui::docking::{Action, Placement};

use super::model::{carry_over, reconcile, tree_of};
use crate::FilmcraftApp;
use crate::dock::PanelKind;

/// Handle a docking action aimed at a panel in a window (a drop on one of its groups, or
/// `window.dockPanel` with such an anchor). `None` when the action is not one.
pub(crate) fn dock_into_window(app: &mut FilmcraftApp, action: &Action<PanelKind>) -> Option<Result<(), String>> {
    let Action::Move { panel, anchor, placement } = action else { return None };
    let id = app.ui.floating.iter().find(|group| group.panels.contains(anchor)).and_then(|group| group.window.as_ref()).map(|window| window.id)?;
    Some(move_into(app, id, *panel, *anchor, placement.clone()))
}

fn move_into(app: &mut FilmcraftApp, id: u32, panel: PanelKind, anchor: PanelKind, placement: Placement<PanelKind>) -> Result<(), String> {
    if panel == anchor {
        return Err("a panel cannot be docked beside itself".into());
    }
    let in_window = |app: &FilmcraftApp| app.ui.floating.iter().any(|group| group.window.as_ref().is_some_and(|w| w.id == id) && group.panels.contains(&panel));
    let inside = in_window(app);
    if !inside {
        join(app, panel, anchor)?;
    }
    let group = app.ui.floating.iter_mut().find(|group| group.window.as_ref().is_some_and(|w| w.id == id)).ok_or("the window has closed")?;
    let mut tree = tree_of(group);
    reconcile(&mut tree, &group.panels, None);
    let moved = crate::dock_shared::apply_tree(&mut tree, Action::Move { panel, anchor, placement });
    // a panel that just joined stays as a tab when the exact spot is refused
    if !moved && inside {
        return Err("the panel cannot be docked there".into());
    }
    group.active = group.panels.iter().position(|p| *p == panel).unwrap_or(group.active);
    if let Some(window) = group.window.as_mut() {
        window.layout = Some(tree);
    }
    app.ui.keys.maximized = None;
    app.ui.focused = panel;
    Ok(())
}

/// The shared-model half: `panel` joins the flat group that holds `anchor`.
fn join(app: &mut FilmcraftApp, panel: PanelKind, anchor: PanelKind) -> Result<(), String> {
    let action = Action::Move { panel, anchor, placement: Placement::Tab { before: None } };
    let mut shared = crate::dock_shared::encode(&app.ui.dock, &app.ui.floating)?;
    shared.validate().map_err(|error| error.to_string())?;
    crate::dock_shared::check_last_docked(&shared, &action)?;
    shared.apply(action).map_err(|error| error.to_string())?;
    let mut layout = crate::dock_shared::decode(shared)?;
    carry_over(&app.ui.floating, &mut layout.floating, None);
    crate::dock_shared::encode(&layout.root, &layout.floating)?;
    app.ui.dock = layout.root;
    app.ui.floating = layout.floating;
    Ok(())
}
