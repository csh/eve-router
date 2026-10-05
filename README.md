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
cargo test
```

The tests use the map files in `sde/` and the small fixtures in `tests/fixtures/`. They do not need network access.

`cargo test regenerate_repo_sde -- --ignored` makes `sde/ships.json` and `sde/wormholes.json` again. It needs the network.
