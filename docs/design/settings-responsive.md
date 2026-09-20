# Settings Responsive Design

Status: Finalized — implementation in progress
Scope: Settings pages only
Target branch: `zerob13/settings-responsive`

## 1. Objective

Make all five Settings pages usable from a minimum logical viewport of approximately 360px wide without changing the main terminal/workspace layout.

The responsive work is scoped to:

- `crates/con-app/src/settings_panel.rs`
- the standalone Settings window sizing and behavior in `crates/con-app/src/workspace/window_actions.rs`

No main terminal, workspace, agent panel, or application-wide breakpoint behavior should change.

## 2. Current Findings

The Settings panel already has a compact mode below 980px and an icon-only navigation rail below 840px. That is not sufficient for phone-sized widths because the content still contains fixed horizontal controls.

At approximately 360px:

```text
Current overlay geometry (approx.)

viewport 360
├── outer settings rail             48
├── content padding                 14 + 14
└── usable form width               ~284

A single row may still require:
label + gap + fixed control + horizontal padding
```

The main constraints are:

| Area | Current constraint | Result at 360px |
|---|---|---|
| `row_field` | input `min_w(160px)`, non-shrinking label | proxy rows can overflow |
| `row_input_with_hint` | input `min_w(180px)` | shell row cannot reliably fit |
| `searchable_select_row` | select `w(236px)`, non-shrinking | font/model selectors overflow |
| `select_row` | select `w(188px)`, non-shrinking | cursor/fit/position rows become cramped |
| `slider_row` | control area `w(260px)`, non-shrinking | sliders overflow |
| Providers | nested provider sidebar `148–180px` | almost no width remains for form content |
| Keys | fixed-height horizontal rows | long labels compete with keycaps |
| Header | text labels plus actions | action group can become crowded |

## 3. Responsive Breakpoints

Use the Settings viewport width only. These breakpoints must not be reused for the main workspace.

| Mode | Width | Navigation | Form behavior |
|---|---:|---|---|
| `mobile` | `< 600px` | horizontal scrollable top tabs | all form controls stack vertically |
| `narrow` | `600–839px` | 48px icon rail | controls become flexible; stacked controls remain allowed |
| `compact` | `840–979px` | 144px labeled rail | normal rows with flexible controls |
| `regular` | `>= 980px` | 160px labeled rail | full desktop layout |

The design target is 360px, but the mobile mode should continue to behave well through 599px. There should be no special behavior that only works at exactly 360px.

### Minimum standalone window

The standalone Settings window should have a minimum logical size near `360 × 520` px if the GPUI window API permits it without affecting other windows. The minimum height is intentionally approximate: content remains vertically scrollable, and the window must not force the main workspace to adopt the same constraint.

If a platform window manager refuses a 360px physical minimum, the Settings content must still remain correct when its viewport is constrained to that width.

## 4. Global Settings Shell

### Regular and compact layout

```text
+-----------------------------------------------------------------------+
| Settings                                      [Saved] [Config] [Save] |
+------------------+----------------------------------------------------+
| General          |                                                    |
| Appearance       |              scrollable page content                |
| AI               |                                                    |
| Providers        |                                                    |
| Keys             |                                                    |
+------------------+----------------------------------------------------+
```

### Mobile layout

The outer sidebar must not consume 48px at mobile width. Replace it with an adaptive, wrapping tab strip. Do not use icon-only navigation as the only mobile label: five icon-only buttons are compact but not sufficiently discoverable.

The tab strip must wrap instead of horizontally scrolling. At 360px, two rows are preferable to hiding part of the Settings information architecture behind a scroll gesture.

```text
+--------------------------------------+
| Settings                    [✓] [▣] [↗]
+--------------------------------------+
| [☷ General] [☼ Appearance] [● AI]   |
| [🔌 Providers] [⌨ Keys]              |
+--------------------------------------+
|                                      |
| General                              |
| Terminal defaults and app behavior.  |
|                                      |
| ...                                  |
|                                      |
+--------------------------------------+
```

Mobile navigation rules:

- use `flex_wrap()` with natural tab widths; do not introduce horizontal scrolling;
- each tab has an icon and a complete text label;
- the active tab uses the existing semantic active fill/accent;
- each tab has a minimum 32px height and an adequate horizontal hit area;
- the navigation container grows with its wrapped rows, normally reaching two rows at 360px;
- at wider mobile widths, the same tabs naturally return to one row when they fit;
- do not clip or ellipsize tab labels to force a single row;
- the page body remains vertically scrollable, while the navigation strip itself never scrolls horizontally;
- no main workspace navigation or sidebar behavior changes.

### Mobile header actions

At `< 600px`, remove non-essential header text but preserve affordances:

```text
regular:  [Saved] [Open config] [Save]
mobile:   [status icon] [config icon] [save icon]
```

Every icon-only action must keep a tooltip/accessibility label. Save remains disabled when there are no changes. The warning/saved state remains visible through the status icon and semantic color.

The unsaved-changes confirmation must wrap instead of forcing a single horizontal line:

```text
+--------------------------------------+
| ! Save changes before closing?       |
| [Keep Editing] [Discard]             |
+--------------------------------------+
```

## 5. Shared Responsive Row Rules

The main implementation should modify the shared helpers before adding page-specific exceptions.

### Desktop row

```text
[label + hint                         ] [control                 ]
```

### Mobile row

```text
[label                                  ]
[hint                                   ]
[control, full available width          ]
```

Apply the following rules below 600px:

- remove fixed `min_w`/`max_w` constraints from form controls;
- use `flex_col()` for `row_field`, `row_input_with_hint`, `searchable_select_row`, `select_row`, and `slider_row`;
- use `w_full().min_w_0()` for inputs, selects, and slider containers;
- preserve the existing 16px card inset where possible, reducing only the outer page padding to 12px;
- keep labels and hints readable; do not solve overflow by globally shrinking text;
- keep select and input hit targets at least 32px high;
- retain desktop max widths at regular widths to avoid oversized controls.

### Toggle rows

Keep the switch visible on the right while allowing the descriptive text to occupy the remaining width:

```text
[label + wrapped hint                  ] [switch]
```

The text container must be `min_w_0()`. For long explanatory text, the label and hint may occupy two or more lines; the switch must not be pushed outside the card.

### Slider rows

Desktop:

```text
[label + hint                         ] [value]
                                         [slider --------]
```

Mobile:

```text
[label                              ] [58%]
[hint                               ]
[slider-------------------------------]
```

The slider must use the full available width. The value badge stays on the label line and must not be part of a fixed 260px control column.

### Long text and errors

- remove `whitespace_nowrap()` from user-facing explanatory/error text where it can overflow;
- keep `whitespace_nowrap()` only for compact labels, keycaps, and deliberately atomic controls;
- status/error surfaces should wrap and remain inside the card;
- path chips must cap long paths and ellipsize the path text while preserving the remove action.

## 6. Page Designs

## 6.1 General

Current sections: Updates, Terminal, Continuity, Security, Skills, and Network.

Mobile structure:

```text
+--------------------------------------+
| General                              |
| Terminal defaults and app behavior.  |
+--------------------------------------+
| TERMINAL                             |
| Default Shell                        |
| Command used by new panes...         |
| [                                  ] |
+--------------------------------------+
| CONTINUITY                           |
| Restore Terminal Text            [●] |
| Keep terminal text on restart...     |
+--------------------------------------+
| SECURITY                             |
| Clipboard Writes                 [●] |
| Allow terminal programs to copy...   |
+--------------------------------------+
| SKILLS                               |
| Project paths                        |
| [skills ×] [con local ×] [+ Agents]  |
| Global paths                         |
| [~/.config/con/skills ×] [+ Agents]  |
+--------------------------------------+
| NETWORK                              |
| HTTP Proxy                           |
| [                                  ] |
| HTTPS Proxy                          |
| [                                  ] |
+--------------------------------------+
```

Specific behavior:

- Updates actions use wrapping; at very narrow widths, the secondary action may occupy the next line instead of squeezing the status text.
- Default Shell uses the stacked input helper.
- HTTP Proxy and HTTPS Proxy use stacked inputs; this is required, not optional.
- Skill chips wrap. Individual path text is ellipsized inside a bounded chip so one long path cannot create horizontal scrolling.
- Existing update, skill, and network behavior remains unchanged.

## 6.2 Appearance

Current sections: App Icon, Fonts, Cursor, Transparency, Pane, Background Image, Terminal Theme, and Import Theme.

Mobile structure:

```text
+--------------------------------------+
| Appearance                           |
| Tweak Con's textures, tastes...      |
+--------------------------------------+
| APP ICON                             |
| [icon] [icon]                        |
| [icon] [icon]                        |
+--------------------------------------+
| FONTS                               |
| Terminal Font                       |
| Terminal and mono UI...             |
| [Search fonts...                  v] |
| Add Fallback                        |
| [Search installed fonts...        v] |
| UI Font                             |
| [Search fonts...                  v] |
| UI Size                             |
| [                                  ] |
+--------------------------------------+
| TRANSPARENCY                        |
| Terminal Glass                 [58%]|
| Blend terminal surface...           |
| [slider-----------------------------]|
| Terminal Blur                  [●]  |
+--------------------------------------+
| BACKGROUND IMAGE                    |
| Image Path                          |
| [                                  ] |
| [Browse...]                         |
| Fit                                 |
| [Select                           v] |
| Image Strength                 [40%]|
| [slider-----------------------------]|
+--------------------------------------+
| TERMINAL THEME                      |
| [theme preview, one column]         |
| [theme preview, one column]         |
+--------------------------------------+
```

Specific behavior:

- Searchable selects become full-width stacked controls.
- Simple selects become full-width stacked controls.
- Sliders become full-width stacked controls with the value badge on the label line.
- Background image path input and Browse action stack vertically at mobile width.
- Theme cards may naturally fall back to one column. Preserve preview legibility over forcing a two-column grid.
- App icon cards may wrap into two columns; they must not be squeezed below a usable hit target.
- Import Theme actions wrap or stack; the preview/status card must stay within the page width.

## 6.3 AI

Current sections: Routing and Behavior. The Behavior fields are already stacked and should remain so.

```text
+--------------------------------------+
| AI                                   |
| Model selection and AI configuration.|
+--------------------------------------+
| ROUTING                              |
| Active Provider                      |
| Default provider for the agent...    |
| [Select a provider...              v]|
| Active Model                         |
| [Select a model...                 v]|
| Auto-Approve Tools               [●] |
| Allow the agent to run tools...      |
| AI Command Suggestions            [●] |
| Use the suggestion provider...       |
| Suggestions Provider                 |
| [Select a provider...              v]|
| Suggestions Model                   |
| [Select a suggestion model...      v]|
+--------------------------------------+
| BEHAVIOR                             |
| Max Turns                            |
| Tool-use turns before...             |
| [                                  ] |
| Temperature                         |
| Blank for provider default.          |
| [                                  ] |
+--------------------------------------+
```

Specific behavior:

- The shared searchable-select helper provides the mobile layout; no separate AI-specific selector implementation is needed.
- Disabled suggestion controls retain the current reduced-opacity state, but remain full-width and visually understandable.
- Toggle descriptions wrap without squeezing the switch into the page edge.
- No behavior or provider/model selection semantics change.

## 6.4 Providers

Providers is the one page that needs a page-specific structural change. The current nested provider sidebar is not viable at 360px.

### Regular and narrow layout

```text
+------------------+------------------------------------+
| provider list    | Provider configuration              |
| Anthropic        | Default Model                       |
| OpenAI           | Connection                          |
| ChatGPT          | Limits                              |
| ...              |                                    |
+------------------+------------------------------------+
```

### Mobile layout

Replace the nested provider sidebar with the existing `Select` component as a full-width provider picker.

```text
+--------------------------------------+
| Providers                            |
| Configure model hosts and credentials.|
+--------------------------------------+
| Provider                             |
| [OpenAI                            v] |
+--------------------------------------+
| DEFAULT MODEL                        |
| Model                                |
| [                                   ]|
| [Fetch Models]                       |
| status text wraps here               |
+--------------------------------------+
| CONNECTION                           |
| Protocol                             |
| [OpenAI                            v] |
| API Key                              |
| [                                   ]|
| Base URL                             |
| [                                   ]|
| Endpoint Preset                      |
| [                                   ]|
+--------------------------------------+
| LIMITS                               |
| Max Tokens                           |
| [                                   ]|
+--------------------------------------+
```

Specific behavior:

- `< 600px`: hide the nested provider list and render one full-width searchable provider Select.
- `>= 600px`: preserve the existing provider list unless later visual testing shows a problem.
- The selected provider, OAuth state, protocol toggle, model fetching, endpoint preset, and model list semantics remain unchanged.
- Fetch Models status and button stack/wrap. Status text must not reserve a fixed 220px minimum.
- OAuth actions and verification URI controls wrap or stack; long URLs are clipped/ellipsized inside a bounded text area with a copy action.
- Provider form controls use the shared mobile stacked helpers.
- Do not replace the provider picker with custom chips; use the existing `Select` component for search, keyboard navigation, and consistency.

## 6.5 Keys

Keys contains many repeated shortcut rows and optional global/quick-terminal sections.

```text
+--------------------------------------+
| Keys                                 |
| Editable keyboard shortcuts.         |
+--------------------------------------+
| GLOBAL HOTKEY                        |
| Show Con from anywhere...            |
| [Enable switch]                      |
| Shortcut                            |
| [⌥ Space]                           |
+--------------------------------------+
| GENERAL                              |
| New Window                    [⌘ N]  |
| New Tab                       [⌘ T]  |
| Toggle Input / Terminal       [⌘ I]  |
| Ask AI About Selection        [⌘ ?]  |
| ...                                  |
+--------------------------------------+
| PANES                                |
| Split Right                   [⌘ D]  |
| ...                                  |
+--------------------------------------+
```

Specific behavior:

- Ordinary key rows retain the label/keycap horizontal relationship.
- The label container becomes `min_w_0()` and may wrap to two lines; keycaps remain non-shrinking.
- Increase the mobile row's minimum height when a label wraps; do not clip the action name.
- Global Hotkey and Quick Terminal cards stack their description, switch, and shortcut badge below 600px.
- Recording state (`Press shortcut…`) remains visually prominent and must not be truncated.
- Reset buttons remain adjacent to the shortcut badge and retain their tooltip.
- Fixed shortcuts such as tab-number selection may wrap if needed; keycap meaning must remain intact.

## 7. Interaction and State Requirements

The responsive layout must preserve all existing behavior:

- active section changes immediately after a tab click;
- keyboard navigation continues to work in the Settings window;
- Save keeps the current dirty-state behavior;
- close confirmation remains available when changes are unsaved;
- errors remain visible and readable at mobile width;
- provider OAuth and model-fetching states do not change;
- Appearance preview events continue to update the terminal immediately;
- shortcut recording continues to capture the next keystroke and supports cancellation through the existing path.

Mobile-specific interaction requirements:

- top tabs are horizontally scrollable but the page body remains vertically scrollable;
- controls must not require horizontal scrolling;
- Select popups may extend beyond the form card as normal overlays, but their trigger must remain full-width;
- all icon-only header actions have tooltips/accessibility labels;
- focus remains in the edited input after Save and after a field-level interaction where the current behavior already preserves focus.

## 8. Implementation Shape

Prefer one shared responsive mode value calculated from the Settings viewport:

```text
ResponsiveMode::Mobile
ResponsiveMode::Narrow
ResponsiveMode::Compact
ResponsiveMode::Regular
```

Recommended implementation order:

1. Add the Settings-only responsive mode and mobile navigation shell.
2. Update shared row helpers to accept the mode and remove fixed widths in mobile mode.
3. Update header/confirmation/error wrapping behavior.
4. Collapse the Providers nested sidebar into a provider Select on mobile.
5. Adjust Keys long-label and global/quick-terminal layouts.
6. Add or update unit tests for pure breakpoint/width decisions.
7. Build and visually inspect at 360, 390, 600, 840, 980, and 1200px widths.

Avoid duplicating five independent mobile implementations. The page render functions should mostly continue composing the same shared helpers.

## 9. Validation Matrix

| Viewport | Expected result |
|---:|---|
| 360×520 | no page-level horizontal overflow; mobile tabs; stacked controls; Providers picker instead of nested sidebar |
| 390×700 | same mobile structure with more visible tab/content width |
| 600×700 | transition to icon rail; flexible controls; provider list may return |
| 840×700 | labeled 144px rail; compact rows |
| 980×720 | labeled 160px rail; regular row widths |
| 1200×800 | current desktop hierarchy retained |

Validation should specifically inspect:

- Appearance selectors and sliders;
- Providers model fetch/OAuth rows;
- Keys long labels and recording state;
- unsaved confirmation and save errors;
- theme and app-icon grids;
- vertical scroll reachability for the bottom of every page.

## 10. Acceptance Criteria

- All five Settings pages render without horizontal page overflow at 360px.
- Every input/select/slider remains usable with a full-width or otherwise sufficient hit area.
- Providers does not show two sidebars at mobile width.
- No main terminal/workspace page changes are required or introduced.
- Desktop layout at 980px and above remains visually equivalent except for controls becoming more flexible where necessary.
- Existing save, preview, OAuth, model-fetch, shortcut-recording, error, and close-confirmation behavior remains intact.
