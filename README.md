# EVE Router

A route planner for EVE Online. 

> [!NOTE]
> I'm really bad at updating READMEs as I go so this will probably be light on details until I consider the application feature-complete or close enough.

## AI Usage Disclaimer

This project is an AI assisted/driven adaptation of a [library I previously wrote](https://github.com/csh/eve-arbitrage-dashboard) for arbitrage trading using the Cloudflare Workers platform, utilising Dijkstra and Held-Karp to solve the [traveling salesman problem](https://en.wikipedia.org/wiki/Travelling_salesman_problem) with the goal of producing the shortest route possible for asset pickup and drop off. 

This port currently lacks several features it had, such as overlaying zKillboard data to check for gatecamps to affect system ranking and provide re-routes in realtime.

With this in mind, some of the code will inevitably be dogshit, however I am careful to pause and review steps as I go. 

## Features

- Favourite systems to see the shortest route at a glance.
- Automatic updates of required SDE files.
- Calculate the top-n routes through any number of midpoints with similar navigation options as ingame: shortest, prefer highsec and less secure.
- Wormhole connections from a Nexum map, fetched at startup. The router uses the wormhole size, mass status and expiry. This needs a hosted Nexum instance and an API key. Without them, the router uses only the EVE-Scout wormholes.
- The signature of each wormhole jump, for example `Wormhole · Sig. ABC · XL`.
- Thera and Turnur wormholes from the public [EVE-Scout](https://www.eve-scout.com/) feed, fetched at startup. Each hub has its own stance.
- right-click a step in the route table to avoid its system or its region. The Avoid button lists the entries and removes them. Each entry is "Prefer" (a route crosses it only if no other route exists, or the other routes are much longer) or "Never".
- `F5` or the Refresh button fetches the wormholes at once, with at least 15 seconds between two fetches. The Log button opens a window with one row for each fetch and its result.

### Work in Progress

- Import Ansiblex data in SMT format.

Ansiblex support uses a best-effort attempt to solve for zones and maximum capacitor usage.
Maximum capacitor drain supported.

## Usage

```sh
cargo run --release
```

The window loads the map and shows the route summaries. Use "Copy route" to copy the selected route as tab-separated text. Slack and spreadsheets paste it as a table.

### Crates

| Crate                 | Contents                                                                 |
|-----------------------|--------------------------------------------------------------------------|
| `crates/router_core`  | The SDE, the map graph, the overlays and the route search (library)      |
| `crates/router_egui`  | The egui window (binary `eve-router`)                               |

### Settings

- Alliance capital for usage with Ansiblex network.
- Capacitor usage limit per jump bridge taken.
- Favourite systems list.
- Nexum URL, API key and map. Nexum is optional, but it needs a hosted instance and an API key. The key needs only the `read` scope. The router sends GET requests only. A change loads the new map at once. The key lives in the OS keyring (service `com.smrkn.eve-router.nexum`), not in `eve-router.json`. A key in the file of an older version moves to the keyring at the first start. Without a keyring, a new key stays in memory until the app closes, and a key from an old file stays in the file.
- Engine page, Space access: Allow, Avoid or Block for each class of J-space from 1 to 6, for the Drifter systems and for Pochven. Avoid uses the systems only when a trip must go through or into them. Block never does. While wormholes are off, the J-space and Drifter rows have no effect.
- Engine page, Hubs: Allow, Avoid or Block for Thera, Turnur and the filaments. All three are on Allow by default. A change applies at once. Avoid uses the hub wormholes, or a filament, only when a trip has no other way. A blocked Pochven turns the filaments off.

Config files are stored in `com.smrkn.eve-router` under the platform config directory, whilst SDE files are located in the platform data directory. 
See the [`dirs`](https://crates.io/crates/dirs) crate for platform specific paths.

### Wormholes

At startup, the router gets the Nexum map from the API and keeps a copy in the platform cache directory. The router reads Nexum data only from the API. A copy that is less than 5 minutes old stops the fetch. If Nexum is offline, or the key is rejected, the router uses the copy and shows the problem on the status line.

The router also gets the EVE-Scout feed at each startup, with the same cache and fallback. The feed is public and needs no key. Each entry gives an exact expiry, both signatures and a ship size, but no mass status.

While the router runs, it gets both sources again each 5 minutes and builds a new map. The route list searches again with the same waypoints. The selected route stays selected when its path is still in the list. If a wormhole on the selected route closes, the status line says so. If a fetch fails, the router uses the cache or the data of the last refresh, and shows the problem on the status line.

#### Signatures

A route step shows the first three letters of the signature in the system that the jump leaves. EVE-Scout gives both signatures. For Nexum, the router gets the signatures of each system at a wormhole end, with 8 requests at a time. A connection uses the signature that a scout linked to it. Else it uses the one wormhole signature whose "leads to" text is the name of the other system. When two connections join the same systems, or two signatures lead to the same name, the router shows no signature. When several connections link the same signature row, only the connection with a wormhole type (for example K162) keeps the signature. If none or more than one has a type, none keeps it.

The Thera and Turnur stances act on each wormhole with an end in that hub, from Nexum or EVE-Scout. The gates into Turnur stay open. A file of an older version holds `true` and `false`, and they read as Allow and Block. In `eve-router.json`, a hub is `true` for Allow, `false` for Block and `"avoid"` for Avoid:

```json
"eve_scout": { "thera": true, "turnur": "avoid" }
```

The Shortcuts box shows the count of systems with a wormhole to each hub. A count shows in gray when its stance is Block.

The router does not use a wormhole when:

- its mass status is critical,
- its expiry time is past,
- the hull mass is more than the per-jump limit of the wormhole,
- its end is in Thera or Turnur, and the stance of that hub is Block.

The size check uses the hull mass from the SDE. Fitted modules, for example plates and propulsion modules, add mass. The check does not know about them. A wormhole of a known type uses the per-jump limit of that type. A K162 or a wormhole with no type uses the lowest limit of its size class. For EVE-Scout, the size class comes from the ship size of the entry. If you set no hull, the router does no size check.

## EVE login and the active route

The router can send a route to the in-game autopilot of your character, and then track your progress along the route.

### Setup

1. Make an application at [developers.eveonline.com](https://developers.eveonline.com/applications).
2. Set the callback URL to `http://localhost:21404/callback`.
3. Give the application these four scopes: `esi-ui.write_waypoint.v1`, `esi-location.read_location.v1`, `esi-location.read_ship_type.v1` and `esi-location.read_online.v1`.
4. Set `EVE_ROUTER_CLIENT_ID` to the client ID of the application. The build reads it, and the same variable at run time overrides it.

The repository holds no client ID. Without one, the window hides the Characters button, and the router makes no ESI call.

EVE Metro, the Pochven wormhole map, also needs an API key. Set `EVE_ROUTER_METRO_KEY`. The build reads it, and the same variable at run time overrides it. The config file never holds the key. Without a key, the window hides the EVE Metro settings.

The login uses OAuth 2.0 with PKCE. The router listens on `127.0.0.1:21404` for the browser redirect. If that fails, for example over SSH, paste the redirected URL into the login window.

### Tokens

| Item | Where |
|---|---|
| Refresh token | The OS keyring: Windows Credential Manager, macOS Keychain, or the Secret Service on Linux |
| Access token | Memory only |
| Character list (ID, name, scopes) | `characters.json` next to `eve-router.json`. It holds no token |
| Active route | `active-route.json` next to `eve-router.json` |

If the system has no keyring, the router offers "Session only": the tokens stay in memory and go when the app closes. The router never writes a token to a file. "Remove" revokes the token at EVE SSO and deletes it.

### Start a route

- Push "Start route #n" in the route table header. Push "Characters (n)" in the top bar for the character list.

With two or more characters, you choose one. The confirm step names the character before anything goes to the game. If the character is not on the route, the default is a route from its current system.

The in-game autopilot follows gates only. Thus the router sends the route in segments. A segment ends before each jump bridge or wormhole, and the table marks these hops as "manual". After you take the hop, the router sends the next segment.

### While a route is active

- The planner is hidden. The table shows the progress, and the avatars or initials of your characters in each system.
- The keys and the settings that change the route are locked, so the app and the in-game waypoints stay the same. The favourites stay editable.
- After two location polls off the route, the router offers "Re-route from here".
- "Stop route" stops the tracking. The in-game waypoints stay, because ESI cannot clear them.
- If you close the app, the next start offers to resume the route.

The tracker polls the location of the active pilot every 5 seconds, the other pilots every 30 seconds, and the online state every 60 seconds. It obeys the `Expires` header and slows down when the ESI error budget is low.

A segment of more than 100 systems sends its stops only. The real in-game waypoint cap is not measured yet.

## Static data

At startup, the app compares its local build with [the latest SDE build](https://developers.eveonline.com/docs/services/static-data/#automation). When a new build is available, HTTP range requests are used to download only the files required.

| File                    | Use                                                                                                                                      |
|-------------------------|------------------------------------------------------------------------------------------------------------------------------------------|
| `mapSolarSystems.jsonl` | Systems, security, positions and the NPC faction of a system                                                                             |
| `mapStargates.jsonl`    | Stargate connections                                                                                                                     |
| `mapRegions.jsonl`      | Region names and the NPC faction of a region                                                                                             |
| `mapConstellations.jsonl` | The NPC faction of a constellation                                                                                                     |
| `_sde.jsonl`            | Last known build number                                                                                                                  |
| `ships.json`            | Distilled from `types.jsonl`, `typeDogma.jsonl` and `groups.jsonl` to maintain a list of Ansiblex jump cost & provide searchable ship UI |
| `wormholes.json`        | Distilled from `types.jsonl` and `typeDogma.jsonl`: the per-jump mass and the maximum life of each wormhole type                         |

An Ansiblex or a capital must be in nullsec that an alliance can hold, because a sovereignty hub cannot exist elsewhere. The SDE has no flag for this. The router treats a system as NPC space when its system, constellation or region has a `factionID`. This rule agreed with the ESI sovereignty map for all 3538 nullsec systems on 2026-10-20.

Zones are not in the SDE, so they come from the [capacitor update](https://www.eveonline.com/news/view/force-projection-ansiblex-capacitor-update) page.

Set `EVE_ROUTER_SKIP_SDE_CHECK=1` to skip this check on startup.

## Log file

The app writes one log file for each day, `eve-router.YYYY-MM-DD.log`, in the `logs` folder of the cache folder. It keeps the files of the last 7 days. By default the log holds warnings and errors only.

To collect more detail for a bug report, set `EVE_ROUTER_LOG` before you start the app, then attach the log file:

```sh
EVE_ROUTER_LOG=router_core=info,eve_router=info
```

In PowerShell: `$env:EVE_ROUTER_LOG = "router_core=info,eve_router=info"`. Use `debug` in place of `info` for the most detail.

## Development

```sh
cargo test               # router_egui and router_core
```

Set `UPDATE_SNAPSHOTS=1` to write the snapshot files in `tests/snapshots/` of each crate again.

The tests use the map files in `sde/` and the small fixtures in `crates/router_core/tests/fixtures/`. They do not need network access.

### CI and releases

GitHub Actions runs `.github/workflows/ci.yml` for each pull request and each push to `main`. It runs rustfmt, clippy, the tests on Linux, Windows and macOS, and `cargo deny`. CI uses the `ci` profile: the dev profile with line tables only and no incremental files. Thus the build cache stays small.

A tag `v*` starts `.github/workflows/release.yml`. First it runs the CI. Then it builds the binary with the `dist` profile: full LTO, one codegen unit and no symbols. The targets are Linux x86-64, Windows x86-64, and macOS on Apple silicon and Intel. The archives and a `SHA256SUMS` file go on the GitHub release of the tag. To build the client ID into the binary, set the repository secret `EVE_ROUTER_CLIENT_ID`.

```sh
git tag v0.1.0 && git push origin v0.1.0
```

`cargo deny check` examines the dependencies: the RustSec advisories, the licenses and the sources. See `deny.toml`.

`cargo test -p router_core keyring_round_trip -- --ignored` writes and deletes one test token in the OS keyring.

`cargo test regenerate_repo_sde -- --ignored` makes `sde/ships.json` and `sde/wormholes.json` again. It needs the network.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.

Unless you state otherwise, a contribution that you send for this project is under the same two licenses, with no extra terms.
