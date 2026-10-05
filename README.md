# EVE Router

A terminal route planner for EVE Online. 

## Features

- Favourite systems to see the shortest route at a glance.
- Automatic updates of required SDE files.
- Calculate the top-n routes through any number of midpoints with similar navigation options as ingame: shortest, prefer highsec and less secure.

### Work in Progress

- Import map data from Nexum to factor in wormhole connections.
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

Config files are stored in `com.smrkn.eve-router` under the platform config directory, whilst SDE files are located in the platform data directory. 
See the [`dirs`](https://crates.io/crates/dirs) crate for platform specific paths.

## Static data

At startup, the app compares its local build with [the latest SDE build](https://developers.eveonline.com/docs/services/static-data/#automation). When a new build is available, HTTP range requests are used to download only the files required.

| File                    | Use                                                                                                                                      |
|-------------------------|------------------------------------------------------------------------------------------------------------------------------------------|
| `mapSolarSystems.jsonl` | Systems, security and positions                                                                                                          |
| `mapStargates.jsonl`    | Stargate connections                                                                                                                     |
| `mapRegions.jsonl`      | Region names                                                                                                                             |
| `_sde.jsonl`            | Last known build number                                                                                                                  |
| `ships.json`            | Distilled from `types.jsonl`, `typeDogma.jsonl` and `groups.jsonl` to maintain a list of Ansiblex jump cost & provide searchable ship UI |

Zones are not in the SDE, so they come from the [capacitor update](https://www.eveonline.com/news/view/force-projection-ansiblex-capacitor-update) page.

Set `EVE_ROUTER_SKIP_SDE_CHECK=1` to skip this check on startup.

## Development

```sh
cargo test
```

The tests use the map files in `sde/` and the small fixtures in `tests/fixtures/`. They do not need network access.
