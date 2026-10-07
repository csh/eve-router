# EVE Router

A terminal route planner for EVE Online. 

## Features

- Favourite systems to see the shortest route at a glance.
- Automatic updates of required SDE files.
- Calculate the top-n routes through any number of midpoints with similar navigation options as ingame: shortest, prefer highsec and less secure.
- Wormhole connections from a Nexum map, fetched at startup. The router uses the wormhole size, mass status and expiry.
- The signature of each wormhole jump, for example `Wormhole · Sig. ABC · XL`.
- Thera and Turnur wormholes from the public [EVE-Scout](https://www.eve-scout.com/) feed, fetched at startup. Each hub has its own switch.

### Work in Progress

- Import Ansiblex data in SMT format.

Ansiblex support uses a best-effort attempt to solve for zones and maximum capacitor usage.
Maximum capacitor drain supported.

## Usage

```sh
cargo run --release -- Jita Amarr
cargo run --release -- "Jita > Rens > Amarr" --mode prefer-highsec -n 3
cargo run --release -- --print --capital JK-Q77 --hull Sin UALX-3 Jita
```

Use `--print` to echo all solved routes, omitting this launches the TUI.

The egui window is in an early state. It loads the map and shows the route summaries:

```sh
cargo run --release -p router_egui
```

### Crates

| Crate                 | Contents                                                                 |
|-----------------------|--------------------------------------------------------------------------|
| `crates/router_core`  | The SDE, the map graph, the overlays and the route search (library)      |
| `crates/router_tui`   | The CLI, `--print` and the terminal UI (binary `eve-router`)             |
| `crates/router_egui`  | The egui window (binary `eve-router-egui`)                               |

### Settings

- Alliance capital for usage with Ansiblex network.
- Capacitor usage limit per jump bridge taken.
- Favourite systems list.
- Nexum URL, API key and map. The key needs only the `read` scope. The router sends GET requests only. A change applies at the next start.
- EVE-Scout: a Thera switch and a Turnur switch. Both are on by default. A change applies at once.

Config files are stored in `com.smrkn.eve-router` under the platform config directory, whilst SDE files are located in the platform data directory. 
See the [`dirs`](https://crates.io/crates/dirs) crate for platform specific paths.

### Wormholes

At startup, the router gets the Nexum map from the API and keeps a copy in the platform cache directory. The router reads Nexum data only from the API. A copy that is less than 5 minutes old stops the fetch. If Nexum is offline, or the key is rejected, the router uses the copy and shows the problem on the status line.

The router also gets the EVE-Scout feed at each startup, with the same cache and fallback. The feed is public and needs no key. Each entry gives an exact expiry, both signatures and a ship size, but no mass status.

#### Signatures

A route step shows the first three letters of the signature in the system that the jump leaves. EVE-Scout gives both signatures. For Nexum, the router gets the signatures of each system at a wormhole end, with 8 requests at a time. A connection uses the signature that a scout linked to it. Else it uses the one wormhole signature whose "leads to" text is the name of the other system. When two connections join the same systems, or two signatures lead to the same name, the router shows no signature.

The Thera and Turnur switches act on each wormhole with an end in that hub, from Nexum or EVE-Scout. The gates into Turnur stay open. In `eve-router.json`:

```json
"eve_scout": { "thera": true, "turnur": false }
```

The Shortcuts box shows the count of systems with a wormhole to each hub. A count shows in gray when its switch is off.

The router does not use a wormhole when:

- its mass status is critical,
- its expiry time is past,
- the hull mass is more than the per-jump limit of the wormhole,
- its end is in Thera or Turnur, and the switch of that hub is off.

The size check uses the hull mass from the SDE. Fitted modules, for example plates and propulsion modules, add mass. The check does not know about them. A wormhole of a known type uses the per-jump limit of that type. A K162 or a wormhole with no type uses the lowest limit of its size class. For EVE-Scout, the size class comes from the ship size of the entry. If you set no hull, the router does no size check.

## EVE login and the active route

The router can send a route to the in-game autopilot of your character, and then track your progress along the route.

### Setup

1. Make an application at [developers.eveonline.com](https://developers.eveonline.com/applications).
2. Set the callback URL to `http://localhost:21404/callback`.
3. Give the application these four scopes: `esi-ui.write_waypoint.v1`, `esi-location.read_location.v1`, `esi-location.read_ship_type.v1` and `esi-location.read_online.v1`.
4. Set `EVE_ROUTER_CLIENT_ID` to the client ID of the application. The build reads it, and the same variable at run time overrides it.

The repository holds no client ID. Without one, "Add character" is off and shows the reason.

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

- TUI: select a route, then press `g`. Press `c` for the character list.
- GUI: push "Start route #n" in the route table header. Push "Characters (n)" in the top bar for the character list.

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
| `mapSolarSystems.jsonl` | Systems, security and positions                                                                                                          |
| `mapStargates.jsonl`    | Stargate connections                                                                                                                     |
| `mapRegions.jsonl`      | Region names                                                                                                                             |
| `_sde.jsonl`            | Last known build number                                                                                                                  |
| `ships.json`            | Distilled from `types.jsonl`, `typeDogma.jsonl` and `groups.jsonl` to maintain a list of Ansiblex jump cost & provide searchable ship UI |
| `wormholes.json`        | Distilled from `types.jsonl` and `typeDogma.jsonl`: the per-jump mass and the maximum life of each wormhole type                         |

Zones are not in the SDE, so they come from the [capacitor update](https://www.eveonline.com/news/view/force-projection-ansiblex-capacitor-update) page.

Set `EVE_ROUTER_SKIP_SDE_CHECK=1` to skip this check on startup.

## Development

```sh
cargo test               # router_core and router_tui
cargo test --workspace   # also router_egui
```

Set `UPDATE_SNAPSHOTS=1` to write the snapshot files in `tests/snapshots/` of each crate again.

The tests use the map files in `sde/` and the small fixtures in `crates/router_core/tests/fixtures/`. They do not need network access.

### CI and releases

GitHub Actions runs `.github/workflows/ci.yml` for each pull request and each push to `main`. It runs rustfmt, clippy, the tests on Linux, Windows and macOS, and `cargo deny`. CI uses the `ci` profile: the dev profile with line tables only and no incremental files. Thus the build cache stays small.

A tag `v*` starts `.github/workflows/release.yml`. First it runs the CI. Then it builds both binaries with the `dist` profile: full LTO, one codegen unit and no symbols. The targets are Linux x86-64, Windows x86-64, and macOS on Apple silicon and Intel. The archives and a `SHA256SUMS` file go on the GitHub release of the tag. To build the client ID into the binaries, set the repository secret `EVE_ROUTER_CLIENT_ID`.

```sh
git tag v0.1.0 && git push origin v0.1.0
```

`cargo deny check` examines the dependencies: the RustSec advisories, the licenses and the sources. See `deny.toml`.

`cargo test -p router_core keyring_round_trip -- --ignored` writes and deletes one test token in the OS keyring.

`cargo test regenerate_repo_sde -- --ignored` makes `sde/ships.json` and `sde/wormholes.json` again. It needs the network.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.

Unless you state otherwise, a contribution that you send for this project is under the same two licenses, with no extra terms.
