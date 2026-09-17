# TUI navigation decisions

The application uses a persistent top navigation bar and one content pane.
It starts the read-only storage audit automatically, then opens the Storage
audit when the inventory is ready. There is no separate Overview destination:
the first screen is the work that produces the evidence needed for a decision.
The implementation stays in Ratatui; changing frameworks would not resolve the
input-routing bugs and would add a large migration to a small interaction fix.

## Research

- [Ratatui's List example](https://ratatui.rs/examples/widgets/list/) demonstrates
  `List`, `ListState`, a selected item, highlight styling, and a focus marker.
  We use that widget instead of painting each menu item independently. Mouse
  hit testing derives from the same list rectangle and item heights.
- [Textual's input guide](https://textual.textualize.io/guide/input/) describes a
  single input-focused widget, focus styling, click-to-focus, and Tab/Shift+Tab
  traversal. This is the basis for explicit menu/content focus and exclusive
  dispatch of ordinary keys.
- [Terminal.Gui's overview](https://gui-cs.github.io/Terminal.Gui/docs/overview.html)
  also assigns Tab and Shift+Tab to the next and previous logical view. This
  supports using those familiar keys rather than requiring F8 to reach the menu.

These references support the interaction patterns. The two-pane layout and the
specific shortcut mapping below are application design choices, not a claim
that one menu pattern is universally best.

## Behavior

| Input | Behavior |
| --- | --- |
| Tab / Shift+Tab | Move focus between the top menu and content, preserving content state. |
| F8 | Alternative pane-focus shortcut. |
| Up / Down, j / k | Move within the focused list; skip unavailable destinations. |
| Home / End in menu | First / last available destination. |
| Enter / Right in menu | Open the chosen section and focus its content. |
| Esc in menu | Return focus to the current content without activating the choice. |
| 1–3 | Open a section directly, when navigation is available. |
| Click | Focus the clicked pane; a menu row also opens that section. |
| Mouse wheel | Scroll the pane under the pointer. |
| e / h / f / v or [ / ] in storage content | Choose Explore, Heatmap, Cleanup decisions, or Scan coverage. |
| Click a storage-map rectangle | Open that exact folder or inspect that file; preserve cleanup selection. |
| h / e in storage content | Expand the selected item’s map / return to the folder browser. |
| i in storage content | Read full information and decision guidance for the selection. |
| F9 / ? | Commands / keyboard help. |

The open section remains marked when the menu cursor moves elsewhere. Only the
focused top-menu item receives its strong single-line highlight; the status bar
identifies the scan state and keyboard focus. The menu remains visible at the
minimum 60×16 viewport. Read-only sessions visibly disable Move data and skip it
during menu navigation.

Ordinary menu input is fully consumed: c, d, Space, a, m, and storage-view
shortcuts cannot operate on the content while the menu owns focus. Confirmation
dialogs and work in progress retain their existing navigation restrictions.
