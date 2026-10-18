# Uberector theme

**Uberector Theme** restyles FilmCraft after Uberector, the maintainer's production-planning app: navy surfaces with pink, blue and mint accents in the dark modes, a pale blue-white in light mode, and graph paper behind every blank surface instead of flat colour. It is one switch on top of the existing appearance settings, so Appearance Mode (Sync with system / Dark / Light) keeps choosing the family and the rest of Settings ▸ Appearance keeps working.

The code is `crates/ui-egui/src/theme_uberector.rs`.

## Using it

- **Switch it on:** Settings ▸ Appearance ▸ **Uberector Theme**, or View ▸ Appearance ▸ **Uberector Theme** (a checkmark shows it is on). The theme cards on the Appearance page preview the look as soon as the box is ticked, before OK.
- **Light and dark:** Appearance Mode still decides. Darkest and Dark both show the navy palette; Light shows the pale one. Sync with system follows the OS as before.
- **Highlight colour:** while Settings ▸ Appearance ▸ Highlight Color is the default, the theme's pink colours selections, the focus outline, the playhead and timecodes. A highlight of your own wins, on this theme as on the others.
- **Switch it off:** untick the box or choose the menu item again. The stock themes come back exactly as they were.

The preference is `appearance.uberectorTheme` in `preferences.json` (`false` by default; see [project-files.md](project-files.md)).

## Commands (control channel and MCP)

| Command | Params | Does |
|---|---|---|
| `prefs.set` | `{"key": "appearance.uberectorTheme", "value": true}` | Switches the theme on (`false` off); the UI re-themes on the next frame |
| `prefs.get` | `{"key": "appearance.uberectorTheme"}` | `true` or `false` |
| `ui.menu.invoke` | `{"id": "view.theme.uberector"}` | Toggles it, as the menu item does |
| `ui.set` | `{"theme": "dark" \| "medium" \| "light"}` | Picks the family, as before; the Uberector look stays on |

`ui.menu.list` reports the item's `checked` state; `ui.inspect` reports `ui.dark` for the family.

## What it changes

Everything drawn from the design tokens (`crates/ui-egui/src/theme.rs`): panel chrome, headers, tabs, buttons, fields, menus and popups, the timeline's backgrounds, ruler and track rows, the monitors, the Project panel, mixers, meters and plots, selection, focus, playhead and timecodes. Clip colours, Lumetri, the scopes, the graphics editor and the other places that paint fixed colours keep their current look.

**Graph paper.** One-pixel lines every 12 points and a heavier line every 60 (five cells), the blue `#366fb4` at low opacity (8 % / 14 % on the dark paper, 12 % / 22 % on the light one). It is painted wherever a blank surface is: the window and the gutters between panels, every panel body's empty area, the timeline below the last track, the monitor letterbox around the picture, the Project panel's well, the Effect Controls and transition lanes, the Essential Sound, Text, Text-to-Speech, Import and Export panels, and undocked panel windows. The grid is aligned to the screen, not to each panel, so it runs on across edges. Tab strips, track rows, headers, the ruler, fields, menus and the picture itself stay flat.

**Palette (dark modes)**

| Surface | Colour |
|---|---|
| Paper (window, panel bodies, timeline) | `#2b3067` |
| Headers, tab strips, track rows, ruler, fields, mixer strips | `#232856` |
| Menus, popups, dialogs, plots, meters, the monitor behind the picture | `#1e2348` |
| Text / secondary / muted | `#edf2f4` / `#a8b0c8` / `#6b7394` |
| Accent, focus, playhead, timecode | `#f691df` (hover `#fdd4f6`) |
| Hover / pressed | `#3c4173` / `#444979` |
| Danger | `#ef4444` |

**Palette (light mode)**

| Surface | Colour |
|---|---|
| Paper | `#f0f2fa` |
| Headers, tab strips, track rows, fields, menus, plots | `#ffffff` |
| Wells, meters, the monitor behind the picture | `#e8eaf5` |
| Text / secondary / muted | `#2b3067` / `#64748b` / `#94a3b8` |
| Accent and focus | `#f691df`; playhead, timecode and hot text `#e070c5` (the lighter pink is too faint for thin lines on white) |
| Hover / pressed | `#e4e6f1` / `#dcdfeb` |

## Code

- `theme_uberector::tokens(kind)` is the palette over `Tokens::for_kind(kind)`; `resolve` and `refresh` pick it in `FilmcraftApp::set_theme` / `apply_prefs` and keep the paper tile in step with the pixel scale.
- `Tokens` carries `popup_bg` (the elevated surface; the panel colour on the stock themes) and `paper: Option<Paper>` (`None` on the stock themes).
- `Tokens::fill(painter, rect, corner_radius, base)` replaces `rect_filled` at every blank-surface fill. On the stock themes it is exactly `rect_filled`; with paper it adds one rounded rect textured with a repeating one-cell tile (`TextureOptions::NEAREST_REPEAT`), its texture coordinates taken from the screen position so the grid is continuous.
- The hooks in existing files are one-liners: the two `Tokens` fields and `window_fill` in `theme.rs`; `paper_tile`, `resolve`, `refresh` and the window fill in `lib.rs`; the fills in `dock.rs`, `os_windows/render.rs` and the panels listed above; the preference, schema row and Bool sanitising in `crates/engine/src/settings.rs`; the command, dispatch and checkmark in `menus.rs`; the card preview and modal surface in `panels/settings.rs`.
- Tests: `theme_uberector` unit tests, `crates/engine/src/settings_tests.rs` (the preference), `crates/ui-egui/tests/uberector_ui.rs` (paper, menu, families, focus outline).

## Limits

- Panels keep square corners (the stock panel radius); rounded Uberector-style cards would be a follow-up.
- An undocked panel window on a monitor with a different scale samples the main window's tile, so its lines scale with it.
- Accessible colour contrast keeps its grey overrides on the navy palette.
- Fixed colours outside the tokens (clips, scopes, Lumetri) are not restyled.
