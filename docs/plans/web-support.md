# Plan: Web Application Support (WASM Target & Cloudflare Worker)

Status: Draft 3. Revised after a code review and benchmark runs on 2026-10-09. Plan for compiling `router_egui` to `wasm32-unknown-unknown` and deploying it with Cloudflare Workers.

**Terms.** "Web Worker" is a browser thread. "Cloudflare Worker" is code that runs at the edge. This document never uses "Worker" alone.

The work is not one step. Section 6 splits it into tasks. Each task ends in a commit, and the native tests stay green after each one.

---

## 1. Executive Summary & Goals

Compiling the EVE Router application to WebAssembly (`wasm32-unknown-unknown`) lets pilots use the route planner, the live wormhole network, and pilot tracking in a modern web browser, with no local install.

### Key Architectural Decisions

- **Frontend & rendering:** `eframe::WebRunner` with WebGL2 (`glow`) into an HTML5 `<canvas>`. `wgpu` adds size and gives no gain for a panel UI.
- **Compute:** three stages (section 5.6). Stage 1 is single-threaded wasm on the UI thread. The existing `rayon` calls run unchanged, because `rayon-core` falls back to one thread on this target. Stage 2 moves the route engine into a Web Worker. Stage 3 adds `wasm-bindgen-rayon` threads inside that Web Worker, only if a spike proves the gain. `+simd128` gives no gain and is not used.
- **Client-side SDE:** range streaming from CCP (`developers.eveonline.com`), then a **distilled `rkyv` blob** (a flat map record, not the `Universe` graph) cached in `IndexedDB` (section 2.1).
- **Seams, not gates:** three traits make one logic path for native and web: `Clock`, `Storage`, and an async `Http` (section 5).
- **Edge gateway:** Cloudflare Workers Static Assets for the bundle and headers. A small Cloudflare Worker script serves only the Nexum proxy. The web build sends all Nexum traffic through it, because a tested Nexum server allows only its own origin (section 2.4). Decision of the project owner, 2026-10-09: proxy only, no direct Nexum call.
- **Portraits:** direct load from `images.evetech.net`, no proxy.
- **Authentication:** client-side PKCE with its own EVE app registration (section 3).
- **State:** `localStorage` for small settings and tokens, `IndexedDB` for large data.

### Changes from Draft 1

| Topic | Draft 1 | Draft 2 | Reason |
| :--- | :--- | :--- | :--- |
| Parallelism | `wasm-bindgen-rayon` from the start | Three stages: single thread, then a Web Worker, then optional threads | One thread is fast enough for top 5 (30 ms in wasm). Threads give a 3 times gain on top 20 of a long route, so they stay as a staged option behind a spike (section 5.6). `+simd128` gives no gain. |
| SDE cache | `rkyv` of the `Universe` | `rkyv` of a distilled flat record | `Universe` holds a `petgraph` graph and `HashMap`s, so it cannot archive as it is. A flat record can. |
| Native-only code | Gate `mimalloc`, `keyring`, `dirs`, `fs`, `Instant` | Add `Clock`, `Storage`, `Http` seams | Gating removes persistence. The list also missed `ureq`, `SystemTime`, threads, and more. |
| Network | Async client | One async `Http` trait, native impl on `ureq` | Blocking calls and threads sit in about 10 files. |
| Nexum proxy | `?url=` relay, `OPTIONS`, `ACAO: *` | Allowlisted same-origin proxy | The old design is an open relay with no need for CORS. A tested Nexum server allows only its own origin, so the proxy is needed (section 2.4). |
| Hosting | A Cloudflare Worker serves everything | Static Assets plus `_headers` | Headers need no code. |
| Phases | 4 phases | 10 tasks | Each task fits in one commit. |

---

## 2. Architecture & Data Flow

### 2.1 Client-Side SDE Lifecycle & the Distilled Blob

The router needs about 8.5 MB of JSONL (`mapSolarSystems`, `mapStargates`, `mapRegions`, `_sde`). A start parses all of it. Native single-thread cost is 15 ms for the parse and 3 ms for the graph build. The web build caches a distilled blob instead, so a later start skips the JSON parse and the 8.5 MB read.

**The blob (`SdeBlob`)** is a flat record that derives `rkyv::Archive`:

- a format version, and the SDE build number,
- the region names,
- the systems: ID, name, region index, security, position,
- the stargates, as pairs of system indexes.

It does not hold the `Universe`. The `Universe` has a `petgraph` graph, `HashMap`s, and the `Link::Wormhole` enum. A refresh clones the base `Universe` and adds wormholes. A flat record gives `rkyv` something it can archive, and `Universe::from_blob` builds the graph from the archived slices.

`ships.json` and `wormholes.json` are small (about 60 KB). They stay as JSON under their own `IndexedDB` keys.

**What "zero-copy" means here.** A browser has no `mmap`. The app reads the blob from `IndexedDB` into an aligned buffer (`rkyv::util::AlignedVec`, one copy of a small buffer) and reads the archive in place. This skips the JSON parse. It does not skip the graph build (3 ms native). The blob size is not measured yet. Task 4 measures it. Expect well under 1 MB.

**Rules for the blob**

1. Check the bytes with `rkyv` validation (`bytecheck`) before the app reads them. `IndexedDB` data is not trusted.
2. Store a format version in the blob and in the key name. A wrong version or a failed check deletes the blob and starts a download. The raw SDE can always rebuild it.
3. The same code builds the blob on native and on web (`SdeBlob::from_files`). A native test makes the blob from the repository `sde/` files and compares `Universe::from_blob` with `Universe::from_sde` (8490 systems, 13978 stargates, same names).
4. Add the dependency with `cargo add rkyv`.

**Startup**

1. Read the blob and the build number from `IndexedDB`. If they exist, build the `Universe` and show the planner at once.
2. In the background, `GET` `latest.jsonl` (at most once each 5 minutes, see section 2.3). If the build matches, stop. No more traffic.
3. If the build is new, or no blob exists, stream the ZIP:
   - `HEAD` for the length. Read the central directory (about 65 KB) with an explicit `bytes=start-end` range.
   - Fetch the needed entries (about 1.8 MB compressed): the four map files and the ship source files (`types.jsonl`, `typeDogma.jsonl`, `groups.jsonl`).
   - Inflate with `flate2` (`miniz_oxide`). Check size and CRC-32, as `extract` does now.
   - Derive `ships.json` and `wormholes.json`. Build the `SdeBlob`. Write all of it to `IndexedDB`. Write the build number last, so an interrupted update leaves the old build.
   - Swap the active `Universe` when the new one is ready.

The decompress and parse run on the main thread at first. Task 7 measures the first-run pause and the peak memory. The ship source files are the unknown, because `download()` inflates them as well. Add a Web Worker for this job only if the pause is over about 100 ms. If stage 2 of section 5.6 exists, reuse that Web Worker. A Web Worker needs its own wasm instance.

```
[ App Launch ]
       |
       +-------------------------------------+
       v (instant path)                       v (background)
Read SdeBlob from IndexedDB          GET latest.jsonl (CORS ok, max-age 300)
Validate, build Universe                      |
       |                                      v
   planner ready                      build == stored build?
                                      |-- yes --> done, 0 bytes
                                      '-- no  --> HEAD zip, range-read directory (~65 KB)
                                                   |
                                                   v
                                          range-read ~1.8 MB of entries
                                                   |
                                                   v
                                    inflate, check CRC, derive tables,
                                    build SdeBlob, write IndexedDB
                                                   |
                                                   v
                                           swap the active Universe
```

### 2.2 Cloudflare Edge Gateway

**1. Static assets and headers (no code).** Cloudflare Workers Static Assets serves `index.html`, the wasm, and the JS glue. A `_headers` file sets the response headers:

- A strict `Content-Security-Policy`: same-origin scripts, `'wasm-unsafe-eval'`, and a fixed `connect-src`: `'self'` (this covers the Nexum proxy), `https://developers.eveonline.com`, `https://images.evetech.net`, `https://esi.evetech.net`, `https://login.eveonline.com`, and `https://api.eve-scout.com`. No user-chosen host appears, because all Nexum traffic uses the same-origin proxy. The CSP limits the harm of a stolen script, because the refresh tokens and the Nexum key sit in `localStorage` (section 3).
- `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp`. Stages 1 and 2 of section 5.6 do not need them. Stage 3 (threads) does. Turn them on from the first web build, so any COEP problem (section 2.3 item 5) shows early and not at stage 3. The app has no popups (SSO uses a redirect) and no third-party embeds.

**2. Nexum proxy (Cloudflare Worker script).** Nexum uses a bearer key, so a browser `GET` triggers a preflight. A tested Nexum server answers the preflight but allows only its own origin (section 2.4). A browser at our origin cannot read its responses. The web build therefore sends every Nexum request (the map, the map list, and the signatures) through the proxy. There is no direct path. This keeps the CSP fixed (see above). The native build still calls Nexum directly with `ureq`.

- The proxy is same-origin (`/api/proxy/nexum`). A same-origin `fetch` needs no CORS headers and no `OPTIONS` handler. Do not send `Access-Control-Allow-Origin: *`. It would let any website use the relay with a user key.
- The proxy takes the target as a parameter but is not an open relay. It accepts only `GET`. It requires `https` and a host name (no IP literal). It allows only these paths: the maps list, one map, and `systems/<id>/signatures`. It caps the response at 64 MB (the `MAX_BODY` of `sources/mod.rs`). It drops cookies. It never logs the `Authorization` header. It rate-limits each client.
- The key crosses our edge on every request. The settings page must say so in plain words, next to the Nexum key field. The key also sits in `localStorage` (section 3.3 risks), so it has the same exposure as a refresh token.
- All Nexum traffic is on the proxy, so its limits matter: the per-client rate limit and the response cap must allow a map fetch plus one signature request for each wormhole system (`SIG_WORKERS` requests at a time on native). Size the limits against a large alliance map before task 8.

**3. No proxy for CCP endpoints.** The SDE files, the portrait CDN, ESI, SSO, and EVE-Scout allow cross-origin reads (section 2.3). Do not proxy them. Do not add a general fallback proxy.

### 2.3 CORS Findings for CCP Endpoints (checked 2026-10-09)

Method: `curl` with an `Origin` header, with and without `Range`, plus an `OPTIONS` preflight. `curl` does not enforce CORS, so the same checks also ran in a Firefox console, from an `https://` page (second table below). Chrome and Safari are not tested. The page was not cross-origin isolated, so COEP is still untested (section 7).

| URL | `Access-Control-Allow-Origin` | Range | `OPTIONS` preflight | Result |
| :--- | :--- | :--- | :--- | :--- |
| `/static-data/tranquility/latest.jsonl` | `*` | 206 | 403 | OK for a simple `GET` |
| `/static-data/tranquility/eve-online-static-data-<build>-jsonl.zip` | `*` (also on `HEAD`) | 206, `Accept-Ranges: bytes` | 403 | OK with a safelisted `Range` |
| `/static-data/eve-online-static-data-latest-jsonl.zip` | none | n/a | 204, no CORS headers | **Not usable from a browser.** It sends a 302 with no CORS headers. |
| `images.evetech.net/characters/<id>/portrait` | `*` | 206 | 403 | OK for a `fetch` request |

All CCP responses send `Access-Control-Expose-Headers: *`, so `Content-Range` and `Content-Length` are readable. None of them sends `Cross-Origin-Resource-Policy`.

**Consequences**

1. **No proxy for the SDE.** The `GET` responses allow any origin. The Cloudflare Worker does not need to relay them.
2. **Do not send a preflight.** Every `OPTIONS` request returns 403 or no CORS headers. A preflight would block the request. A `Range` header with the form `bytes=<start>-<end>` is CORS-safelisted, so the browser sends no preflight. Do not add other custom request headers (for example `User-Agent` or `Authorization`) to these requests.
3. **Suffix ranges also work on the server, but they trigger a preflight.** A `Range: bytes=-65536` header is not safelisted. The existing `HttpSource` in `sde_update.rs` already uses `HEAD` for the length and explicit `start-end` ranges. The web port must keep this pattern. Do not "optimize" it to a suffix range.
4. **Use the build-numbered ZIP URL.** `sde_update.rs` already builds `.../tranquility/eve-online-static-data-{build}-jsonl.zip` from `latest.jsonl`. Keep it. Never use the `-latest-` URL in the browser.
5. **COEP `require-corp` works for `fetch`, not for a plain `<img>`.** The portrait CDN has no CORP header, but it has `Access-Control-Allow-Origin: *`. A CORS-mode `fetch` passes COEP. An `<img>` tag without `crossorigin="anonymous"` is blocked. egui loads images with `fetch`, so portraits should work. This matters only if a later task turns on COOP/COEP. If a case fails, use `Cross-Origin-Embedder-Policy: credentialless` (not available in Safari).
6. **Cache time.** `latest.jsonl` has `Cache-Control: max-age=300`. The version check can run at most once every 5 minutes and still see a fresh answer. The ZIP has `max-age=86400` and a fixed name for each build.
7. **Send no cookies.** `*` is not valid for a credentialed request. Use `credentials: "omit"` (the `fetch` default for cross-origin).

**Browser results (Firefox console, 2026-10-09)**

| Request | Result | Meaning |
| :--- | :--- | :--- |
| `latest.jsonl` `GET` | readable, 200 | Pass |
| ZIP `HEAD` | readable, 200 | Pass |
| ZIP `Range: bytes=0-99` | readable, 206, `Content-Range` visible | Pass. No preflight. |
| ZIP `Range: bytes=-65536` | blocked | As predicted. The preflight fails. |
| `-latest-` ZIP (302) | blocked | As predicted |
| Portrait `fetch` | readable, 200 | Pass |
| Portrait `<img>`, with and without `crossorigin` | loads | No COEP on the test page, so this proves nothing about COEP |
| ESI `GET /status` | readable, 200 | Pass |
| ESI `GET` with `Authorization` header | readable, 401 | Pass. ESI answers the preflight. |
| ESI `POST` with `Authorization` header (waypoint) | readable, 401 | Pass. ESI answers the preflight for `POST`. |
| SSO `POST /v2/oauth/token`, form body, fake client | readable, 401, HTML body | CORS passes. An unknown client gives an HTML 401. |
| SSO `OPTIONS /v2/oauth/token` | readable, 200 | Preflight answered |
| EVE-Scout `GET /v2/public/signatures` | readable, 200 | Pass |

**ESI results (`curl`, 2026-10-09, stand-in `Origin`)**

The tests send the exact headers of the ESI client: `Authorization` and `X-Compatibility-Date: 2025-08-26` (`esi/client.rs`).

| Request | Result |
| :--- | :--- |
| `OPTIONS` for `GET /characters/<id>/location`, request headers `authorization,x-compatibility-date` | 204. Allows both headers. `ACAO: *`. Methods `GET, POST, PUT, DELETE`. `Access-Control-Max-Age: 86400`. |
| `OPTIONS` for `POST /ui/autopilot/waypoint?...`, same request headers | 204. Same answer. |
| `GET` with a fake token and `X-Compatibility-Date` | 401, JSON, CORS headers present. Sends `X-Esi-Error-Limit-Remain: 99` and `X-Esi-Error-Limit-Reset: 60`. |
| `GET /status` with `X-Compatibility-Date` | 200, with `Expires` |

ESI lists its exposed headers one by one: `Etag, Retry-After, X-Compatibility-Date, X-Esi-Error-Limit-Remain, X-Esi-Error-Limit-Reset, X-Pages, X-Ratelimit-*`. The client reads four headers, and the browser can read all four:

| Header (`esi/client.rs`) | Why it is readable |
| :--- | :--- |
| `Expires` | CORS-safelisted response header |
| `X-Esi-Error-Limit-Remain` | In `Access-Control-Expose-Headers` |
| `X-Esi-Error-Limit-Reset` | In `Access-Control-Expose-Headers` |
| `Retry-After` | In `Access-Control-Expose-Headers` |

**SSO results (`curl`, 2026-10-09, stand-in `Origin`, the real web `<CLIENT_ID>`)**

| Request | Result |
| :--- | :--- |
| `POST /v2/oauth/token`, `grant_type=refresh_token`, `<CLIENT_ID>`, fake token | 400, JSON `{"error":"invalid_grant", ...}`, `Access-Control-Allow-Origin: *` |
| Same request, wrong `client_id` (control) | 401, HTML body |
| `POST`, `grant_type=authorization_code`, `<CLIENT_ID>`, fake code and verifier | 500, HTML body, `Access-Control-Allow-Origin: *` |
| `OPTIONS` preflight (`POST`, `content-type`) | 204, allows `content-type` and `POST`, `ACAO: *` |

**Consequences**

1. **Direct ESI works.** The browser can call ESI with a bearer token, including the waypoint `POST`. The preflight also passes with the exact headers of the ESI client (`Authorization` and `X-Compatibility-Date`), for both `GET` and `POST`. The browser can read the four response headers that the tracker uses. The tracker logic does not change. (The first test sent only `Authorization`. The ESI results table above closes that gap.)
2. **The PKCE token exchange can run in the browser.** A form-encoded `POST` is a simple request, so it needs no preflight. The response is readable. No Cloudflare Worker relay is needed for SSO.
3. **EVE-Scout works directly.** Do not route it through the Cloudflare Worker. Only Nexum uses the proxy.
4. **The `client_id` works with no secret.** A fake refresh token gives `invalid_grant`, not `invalid_client`. This fits a public PKCE app. A real login proves it.
5. **A browser can read every SSO response.** All answers send `Access-Control-Allow-Origin: *`, so the app reads the status even when the body is HTML.
6. **Do not require JSON on an error.** A fake authorization code gave a 500 with an HTML body, probably because the code was malformed. The real error shape of the code exchange is unknown until a real login. The web client must decide by HTTP status, and use the JSON `error` field only when the body parses.
7. **Callback rules (from the EVE app registration, reported by the project owner, 2026-10-09).** A callback URL can use `http` for `localhost` only. Any other host must use `https`, or the `eveauth-app://` scheme. The web origin (`https://<APP_DOMAIN>/`) and a dev origin (`http://localhost:<port>/`) both fit. **One app holds exactly one callback URL.** Thus production and development are two separate registered apps, each with its own `client_id`. The native app is a third app (its callback is `http://localhost:21404/callback`). `curl` cannot test registration. The project owner registers the two web apps in the developer portal (task 0).
8. **Still untested:** Chrome, Safari, and COEP. Nexum is in section 2.4.

### 2.4 CORS Findings for Nexum (checked 2026-10-09)

Method: `curl` with a stand-in `Origin` header (`https://eve-router.example`), a read-only bearer key, and an `OPTIONS` preflight. The server is a self-hosted Nexum instance. This document calls it `https://nexum.example.org`. `curl` does not enforce CORS, so the results show the headers and not the browser verdict. The browser verdict follows from the CORS rules.

| Request | Result |
| :--- | :--- |
| `GET /api/v1/maps/<map-id>` with `Origin` and bearer key | 200, 159,744 bytes (160 KB) |
| `GET /api/v1/maps` | 200 |
| `GET /api/v1/maps/<map-id>/systems/<system-id>/signatures` | 200 |
| `GET` with no key | 401, and the response still carries the CORS headers |
| `OPTIONS` preflight (`GET`, `Access-Control-Request-Headers: authorization`) | **204**. Allows `GET,HEAD,PUT,PATCH,POST,DELETE` and the `authorization` header. Sends `Access-Control-Allow-Credentials: true`. |
| `Access-Control-Allow-Origin` on every response | **`https://nexum.example.org` only** (the server's own origin) |

The preflight returned the same fixed `Access-Control-Allow-Origin` for five `Origin` values: `https://eve-router.example`, `http://localhost:8080`, `https://evil.example`, `null`, and no `Origin`. The server does not echo the caller.

**Consequences**

1. **A direct call from our origin fails.** The browser compares `Access-Control-Allow-Origin` with the page origin. They differ, so the browser blocks the response. The preflight passes, but the real `GET` is unreadable.
2. **The proxy is the only Nexum path on web.** Decision of the project owner, 2026-10-09. Even a server that allowed our origin would need a host in `connect-src`, and the host is user-chosen. A fixed CSP is worth more than a direct path.
3. **The first draft was wrong, with no harm.** Draft 1 said Nexum rejects `OPTIONS` with 405. This server answers it. The same-origin proxy needs no `OPTIONS` handler either way (section 2.2).
4. **Do not rely on an operator change.** If a Nexum operator adds our origin to its CORS allowlist, nothing changes on web. The proxy stays. Reopen this only if the key exposure becomes a problem.
5. **The sizes are small.** The test map is 160 KB, so the 64 MB response cap of the proxy is generous.
6. **Not tested:** a browser run (Chrome, Firefox, Safari) and a large alliance map.

---

## 3. Web SSO & Authentication Architecture (PKCE in egui)

### 3.1 Why Client-Side PKCE

EVE Online SSO v2 supports public clients with **OAuth 2.0 PKCE**.

- No backend state. No database, KV, or token handling on Cloudflare.
- Direct ESI requests from the browser, with the CORS results of section 2.3.

**Sources.** The flow and the parameters come from the CCP SSO page (`developers.eveonline.com/docs/services/sso/`). It confirms: no client secret in the PKCE flow, an exact match of the callback URL, refresh tokens that last until the user revokes access, and the JWT checks (issuer `https://login.eveonline.com/` or `login.eveonline.com`, audience with the `client_id` and "EVE Online"). The page advises a JWKS signature check. `sso.rs` skips it on purpose, because the token comes straight from the token endpoint over TLS, and `fetch` gives the same guarantee. The page does not cover CORS, token error bodies, the number of callback URLs, `localhost` and `http` rules, rate limits, or refresh token rotation. Our own tests (section 2.3) answer CORS and errors. Task 0 answers the callback limits.

### 3.2 What differs from the native login

The native login (`esi/sso.rs`) listens on `http://localhost:21404/callback` with `tiny_http`, and opens the system browser with `webbrowser`. EVE SSO matches the registered callback URL exactly. Thus the web build needs:

1. **Two EVE app registrations, one for each environment.** One app holds exactly one callback URL, so the web build needs its own apps:
   - Production: callback `https://<APP_DOMAIN>/`. The production domain is likely `router.smrkn.com` (project owner, 2026-10-09, not final). Treat it as final only when the project owner confirms.
   - Development: callback `http://localhost:<port>/`, for example `http://localhost:8080/`. The rules allow `http` only for `localhost`. Use the host name `localhost`, not an IP address, and not a LAN name. The dev server must always use the same port.
   - Together with the native app, the project has three apps and three `client_id` values. No two environments share a `client_id`.
   - A Cloudflare preview deployment has a different origin, so SSO login does not work there. Test SSO on the dev build or on production.
   - The `client_id` stays out of the repository, as for native (`EVE_ROUTER_CLIENT_ID`, read with `option_env!`). The web build has no run-time environment, so it reads the value at build time only. CI passes the production value as a secret, and a developer sets the dev value in the local shell for `trunk serve`. The value is public in the login URL, so it is not a secret in the bundle, but keep it out of git.
   - Task 0 registers both apps.
2. **No `Listener` and no `webbrowser` on web.** The login sets `window.location`. Gate `Listener`, `tiny_http`, and `webbrowser` to native.
3. **An async token client.** `sso.rs` uses the `oauth2` `SyncHttpClient` over `ureq`. The web build implements `oauth2::AsyncHttpClient` with `fetch`. Keep the PKCE helpers, `parse_callback`, and all JWT checks (issuer, audience, expiry, subject).
4. **A `TokenStore` for the browser.** `esi/store.rs` already has the `TokenStore` trait. Add a `localStorage` impl.

### 3.3 Flow

1. The user clicks "Login Pilot". The app makes a `code_verifier` (32 random bytes, base64url), the `S256` challenge (the SHA-256 of the verifier, base64url without padding), and a `state` nonce. It stores the verifier and the state in `sessionStorage`, then sets `window.location` to `https://login.eveonline.com/v2/oauth/authorize?response_type=code&client_id=<CLIENT_ID>&code_challenge=<CHALLENGE>&code_challenge_method=S256&redirect_uri=https://<APP_DOMAIN>/&scope=<SCOPES>&state=<STATE>`. The scopes are the same as `esi::SCOPES`.
2. SSO redirects back to `https://<APP_DOMAIN>/?code=<CODE>&state=<STATE>`.
3. On start, the app reads the query. If `code` and `state` exist, it checks the state against `sessionStorage`, then `POST`s the form body (`grant_type=authorization_code`, `client_id`, `code`, `code_verifier`) to `https://login.eveonline.com/v2/oauth/token`.
4. The app removes the query with `history.replaceState`, validates the token, and saves the refresh token with the `TokenStore`.
5. The tracker starts for the character.

**Risks**

- A refresh token in `localStorage` is readable by any script on the page. Mitigation: the CSP of section 2.2, no third-party scripts, and a clear "log out" that revokes the token (`REVOKE_URL` exists).
- The Nexum key sits in `localStorage` too, and each Nexum request carries it to our edge (section 2.2). It has the same exposure as a refresh token. The CSP and a clear settings text are the controls.
- Two tabs can poll for the same character. Decide whether to lock with `BroadcastChannel` or accept it (section 7).

---

## 4. Findings from the Code Review

### 4.1 The wasm build fails today

`cargo check -p router_core --target wasm32-unknown-unknown` fails. `rustls` and `ring`, which `ureq` pulls in, do not compile for this target. The checks after that error were not run, so the full list below comes from reading the code.

### 4.2 Native-only calls

About 24 files in `router_core` and `router_egui` hold a native-only call. Roughly half need a redesign. The rest need a one-line swap.

| Area | Where | Wasm problem |
| :--- | :--- | :--- |
| HTTP | `sources/mod.rs`, `esi/client.rs`, `esi/sso.rs`, `sde_update.rs`, `pilots_view.rs` (portraits) | `ureq` does not build. Calls block. |
| Threads and channels | `refresh.rs`, `sources/nexum.rs` (`thread::scope`, `SIG_WORKERS`), `sources/evescout.rs`, `esi/tracker.rs`, `esi/pilots.rs`, `esi/sso.rs`, `app.rs`, `pilots_view.rs`, `settings_window.rs`, `view.rs` | `std::thread::spawn` is not available. A blocking `recv_timeout` loop cannot run. |
| Files | `config.rs`, `sources/mod.rs` (caches), `esi/store.rs`, `esi/active.rs`, `overlay.rs`, `sde.rs`, `sde_update.rs`, `ships.rs`, `wormhole_types.rs` | No file system. Most functions take `&Path`. |
| Clock | `wormhole.rs:12` (`SystemTime::now`), `esi/tracker.rs` (16 `Instant` sites), `esi/sso.rs`, `esi/client.rs`, `startup.rs`, `app.rs`, `view.rs` | `SystemTime::now()` and `Instant::now()` panic. |
| Sleep | `app.rs:561` (frame limiter) | `std::thread::sleep` is not available. |
| OS | `keyring`, `dirs`, `webbrowser`, `tiny_http`, `mimalloc`, `std::env::args`, `std::env::var` | No such API in a browser. |
| Time zone | `log.rs` (`chrono::Local`) | Needs the `wasmbind` feature. |
| Random | PKCE (`oauth2`, `getrandom`) | Needs the `wasm_js` backend. |
| Features | `eframe` features `wayland`, `x11` | Native only. Move to a native-only dependency table. |

### 4.3 Benchmarks (native, 32 logical cores)

`cargo bench -p router_core --features test-support --bench core`, once with `RAYON_NUM_THREADS=1` and once with the default.

| Bench | 1 thread | 32 threads |
| :--- | :--- | :--- |
| `sde_parse` | 15.0 ms | 5.0 ms |
| `universe_build` | 3.1 ms | 4.0 ms |
| route Jita to Amarr, top 20 | 18.0 ms | 16.6 ms |
| route Jita to UALX, top 5 | 26.7 ms | 7.2 ms |
| Yen k=50 | 22.9 ms | 12.1 ms |
| 6 waypoints, top 3 | 9.4 ms | 3.2 ms |
| optimize, 3 sizes | 2.5 / 4.2 / 11.4 ms | 0.9 / 1.1 / 3.0 ms |

The benches use short routes. The worst single-thread case there is 27 ms. The app waits `APPLY_DELAY` (500 ms) before it searches.

### 4.4 Wasm and thread measurements (long route)

**Method.** A scratch crate (not in the repository) includes the real `route.rs`, `universe.rs`, `sde.rs`, `ships.rs`, `wormhole.rs`, and `ansiblex.rs` by `#[path]`. It builds for `wasm32-wasip1` and runs in Node 24 (V8 13.6, the same engine as Chrome) through `node:wasi`. It builds with `opt-level = 3`, `lto = "fat"`, and `panic = "abort"`, as the release profile does. The route is Jita to ND-X7X: 40 gate jumps, no wormholes, default options. Each value is the median of 20 runs. Rerun this method on any change to `route.rs`.

| Build | Top 1 | Top 5 | Top 20 |
| :--- | :--- | :--- | :--- |
| Native, 1 thread | 0.67 ms | 33.7 ms | 133 ms |
| Native, 32 threads | 0.67 ms | 10.2 ms | 36.0 ms |
| Wasm, no SIMD (Node, 1 thread) | 0.57 ms | 30.2 ms | 150 ms |
| Wasm, `+simd128` (Node, 1 thread) | 0.64 ms | 29.3 ms | 135 ms |

SDE parse and graph build: 13.5 ms and 3.4 ms native, 27 ms and 5.5 ms wasm.

**Thread scaling (native, same route, median of 20).** These are best-case numbers for wasm threads, because wasm adds the cost of pool start and shared memory.

| Threads | Top 5 | Top 20 |
| :--- | :--- | :--- |
| 1 | 31.5 ms | 129 ms |
| 2 | 18.8 ms | 75.5 ms |
| 4 | 12.7 ms | 48.3 ms |
| 8 | 10.3 ms | 37.3 ms |
| 16 | 9.4 ms | 32.0 ms |

**Findings**

1. One-thread wasm runs at 0.9 to 1.15 times the native time. The SDE parse is the exception (about 2 times slower).
2. `+simd128` gives no gain (under 10 percent, inside the noise). Graph search follows pointers and branches, so the compiler cannot vectorize it.
3. Top 5 on a 40-jump route costs about 30 ms. That is two frames at 60 fps, after the 500 ms delay. Top 1 costs under 1 ms.
4. Top 20 costs about 150 ms in wasm. That is a visible freeze. Cost grows with the path length times the number of routes, because Yen's algorithm runs one search for each node of the previous path.
5. At 4 threads, top 20 drops to about 50 ms (3 times). Top 5 drops to about 13 ms.

**Limits of these numbers.** Only V8 is tested. Firefox and Safari can differ. Node runs the code outside a browser, and the page also draws frames on the same thread. The app wasm also holds the `eframe` code. `opt-level = "s"` (section 5.7) can be slower than the level 3 used here.

---

## 5. Core Refactoring (`router_core` & `router_egui`)

Principle: native behavior does not change. Build each seam on native first. The native tests (local servers, `std::fs`) prove it.

### 5.1 `Clock`

Use `web_time::Instant` and `web_time::SystemTime` in place of the `std` types in both crates. On native they are the `std` types. On web they use `performance.now()` and `Date.now()`. Cover `SystemTime`, not only `Instant`.

### 5.2 `Storage`

A trait for named byte values (`get`, `set`, `delete`). Impls: file system (native), `localStorage` (web, values under about 50 KB), `IndexedDB` (web, large values).

- Move `config.rs`, the source caches (`sources/mod.rs`), `esi/store.rs` (character list), `esi/active.rs`, `overlay.rs` (bridges), and `ships.rs` / `wormhole_types.rs` onto it. Functions then take a key, not a `&Path`.
- `localStorage`: settings, avoid lists, theme, Nexum URL, key, and map ID, the character list, the active route, bridge JSON.
- `IndexedDB`: the SDE blob, the SDE build number, the Nexum and EVE-Scout caches (a large alliance map is a few MB, which is too large for `localStorage`), and `ships.json` / `wormholes.json`.
- `TokenStore` already exists. Add a `localStorage` impl next to `Keyring`.

### 5.3 `Http`

One async trait for `get`, `head`, `get_range`, `post_form`, with headers and a body limit. Impls:

- Native: wraps `ureq`. Callers run it with `pollster::block_on` on the existing threads, so native keeps its thread model.
- Web, Nexum: every Nexum request goes to the same-origin proxy (section 2.2), with the target host and path as parameters. No request goes to a Nexum host directly.
- Web: `fetch` through `web-sys` and `wasm-bindgen-futures`. Send `credentials: "omit"`. For SDE requests send only `Range` as a custom header (section 2.3).

Convert `sources/mod.rs`, `nexum.rs` (the `SIG_WORKERS` pool becomes `buffer_unordered`), `evescout.rs`, `esi/client.rs`, `esi/sso.rs`, `sde_update.rs`, and the portrait load. Keep the 5-second and 10-second timeouts (`AbortController` on web). The `RangeSource` trait gets an async twin. `WebRangeSource` keeps `HEAD` for the length and explicit `start-end` ranges.

### 5.4 SDE as bytes

- `sde::load(&Path)` becomes `sde::parse(files)` on byte slices. A thin native wrapper reads the files.
- `sde_update::download` already holds the files in a `HashMap` before it writes them. Split it into a pure step (bytes in, files out) and a write step.
- Add `SdeBlob` and `Universe::from_blob` (section 2.1).

### 5.5 Background loops

`Refresher` and `Tracker` run `recv_timeout` loops on threads. Make each loop an async fn that selects over the control channel and a timer. On web, `spawn_local` and `gloo_timers` drive it. On native, a thread and `pollster` drive it. The UI calls `request_repaint()` after each event, as it does now.

The startup `load` (up to 35 s) becomes an async task on web. The frame limiter (`app.rs:561`) is native only, because the browser already paces frames.

### 5.6 Parallelism: three stages

`rayon-core` 1.13 falls back to one thread on `wasm32-unknown-unknown` (see "Global fallback when threading is unsupported" in its `lib.rs`). The `par_iter` and `rayon::join` calls then compile and run with no code change. This is stage 1.

Threads can pay off. Section 4.4 shows a 3 times gain for top 20 on a long route at 4 threads. The cost is real too, so the plan adds threads in stages, and each stage needs the one before it.

**Terms.** A rayon thread on wasm runs as a Web Worker. A single Web Worker that hosts the route engine does not need rayon. Stage 2 uses one Web Worker. Stage 3 adds a pool of more Web Workers inside it.

| Stage | What | Gives | Cost |
| :--- | :--- | :--- | :--- |
| 1 | One thread on the UI thread. `rayon` falls back. No code change. | Top 5 at about 30 ms. Top 20 at about 150 ms (a freeze). | None. This is the baseline (task 7). |
| 2 | The route engine runs in one Web Worker. The UI posts a request and gets a result. Search is progressive: show route 1 at once (under 1 ms), then add one route in each pass. `k_shortest` adds one route in each loop pass, so it splits cleanly. | The UI never waits, for any top-N. | An async message API. A second wasm instance, with its own copy of the `Universe`. |
| 3 | `wasm-bindgen-rayon` threads inside the Web Worker of stage 2. Cargo feature `web-threads`. A check of `crossOriginIsolated` at run time, with a fallback to one thread. | Top 20 in about 50 ms at 4 threads. Top 5 in about 13 ms. | See the table below. |

**Cost of stage 3**

| Item | Status |
| :--- | :--- |
| Nightly toolchain, `-Zbuild-std`, and atomics | True, but contained. The `wasm-bindgen-rayon` README says to pin a nightly (it tested `nightly-2025-11-15`). Only the web build uses it. Native stays on stable (`rust-toolchain.toml`). |
| COOP/COEP on every page, including `trunk serve` | Set in `_headers` from the first web build (section 2.2). The app has no popups and no third-party embeds. All CCP calls use CORS. |
| Calls that block on the main thread | **Unverified.** A browser forbids `Atomics.wait` on the main thread. `rayon` blocks a caller that is not a pool thread. The UI thread calls `Router::routes` and `rayon::join` directly (`app.rs:305`). I expect these calls to fail there. The README does not say. If this is true, stage 2 is a prerequisite for stage 3, not an option. |
| Shared memory size | **Unknown.** The README sets `--max-memory=1073741824` (1 GiB). Shared memory cannot shrink. iOS Safari may refuse a large shared memory. |
| Safari and iOS support | **Unknown.** The README does not cover them. |
| Real wasm thread overhead | **Unknown.** Section 4.4 gives native numbers. |

**Spike (task 11).** Build the scratch crate of section 4.4 with `wasm-bindgen-rayon` and the pinned nightly. Time top 5 and top 20 with 4 Web Workers in Node, then in Chrome, Firefox, and Safari (including iOS). Test a call from the main thread. Ship stage 3 only if both of these hold: top 20 is at least 2 times faster than stage 1, and no browser fails.

**When to build stage 2.** Build it if task 7 shows a hitch the user can see, or if the spike shows that stage 3 needs it. `+simd128` is not used at any stage.

### 5.7 Platform gating and web entry

- Put native-only dependencies (`ureq`, `keyring`, `dirs`, `tiny_http`, `webbrowser`, `mimalloc`, the `wayland` and `x11` features) in `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`. Put web-only dependencies (`web-sys`, `wasm-bindgen-futures`, `gloo-timers`, `web-time`, `getrandom` with the `wasm_js` feature, `chrono` with `wasmbind`) in the `wasm32` table. Add each with `cargo add`.
- Gate `#[global_allocator]` (`mimalloc`) in `main.rs`.
- Add a `lib.rs` to `router_egui` with a `#[wasm_bindgen(start)]` entry that calls `eframe::WebRunner`. Add `index.html` and a `Trunk.toml`.
- Web build profile: try `opt-level = "s"` and `opt-level = 3`, each with `wasm-opt`. Keep level 3 if "s" slows the route search by more than about 10 percent (section 4.4 used level 3). Set a size budget after the first build, and serve with Brotli.
- Accepted limits of a canvas UI: no native text selection, no page find, and a weaker mobile keyboard.

### 5.8 Overlays (Ansiblex bridges)

- A paste modal for the bridge JSON. `eframe` also accepts dropped files on the web. Skip `rfd` at first.
- After an import, save the JSON with `Storage` (`localStorage`). Load it on each start, so a custom bridge network survives reloads.

---

## 6. Tasks

Each task ends in a commit. The native tests pass after each task. Tasks 1 to 3 are small and independent. Task 5 is the largest. Tasks 10 and 11 are optional performance stages (section 5.6). Task 11 (the spike) can run at any time after task 6.

| # | Task | Done when |
| :--- | :--- | :--- |
| 0 | No-code checks. ~~`curl` Nexum for CORS and the preflight~~ (done, section 2.4). ~~Test ESI with `X-Compatibility-Date`~~ (done, section 2.3). ~~Test SSO with the real web `client_id`~~ (done, section 2.3). Register two web apps in the developer portal: production (`https://<APP_DOMAIN>/`) and development (`http://localhost:<port>/`). The project owner does this. **Deferred by the project owner (2026-10-09).** Tasks 1 to 8 do not need it. Do it before task 9. | Both apps exist, and both `client_id` values are set in CI and in the dev shell. Task 0 is then closed. |
| 1 | `Clock` seam: `web_time` for `Instant` and `SystemTime` in both crates. | Native tests pass. |
| 2 | `Storage` trait and the file impl. Move config, caches, characters, active route, and overlays onto it. | Native tests pass. No `&Path` in the logic. |
| 3 | Split `sde::load` and `sde_update::download` into bytes-in, files-out. | Native tests pass. |
| 4 | `SdeBlob` with `rkyv` (`cargo add rkyv`), `Universe::from_blob`, validation, and a format version. | A native test builds a blob from `sde/` and gets the same `Universe` (8490 systems, 13978 stargates). The blob size is on record. |
| 5 | Async `Http` trait and the `ureq` impl. Convert `sources`, `esi`, `sde_update`. Make `Refresher` and `Tracker` async fns driven by `pollster` on native. | Native tests pass. |
| 6 | Target-gated dependencies. `router_core` compiles for wasm. | `cargo check -p router_core --target wasm32-unknown-unknown` is green. |
| 7 | Web shell: `WebRunner`, `trunk`, the `fetch` `Http`, the `IndexedDB` `Storage`, SDE stream, blob cache. Measure routes, first-run pause, peak memory, and wasm size. | The planner runs in a browser with the real map. Numbers are in this document. |
| 8 | Live overlays: EVE-Scout, then Nexum through the same-origin proxy (the Cloudflare Worker script), `_headers` (the fixed CSP and COOP/COEP). | Wormholes show in a browser. The proxy limits fit a large alliance map. |
| 9 | SSO, `Tracker`, and the `localStorage` `TokenStore`. | A pilot shows live in a browser. |
| 10 | Stage 2: the route engine in a Web Worker, with a message API and a progressive top-N. Do this only if task 7 shows a visible hitch, or if task 11 needs it. | Top 20 on a 40-jump route never blocks the UI. Route 1 shows in under 10 ms. |
| 11 | Stage 3 spike: `wasm-bindgen-rayon` on the pinned nightly, tested in Node, then Chrome, Firefox, and Safari (including iOS). Test a call from the main thread. If it passes, add the `web-threads` feature. | Top 20 is at least 2 times faster than stage 1, and no browser fails. The result is in section 4.4. |

---

## 7. Open Questions & Risks

| Item | Why it matters | Closed by |
| :--- | :--- | :--- |
| ~~Does Nexum send CORS headers and answer a preflight?~~ | **Closed 2026-10-09 (section 2.4).** It answers the preflight with 204, but allows only its own origin. The proxy is needed for this server. | Done |
| ~~Does a Nexum server exist that allows our origin?~~ | **Closed 2026-10-09 by decision.** The web build uses the proxy only (section 2.2). | Done |
| ~~Does ESI answer the preflight with `X-Compatibility-Date`?~~ | **Closed 2026-10-09 (section 2.3).** Yes, for `GET` and `POST`. | Done |
| Proxy limits (rate, response cap) against a large alliance map | All Nexum traffic uses the proxy. The test map is 160 KB. A large map is "a few MB", and it needs one signature request for each wormhole system. | Task 8 |
| Size and parse time of a large alliance map in `IndexedDB` and on the main thread | Only the 160 KB map is tested. | Task 7 |
| Wasm size budget | No number exists, so the size review has no pass mark. Pick one before task 7. | Before task 7 |
| `web-test-strategy.md` is not reconciled with Draft 3 | The plan and the test strategy can disagree. | Before task 1 |
| ~~Does SSO return a JSON `400` for the real `client_id`?~~ | **Partly closed 2026-10-09 (section 2.3).** The refresh grant gives a JSON `400` (`invalid_grant`). A fake authorization code gives a 500 with HTML, so the code-exchange error shape is unknown. | Task 9 (a real login) |
| Are the production and development apps registered? | The rules are known: `http` for `localhost` only, else `https` or `eveauth-app://`, and one callback for each app. Each environment needs its own app and `client_id`. **Deferred by the project owner (2026-10-09).** | Before task 9 |
| Is `router.smrkn.com` the final production domain? | The project owner says it is likely final. The production callback must match the final origin exactly, and a domain change needs a new registration. | Confirm before task 8 |
| Do refresh tokens rotate? | The CCP page does not say. It matters for two tabs. | Task 9 |
| Peak memory and pause when inflating the ship source files | Decides if the SDE job needs a Web Worker. | Task 7 |
| Route times in a browser (Firefox, Safari, Chrome) | Section 4.4 tests V8 in Node only. | Task 7 |
| Does a `rayon` call on the browser main thread fail? | If yes, stage 2 is a prerequisite for stage 3. | Task 11 |
| Does iOS Safari accept the 1 GiB shared memory? Does Safari run the threads? | Decides if stage 3 can ship. | Task 11 |
| Real wasm thread overhead (pool start, shared memory) | Section 4.4 gives native best-case numbers. | Task 11 |
| `opt-level = "s"` against 3 for the route search | Size against speed. | Task 7 |
| Blob size and `IndexedDB` alignment | Sets the cost of the copy into an aligned buffer. | Task 4 |
| Two tabs poll for one character | Doubles ESI traffic and can race on token refresh. | Task 9 |
| Chrome and Safari CORS behavior | Only Firefox is tested. | Task 7 |

---

## 8. Verification & Testing Strategy

The test strategy lives in `web-test-strategy.md`. It is not yet checked against this revision.

1. **Compilation matrix:**
   ```bash
   cargo check --workspace --target x86_64-pc-windows-msvc
   cargo check -p router_core --target wasm32-unknown-unknown
   cargo check -p router_egui --target wasm32-unknown-unknown
   ```
2. **Native regression:** `cargo test` passes after every task. The seams must not change native behavior.
3. **Blob:** the round-trip test of task 4, a corrupt-blob test (flip a byte, expect a rebuild), and a wrong-version test.
4. **SDE streaming and cache:** first boot downloads about 1.8 MB of ranges and writes the blob. A reload reads the blob and sends no SDE request except the `latest.jsonl` check.
5. **CORS in a real browser:** with the final headers, load the page. Confirm that `latest.jsonl`, the ZIP `HEAD` and range `GET`, and a portrait load with no console errors. Repeat in Chrome, Firefox, and Safari.
6. **Nexum:** a map loads through the proxy, and the page makes no direct call to a Nexum host (the CSP blocks it). Test that the proxy refuses a non-`GET` request, a private host, an IP literal, a path outside the allowlist, and an oversized response.
7. **Storage:** settings, avoid systems, Nexum values, theme, and bridge data survive a full page reload.
8. **SSO:** the login redirect, the callback, the token exchange, the state check (a wrong state fails), and a logout that revokes the token.
9. **Performance:** record route times, first-run pause, peak memory, and the compressed wasm size. Compare them with section 4.4 and the limits in sections 5.6 and 5.7. Rerun the method of section 4.4 after any change to `route.rs`.
