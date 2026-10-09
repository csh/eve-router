# Plan: the zKillboard overlay

Status: deferred. The live refresh of Nexum and EVE-Scout comes first, because this overlay uses its background worker, its snapshot, and its status line. This note keeps the decisions and the measurements from the first design talk, 2026-10-07.

## Goal

Warn about a gatecamp and about a system with a high kill count on a route. The user chooses to avoid a camp or to go through it.

## Decisions

- **Warning first.** A gatecamp and a system with a high kill count each get at least a warning on the route step.
- **The user chooses the cost.** A setting makes the router avoid a camp (a danger cost through `Router::set_danger`) or ignore it. The warning shows in both cases.
- **pvp kills only.** Only a kill with `pvp` in `zkb.labels` counts. A kill with `npc` does not.
- **Respect the API.** zKillboard is a free service. Send a User-Agent with a project URL, send `Accept-Encoding: gzip`, and obey each rate and cache rule below.
- **Memory only.** zKillboard data never goes to the disk cache.
- **Out of the graph.** Kills change no edges. They change only the cost and the warning of a system, so this overlay has no `Universe` ownership problem.

## The source: a hybrid

1. At start, read `https://r2z2.zkillboard.com/ephemeral/sequence.json` and read each kill file from that number on: `https://r2z2.zkillboard.com/ephemeral/{sequence}.json`. On a 404, wait at least 6 seconds before the next try.
2. Keep each pvp kill at a stargate in memory. Drop a kill when it is older than the time window.
3. When a route shows, fetch `https://zkillboard.com/api/kills/regionID/{id}/pastSeconds/3600/` for each region on the route with no backfill. Keep each result for 1 hour, to obey the 1-hour client cache rule.
4. If the same `killmail_id` comes from both sources, keep one copy. A reprocessed kill can come back under a new sequence number.
5. After the app runs for one full time window, R2Z2 covers the window, so the region requests stop.

Why a hybrid: R2Z2 alone has no kills from before the app starts. The region API alone holds back kills less than 5 minutes old, and a 5-minute poll of each region breaks the cache rule. A single global "last hour" query is not possible (see below).

## API rules (from https://zkillboard.com/api/docs/, read 2026-10-07)

| Rule | Value |
|---|---|
| R2Z2 rate limit | 15 requests per second per IP, non-empty User-Agent |
| R2Z2 after a 404 | wait at least 6 seconds |
| R2Z2 file life | at least 24 hours |
| R2Z2 `sequence.json` | updates every 51 kills, 1-day cache |
| Killmail query cache | 1-hour client cache |
| Killmail query age | kills less than 5 minutes old are held back |
| `pastSeconds` | up to 604,800, in steps of 3,600 |
| Page size | up to 200 kills, pages 1 to 100, newest first |
| Path | a trailing slash, no query string |

## Measurements (2026-10-07)

- **Feed rate.** About 3,000 sequence numbers in 3.5 hours: about 860 kills per hour, or 0.24 requests per second to read the feed.
- **File size.** 600 B to 760 B for each R2Z2 kill, with gzip.
- **File age.** A file 45 hours old was there. A file about 10 days old gave 404.
- **The newest kill.** `sequence.json` gave the newest number. The numbers after it gave 404.
- **Filters.** `kills/pastSeconds/3600/`, `kills/w-space/pastSeconds/3600/` and `kills/label/pvp/pastSeconds/3600/` all give `{"error":"Please provide an entity filter first."}`. A region or system filter works.
- **Kill content.** Each R2Z2 file has `killmail_id`, `hash`, `esi` (the full ESI killmail), `zkb`, `uploaded_at` and `sequence_id`. The region API gives the ESI killmail with a `zkb` object at the top level.
- **Gate kills.** In 20 kills in The Forge, 17 had a `zkb.locationID` that is a stargate ID (`_key` in `mapStargates.jsonl`). The `victim.position` was 4.8 km to 28.6 km from that gate.
- **Smartbombs.** The SDE has 138 types in group 72 (Smart Bomb). A camp kill with a smartbomb `weapon_type_id` can give a "smartbomb camp" label.

## Code facts

- `Router::set_danger` takes one danger cost for each system, in milli-jumps.
- `sde.rs` reads `mapStargates.jsonl`, but drops the gate `_key` and `position`. This overlay needs both.

## Open questions

- The largest distance from a gate for a camp kill.
- The kill count and the time window that make a camp.
- The kill count and the time window that make a "high kill count" system.
- The warning text in the GUI and the TUI.
- The default for the avoid-or-ignore setting.
