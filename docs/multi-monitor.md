# Panel windows on several monitors

On the desktop, every undocked panel group is a real OS window. You can drag it to any monitor, snap it with Windows Snap, maximize it, or make it full screen. Inside a window, panels split and tab exactly as in the main window, so one window can show several panels at once. A typical three-monitor setup:

| Monitor | Holds |
|---|---|
| Left | One window: Project on top, Source below (Effects beside Project, if you like) |
| Centre | The main window: Timeline and the rest of the docked panels |
| Right | Program, full screen |

The code is in `crates/ui-egui/src/os_windows/`.

## Using it

- **Undock a panel:** use **Undock Panel** in the panel's ≡ menu, or drag its tab out of the main window. A tab dropped on empty desktop opens a new window under the cursor. Ctrl while dragging always undocks.
- **Arrange panels inside a window:** drag a tab onto a group in the window. Drop it on the group's tab strip or centre to add a tab; drop it near a side to split the group there. Drag the divider between groups to resize them. This works the same whether the panel comes from the main window, another window or the same window.
- **Move panels between windows:** drag a tab into another window, or onto a docked group or the edge of the main window to dock it again.
- **Full screen:** a panel window's ≡ menu has **Full Screen Window**. **Ctrl+`** pressed in that window does the same. **Escape** leaves full screen when no field or menu wants the key. The window goes full screen on the monitor it is on.
- **Dock everything back:** ≡ ▸ **Dock Window**, or close the window with its close button.
- **Keep the layout:** Window ▸ Workspaces ▸ **Save as New Workspace…** (or **Save Changes**) saves the windows with the workspace: monitor position, size, full screen and maximized state, and how each window's panels are split and tabbed. Switching to the workspace, or starting FilmCraft with it, puts every window back.
- **Lost a window?** For example when a monitor is unplugged. Window ▸ **Gather Panel Windows** takes every window out of full screen and stacks them over the main window.

Shortcuts work in every panel window: Space, J/K/L, In and Out marks, and the rest. Panel-specific keys follow the panel you clicked last.

## Commands (control channel and MCP)

| Command | Params | Does |
|---|---|---|
| `window.setWindow` | `panel?`, `rect?` `[x, y, width, height]` in desktop pixels, `fullscreen?`, `maximized?` | Puts the panel's group in its own window (undocking it if needed) and places it |
| `window.toggleWindowFullScreen` | `panel?` | Full screen on or off for the panel's window (for a docked panel, the main window) |
| `window.dockWindow` | `panel?` | Docks every panel of that window |
| `window.gatherWindows` | | Takes all windows out of full screen and stacks them over the main window |
| `window.listWindows` | | `main` (the main window's live geometry) and `windows`: id, panels, saved rect, full screen, live geometry, automation elements |

The panel defaults to the focused one. `window.dockPanel` with an `anchor` panel that is in a window docks into that window (`zone` `left`/`right`/`top`/`bottom` splits beside the anchor, `center` adds a tab).

Each saved floating group carries a `window` object: `{id, fullscreen, maximized, scale, layout?}`. `layout` is the window's dock tree, in the same form as a workspace's `layout`; without it the window is one tab group of the group's `panels`. The shared docking model sees a window as that flat `panels` list, and the tree is kept in step with it after every change. The group's `rect` is in desktop pixels: the window's outer position and inner size. Older workspace files without `window` still load.

## Limits

- **Screenshots and synthetic clicks.** `ui.screenshot`, `ui.click` and `ui.drag` reach only the main window. Panel windows' elements are listed by `window.listWindows`, in that window's points. Drive panel windows with the commands above.
- **Dialogs** opened from a panel window appear over the main window.
- **Import and Export modes** close the panel windows. They reopen where they were on returning to Edit.
- **Overlapping windows.** egui cannot see which OS window is in front, so a drag over a spot where windows overlap assumes the panel window is in front.
- **Performance.** All windows redraw together. Playback with three windows measured about 75 UI frames per second with no dropped frames on a 3 × 1920×1200 setup.
