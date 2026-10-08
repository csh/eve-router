# UI and UX review of `router_egui`

Date: 2026-10-08. Branch: `live-refresh`.

## Method and limits

- I read every file in `crates/router_egui/src` and the label code in `router_core`.
- I read the code first. Then I ran the release build and took screenshots at 1260 x 560, 1260 x 820 and 900 x 700, with 6 waypoints (Jita, Perimeter, Amarr, Dodixie, Rens, Hek). The shots are not in the repo, because they show a real pilot name.
- I forced the 900 px size with a script. A user cannot drag below 1260 px today, so the 900 px shot shows what a smaller window would do.
- A second reviewer tried to refute 14 findings. No finding was refuted. The result is in the last section.
- The reader is an EVE pilot who wants to get from A to B fast, often with the game on the other half of the screen.

## The seven problems that matter most

1. **The window cannot shrink.** The minimum size is 1260 x 560 (`main.rs:26`). The sidebar is fixed at 280 px (`view.rs:142`). The top bar is one row that does not wrap (`view.rs:188`). A half-desktop window or a sidebar window is not possible today.
2. **The answer is not the hero.** The planner takes the top of the window. At 560 px high the route table does not show at all, and the route list shows two rows and a half. At 820 px the table shows 7 rows (F13, F18).
3. **No route summary shows the risk.** A pilot asks "does this go through lowsec?" The route list shows only "30 jumps (2 wormholes)". The table shows security one row at a time.
4. **Every message is amber.** `status_bar` paints all text with the warning color (`view.rs:667`). "Config saved!", "Added 2 systems" and a real warning look the same.
5. **The settings window is a modal with mixed save rules.** Some controls apply at once. Others need Apply or Save. Closing the window drops typed text that was not applied. The pilot cannot see the route change while they tune it.
6. **Router internals reach the player.** Examples: "in the format of --print", "jumps per 1% of a gate", "Shortcuts". The search time ("6.2 ms") stays, by the owner's decision (M6).
7. **The look is close to the game but not inside it.** The security colors match the game. The font, the window chrome, the controls and the route display do not. See "EVE look".

## Findings

Severity: **H** blocks a use case or misleads. **M** costs time or trust. **L** polish.

### Layout and responsive

| ID | Sev | Finding | Fix |
|---|---|---|---|
| F1 | H | Minimum size 1260 x 560. Fixed sidebar. Non-wrapping top bar. Fixed table columns add up to about 850 px. **Measured at 900 px:** the title "EVE ROUTER" draws over "Optimize order", and the mode box is cut at the left. | Three width classes (see "Use cases"). Remove the minimum width. |
| F2 | H | The planner (search row, pilot row, up to 200 px of waypoints, buttons) comes before the route. | Collapse the planner to one line once a route exists. Waypoint list opens on demand. |
| F3 | M | The top bar mixes route options (mode, bridges, wormholes, optimize, count) with app buttons (Settings, Avoid, Characters). | Route options go to one "Route options" popover plus a mode control. App buttons go to the right. |
| F4 | M | The top bar says "Routes - 5 +". The panel below says "Routes (3)". One word, two meanings. | Name the first "Show up to 5 routes". Move it into the popover. |
| F5 | M | During an active route the planner and route list vanish. The disabled route controls stay visible. | Hide the disabled controls. Show a compact travel view (see "Warp overlay"). |
| F6 | L | Pilot picker and search result rows use fixed text offsets (+170, +200, +220 px). Long names overlap. | Use table columns with clipping. |
| F7 | L | The Avoid window is fixed at 760 px, Settings at 560 px, Characters at 520 px. | Use `min(width, window - 32)`. |

### Flow

| ID | Sev | Finding | Fix |
|---|---|---|---|
| F8 | H | A to B needs one search box, Enter, search again, Enter. Nothing says "From" or "To". The first and last items become start and destination by position only. | Two labeled fields, From and To, plus "Add stop". Paste list stays. |
| F9 | M | Search box keeps focus after a pick (`search.rs:199`). Route selection with Up and Down needs no focused widget (`view.rs:125`). After a keyboard add, the arrows do nothing. | Handle arrows when the search result list is closed. Or use Alt+Up and Alt+Down. |
| F10 | M | "+ Add waypoint" only focuses the search box when the box is empty or has no result. It does nothing the pilot asked for. | Disable it while there is nothing to add. |
| F11 | M | The pilot's current system is not the default start. Click on a pilot in the sidebar sets the start, but only the tooltip says so. | When a pilot is online, pre-fill From with "Current location (Jita)". |
| F12 | M | Avoiding a system is a right click on a route row. Nothing shows this. The only hint sits in the empty Avoid window. | Add a small row action on hover. Keep the right click as a second way. |
| F13 | H | **Measured.** At 1260 x 560 with 6 waypoints, the route table is not on screen. The pilot sees the planner and a cut route list. | See F2. Also give the table a minimum height. |
| F17 | H | **Measured.** Route #1 and #2 both read "42 jumps (1 wormhole)". The list gives no way to tell them apart. | Show the difference: which wormhole, the added jumps, the lowsec count. See "The route strip". |
| F18 | M | **Measured.** At 820 px high the route list shows two rows and a half of 5. The third row is cut in the middle. | Size the list to its rows, up to 5, or scroll with a visible bar. |
| F19 | L | **Measured.** The Pilots column of the route table takes 190 px and is empty when no pilot is on the route. | Hide the column unless a pilot is on a step. |
| F14 | L | The "Pilot" row asks the player to pick a pilot or a hull. The player thinks "ship". The hull matters only for jump bridges. | Rename to "Ship". Show it as a chip in the top bar. |
| F15 | L | The sidebar panel "Shortest route" shows jumps to favourites. It is not a route. | Rename to "Favourites" with the info "from Jita". |
| F16 | L | Favourites are edited in Settings but shown in the sidebar. | Edit them in the sidebar: add, remove, drag to reorder. |

### Messages and wording

| ID | Sev | Finding | Fix |
|---|---|---|---|
| M1 | H | All status text is amber (`view.rs:667`). | Three levels: info (dim), success (green, 2 s), warning (amber, stays until fixed). |
| M2 | M | "Config saved!" shows after many changes. Avoid and mode changes are quiet. Closing Settings saves again and shows it. | Never show a save message. Show an error only. |
| M3 | M | `recompute` sets "Jump bridges off: ..." at every search (`app.rs:291`). A pilot who never uses bridges sees it all the time. | Show it as a hint on the bridges control only. |
| M4 | M | The idle status text is "Ready". It says nothing. | Show the route summary, or nothing. |
| M5 | M | The refresh note ("Wormholes updated: a new route is first") stays until the next search, in the status bar. It matters, but it is easy to miss. | Show it as a dismissable line above the route list, with a "changed" mark on the route. |
| M6 | L | The search time ("6.2 ms") shows in the route list header. **Keep it.** It tells users whether the router performs well. It is unlabeled, so a new user reads it as noise. | Keep the value. Add a tooltip: "Time to find these routes". Keep it visible when the route list shrinks or moves (F2, F17). Optional: turn it amber above a set limit, for example 250 ms. |
| M7 | M | The "Copy route" tooltip says "in the format of --print". | "Copy the route as a list of systems". |
| M8 | M | Jump bridges button says "off (no capital)" for every blocked reason (`view.rs:232`). A hull ban gives the wrong text. | Use the reason text, shortened. |
| M9 | M | The Never and Prefer button shows the current state. A pilot cannot tell if it is the state or the action. | Use a two-part switch: Prefer / Never. Add one line of help. |
| M10 | L | The word "Stop" has three meanings: the "Stop" column (Start, Midpoint 2, Destination), the action "Stop route", and "Stop avoiding X" in the context menu. The AVOID tag sits in the Stop column. | Rename the column "Role", or draw icons. Use "Remove from avoid list" in the menu. Put AVOID in the Via column. |
| M11 | L | "Shortcuts", "Skipped", "TJ", "Zone 1 > 2" have no explanation. | Say "Connections loaded". Add a tooltip with the full term. |
| M12 | L | Hover texts use contractions ("You'd rather", "there's"). The newer texts do not. | One style. |
| M13 | L | The splash failure has no Retry, no Quit and no config path (`view.rs:716`). The load can take 35 s with no hint. | Add Retry, Open config folder, Quit. After 10 s say "Still loading the map". |

### Settings

| ID | Sev | Finding | Fix |
|---|---|---|---|
| S1 | H | Three save models in one window: instant (checkbox), after 500 ms (drag), on button (Max TJ, Nexum URL, key). Closing drops unsaved text. | Every control applies. Text fields commit on Enter or on blur. No Apply buttons. |
| S2 | M | One error slot at the bottom serves every field. | Show the error under the field. |
| S3 | M | The window is modal. The pilot cannot see the route react to a change. | Use a popover that does not block the window (see "Settings popover"). |
| S4 | M | `min_life` ("wormholes must have N minutes left") exists in `Settings` but has no control. This is the setting a wormhole user wants most. | Add it as "Skip wormholes closing within [30] min". |
| S5 | M | The cost sliders say "jumps per 1% of a gate". Only the router author reads this. | Use three presets: Avoid bridges, Balanced, Prefer bridges. Keep the numbers under "Advanced". |
| S6 | L | The comment above the Nexum rows says changes apply at the next start. The code applies them at once (`app.rs:505`). | Fix the comment. |
| S7 | L | Wormholes and Jump bridges toggles reset at every start. Thera and Turnur do not. | Save both, or say "this session" in the tooltip. |
| S8 | L | The text color is `Color32::WHITE` in some rows and `theme::TEXT` in others. | Use the theme names only. |

### Accessibility

| ID | Sev | Finding | Fix |
|---|---|---|---|
| A1 | M | Route list rows, favourites and pilot rows are painted by hand with `Sense::click`. A keyboard cannot reach them. | Use selectable widgets, or add focus handling. |
| A2 | L | `TEXT_DIM` (#6E7E88) on the panel color is about 4.5 : 1, computed by hand. It is used at 11 px for the only help text. | Raise it to #8394A0 for text below 13 px. |
| A3 | L | Much information is hover only: favourite click, pilot click, avoid reason, truncated Via text. | Show the key actions without hover. |

### Code patterns

- `view.rs:216` uses `add_enabled(true, ...)`. Use `ui.add`.
- `top_bar` computes `locked` two times (`view.rs:201`, `view.rs:210`).
- `app.rs:529` sleeps in `ui()` to cap the frame rate. This blocks input for up to 16 ms. Prefer `request_repaint_after`.
- `Session.status` is a global message bus. `recompute` clears it, so a message set before a search can vanish. Use a message list with a level and an age.
- `view.rs` holds 750 lines. The top bar, planner, route table and sidebar fit in four modules.

## Use cases

Three width classes. One code path with a layout switch, not three apps.

| Class | Width | Layout |
|---|---|---|
| **Sidebar** | 320 to 520 px | One column. A tab strip: Plan, Route, Pilots. The route summary strip stays above the tabs. Route table shows System, Security, Via only. |
| **Half desktop** | 640 to 1100 px | Two columns: planner and routes at the left, route table at the right. Favourites and pilots move into a drawer that opens from a left rail. |
| **Full / floating** | 1100 px and up | The current three zones, with a resizable sidebar. |

More use cases worth a design:

- **Second monitor, left open all session.** Priority: the active route and the next action. Needs the compact travel view.
- **In-game overlay over a borderless EVE window.** The Warp overlay below.
- **Fleet or alt use.** Two characters in one route. The avatar column already helps. A "who is where" strip fits the Sidebar class.
- **Scanning chain.** The pilot wants wormholes only, sorted by time left. Not a router view, so out of scope here.

## The route strip (the one bold element)

Spend the boldness here. The game shows a route as a row of dots, one for each system, colored by security. Pilots read it in one glance. Draw it above the route table and inside each route list row:

```
Jita ●●●●●●●●○○●●●●●●●●●●●●●●●●●●●●● Amarr      30 jumps · 4 lowsec · 2 wormholes
```

- One dot for each system. Color from `sec_rgb`. Wormhole and bridge hops draw as a short bar in the `WORMHOLE` or `BRIDGE` color.
- A click on a dot selects the step. The current system gets a ring.
- Each route list row becomes two lines: the strip, then the counts. The best route gets a label ("Fewest jumps"). Others show the difference ("+2 jumps, all highsec").
- The summary line adds "lowsec: 4" and "nullsec: 0". This answers the safety question without a table read.

## Settings popover

Goal: tune routing and see the route change at once.

**Shape.** A popover that anchors to the "Route options" button. It is 360 px wide and does not dim or block the window. In the Sidebar class it becomes a full-width sheet from the top. Escape and a click outside close it. Every control applies at once.

**Always visible in the top bar** (they change on every trip):

```
[ Shortest | Safer | Riskier ]   [ Ship: Sin (Black Ops) ]   [ Route options v ]   [ Avoid 2 ]   [ Characters 2 ]
```

Rename the modes in the player's words. "Prefer highsec" becomes "Safer". "Less secure" becomes "Riskier". The tooltip keeps the full description.

**Inside the popover**, in this order (most used first):

1. **Ship.** Pilot or hull picker. Result line: "Can use Ansiblex: yes, 36 TJ per jump". Covers F14 and M8.
2. **Wormholes.** Use wormholes (on/off). Thera. Turnur. "Skip wormholes closing within [30] min" (S4). Unknown signature: a choice "Allow, costs [3] extra jumps" or "Skip".
3. **Jump bridges.** Use bridges (on/off). Alliance capital (search). Max TJ per jump. A preset: Avoid, Balanced, Prefer.
4. **Show up to [5] routes.** A stepper.
5. **Data sources.** One row for each source: EVE-Scout and Nexum. Each row shows a status dot, the age, and a "Refresh" button. Nexum expands to URL, key and map. This replaces the sync dots in the status bar and the Shortcuts panel.
6. **Advanced** (closed by default). The cost numbers. "Reset to defaults".

Rules:

- Text fields commit on Enter or blur. A bad value shows red under the field and does not apply.
- A control that the state disables shows the reason in line, not only on hover. Example: "Locked while a route is active".
- A changed value shows a small dot beside its section, so the pilot sees what differs from the defaults.
- The popover footer says nothing about saving. Saving is silent.

**Avoid** stays its own window in the Full class. In the Sidebar class it becomes a tab in the popover. Fix M9 and the region field (Enter does not pick the first match).

## Warp overlay

A small always-on-top window for the moment of a wormhole jump.

```
+--[ icon ]------------------------------ [_] [x] --+
|  WARP TO  ABC-123                       [ Copy ]  |
|  Perimeter  >  Amarr     Large  3h 10m left       |
|  Jump 3 of 17   o o o o O o o o o                 |
+---------------------------------------------------+
```

- **Size and behavior.** About 340 x 110 px. Always on top. No OS title bar. A custom drag strip. Opacity 85%, set by a slider. A transparent viewport (`ViewportBuilder::with_transparent`, `with_always_on_top`, `with_decorations(false)`).
- **When it shows.** When the tracked pilot is in the system before a wormhole step (`progress + 1 == step`). It also shows the next wormhole in advance in a dim style.
- **States.** The data already exists in `wormhole_hint`.
  - Signature known: "WARP TO ABC-123", then the target system.
  - Signature unknown: "SCAN Perimeter" and "wormhole to Amarr, signature not known".
  - Gate hops between: "NEXT: Dodixie, 2 gates to the wormhole".
  - Arrived: green, "Arrived at Amarr". Off route: amber, with "Re-route".
- **Text source.** Today the hint is one prose string. Return a struct (`action`, `target`, `detail`) from `router_core`, so the overlay and the banner share it.
- **Click to copy** the signature. A pilot can then paste it into the probe scanner filter.
- **Limit to state.** An exclusive fullscreen EVE client hides any overlay. Say so in the first-use text: "Use windowed or borderless mode."
- **Neocom link.** The overlay is a narrow Photon panel. Its left edge is a 28 px icon column, like the neocom. Icons: route, wormhole, copy, settings. The same column can become the left rail of the Half desktop class.

The overlay also solves the Sidebar use case for travel. The main window can stay small, or closed to the tray.

## EVE look: where the app is, and where it is not

**What matches.**

- Dark panels with 1 px lines and square corners.
- One cyan accent. Amber for warning, red for error, green for ok.
- Security colors. `sec_rgb` is the game's scale, the same hex values.
- Ship and character portraits from the image server.
- Header strips with letter-spaced capitals.

**What does not match.**

| Area | Now | Target |
|---|---|---|
| Font | Oxanium (`theme.rs:45`). A side-by-side with the client shows a similar angular face. | Keep it. Check body text at 12 to 13 px, which is the client's size. |
| Window chrome | A normal OS window with panels. | Panels with a draggable header, collapse arrow and small icon buttons, as in the game. At least for the overlay and popover. |
| Navigation | A top bar of text buttons. | A left rail (neocom) of icons with tooltips. |
| Route display | A table only. | The security dot strip (see above). The game shows this strip in its route panel. |
| Controls | Stock egui checkbox, `DragValue`, `ComboBox`, `Spinner`. | Restyle them: flat toggles, a segmented control for modes, and a number field with a stepper. |
| Icons | Unicode glyphs (⚙ ⏶ ⏷ 🗙) from fallback fonts. They differ in weight and size. | One small icon set drawn as vector paths. |
| Hover | An accent border on every widget. | A brighter fill, as in the game. Keep the accent border for focus only. |
| Depth | The panel color has alpha, but it sits on a solid background. No effect shows. | Real translucency in the overlay (transparent viewport). |
| Corner ticks | Drawn on every panel (`theme.rs:168`). | Keep them on the main panels. Drop them on small nested panels. |

Do not copy CCP art or font files. Draw the icons and choose an open font.

## Order of work

| Step | What | Estimate |
|---|---|---|
| 0 | Done. Screenshot pass at 1260 x 560, 1260 x 820 and 900 x 700. Confirms F13 and F1, and adds F17 to F19. | 30 minutes |
| 1 | Message levels and message cleanup (M1 to M8, M13). Pure text and color. | 3 hours |
| 2 | Done. Route strip and route list rows (the bold element, F2, F13, F17, F18). | 1 day |
| 3 | Responsive shell: remove the minimum size, width classes, tab strip (F1, F3, F5). | 2 days |
| 4 | Settings popover (S1 to S5, M9). Needs the layout from step 3. | 1.5 days |
| 5 | From / To fields and the current-location default (F8, F11). | 0.5 day |
| 6 | Warp overlay and the hint struct. | 1.5 days |
| 7 | Restyle of controls and icons ("EVE look"). | 2 days |

Steps 1, 2 and 5 do not depend on the settings revamp and can start first.

## Adversarial check

A second reviewer tried to refute 14 findings by reading the code.

- **Confirmed (11):** M8, M1, F9, S1 (both the dropped text and the extra "Config saved!"), M9 (both parts), M6, M7, S7, S6, M13 (the screen has no button and no path), M10. M13 shows the error string only, so the config path appears only if the error text holds it. I did not check that string.
- **Partly right (2):**
  - F1. The hard floor is 1260 px from `main.rs:26`. The top bar would only clip below that, and the OS stops the resize first. The finding stands, and the text now says 1260.
  - F10. The button adds the result only when the box has text and at least one result. An empty box or a query with no result also falls to "focus the box". The fix text says "disable while there is nothing to add".
- **Confirmed by screenshot (1):** F13. The reviewer estimated 0 to 20 px for the route table at 560 px high with 6 waypoints. The screenshot shows 0 px: the table is not on screen.
- **Refuted:** none.

Added after the check:

- M10 has a third meaning of "Stop": the context menu says "Stop avoiding X".
- The Log button turns amber on a failure, and the status text is always amber. The two signals clash (part of M1).
- `ui.disable()` in `top_bar` also disables the route count buttons and the mode combo while a route is active. This is the intended lock, but it shows as greyed controls (F5).
- The screenshot pass (step 0) found F17, F18 and F19, which the code reading did not.

## Step 2 result

Measured in the release build with 6 waypoints. The 1260 x 560 window used to show no route table rows.

| Window | Before | After |
|---|---|---|
| 1260 x 560 | 0 table rows. List cut at 2.5 rows. | 3 table rows, and the first route in full. |
| 1260 x 820 | 7 table rows. List cut at 2.5 rows. | 10 table rows. The list shows 3 routes in full. |

What changed (after the first review of the result):

- The route strip draws in each route list row. It follows the game: one square for each system (10 px, 2 px gap), and a plus in place of the square for each system the pilot gave (start, midpoints, destination). The plus has the color of the security of its system. The strip has no border. A wormhole or bridge jump shows as a purple or blue bar below the square. The route table has no strip.
- A route list row has two lines. Line 1 is the number, then one summary in one font and one color, for example "14 jumps · 1 wormhole · 2 jump bridges · 0 TJ · 0.0% of a gate". At the right it gives the jumps of each leg, for example "2 + 12". Line 2 is the strip. The route table header shows the same summary.
- The lowsec and nullsec count is gone. The security colors of the squares show it.
- The strip draws in whole device pixels, so every edge is sharp at any display scale. The bar under a square for a wormhole or a bridge jump is as wide as the square.
- The summary still shows "0 TJ" and "0.0% of a gate", for the layout work. Show each only when it is above 0 when the design is locked. "% of a gate" is the share of the Ansiblex capacitor that the bridge jumps of the selected ship use. A stargate needs none.
- The "Leaves #1 at ..." text is gone. A leg is the part of a route between two given systems.
- The route table groups the steps by leg when the route has two or more legs. A heading before each leg reads "PERIMETER » AMARR · 12 JUMPS". The stop that ends a leg stays in that leg.
- The planner closes its waypoint list when a route exists. A button opens it again. The buttons of the planner ("+ Add waypoint", "Paste list…", Pilot, Reverse, "Clear route") stay at the right edge.
- The route list gives up height to the route table when the window is short. The search time label stays.
- Core helpers `Route::legs`, `route_notes` and `route_summary` have tests.

Open points:

- At 560 px high the table shows 3 rows, not the 200 px target. The rest needs the layout change of step 4.
- F19 (the empty Pilots column) is not done.
- Two routes can have the same jumps and the same legs. They then differ inside a leg, and the list does not show where. A later change can dim the squares that two routes share.
- The strip uses a space of 4 to 12 px for each system. A route of more than 210 systems draws past its box at 840 px wide. Routes of that length are not in the test data.
