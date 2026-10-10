# Web client: plan

Status: draft 3, 2026-10-10. Draft 2 adds the crate choices of spec section 7.1. Draft 3 adds task 4a and the `Store` layout. The tasks that make `router_egui` run in a browser. The requirements and the reasons are in [spec.md](spec.md). Section numbers below point into that document.

## Rules for every task

1. Each task ends in one commit, or a short series of commits on one branch.
2. `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` pass after each task. Native behavior does not change.
3. A seam lands on native first. The native tests prove it before any web code uses it.
4. Add each dependency with `cargo add`.
5. Estimates are working days for one developer who knows the code. They are rough. Re-estimate after task 1 and after milestone M1.

## Milestones

| Milestone | After task | What a user sees |
| :--- | :--- | :--- |
| M1: planner in a browser | 13 | The page loads, syncs the SDE, and plans gate and bridge routes. One engine thread. |
| M2: threads | 14 | The same, with the pool on isolated pages. |
| M3: live wormholes | 16 | EVE-Scout and Nexum wormholes, refreshed each 5 minutes. |
| M4: parity | 17 | Login, tracker, active route and portraits. |
| M5: public | 18 | Deployed to the production domain, tested in all target browsers. |

## Order

```
 0 (owner, any time before 17)
 1 spike ──────────────────────────────────────────────┐
 2 clock ─┐                                            │
 3 store ─┼─> 7 http ─> 8 loops ─> 9 gate ─> 10 engine ─> 11 shell ─> 12 build ─> 13 bridges  (M1)
 4 distill ─> 4a hulls ─> 5 blob ─> 6 map + protocol ─┘                                │
                                                                    14 threads (M2) <──┤
                                                                    15 scout ─> 16 nexum (M3)
                                                                    17 sso (M4) ─> 18 deploy (M5)
```

Tasks 2, 3 and 4 do not depend on each other. Task 4a needs no other task, but must land before task 5. Task 1 can run beside them.

---

## Tasks

### Task 0: owner setup (no code)

The project owner does this. Tasks 1 to 16 do not need it.

- Confirm the production domain (`router.smrkn.com` is likely, but not final).
- Register two EVE apps with the four scopes of `esi::SCOPES`: production with callback `https://<APP_DOMAIN>/`, and development with callback `http://localhost:8080/`.
- Add the production `client_id` to CI as the `EVE_ROUTER_CLIENT_ID` secret.
- Make the Cloudflare project and the deploy token.

**Done when:** both `client_id` values exist, and CI has the secret and the token.

### Task 1: thread spike (about 1.5 days)

The largest unknown goes first. Work on a scratch branch. Do not merge it.

1. Make a scratch `cdylib` that includes `route.rs`, `universe.rs`, `sde.rs`, `ships.rs`, `wormhole.rs` and `ansiblex.rs` by `#[path]`, and the repository `sde/` files by `include_bytes!`.
2. Build it for `wasm32-unknown-unknown` with a pinned nightly, atomics, `build-std` and `wasm-bindgen-rayon`.
3. Load it in a Web Worker on a page with COOP and COEP. Time top 5 and top 20 from Jita to ND-X7X with 1, 2, 4 and 8 threads.
4. Run it in Chrome, Firefox and Safari, and in iOS Safari if a device is available.
5. Start the worker two ways: a hand-written loop with a transferred `ArrayBuffer`, and `gloo-worker` with a custom `Codec` that carries the `rkyv` bytes. Each one must start the `wasm-bindgen-rayon` pool before the first search.

**Done when:** a table with the times and a pass or fail for each browser is in spec section 6. Pin the nightly that worked. Spec section 7.1 records the `gloo-worker` result: keep it only if it starts the pool in Chrome and Firefox, else keep the hand-written loop. If no browser gets a 2-times gain at 4 threads, stop and talk to the owner before task 14.

### Task 2: clock seam (about 0.5 day)

Use `web_time::Instant` and `web_time::SystemTime` in place of the `std` types in `router_core` and `router_egui`. Cover `wormhole::now`.

**Done when:** no `std::time::Instant` or `std::time::SystemTime` is left outside tests. Native tests pass.

### Task 3: storage seam (about 2 days)

1. Add `router_core::store` with `mod.rs` and `native.rs` (spec section 5.5). `Store` has `async fn open`, `fn get`, `fn set`, `fn delete` and `async fn flush`. On native, `open` and `flush` return at once. Write no trait.
2. Move these onto it: `config.rs`, the source caches in `sources/mod.rs`, the character list in `esi/store.rs`, `esi/active.rs`, and the bridge file of `overlay.rs`. Functions take a `&Store` and a key, not a `&Path`. `TokenStore` stays as it is on native.
3. Write the key rules into the module doc of `mod.rs`.
4. Tests keep using temp directories.

**Done when:** no `&Path` argument is left in this logic. The file paths on disk do not change. No caller of `get`, `set` or `delete` is async. Native tests pass.

### Task 4: SDE as bytes, and the streaming distill (about 2 days)

1. Change `sde::load(&Path)` to `sde::parse` on byte slices. A thin native wrapper reads the files.
2. Split `sde_update::download` into three steps: fetch the compressed entries, distill, write the files.
3. Write the streaming distill of spec section 5.3: inflate each entry as a stream, keep only the needed rows, and check the CRC-32 and the size at the end. It replaces `ships::parse` and the full `Sources` struct.
4. Measure the peak memory of an update on native, before and after.

**Done when:** a native test runs the distill on the fixture zip and gets the same `ships.json` and `wormholes.json` as the old code. The `regenerate_repo_sde` test gives files identical to the ones in `sde/`. The peak memory numbers are in spec section 5.3.

### Task 4a: hull table owned by the map (about 1 day)

The reasons are in spec sections 2 and 4.3.

1. Add `HullRef` (`Ship(type_id)` or `Group(group_id)`, spec section 4.3). Replace `HullClass = &'static Hull` with it in `ansiblex.rs`, `settings.rs`, `labels.rs`, `esi/pilots.rs` and `settings_window.rs`. Keep `group_id` on each `Hull`, from `ships.json`.
2. Remove the `static TABLE: OnceLock`, `ansiblex::init` and `ansiblex::table`. Put an `Arc<HullTable>` in `Universe`. `BridgeRules::cost` and the other readers take it from the `Universe`.
3. Make `same_hull` and `same_class` compare IDs, not pointers.
4. Keep the hull name in the config file. A ship name resolves to `Ship`, and a group name or key resolves to `Group`.
5. Make sure that the picker and a followed pilot always give `Ship`.

**Done when:** no `OnceLock` and no `&'static Hull` are left in `router_core`. A test builds two `Universe` values with different hull tables in one process. A test gives two ships of one group with different masses, and expects each ship's own mass, not the group maximum. The config file format does not change. Native tests pass.

### Task 5: the blob (about 1.5 days)

1. Add `rkyv` with validation. Add `SdeBlob` (spec section 5.4) and `SdeBlob::from_parts`.
2. Add `Universe::from_blob`. It builds the `HullTable` from the ship table of the blob.
3. Add the format version and the validation rules.

**Done when:** a native test builds a blob from `sde/`, and `Universe::from_blob` gives the same graph as `Universe::from_sde`: 8490 systems, 13978 stargates, the same names, and the same edge list in the same order. A test flips one byte and expects a validation error. A test with a wrong version expects a rebuild. The blob size is in spec section 5.4.

### Task 6: pure map build and the engine protocol (about 1 day)

1. Split `refresh::build` into a pure `build_map(base, sources, now)` that the startup, the refresh and the UI mirror all call.
2. Add `Universe::fingerprint` (a hash of the node and edge lists, and of the `HullTable`).
3. Add `router_core::engine`: the messages of spec section 4.3, and the `rkyv` encoding. `Settings` derives `rkyv` directly.
4. Add an in-process `Engine` that answers the messages on native. Tests drive it.

**Done when:** a test builds a map two times from the same inputs and gets the same fingerprint. A test sends a `Route` message to the in-process engine, and the reply routes match `Settings::router(...).routes(...)` on the mirror. Native tests pass.

### Task 7: move HTTP to `reqwest` (about 2 to 3 days, the largest task)

1. Add `reqwest` 0.13 and `tokio` (feature `rt`). Pick the TLS features (spec section 7.1): the default `aws-lc-rs`, or `rustls-no-provider` with `ring`. Build on each release target before you pick.
2. Convert `sources/mod.rs`, `sources/evescout.rs`, `sources/nexum.rs` (the `SIG_WORKERS` threads become `buffer_unordered(8)`), `esi/client.rs`, `sde_update.rs` (an async `RangeSource` that keeps `HEAD` and explicit `start-end` ranges), and the portrait load. Each blocking caller enters a current-thread `tokio` runtime with `block_on`.
3. In `esi/sso.rs`, replace the `ureq` `Http` struct with the `oauth2` closure of spec section 7.1, rule 5.
4. Keep the 5, 10 and 30 second timeouts, the `MAX_BODY` limit, and no redirects on the token endpoint.
5. Remove `ureq` from `Cargo.toml`.

**Done when:** the workspace has no `ureq`. No shared async function has a `Send` bound. The local test servers still pass every test.

### Task 8: async loop bodies (about 2 days)

Make the loop bodies of `Refresher` and `Tracker` async functions. Use `futures_timer::Delay` for the intervals. On native, the current threads run them with `tokio` `block_on`, and the control channels and intervals do not change.

**Done when:** native tests pass, and the loop bodies use no blocking call.

### Task 9: target gating (about 1 day)

1. Move the native-only dependencies and features of spec section 7 into `cfg(not(target_arch = "wasm32"))` tables. Gate `#[global_allocator]`, the frame limiter, `std::env::args`, `Listener`, `Keyring` and the file store.
2. Add `chrono/wasmbind`, `getrandom` 0.2 with `js`, and `futures-timer/wasm-bindgen` for wasm. Make `tokio` native-only.
3. Add a CI job: `cargo check` and `cargo clippy` for `router_core` and `router_egui` on `wasm32-unknown-unknown`.

**Done when:** both checks are green in CI, on stable. Record the size of the `cfg` gates in `router_core`, and decide whether to move `sde_update`, `sources` and `store` to a `router_data` crate (spec section 2). Split only if the engine build can then drop `esi` and its dependencies, and the gates are hard to read.

### Task 10: the engine worker, single thread (about 3 days)

1. Add the `router_engine` crate (`cdylib`) with a `#[wasm_bindgen]` worker entry and a message loop.
2. Add `store/web.rs` with `indexed_db_futures`: the same public API as `native.rs`, with the in-memory copy and the background writes of spec section 5.5. The engine awaits `flush` after a blob write. `reqwest` already works in the worker scope.
3. Implement the SDE start of spec section 5: read the blob, else download, distill and write the full blob (section 5.2). Send `Progress`, then `Blob` and `Map`.
4. Answer `Route` messages with the drop rules of spec section 4.4.

**Done when:** a `wasm-bindgen-test` run in headless Chromium syncs the fixture zip from a local server, writes the blob, reads it again on a second start, and answers one `Route` message.

### Task 11: the UI shell (about 3 days)

1. Add `router_egui/src/lib.rs` with a `#[wasm_bindgen(start)]` entry that calls `eframe::WebRunner`.
2. Add `web/index.html` and the loader. The loader picks `engine-st` for now.
3. Add the engine client: post messages, keep the newest request ID, apply `Map` to the mirror, compare fingerprints.
4. Show the SDE progress on the splash screen.
5. Open the `Store` in the async start function, before `WebRunner` starts, and load the config from it.
6. Measure the first load in Chrome: download time, distill time, and the peak memory of the engine worker.

**Done when:** the planner runs in a browser with the real CCP SDE, and a reload reads the blob and sends no SDE request except `latest.jsonl`. The measurements are in spec section 5.3.

### Task 12: build script and smoke test (about 1.5 days)

1. Write `web/build.sh` and `Trunk.toml` (spec section 11). Pin the nightly in `build.sh`.
2. Set the COOP and COEP headers of spec section 10 in `[serve] headers`, so `trunk serve` is the development server. Write no server of our own.
3. Add a CI job that builds `dist/` and runs a Playwright test: open the page, wait for the planner, enter Jita and Amarr, and expect a route.
4. Record the compressed wasm sizes and set the size budget.

**Done when:** the CI job is green and enforces the size budget.

### Task 13: bridges and settings on web (about 1 day)

1. Add a paste box for the SMT bridge list. Save the text in the `Store`. Send it to the engine with `SetBridges`.
2. Check that settings, avoid lists, favourites and theme survive a full reload.

**Done when:** a pasted bridge list survives a reload and shows in the Shortcuts box. This is milestone M1.

### Task 14: threads (about 2 days, plus browser time)

1. Add the `threads` feature to `router_engine` with `wasm-bindgen-rayon`. Build `engine-mt` with the flags of spec section 6.1.
2. The loader reads `crossOriginIsolated` and picks `engine-mt` or `engine-st`. A failed pool start uses `engine-st`.
3. Run searches with `rayon::spawn`, and add the shared cancel flag of spec section 4.4.
4. Measure the targets of spec section 6.4 in Chrome, Firefox and Safari. Set the pool size and the shared memory maximum.

**Done when:** the targets pass in Chrome and no target browser fails. Else ship `engine-st` only, and record why. The numbers are in spec section 6. This is milestone M2.

### Task 15: EVE-Scout and the refresh (about 1 day)

Run the `Refresher` in the engine worker with `spawn_local` and `futures_timer::Delay`. Fetch EVE-Scout directly. Keep its cache in IndexedDB. Send a `Map` message after each refresh. Wire `F5` to `Refresh`.

**Done when:** EVE-Scout wormholes show in a browser and refresh each 5 minutes. The `Log` window shows each fetch.

### Task 16: Nexum proxy (about 2 days)

1. Write the Cloudflare Worker script of spec section 9, with unit tests: it refuses a non-`GET` request, an IP literal, `localhost`, a path outside the allowlist, and an oversized response.
2. Send all web Nexum requests to `/api/proxy/nexum`.
3. Add the key notice to the settings window.
4. Finish `_headers` with the CSP of spec section 10.
5. Test the rate limit against the largest Nexum map available.

**Done when:** a Nexum map loads through the proxy, and the browser console shows no CSP error and no direct call to a Nexum host. This is milestone M3.

### Task 17: login, tracker and portraits (about 3 days)

Needs task 0.

1. Add the web login of spec section 8.2, and a `TokenStore` on the `Store`. Keep the login state in `sessionStorage` with `gloo-storage`.
2. Run the `Tracker` on the UI thread with `spawn_local`.
3. Load portraits with `reqwest` (CORS mode by default) and decode them with `image`.
4. Decide the two-tab rule (spec section 12). The candidate is a Web Lock around the tracker.
5. Record the code-exchange error shape and whether refresh tokens rotate, from a real login.

**Done when:** a pilot logs in on the development build, shows live on the route, gets waypoints, and "Remove" revokes the token. A wrong `state` stops the login. This is milestone M4.

### Task 18: deploy and browser matrix (about 1.5 days)

1. Add a deploy workflow to Cloudflare with `wrangler`, for the production branch or tag.
2. Test the deployed page in Chrome, Firefox, Safari and iOS Safari: the SDE fetch and ranges, ESI, the token exchange, portraits under COEP, and `engine-mt`.
3. Add a "Web" section to the README.

**Done when:** each browser passes, or its gap is in spec section 12 with a decision. This is milestone M5.

---

## Total

About 33 working days, so 6 to 7 weeks for one developer. Task 7 and the browser testing of tasks 1, 14 and 18 carry the most risk.
