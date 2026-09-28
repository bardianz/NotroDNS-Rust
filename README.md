# NotroDNS

A lightweight, native Windows DNS manager written in modern stable Rust.
No Electron, no Tauri, no WebView, no .NET, no Python — a single small
`.exe` built on `egui`/`eframe`, `tokio`, and `reqwest`.

NotroDNS fetches a curated list of public DNS servers, benchmarks them
concurrently by actual DNS-query latency (with optional ICMP ping as a
secondary metric), and lets you apply, or roll back, DNS settings on a
chosen Windows network adapter.

## Features

- **Async, non-blocking GUI.** The `egui`/`eframe` UI runs on the main
  thread; all network I/O and `netsh` calls run on a dedicated worker
  thread that owns a `tokio` runtime, talking back to the UI over a plain
  message channel. The window never freezes during a fetch or a benchmark.
- **Live server list with offline fallback.** Fetches
  `https://dnschanger.pythonanywhere.com/api/` (the legacy
  `name` / `preferred_ip` / `alternate_ip` schema), with timeouts and
  retries. On failure, it transparently falls back to the last successful
  response, cached locally as JSON — the app stays fully usable offline.
- **Concurrent benchmarking.** Every server's `preferred_ip` is probed
  with a bounded number of concurrent, timeout-guarded UDP DNS queries
  (a hand-built minimal `A`-record query — no heavyweight resolver crate
  needed). An optional secondary ICMP-style ping (via the system `ping`
  utility, not raw sockets, so it needs no elevation) can be enabled too.
  Results are sorted by latency, with failures pushed to the bottom.
- **Adapter-aware DNS management.** Lists Windows network adapters,
  reads an adapter's current DNS configuration, sets static IPv4/IPv6 DNS,
  and restores automatic (DHCP) DNS — all via `netsh`, invoked with
  argument vectors (never a shell string), with every IP address and
  adapter name validated first.
- **"Restore previous DNS."** Before NotroDNS changes an adapter's DNS
  for the first time, it snapshots whatever was configured (DHCP or
  static) to a small local JSON file, so you can undo the change later —
  even after restarting the app — without having to remember what it was.
- **Elevation only when needed.** NotroDNS runs, browses, and benchmarks
  as a standard user. It only prompts for a UAC-elevated restart right
  before an action that genuinely requires admin rights (applying or
  restoring DNS).
- **Small release binary.** `opt-level = "z"`, LTO, single codegen unit,
  stripped symbols, `panic = "abort"`, and a deliberately small
  dependency list (see [Architecture](#architecture)).

## Building

Requires a recent stable Rust toolchain (`rustup`) targeting
`x86_64-pc-windows-msvc`.

```powershell
git clone <this-repo>
cd NotroDNS
cargo build --release
```

The optimized binary is written to
`target\release\notrodns.exe` (or `target\x86_64-pc-windows-msvc\release\notrodns.exe`
when cross-targeting). No installer, no runtime dependencies to ship.

### Running tests / lints locally

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
```

## Usage

1. Launch `notrodns.exe`. It loads the DNS server list (live, or from
   cache if offline) and detects your network adapters — no admin prompt
   yet.
2. Pick a network adapter, optionally enable "Include ping" for the
   secondary metric, and click **Benchmark all**.
3. Select a server from the sorted list and click **Apply selected DNS**.
   If NotroDNS isn't already running elevated, it will ask to restart
   itself with a UAC prompt — only for this step.
4. Use **Restore automatic (DHCP)** to hand DNS back to your router/ISP,
   or **Restore previous DNS** to recreate whatever was configured before
   NotroDNS first touched that adapter.

## Architecture

```
src/
├── main.rs           Entry point: logging + eframe window setup
├── lib.rs             Library root (re-exports everything below so
│                       integration tests can exercise it)
├── app.rs              GUI (egui/eframe) + app state machine + the
│                       worker thread bridging the sync GUI to async/
│                       blocking work
├── api.rs              DNS-server-list API client: fetch, retry,
│                       timeout, input validation/sanitization
├── cache.rs             Local JSON cache of the last good server list
│                       (offline fallback)
├── restore_state.rs     Per-adapter "DNS before NotroDNS" snapshots,
│                       persisted to JSON, for "Restore previous DNS"
├── bench.rs              Concurrent DNS-latency benchmarking (bounded
│                       concurrency, timeouts) + optional ping metric
├── config.rs             Constants and platform-appropriate file paths
├── error.rs              Centralized AppError / AppResult
└── windows/
    ├── mod.rs             Public Windows abstraction layer (with a
    │                       non-Windows stub for cross-platform `cargo
    │                       check` during development)
    ├── adapters.rs         Adapter enumeration + current-DNS reads,
    │                       via `netsh` (argument vectors, no shell)
    ├── dns_set.rs           Set/restore DNS via `netsh`, with strict
    │                       IP/adapter-name validation before every call
    └── elevation.rs          UAC elevation detection (`windows-sys`)
                            and relaunch
```

**State machine.** The GUI always reflects one of `Loading`, `Ready`,
`Benchmarking`, `Applying`, `Offline` (serving cached data), or `Error`,
shown as a colored badge — see `app::AppState`.

### Why `netsh` instead of raw Win32 networking APIs?

Reading and writing adapter DNS configuration through the IP Helper API
directly is possible, but pulls in a much larger surface of unsafe FFI
for comparatively little benefit here. `netsh` is present on every
supported Windows version, requires no admin rights to *read* config, and
its `set`/`add`/`source=dhcp` subcommands are the documented, supported
way to change DNS. Every call goes through `std::process::Command` with
an argument array — never a shell/cmd string — and every IP address and
adapter name is validated before it's used, so there's no
command-injection surface. UAC elevation (via `windows-sys`, the only
Windows-specific crate this project depends on) is the one place raw
Win32 calls are used, since there's no `netsh` equivalent for that.

**Known limitation:** parsing `netsh interface ip show config` to detect
whether an adapter is currently using DHCP or static DNS assumes
English-language Windows output. This only affects the pre-change
snapshot used by "Restore previous DNS" (it degrades to "assume DHCP" —
never to acting on wrong IPs); adapter listing and DNS-setting are
unaffected.

## Security notes

- Every DNS server entry returned by the API is validated
  (`api::sanitize_servers`) before it's shown, benchmarked, or ever
  reaches a `netsh` argument — a malformed or malicious API response
  can't inject extra command-line arguments.
- The optional ping check shells out to the system `ping.exe` with a
  fixed argument list and an already-`IpAddr`-parsed address — never a
  formatted string containing user or API input.
- Elevation is requested via the standard `ShellExecuteW` "runas" verb
  (a normal UAC prompt); NotroDNS itself never runs unnecessarily
  elevated, minimizing the window where it holds admin rights.

## CI

`.github/workflows/build.yml` runs on every push, pull request, and
manual dispatch, on `windows-latest`:

1. `cargo fmt --all -- --check`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo test --all`
4. `cargo build --release --target x86_64-pc-windows-msvc`
5. Uploads the resulting `notrodns.exe` as the **`NotroDNS-Windows-x64`**
   artifact via `actions/upload-artifact@v4`.

## License

MIT — see `Cargo.toml`. Adjust as needed for your own fork/distribution.
"# NotroDNS-Rust" 
