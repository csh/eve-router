# Agent notes for eve-router

## Run the app for inspection

When an agent starts the app, use this command:

```sh
EGUI_INSPECTION=1 cargo run --features eframe/inspection -p router_egui
```

In PowerShell:

```powershell
$env:EGUI_INSPECTION = "1"; cargo run --features eframe/inspection -p router_egui
```

- The inspection server binds to `127.0.0.1:5719`. Without `EGUI_INSPECTION=1` it stays off.
- The `inspection` feature is not in `Cargo.toml`. Pass it on the command line only.
- The MCP server `egui-mcp` (https://github.com/rerun-io/egui_crates/tree/main/crates/egui_mcp) connects to the running app.
  Install it with `cargo install --git https://github.com/rerun-io/egui_crates egui_mcp --locked`.
  Register it with `claude mcp add egui egui-mcp`.
- After the app starts, call the `attach` tool of the `egui` server. Then read the widget tree, send input, or take a screenshot.
- Screenshots need a window that is not minimized. A window behind another window works. A minimized window stops painting, and the screenshot times out.

## Rules for a started instance

- The inspection tools act only inside the egui window. Do not use desktop-level automation (SendKeys, mouse injection, computer use). 
  this is a game companion tool; anticheats may flag other automation methods.
- Never end the instance that the user runs. Record the PID of each instance you start, and end only that PID.
- A started instance shares the real config and the refresh tokens. Do not log in a pilot unless asked. Do not remove a pilot unless asked.
- A running `eve-router.exe` locks its file on Windows. If the link step fails, build with another `--target-dir`.

## Build and test

- `cargo test` runs the tests of `router_egui` and `router_core`.
- CI runs rustfmt, clippy, the tests, and `cargo deny`. See `.github/workflows/ci.yml`.
- The `target` directory can grow very large. Delete `target\debug\incremental` to free up space.
