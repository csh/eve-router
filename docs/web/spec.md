# Web client: specification

Status: draft 3, 2026-10-10. Draft 2 adds the crate choices (sections 2 and 7.1). Draft 3 adds the hull IDs (section 4.3) and the `Store` layout (section 5.5). This document says what the browser build of `router_egui` must do, and why. The task order is in [plan.md](plan.md).

**Terms**

| Term | Meaning |
| :--- | :--- |
| UI thread | The browser main thread. It runs the `router_egui` wasm and draws the canvas. |
| Engine worker | One Web Worker. It runs the new `router_engine` wasm. It holds the map, syncs the SDE, fetches the wormholes and runs the route search. |
| Pool thread | A Web Worker that `wasm-bindgen-rayon` starts inside the engine worker. It shares the wasm memory of the engine worker. |
| Cloudflare Worker | Code at the Cloudflare edge. The project uses it for the Nexum proxy only. This document never writes "Worker" alone. |
| Blob | The distilled SDE, as one `rkyv` archive (`SdeBlob`). |
| Generation | A number that names one built map. Each new map gets the next number. |

---

## 1. Goal and scope

A pilot opens a URL and uses the router. Nothing installs. The page is a static site. All logic runs in the browser. The one exception is a small same-origin proxy for Nexum (section 9).

**In scope**

- The route planner: waypoints, top-N routes, modes, optimize order, avoid lists, favourites, route costs.
- The SDE: a fetch from CCP at load time, a distill step in the browser, and a cache as a blob in IndexedDB.
- The route search in a Web Worker, with real parallel threads where the browser allows them.
- Live wormholes: EVE-Scout direct, Nexum through the proxy, a refresh each 5 minutes.
- Jump bridges: a paste box for the SMT list. The text stays in storage across reloads.
- Pilots: EVE SSO login with PKCE, the location tracker, autopilot waypoints, the active route, portraits.
- The settings window and the log window.

**Out of scope**

- The TUI in a browser.
- Server-side state of any kind: no database, no session store, no token relay.
- Offline use (a service worker cache). The page needs the network at load time.
- A new layout for phones. The current compact mode is the phone layout.
- Any change in native behavior. The native tests stay green after each task.

---

## 2. Decisions

The project owner made these decisions on 2026-10-10.

| Topic | Decision | Reason |
| :--- | :--- | :--- |
| Rendering | `eframe::WebRunner`, WebGL2 through the existing `glow` feature | The native build already uses `glow`. `wgpu` adds size and gives no gain for a panel UI. |
| Hosting | Cloudflare Workers Static Assets, with a `_headers` file | Threads need the COOP and COEP headers. Cloudflare sets them with no code. The same project hosts the Nexum proxy. |
| Parallelism | One engine worker, plus a `wasm-bindgen-rayon` pool inside it | The existing `par_iter` and `rayon::join` calls run unchanged on real threads. The UI thread never waits for a search. |
| SDE | Fetch from CCP at load time. Distill in the engine worker. Store as an `rkyv` blob in IndexedDB. The app opens only when the full blob exists (section 5.2). | A later load reads one small record and skips the 191 MB of JSON. One load step means one blob, no map without hulls, and no hull that waits for a table. |
| Nexum | A same-origin proxy only. No direct call from the browser. | A tested Nexum server allows only its own origin (section 3). |
| Login | Client-side PKCE, with separate EVE app registrations for production and development | EVE SSO supports public clients. No server holds a token. |
| Toolchain | Native stays on stable. The web build uses one pinned nightly. | The threaded engine needs `-Z build-std` and atomics. |
| HTTP | `reqwest` 0.13 on both targets. It replaces `ureq`. | One client for native and wasm. On wasm it uses `fetch`, with timeouts through `AbortController`. Our own `Http` trait and two implementations are not needed. |
| Async on native | A current-thread `tokio` runtime on each existing loop thread. Our only `tokio` feature is `rt`. | Async `reqwest` needs `tokio` on native, and `pollster` cannot drive it. |
| Storage | One concrete `Store` type in `router_core::store`, with one file for each target: `native.rs` (files) and `web.rs` (IndexedDB). `cfg` picks the file. No trait, no `localStorage`. See section 5.5. | The pattern of `std`, `getrandom` and `web-time`. Each backend reads as plain code, jump to definition goes to the real code, and the wasm CI check fails if the two files drift. An async trait does not work with `dyn` without boxing. One `cfg` type cannot hold two wasm backends, so the UI and the engine both use IndexedDB. |
| Binary format | `rkyv` for the blob and for the engine messages | `rkyv` is small, and it removes the need to write and maintain our own container formats. |
| Browser APIs | `indexed_db_futures` for IndexedDB. `gloo-storage` for `sessionStorage` only (the login state). | Raw `web-sys` for these APIs is callbacks and `JsValue` errors. |
| Timers | `futures-timer` on both targets | One API on every desktop target and on wasm32 |
| Hull identity | A hull is the EVE type ID of one ship. A group is only a fallback that the user names in the config or on the command line. The map owns the hull table. No global table, no `&'static Hull`. | A pointer means nothing in the memory of another wasm module, and it cannot go into a message. A type ID can, and it is stable across SDE builds. A group uses the highest cost and mass of its ships, so the engine must get the exact ship when the app knows it. See section 4.3. |
| Crate layout | `router_core` stays one crate. Plan task 9 decides on a `router_data` split. | A split pays only when a build can drop a dependency. Under a pure-map split, each build still needs `router_core`, so the split adds moves and no smaller build. |
| Worker plumbing | Hand-written message loop. Plan task 1 also tests `gloo-worker`. | `gloo-worker` has serde-only codecs and no transfer (section 7.1). Nobody has shown it with `wasm-bindgen-rayon`. |

---

## 3. Network rules from the CORS and PKCE findings

Source: `docs/plans/web-support.md` (deleted in commit `14264cd`, read it with `git show 088d9da:docs/plans/web-support.md`), sections 2.3, 2.4 and 3. Those checks used `curl` and a Firefox console on 2026-10-09, plus the CCP SSO documentation. This specification does not test them again. It copies the rules that the code must obey, so this document stands alone.

| Endpoint | Rule |
| :--- | :--- |
| `developers.eveonline.com/static-data/tranquility/latest.jsonl` | A plain `GET`. `ACAO: *`. `Cache-Control: max-age=300`, so check at most once each 5 minutes. |
| `.../tranquility/eve-online-static-data-<build>-jsonl.zip` | Use `HEAD` for the length. Use explicit `Range: bytes=<start>-<end>` values only. A suffix range (`bytes=-65536`) is not CORS-safelisted, it sends a preflight, and every `OPTIONS` request to CCP fails. Send no other custom header. |
| `.../eve-online-static-data-latest-jsonl.zip` | Never use it. Its 302 has no CORS headers. |
| `images.evetech.net` (portraits) | Load with a CORS-mode `fetch`. It has `ACAO: *` and no CORP header, so a CORS `fetch` passes COEP `require-corp`. A plain `<img>` does not. |
| `esi.evetech.net` | Direct. The preflight allows `Authorization` and `X-Compatibility-Date` for `GET` and `POST`. The browser can read `Expires`, `X-Esi-Error-Limit-Remain`, `X-Esi-Error-Limit-Reset` and `Retry-After`. |
| `login.eveonline.com/v2/oauth/token` | Direct. A form-encoded `POST` is a simple request. Every answer has `ACAO: *`. Decide by the HTTP status. Read the JSON `error` field only when the body parses, because a bad code gave a 500 with an HTML body. |
| `api.eve-scout.com` | Direct. |
| A Nexum server | Not direct. The tested server sends `ACAO` with its own origin only, for any caller. The browser blocks the response. All Nexum traffic goes through the proxy. |

All cross-origin requests send no credentials. `reqwest` on wasm has no `credentials: "omit"` setting. It uses the fetch default, `same-origin`, which sends no cookies on a cross-origin request. Thus the default is correct. Never call `fetch_credentials_include`. `reqwest` on wasm uses CORS mode by default, so never call `fetch_mode_no_cors`.

**Still untested:** Chrome, Safari, and COEP on a page that is really cross-origin isolated. Plan task 18 tests them.

---

## 4. Architecture

```
 Browser tab (cross-origin isolated)
 +----------------------------------------------------------------------------+
 |  UI thread: router_egui wasm (no atomics)                                   |
 |    egui canvas, settings, SSO, ESI tracker, portraits, Store (config)       |
 |    map mirror: same Universe as the engine, read-only, for names and draws  |
 |                         |  postMessage (rkyv bytes, transferred)  ^         |
 |                         v                                         |         |
 |  Engine worker: router_engine wasm (atomics, shared memory)                |
 |    SDE sync and distill, blob cache, map build, wormhole refresh,          |
 |    route search                                                             |
 |         | rayon par_iter / join                                             |
 |         v                                                                   |
 |  Pool threads 1..N (wasm-bindgen-rayon, same memory)                        |
 +----------------------------------------------------------------------------+
        |               |                 |                    |
   IndexedDB      CCP SDE (ranges)   EVE-Scout          /api/proxy/nexum
   (blob, caches)                                       (Cloudflare Worker)
```

### 4.1 Who owns what

| Job | Owner | Why |
| :--- | :--- | :--- |
| Rendering, input, settings | UI thread | Only the main thread has the canvas. The UI owns the config keys (section 5.5). |
| SSO, ESI tracker, portraits | UI thread | Small requests. They need `window.location` and `sessionStorage`. |
| SDE check, download, distill, blob | Engine worker | The distill reads 191 MB of JSON. It must not freeze the UI. |
| Wormhole fetch and map build | Engine worker | A large Nexum map is a few MB of JSON, plus one signature request for each wormhole system. |
| Route search, optimize, favourite distances | Engine worker | Yen's algorithm runs one search for each node of the previous path. Top 20 on a long route thus runs hundreds of searches. |
| Top-1 search for "re-route from here" (`esi::pilots::route_from`) | UI thread | It is one A* search, much cheaper than a top-N search. It can stay local. |

The UI owns the config and the bridge text, and only the owner of a key writes it (section 5.5). Thus the UI thread sends the engine worker the values that it needs: the Nexum settings, the EVE-Scout switches and the bridge text.

### 4.2 The map mirror

The UI needs the map each frame: the system search, the route table, the labels, the pilots. A round trip for each of these is not possible in an immediate-mode UI. Thus the UI thread keeps its own `Universe`, built from the same inputs with the same functions:

1. the blob bytes (the engine worker sends them one time),
2. the bridge text,
3. the merged wormhole list and the `now` value that the engine worker used.

The functions are pure and keep their input order. `wormhole::merge` keeps the source order, and `Universe::from_blob` keeps the SDE order. Thus both sides get the same `NodeIndex` and `EdgeIndex` for each system and link, and a route from the engine worker is valid on the mirror.

Rules:

1. Each map has a generation. Each route reply names its generation. The UI drops a reply for an old generation.
2. Each map has a fingerprint: a hash of its node and edge lists, and of its `HullTable`. The engine worker sends it with each map. The UI compares it with its own. A mismatch is a bug. The UI then logs an error and searches on the UI thread with one thread, so the app still works.
3. A native test builds a map two times from the same inputs, and compares the fingerprints.

### 4.3 Messages

The engine protocol lives in `router_core::engine`, so a native test can drive it with no browser. Each message is an `rkyv` archive in an `ArrayBuffer`, transferred and not copied. A hand-written loop on `wasm-bindgen` and `web-sys` sends the messages (section 7.1 explains why not `gloo-worker`). The UI and the engine come from one build, but the first message still carries a protocol version.

**The outbox.** Every message from the engine to the UI goes through one `flume` channel, the outbox. One task, started once with `spawn_local`, receives from it with `recv_async`, encodes each message and posts it. No other code posts to the UI.

```rust
let (outbox, rx) = flume::unbounded::<EngineToUi>();
spawn_local(async move {
    while let Ok(msg) = rx.recv_async().await { post_to_ui(msg); }
});
```

Each sender is a clone of `outbox`. A pool thread, the refresh task and the SDE download all send into it, from any thread. A send from a pool thread wakes the outbox task through the multi-thread futures executor of an atomics build (`Atomics.waitAsync`, or a same-origin helper worker where the browser lacks it). If the engine later streams routes one by one, the pool threads send more messages, and the outbox does not change.

| Direction | Message | Content |
| :--- | :--- | :--- |
| UI to engine | `Init` | Protocol version, Nexum settings, EVE-Scout switches, bridge text, pool size |
| UI to engine | `Route` | Request ID, generation, waypoints, route settings, favourites, top N |
| UI to engine | `Refresh` | Refresh the wormholes now (the `F5` key) |
| UI to engine | `SetNexum`, `SetBridges` | New values. The engine builds a new map. |
| Engine to UI | `Progress` | SDE stage and byte counts, for the splash screen |
| Engine to UI | `Blob` | The full blob bytes, one time for each SDE build. A `Map` message with a new generation always follows. The UI rebuilds its mirror on each `Blob`. |
| Engine to UI | `Map` | Generation, fingerprint, `now`, the merged wormholes, the Shortcuts counts, the log rows |
| Engine to UI | `Routes` | Request ID, generation, routes, favourite distances, the order note, an error text, the search time |
| Engine to UI | `Failed` | A fatal error text for the splash screen |

`Settings` goes into the `Route` message as it is, with no translation record. It holds plain data only: the hull as a `HullRef`, and the capital and favourites as `NodeIndex` values.

The hull rules:

1. `HullRef` is `Ship(type_id)` or `Group(group_id)`, with the EVE IDs from the SDE. Both IDs are stable across SDE builds, and they mean the same thing in both wasm modules. No index has to match.
2. The UI sends `Ship` whenever it knows the ship: the hull picker lists ships only, and a followed pilot gives the type ID of the current ship. The engine then uses the cost and the mass of that exact ship.
3. `Group` is only for a group that the user names in the config or on the command line (for example `black-ops`). A group takes the highest bridge cost and the highest mass of its ships (`ansiblex.rs`, `HullTable::new`). Thus it is a worst case, not the cost of one ship. In the SDE of 2026-10-10, all 48 groups have one cost each, but masses differ in a group (Special Edition Yachts: 1.0 to 13.1 million kg), and CCP can change the cost of one ship.
4. `Universe` holds the `HullTable` as an `Arc<HullTable>`, so a map clone stays cheap. Lookups go from the ID to the row in that table. Code that needs hull data reads it from the `Universe` that it already gets. No function reads a global.
5. "Same hull" compares `HullRef` values, not pointers.
6. On disk, the config keeps the hull name, as now (`cfg.hull`). A user can edit the name in `eve-router.json` by hand. An unknown name gives the "Unknown hull" error, as now.

### 4.4 Stale requests

The UI already waits `APPLY_DELAY` (500 ms) after the last change before it searches. On top of that:

1. The UI keeps only the newest request ID. It drops any older reply.
2. The engine worker keeps only the newest queued `Route` message. It drops older queued ones without a search.
3. In threaded mode, the engine worker runs a search on the pool with `rayon::spawn`, so its own message loop stays free. The pool task sends its `Routes` reply into the outbox (section 4.3). Inside the search, `par_iter` and `rayon::join` spread and collect the work, so each request gives one reply. In single-thread mode, the engine worker calls the search directly and sends the reply into the same outbox. A newer `Route` message sets a shared `AtomicBool`. `k_shortest`, `alternatives` and `routes` read it between loop passes, and stop early. In single-thread mode a search cannot stop. That is acceptable, because rule 2 still drops the queued requests.

---

## 5. SDE pipeline

### 5.1 Measured sizes

Build 3586130, read from the zip central directory on 2026-10-10:

| Entry | Compressed | Inflated | Use |
| :--- | ---: | ---: | :--- |
| `mapSolarSystems.jsonl` | 1.13 MB | 5.14 MB | Map |
| `mapStargates.jsonl` | 0.50 MB | 2.92 MB | Map |
| `mapRegions.jsonl` | 0.18 MB | 0.44 MB | Map |
| `_sde.jsonl` | under 0.01 MB | under 0.01 MB | Build number |
| `groups.jsonl` | 0.14 MB | 0.81 MB | Ship table |
| `typeDogma.jsonl` | 1.26 MB | 27.72 MB | Ship and wormhole tables |
| `types.jsonl` | 23.58 MB | 154.07 MB | Ship and wormhole tables |
| **Total** | **26.79 MB** | **191.11 MB** | |

The map needs 1.8 MB. The ship and wormhole tables need 25 MB, which is 93 percent of the download. The native code inflates all of `types.jsonl` into one 154 MB buffer, then parses every line into a struct. A browser tab must not do that.

### 5.2 The first load

The app opens only when the full blob exists. On a first load, with no blob in IndexedDB:

1. Fetch the seven entries of section 5.1 (26.79 MB).
2. Distill the ship table and the wormhole type table (section 5.3), and build the `Universe`.
3. Write the blob to IndexedDB and `flush`.
4. Send `Blob`, then the first `Map` message. The planner opens.
5. Start the wormhole fetches. The size class of a Nexum wormhole needs the wormhole type table, which now exists.

The splash screen shows the `Progress` messages: the stage, and the bytes so far against the total. A later load reads the blob and opens with no SDE download.

Why one step and not a fast map first: a map without the hull table needs a second blob, a second mirror build, and a hull from the config that waits for a table that does not exist yet. The cost is the wait on a first load only. At 50 Mbit/s the download takes about 4 seconds, and at 10 Mbit/s about 21 seconds, plus the distill time that plan task 11 measures.

### 5.3 Streaming distill

The distill keeps the compressed bytes in memory (at most 27 MB) and inflates each entry as a stream. It never holds a full inflated file.

1. Inflate `groups.jsonl`. Keep the groups of the ship category (6).
2. Inflate `types.jsonl` line by line. Keep a type when its group is a ship group, when its name matches `Wormhole X000`, or when it is the Ansiblex type (35841). Drop each other line at once.
3. Inflate `typeDogma.jsonl` line by line. Keep the attributes of the kept types only.
4. Make the ship table (`ships::derive`) and the wormhole type table (`wormhole_types::derive`) from the kept rows.
5. Keep a running CRC-32 and byte count for each entry. At the end of each entry, compare them with the zip directory, as `extract` does now. A mismatch discards the whole update.

The native build uses the same distill code, so one code path makes `ships.json` and `wormholes.json` on both targets. A native test runs it on the repository `sde/` source data and compares the output with the files in `sde/`.

Target: the distill adds at most 64 MB to the peak memory of the engine worker. Plan task 4 measures it on native, and task 11 measures it in a browser.

### 5.4 The blob

`SdeBlob` derives `rkyv::Archive`. It holds:

- a format version and the SDE build number,
- the region names,
- the systems, in SDE order: ID, name, region index, security, position,
- the stargates, as pairs of system indexes, in SDE order,
- the ship table (`ShipData`),
- the wormhole type table (`WormholeTypes`).

It does not hold the `Universe`. The `Universe` has a `petgraph` graph and `HashMap`s, and each refresh clones it. `Universe::from_blob` builds the graph from the archived slices.

Rules:

1. Check the bytes with `rkyv` validation (`bytecheck`) before any read. IndexedDB data is not trusted.
2. The IndexedDB key holds the format version (`sde-blob-v1`). A wrong version or a failed check deletes the record and starts a download.
3. Read the bytes into an `rkyv::util::AlignedVec` before access. A browser has no `mmap`, so this is one copy of a small buffer.
4. The blob replaces `ansiblex::init(sde_dir)`. The `HullTable` comes from the blob and lives in the `Universe` (section 4.3). A new SDE build found during a session thus gives a new map with a new table. The `HullRef` in `Settings` stays valid, because type IDs and group IDs do not change. A ship that the new SDE removes gives no hull, as an unknown type does now.
5. Expected size: under 1 MB. Plan task 5 records the real size.

### 5.5 Storage

All saved values go through one type, `router_core::store::Store`. It holds named byte values. The module has three files:

```
crates/router_core/src/store/
  mod.rs     the cfg switch, the module doc, the key rules
  native.rs  Store on files
  web.rs     Store on IndexedDB, with the in-memory copy
```

`mod.rs` holds the only `cfg` lines: `native.rs` for every target except wasm32, `web.rs` for wasm32, and `pub use` of the one `Store`. Both files give the same public API:

| Method | Native (`native.rs`) | Web (`web.rs`) |
| :--- | :--- | :--- |
| `async fn open` | Returns at once | Opens the database and reads all keys into memory |
| `fn get` | Reads the file | Reads the in-memory copy |
| `fn set` | Writes the file, as now | Updates the in-memory copy, then starts a background IndexedDB write |
| `fn delete` | Removes the file | Removes the key from memory, then starts a background delete |
| `async fn flush` | Returns at once | Waits until all background writes end |

`get`, `set` and `delete` are sync on both targets, so no caller changes to async. Only the startup code awaits `open`, and only the engine awaits `flush`. Native keeps its file paths and its behavior. The TUI and the egui app still see each other's files.

The web details:

- Database `eve-router`, object store `kv`, one record for each key. The blob is key `sde-blob-v1`, value one `ArrayBuffer`. One record makes each write atomic, so an interrupted update leaves the old blob.
- The UI and the engine worker each open their own `Store` on the same database. IndexedDB works in a window and in a Web Worker in every target browser.
- The engine worker calls `navigator.storage.persist()` one time, so the browser does not evict the records under storage pressure. If the browser refuses, the app still works and downloads the SDE again after an eviction.
- The values are small (the blob is about 0.5 MB), so the in-memory copy costs little. The Origin Private File System gives no gain.

The key rules:

1. Each key has one owner, and only the owner writes it. The UI owns the config, the character list, the active route, the bridge text and the refresh tokens. The engine owns the blob and the source caches. The type system does not enforce this rule, so the module doc states it too.
2. A background write that is in flight when the tab closes can be lost. For a settings change, that is the last few milliseconds. The engine awaits `flush` after each blob write.
3. Two tabs do not see each other's writes until a reload. The last write wins.

### 5.6 Update check

At each load, the engine worker reads the blob first and builds the map. In the background, it reads `latest.jsonl`. If the build number differs from the blob build, it runs the download and the distill again in the background, writes the new blob, and sends `Blob` and a new `Map` message. The app stays usable on the old map until then. A same-build answer sends no more traffic. Native keeps its current behavior.

---

## 6. Route engine and threads

### 6.1 Two engine builds

A wasm module with shared memory needs `SharedArrayBuffer`, and the browser gives that only to a cross-origin isolated page. Thus the deploy holds two engine builds from the same source:

| Build | Toolchain and flags | Used when |
| :--- | :--- | :--- |
| `engine-mt` | Pinned nightly, `-C target-feature=+atomics,+bulk-memory`, `-Z build-std=panic_abort,std`, cargo feature `threads` | `crossOriginIsolated` is true and the pool starts |
| `engine-st` | The same nightly, no atomics, no `threads` feature | Any other case |

The UI loader reads `crossOriginIsolated` and starts the right engine worker. In `engine-st`, the `rayon` calls run on one thread, because `rayon-core` uses the current thread when it cannot start threads. The route code does not change for either build.

The engine runs in a Web Worker in both builds. A browser forbids `Atomics.wait` on the main thread, and `rayon` blocks a caller that is not a pool thread. Thus the pool must run under a Web Worker, never under the UI thread.

### 6.2 Pool size

Default: `min(navigator.hardwareConcurrency - 1, 4)`, at least 1. Each pool thread adds a stack and startup time, and the gain per thread drops as the count grows. Task 14 measures 1, 2, 4 and 8 threads in each browser and sets the final default. The settings window shows the size.

### 6.3 Memory

Shared memory cannot shrink, and iOS Safari can refuse a large maximum. Start with a 512 MiB maximum for `engine-mt`. Task 14 sets the final value from the measured peak (distill plus search) with a margin.

### 6.4 Targets

| Measure | Target |
| :--- | :--- |
| UI thread time for a route request | Under 2 ms (encode and post) |
| Frames dropped during any search | None |
| Top 20, Jita to ND-X7X, `engine-mt` with 4 threads, Chrome | At least 2 times faster than `engine-st` |
| Top 5, same route, `engine-st` | Under 50 ms |

If `engine-mt` misses the 2-times target in Chrome, or fails in any target browser, the deploy ships `engine-st` only, and this section gets the numbers.

---

## 7. Seams in `router_core`

Native behavior does not change. Each seam lands on native first, and the native tests prove it.

| Seam | Native | Web |
| :--- | :--- | :--- |
| Clock | `web_time::Instant` and `web_time::SystemTime`. On native these are the `std` types. | `performance.now()` and `Date.now()` |
| Storage (`Store`, section 5.5) | `store/native.rs`: files, at the current paths | `store/web.rs`: IndexedDB with an in-memory copy, on the UI thread and in the engine worker |
| HTTP (no own trait: a `reqwest::Client`, with the body limit and timeouts set by the caller) | `reqwest` on a current-thread `tokio` runtime | `reqwest` on `fetch`, in the window or the worker scope. `AbortController` gives the timeouts. |
| Loops (`Refresher`, `Tracker`) | The loop body is an `async fn`. Its thread runs it with `tokio` `block_on`, in place of `pollster`. | `wasm_bindgen_futures::spawn_local` |
| Timers | `futures-timer` (one global helper thread) | `futures-timer` with its `wasm-bindgen` feature (`gloo-timers` under it) |

**Storage split on web**

| Value | Store |
| :--- | :--- |
| Config (settings, avoid lists, theme, favourites, Nexum URL, key and map ID) | `Store`, owned by the UI |
| Character list, active route, bridge text | `Store`, owned by the UI |
| Refresh tokens (`TokenStore`) | `Store`, owned by the UI |
| PKCE verifier and state, during a login | `sessionStorage`, through `gloo-storage` |
| SDE blob, Nexum cache, EVE-Scout cache | `Store`, owned by the engine |

**Native-only code to gate.** These crates and calls do not build or do not work on `wasm32-unknown-unknown`. Each goes to a `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` table, or behind a seam:

| Item | Where |
| :--- | :--- |
| `tokio` (the runtime only, `reqwest` gates its own native parts) | The loop threads and the portrait load |
| `std::thread` and blocking channels | `refresh.rs`, `sources/nexum.rs`, `sources/evescout.rs`, `esi/tracker.rs`, `esi/pilots.rs`, `esi/sso.rs`, `app.rs`, `pilots_view.rs`, `settings_window.rs`, `view.rs` |
| `std::fs` and `&Path` arguments | `config.rs`, `sources/mod.rs`, `esi/store.rs`, `esi/active.rs`, `overlay.rs`, `sde.rs`, `sde_update.rs`, `ships.rs`, `wormhole_types.rs` |
| `Instant::now`, `SystemTime::now` | `wormhole.rs`, `esi/tracker.rs`, `esi/sso.rs`, `esi/client.rs`, `startup.rs`, `app.rs`, `view.rs` |
| `std::thread::sleep` (frame limiter) | `app.rs`. The browser paces frames, so the limiter is native only. |
| `keyring`, `dirs`, `webbrowser`, `tiny_http`, `mimalloc`, `std::env::args` | `esi/store.rs`, `config.rs`, `esi/sso.rs`, `main.rs`, `app.rs` |
| `eframe` features `wayland` and `x11` | `router_egui/Cargo.toml` |

**Web-only needs**

- `chrono` with the `wasmbind` feature, for `chrono::Local` in `log.rs` and `wormhole.rs`.
- `getrandom` 0.2 with the `js` feature. `oauth2` pulls it in through `rand` 0.8. It is the only `getrandom` version left on wasm after the gating above.
- `wasm-bindgen`, `wasm-bindgen-futures`, `web-time`. `web-sys` and `js-sys` only where no crate of section 7.1 covers the API.
- `gloo-storage`, `indexed_db_futures`, and `futures-timer` with the `wasm-bindgen` feature. Without that feature, `futures-timer` on wasm32 uses its native code, which starts a thread and fails.
- `wasm-bindgen-rayon`, in `router_engine` behind the `threads` feature only.

Add each dependency with `cargo add`.

### 7.1 Crates

Decided on 2026-10-10. Versions are the newest on that date. Do not write a wrapper where a crate below already does the job.

**Use**

| Crate | Job | Notes |
| :--- | :--- | :--- |
| `reqwest` 0.13 | All HTTP, both targets | Replaces `ureq`. On wasm: `fetch` from the global scope (so it works in a Web Worker), `timeout` through `AbortController`, `bytes_stream`, and custom headers, so explicit `Range` values work. The default TLS is `rustls` with `aws-lc-rs`, which needs a C toolchain and CMake on some hosts. Plan task 7 decides between that and `rustls-no-provider` with `ring`, and builds on each release target. |
| `tokio` | Native runtime | Feature `rt` only. One current-thread runtime for each loop thread, entered with `block_on`. |
| `futures-timer` 3.0 | Timer futures, both targets | Native: pure `std`. wasm: the `wasm-bindgen` feature. |
| `rkyv` | The blob and the engine messages | With validation (`bytecheck`). No container format of our own. |
| `indexed_db_futures` 0.6 | IndexedDB | Inside `store/web.rs` only, on the UI thread and in the engine worker. Futures in place of IndexedDB callbacks. |
| `gloo-storage` 0.4 | `sessionStorage` | The login state only (section 8.2). UI thread only. |
| `flume` 0.12 | All channels: the engine outbox (section 4.3), and the control channels of `Refresher` and `Tracker` | `default-features = false, features = ["async"]`. One type gives `try_recv` (the UI, each frame), `recv` and `recv_timeout` (native threads) and `recv_async` (async loops), on both targets. Speed is not the reason: the outbox carries a few messages each second. It uses `std::sync::Mutex`. In an atomics build, a contended lock blocks with `Atomics.wait`, which a browser forbids on the main thread. Thus never call a blocking `flume` method on the main thread of an atomics build. The UI wasm has no atomics, and the atomics build runs only in Web Workers, so the current layout obeys this rule. |
| `web-time` | Clock | Section 7, clock seam |
| `wasm-bindgen-rayon` | Pool threads | `router_engine`, `threads` feature only |
| `oauth2` 5.0 | PKCE, code exchange, refresh, revoke | Kept. See the adapter rule below. |
| `trunk` | UI build and development server | It builds the UI, runs `wasm-bindgen` and `wasm-opt`, and sets the development COOP and COEP headers through `[serve] headers` in `Trunk.toml`. It also builds `data-type="worker"` assets. `engine-mt` needs its own `RUSTFLAGS` and `-Z build-std`, so `build.sh` builds it outside `trunk` (section 11). |

**Do not use**

| Crate | Why not |
| :--- | :--- |
| `ureq` | `reqwest` covers both targets |
| `pollster` | It cannot drive native `reqwest`. `tokio` `block_on` does the same job. |
| `futures::channel`, `std::sync::mpsc` | `flume` gives one channel type for the sync, blocking and async sides. `futures::channel` has only the async side, and `std::sync::mpsc` only the sync side. |
| `gloo-net` | `reqwest` covers HTTP |
| `rexie`, `idb` | `indexed_db_futures` chosen. `rexie` had no release after 2024-08. |
| A storage trait (`dyn Store`) | One concrete type with a file for each target is easier to read (section 5.5). An async trait does not work with `dyn` without boxing. |
| `localStorage` | One `cfg` type cannot hold two wasm backends. IndexedDB works on both threads, so all saved values use it. |
| The `oauth2` `reqwest` feature | It pins `reqwest` 0.12. That gives two `reqwest` versions, or keeps the app on 0.12. `oauth2` had no release after 2025-01. |
| `gloo-worker` (until plan task 1 says otherwise) | Its `Codec` needs serde types, and the default is `bincode` 1.3. `rkyv` bytes then travel in a second encoding. `post_message` sends no transfer list, so each buffer is copied. It pins `gloo-utils` 0.2, but `gloo-storage` 0.4 pins 0.3. Nobody has shown it with `wasm-bindgen-rayon`. |
| `async_zip` | Pre-1.0, and needs an async reader that can seek over HTTP ranges. The current hand-written zip directory read plus streaming `flate2` is simpler (section 5.3). |
| `gloo-console`, `gloo-events`, `gloo-render`, `gloo-dialogs`, `gloo-file` | `egui` owns the canvas, the input and the frame timing. The log window does the job of the console. |
| `gloo-history`, `gloo-utils` | Optional. The login reads the query and calls `replaceState` one time, so plain `web-sys` is enough. |

**Rules for async code**

1. Never put `#[tokio::main]` on `main`. `eframe` must own the main thread, and macOS requires it.
2. The UI thread never waits on a future. Background work sends results on a channel. The UI calls `try_recv` each frame, and the sender calls `request_repaint`, as now.
3. Shared async code in `router_core` must not require `Send`. On wasm, `reqwest` futures hold a `JsValue` and are not `Send`. Thus no `tokio::spawn` on a multi-thread runtime, and no `Send` bound on shared async functions.
4. On wasm, `futures_timer::Delay` sits in a `SendWrapper`, and it panics when polled on another thread. Keep timers in the engine worker loop or on the UI thread, never on a `rayon` pool thread.
5. The `oauth2` adapter is one closure. `oauth2` 5.0 implements `AsyncHttpClient` for any `Fn(HttpRequest) -> impl Future<Output = Result<HttpResponse, E>>`. The closure converts the request, calls `reqwest::Client::execute`, and copies the status, the headers and the body (with the `MAX_BODY` limit). It replaces the `ureq` `Http` struct in `esi/sso.rs`. Write no adapter type or trait around it.

---

## 8. Login (PKCE)

The rules come from the CCP SSO documentation and the findings of 2026-10-09 (section 3).

### 8.1 App registrations

- One EVE app holds exactly one callback URL. A callback can use `http` for `localhost` only. Any other host needs `https`.
- Thus the web build needs two apps: production (`https://<APP_DOMAIN>/`) and development (`http://localhost:8080/`). The native app is a third app (`http://localhost:21404/callback`). No two environments share a `client_id`.
- A Cloudflare preview deployment has another origin, so login does not work there. Test login on the development build or on production.
- The build reads the `client_id` from `EVE_ROUTER_CLIENT_ID` with `option_env!`, as native does. CI passes the production value as a secret. A developer sets the development value in the shell. The value is public in the login URL, but it stays out of git.

### 8.2 Flow

1. The user clicks "Login Pilot". The app makes a `code_verifier` (32 random bytes, base64url), the `S256` challenge, and a `state` nonce. It stores the verifier and the state in `sessionStorage`.
2. The app sets `window.location` to the authorize URL: `response_type=code`, `client_id`, `code_challenge`, `code_challenge_method=S256`, `redirect_uri` (the page origin plus `/`), the `esi::SCOPES` scopes, and `state`.
3. SSO sends the browser back to `/?code=<CODE>&state=<STATE>`.
4. At start, the app reads the query. It compares `state` with `sessionStorage`. A mismatch stops the login.
5. The app sends a form `POST` to the token URL: `grant_type=authorization_code`, `client_id`, `code`, `code_verifier`. No secret.
6. The app removes the query with `history.replaceState`, runs the existing JWT checks (issuer, audience, expiry, subject), and saves the refresh token in the `TokenStore`, which sits on the `Store` (section 5.5).
7. The tracker starts for the character.

Both builds give `oauth2` the closure of section 7.1, rule 5, on a `reqwest::Client`. The native client sets `redirect::Policy::none()`. A browser `fetch` follows redirects, and `reqwest` on wasm cannot stop that. The web build keeps the PKCE helpers, `parse_callback` and `read_claims`. It does not build `Listener`, `tiny_http` or `webbrowser`. "Remove" revokes the token at `REVOKE_URL`, as on native.

### 8.3 Risks

- Any script on the page can read a refresh token in IndexedDB. The controls: the strict CSP (section 10), no third-party scripts, and a clear "Remove" that revokes the token.
- The Nexum key sits in IndexedDB too, and it crosses our edge on each proxied request. It has the same exposure as a refresh token.
- Two tabs can poll the same character and race on a token refresh. See section 12.

---

## 9. Nexum proxy

A Cloudflare Worker script at the same origin, path `/api/proxy/nexum`. Same-origin requests need no CORS headers and no `OPTIONS` handler.

1. The request carries the target URL as a query parameter, and the user key in `Authorization`.
2. Accept `GET` only.
3. The target must use `https` and a host name. Refuse an IP literal and `localhost`.
4. Allow only these paths: `/api/v1/maps`, `/api/v1/maps/<id>`, `/api/v1/maps/<id>/systems/<id>/signatures`.
5. Cap the response at 64 MB, the `MAX_BODY` of `sources/mod.rs`.
6. Drop cookies in both directions. Never log the `Authorization` header.
7. Rate-limit each client. The limit must allow one map fetch plus one signature request for each wormhole system of a large alliance map, 8 at a time.
8. Never send `Access-Control-Allow-Origin`. A `*` would let any site use the proxy with a user key.

The settings window says in plain words, next to the key field, that the web build sends the key through the project's Cloudflare edge. Native still calls Nexum directly.

---

## 10. Hosting and headers

The `_headers` file sets these headers on every response:

| Header | Value |
| :--- | :--- |
| `Cross-Origin-Opener-Policy` | `same-origin` |
| `Cross-Origin-Embedder-Policy` | `require-corp`. If a portrait fails in a real isolated page, use `credentialless`, but Safari does not support it. |
| `Content-Security-Policy` | `default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; connect-src 'self' https://developers.eveonline.com https://images.evetech.net https://esi.evetech.net https://login.eveonline.com https://api.eve-scout.com; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'` |
| `Cross-Origin-Resource-Policy` | `same-origin` |
| `Cache-Control` | `immutable, max-age=31536000` for hashed files. `no-cache` for `index.html`. |

The app opens no popup (SSO uses a redirect), and it has no third-party embed, so COOP and COEP break nothing known. No user-chosen host appears in `connect-src`, because Nexum uses the proxy.

The development server sends the same COOP and COEP headers, so `engine-mt` runs in development too.

---

## 11. Build layout

```
crates/
  router_core/     + engine protocol, store/ (native.rs, web.rs), SdeBlob, streaming distill
  router_egui/     + lib.rs (#[wasm_bindgen(start)] -> WebRunner), engine client, map mirror
  router_engine/   new: cdylib, the engine worker entry and message loop
web/
  index.html       canvas, loader that picks engine-mt or engine-st
  _headers
  proxy/           the Cloudflare Worker script for Nexum
  build.sh         builds ui, engine-mt and engine-st into dist/
  Trunk.toml       dev-server headers, post_build hook for the engines
  wrangler.toml
```

- `web/build.sh` runs `trunk build` for the UI, and `cargo build` plus `wasm-bindgen --target web` for both engine builds. `trunk serve` is the development server. `Trunk.toml` sets the COOP and COEP headers of section 10 in `[serve] headers`, and a `post_build` hook runs the two engine builds into `$TRUNK_STAGING_DIR`, so `trunk serve` and `trunk build` both give a full `dist/`. The `RUSTFLAGS` for atomics apply to the `engine-mt` command only, never through `.cargo/config.toml`.
- `wasm-opt`: try `-O3` and `-Os`. Keep `-Os` unless it slows top 20 by more than 10 percent.
- Size budget: set after the first build, then enforce in CI. Serve with Brotli.

---

## 12. Open questions

| Question | Why it matters | Closed by |
| :--- | :--- | :--- |
| Does iOS Safari accept the shared memory? Does Safari run the pool? | Decides if `engine-mt` ships on Safari, or if Safari gets `engine-st` | Plan task 14 |
| Does a portrait pass COEP `require-corp` in a real isolated page, in each browser? | Decides `require-corp` against `credentialless` | Plan task 18 |
| Do Chrome and Safari behave as Firefox did for CORS? | Only Firefox is tested | Plan task 18 |
| Peak memory of the distill in a browser | Sets the shared memory maximum | Plan tasks 11 and 14 |
| Two tabs poll one character | Doubles ESI traffic, and can race on a token refresh. Candidate: the Web Locks API, so one tab runs the tracker. | Plan task 17 |
| Do refresh tokens rotate? | Matters for two tabs | Plan task 17 (a real login) |
| The error shape of a failed code exchange | Only a fake code is tested (500, HTML) | Plan task 17 |
| Is `router.smrkn.com` the final production domain? | The production callback must match it exactly | Before plan task 0 |
| Does `gloo-worker` carry `rkyv` bytes and start the `wasm-bindgen-rayon` pool? | If yes, it can replace the hand-written worker loop (section 7.1) | Plan task 1 |
| Proxy rate limit against a large alliance map | Only a 160 KB map is tested | Plan task 16 |
