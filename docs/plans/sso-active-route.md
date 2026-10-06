# Plan: EVE SSO login and the active route

Status: plan only. No code yet.

This plan adds three things to the TUI (`router_tui`) and the GUI (`router_egui`):

- EVE SSO login for one or more characters, with tokens in the OS keyring.
- "Start route": the router sends the waypoints of the selected route to a chosen character.
- An active-route view that hides the planner, shows the route and tracks progress.

An adversarial UX review attacked a first draft. Its rulings and ranking are in [UX rulings](#ux-rulings-ranked). The rest of this plan obeys them.

## ESI endpoints

| Endpoint | Scope | Cache | Use |
|---|---|---|---|
| `POST /ui/autopilot/waypoint` | `esi-ui.write_waypoint.v1` | none | Send one waypoint. Params: `destination_id`, `add_to_beginning`, `clear_other_waypoints`. Acts on the token's character. |
| `GET /characters/{character_id}/location` | `esi-location.read_location.v1` | 5 s | `solar_system_id` gives the progress. |
| `GET /characters/{character_id}/ship` | `esi-location.read_ship_type.v1` | 5 s | `ship_type_id` for the hull warning. |
| `GET /characters/{character_id}/online` | `esi-location.read_online.v1` | 60 s | The online state in the character list and the start dialog. |

Portraits come from `https://images.evetech.net/characters/{id}/portrait?size=64`. That host needs no token.

Every ESI request sends `X-Compatibility-Date`, and the router obeys the `Expires` header. The router reads `X-ESI-Error-Limit-Remain`. Below 20, it slows all polls and shows `ESI limited — slowing updates`.

### The waypoint limits

The in-game autopilot follows gates only. It cannot use an Ansiblex or a wormhole. A waypoint in a system past such a hop gives a long gate detour, or no route at all. Thus the router sends the route in **segments**:

1. A segment ends at the next manual hop (Ansiblex or wormhole), or at the destination.
2. The router sends each system of the segment, so the autopilot follows the exact path. The first call has `clear_other_waypoints=true`. The others append in order.
3. When the location poll shows that the pilot made the manual hop, the router sends the next segment.
4. If a segment has more systems than the in-game waypoint cap, the router sends the stops of that segment only and shows a warning.

NOTE: The in-game waypoint cap is not verified. Measure it in game before the code sets the constant.

ESI has no endpoint that only clears waypoints. "Stop route" thus leaves the in-game waypoints set, and the dialog says so.

## Architecture

### New module: `router_core::esi`

No UI code. Both front ends use it.

| File | Contents |
|---|---|
| `esi/sso.rs` | The `oauth2` client: PKCE (S256), the state, the authorize URL, the code exchange, the refresh, the revoke. The router adds the loopback listener and the pasted-URL parse. JWT claims: `sub` = `CHARACTER:EVE:<id>`, `name`, `scp`. The router checks the issuer, the audience, the expiry and the subject. It does not check the signature: it gets each token directly from the SSO token endpoint over TLS (OpenID Connect Core 3.1.3.7). |
| `esi/store.rs` | The token store. See [Token storage](#token-storage). |
| `esi/client.rs` | The ESI calls above. An access token refreshes 60 s before expiry. Uses `sources::agent`. |
| `esi/tracker.rs` | A thread that polls location, ship and online for each character. It sends `TrackerEvent`s on an `mpsc` channel, like the Nexum map fetch. |
| `esi/active.rs` | `ActiveRoute`: a frozen copy of the route, the settings that made it, the character ID, the segment index and the progress. Pure logic, no I/O, so unit tests cover it. |

SSO details:

- The app is a native app. It uses PKCE and has no client secret.
- The client ID is not in the repository. The build reads `EVE_ROUTER_CLIENT_ID` with `option_env!`, and the same env var at run time overrides it. Without a client ID, `Add character` is disabled and shows `No EVE client ID in this build. Set EVE_ROUTER_CLIENT_ID.`
- The callback is `http://localhost:21404/callback`. The port is fixed, because EVE SSO matches the registered callback exactly.
- The listener binds `127.0.0.1` only, never `0.0.0.0`. It accepts one request with a matching `state`, then closes.
- The login always shows the URL and a "Paste the redirected URL" field. This works over SSH and when the port is busy.
- The login requests all four scopes. A character with a missing scope shows `Re-authorize`.

New dependencies: `oauth2` (PKCE, state, token requests), `keyring` (OS keyring), `base64` (JWT claims), `open` (browser), `image` (PNG decode for egui portraits).

Each dependency must pass `cargo deny check` (see `deny.toml`): no open RustSec advisory, and a license that is compatible with AGPL-3.0 and with MIT OR Apache-2.0. For this reason the plan has no `jsonwebtoken` and no `openidconnect`. Both pull in `rsa` 0.9, which has RUSTSEC-2023-0071 with no fixed version. `oauth2` has no default features: it uses the ureq 3 agent of the router through a small `SyncHttpClient` adapter.

### Token storage

| Item | Where | Why |
|---|---|---|
| Refresh token | OS keyring: service `com.smrkn.eve-router`, user = character ID | Windows Credential Manager, macOS Keychain, Linux Secret Service |
| Access token | Memory only | It expires after 20 minutes |
| Character list (ID, name, scopes, last used) | `characters.json` next to `eve-router.json` | Not secret |
| Active route | `active-route.json` next to `eve-router.json` | Resume after a restart |

- The router never writes a token to a file or a log. A `Token` type masks `Debug`, like `ApiKey`.
- If the keyring is missing (SSH, a container, a minimal window manager), the router offers `Session only`. The tokens then stay in memory and go at exit.
- "Remove" revokes the refresh token at SSO, then deletes the keyring entry and the list entry.
- EVE SSO rotates the refresh token. The store writes the new token before it uses the new access token.

### Active route rules (both front ends)

- `ActiveRoute` holds a copy of the route. Settings changes do not touch it, so the in-game list and the app stay the same.
- Progress = the highest path index seen so far. Skipped rows count as passed, because a pilot can jump twice between polls.
- Off route = the system is not on the path for **two polls in a row**. One poll is not enough: a pod kill, a cyno or a wormhole can cause a single odd reading.
- "Re-route from here" computes a new route from the current system with the frozen settings. It shows the new jump count, then sends on confirm. It never re-routes on its own.
- Polls: the active pilot every 5 s, the other pilots every 30 s, online every 60 s.
- If a refresh fails, the route stays active and a WARN banner shows `Tracking paused — <Name>'s login expired.`

### Start flow (both front ends)

1. The user selects a route and pushes `Start route #n` (GUI) or presses `g` (TUI).
2. With no characters, the button reads `Log in to start`. After login, the flow goes to step 3. It never sends at once.
3. With two or more characters, a picker shows. It preselects the last-used character. Each row shows the name, the system and the online state.
4. The confirm step shows `Send route #1 to <Name>?` and `30 jumps · 28 waypoints · 2 manual jumps`. It can add these lines:
   - Pilot not on the path: `<Name> is in Amarr, not on this route.` Buttons: `Route from Amarr` (default) and `Send as planned`.
   - Pilot offline: `<Name> appears offline. Waypoints need the game client running.` Button: `Send anyway`. The 60 s cache gives false offline states, so this is a warning, not a block.
   - Ship cannot do the route (COULD): a hull warning from `ship_type_id` and the route's hull.
5. The send shows `Sending waypoints 12/28…`. If a call fails, the send stops at `Sent 16 of 28. The game now has a partial route.` Buttons: `Retry` (resend all, `clear_other_waypoints=true`) and `Cancel`.
6. The route is active only after a full send.

## GUI changes (`router_egui`)

### Login

- The top bar gets `Characters (n)`. It opens a popover with one row for each character: portrait, name, `● Online · Jita · Venture`, and `Remove`. The word "Online" or "Offline" is always present, not only the dot color.
- `Add character` opens the browser and shows the copy URL and the paste field.

### Active route view

- `View::show` hides `planner` and `route_list`. The central panel holds a header, a progress bar and `route_table`.
- Header: `Route #1 → UALX-3 · Alice Ander · 12/30 jumps`. The name is in `theme::ACCENT`. The progress bar is in `ACCENT`.
- Passed rows: `theme::TEXT_DIM` with a `✓` prefix. The current row: `ROW_FILL`, `▶` prefix, scrolled into view.
- A manual hop: the Via cell reads `Jump bridge — manual` or `Wormhole — manual`, in `BRIDGE` or `WORMHOLE`.
- The header holds `Stop route`. On arrival, an OK banner shows `Arrived at UALX-3.` and `Done`. `Done` brings back the planner.

### Pilots column

A new fixed-width "Pilots" column goes after "System". It shows the characters whose current system is on that row. The column shows only when one or more characters are logged in, so a user without SSO sees the table as it is now.

- Up to 3 avatars, 20 px, round corners, 4 px gaps.
- The active pilot comes first, with a 1 px `ACCENT` ring. The others follow in alphabetical order.
- An offline pilot draws at 40 % alpha.
- Each avatar shows its name in a tooltip on hover.
- Before the portrait loads, a placeholder shows the initials on `theme::HEADER`.
- Overflow: `+1 other character` or `+4 other characters`. Small font (0.85 × body), `TEXT_DIM`, italic, on the avatar baseline. The tooltip lists the hidden names, one for each line, in `TEXT`.
The column shows in the planner view and in the active view.

Cost: each portrait downloads one time, at 64 px, and goes into an egui texture cache keyed by character ID. `body.rows` draws only the visible rows. A frame thus draws at most about 30 rows × 3 textured quads. The planner needs no extra ESI calls, because the tracker already polls every character.

### Pilots panel

A "Pilots" panel goes in the sidebar, below "Shortest route". It shows the characters that are online.

- One row for each pilot: 20 px avatar, name, and the current system in its security color. The ship and the region go in the hover text, not in the row.
- Order: the active pilot first, then by name.
- The panel title shows the count, for example `Pilots (2)`.
- Offline pilots fold into one `TEXT_DIM` line, for example `3 offline`. A click shows them.
- With no characters, the panel does not show. The top-bar `Characters (n)` button is the one entry point.
- A click on a pilot row sets that pilot's system as the route start (COULD). The hover text reads `Start the route from Jita`.

The panel replaces the "Elsewhere" list from the first draft.

## TUI changes (`router_tui`)

### Login

- `c` opens a Characters page, like the settings page. `a` adds a character, `d` removes one.
- The login popup shows the URL and a text prompt for the redirected URL. The listener and the prompt run at the same time. The first to give a code wins.

### Active route view

- `ui::draw` hides the input box and the route list. The step table takes the full left area under a one-line header and a `Gauge`.
- The table rows use `✓` and `▶` like the GUI. A new narrow column shows the pilot initials, or `+n`.

### Key lock

| Key | Active mode |
|---|---|
| `i`, `/`, `Enter`, `m`, `w`, `j`, `h`, `+`, `=`, `-` | Locked |
| `s` | Opens. Favourites stay editable. Capital, max cap, Nexum, Thera and Turnur rows show `(locked)` |
| `Esc` | Does not quit |
| `q` | Asks `Quit? Route stays in game; resume on next launch. y/n` |
| `x` | Stop route, with a confirm |
| `↑↓`, `PgUp`, `PgDn`, `Home`, `End`, `n`, `p` | Scroll the step table |
| `c` | Characters page |

A locked key shows `Locked while route is active — press x to stop` in WARN for 3 s. The help line shows only the keys that work:

```
x Stop route  ↑↓ Scroll  n/p Next/prev stop  c Characters  s Settings  q Quit
```

The lock lives in one function, `App::route_locked(key) -> bool`, so a test can check every key.

## Clutter rule

Each new element must change a decision the user makes now. If it does not, it goes in hover text or does not show.

- An element with no data does not show. No empty panel, no empty column.
- One line for each pilot. Details go in the hover text.
- The active view removes the planner and the route list. It adds only a header, a progress bar and a banner when needed.

## UX rulings, ranked

The adversarial review gave these, most important first.

| # | Level | Requirement |
|---|---|---|
| 1 | MUST | Show who gets the route before the send: `Send route #n to <Name>?` |
| 2 | MUST | No partial sends. Retry resends all with `clear_other_waypoints=true` |
| 3 | MUST | Split the route at manual hops, and mark each manual hop |
| 4 | MUST | Offer to route from the pilot's current system at start |
| 5 | MUST | Paste fallback for SSO, and a clear message for a busy port |
| 6 | MUST | Monotonic progress that accepts skipped rows |
| 7 | MUST | TUI key lock. `Esc` does not quit. `q` asks first |
| 8 | MUST | Save the active route and offer to resume it |
| 9 | MUST | Token-failure banner, with tracking shown as paused |
| 10 | MUST | Off-route detection after two polls, and a re-route only on confirm |
| 11 | MUST | Obey the ESI error limit |
| 12 | MUST | Honest Stop text: the in-game waypoints stay |
| 13 | SHOULD | `Session only` login when no keyring is present |
| 14 | SHOULD | Pilots column, overflow text and tooltips |
| 15 | SHOULD | Progress and status cues that do not need color |
| 16 | SHOULD | Offline warning with `Send anyway` |
| 17 | SHOULD | Arrived banner with `Done` |
| 18 | SHOULD | Settings stay open in active mode, with routing rows locked |
| 19 | SHOULD | Waypoint cap check |
| 20 | COULD | Hull and ship warning |
| 21 | SHOULD | Pilots panel of online pilots below "Shortest route" |
| 22 | COULD | TUI pilot initials column |

## Phases

| Phase | Work | Estimate |
|---|---|---|
| 1 | `esi::sso` and `esi::store`: PKCE, listener, paste, keyring, `characters.json`. Unit tests for PKCE, JWT claims and URL parse | 1 day |
| 2 | `esi::client` and `esi::tracker`: the four endpoints, refresh, error-limit back-off. Tests with fixture JSON | 1 day |
| 3 | `esi::active`: segments, progress, off-route, persistence. Pure unit tests | 1 day |
| 4 | TUI: Characters page, start flow, active view, key lock. A snapshot test of the active screen and a test for each locked key | 1.5 days |
| 5 | GUI: Characters popover, start dialog, active view, Pilots column, Pilots panel, portrait cache | 2.5 days |
| 6 | Measure the waypoint cap in game, register the SSO app, update the README | 0.5 day |

Total: about 7.5 days. Phases 1 to 3 have no UI and can ship first.

## Decisions

1. The client ID stays out of the repository. See [SSO details](#new-module-routercoreesi).
2. The callback port is 21404, on localhost only.
3. The Pilots column shows in the planner view and in the active view.
