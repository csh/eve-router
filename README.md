# EVE Router

A terminal route planner for EVE Online. 

## Features

- Favourite systems to see the shortest route at a glance.
- Automatic updates of required SDE files.
- Calculate the top-n routes through any number of midpoints with similar navigation options as ingame: shortest, prefer highsec and less secure.
- Wormhole connections from a Nexum map, fetched at startup. The router uses the wormhole size, mass status and expiry.

### Work in Progress

- EVE-Scout wormholes (Thera and Turnur).
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

### Settings

- Alliance capital for usage with Ansiblex network.
- Capacitor usage limit per jump bridge taken.
- Favourite systems list.
- Nexum URL, API key and map. The key needs only the `read` scope. The router sends GET requests only. A change applies at the next start.

Config files are stored in `com.smrkn.eve-router` under the platform config directory, whilst SDE files are located in the platform data directory. 
See the [`dirs`](https://crates.io/crates/dirs) crate for platform specific paths.

### Wormholes

At startup, the router gets the Nexum map and keeps a copy in the platform cache directory. A copy that is less than 5 minutes old stops the fetch. If Nexum is offline, or the key is rejected, the router uses the copy and shows the problem on the status line.

`--nexum <file>` reads a map export and sends no request.

The router does not use a wormhole when:

- its mass status is critical,
- its expiry time is past,
- the hull mass is more than the per-jump limit of the wormhole.

The size check uses the hull mass from the SDE. Fitted modules, for example plates and propulsion modules, add mass. The check does not know about them. A wormhole of a known type uses the per-jump limit of that type. A K162 or a wormhole with no type uses the lowest limit of its size class.

## Static data

At startup, the app compares its local build with [the latest SDE build](https://developers.eveonline.com/docs/services/static-data/#automation). When a new build is available, HTTP range requests are used to download only the files required.

| File                    | Use                                                                                                                                      |
|-------------------------|------------------------------------------------------------------------------------------------------------------------------------------|
| `mapSolarSystems.jsonl` | Systems, security and positions                                                                                                          |
| `mapStargates.jsonl`    | Stargate connections                                                                                                                     |
| `mapRegions.jsonl`      | Region names                                                                                                                             |
| `_sde.jsonl`            | Last known build number                                                                                                                  |
| `ships.json`            | Distilled from `types.jsonl`, `typeDogma.jsonl` and `groups.jsonl` to maintain a list of Ansiblex jump cost & provide searchable ship UI |
| `wormholes.json`        | Distilled from `types.jsonl` and `typeDogma.jsonl`: the per-jump mass and the maximum life of each wormhole type                          |

Zones are not in the SDE, so they come from the [capacitor update](https://www.eveonline.com/news/view/force-projection-ansiblex-capacitor-update) page.

Set `EVE_ROUTER_SKIP_SDE_CHECK=1` to skip this check on startup.

## Development

```sh
cargo test
```

The tests use the map files in `sde/` and the small fixtures in `tests/fixtures/`. They do not need network access.

`cargo test regenerate_repo_sde -- --ignored` makes `sde/ships.json` and `sde/wormholes.json` again. It needs the network.
