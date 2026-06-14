# Headless Server (Web Host)

Besides the desktop app, the project includes a headless server — **`dz-oss-serve`** — that ingests Forza telemetry and serves the *same* dashboard over HTTP to any browser on your network (phones, tablets, another PC). Run it on your gaming PC or a separate always-on box (home server, NUC, VPS).

> Run **either** the desktop app **or** the server on a given machine — a single process owns the telemetry UDP socket and the SQLite database.

There are no prebuilt downloads. Build it from source as below.

## Build from source

Prerequisites: Rust 1.75+ and Node.js 18+.

The server binary embeds the compiled frontend, so build the frontend first, then the binary with the `server` feature:

```bash
npm install
npm run build      # SvelteKit static output -> ./build (embedded into the binary)

cargo build --release --manifest-path src-tauri/Cargo.toml \
  --features server --bin dz-oss-serve
```

The binary lands at `src-tauri/target/release/dz-oss-serve` (`.exe` on Windows). Copy it wherever you like — it's self-contained.

## Run

```bash
# Linux / macOS
./dz-oss-serve --ip 0.0.0.0 --port 8080 --auth-token CHANGE_ME
```

```powershell
# Windows
.\dz-oss-serve.exe --ip 0.0.0.0 --port 8080 --auth-token CHANGE_ME
```

Then open `http://<server-ip>:8080` in a browser.

| Flag | Default | Description |
|------|---------|-------------|
| `--ip` | `127.0.0.1` | HTTP bind address. Use `0.0.0.0` for LAN/remote access. |
| `--port` | `8080` | HTTP port for the dashboard. |
| `--auth-token` | _(none)_ | When set, the browser must log in (HttpOnly session cookie). **Recommended whenever the server is reachable beyond localhost.** |
| `--udp-port` | `20440` | Forza telemetry UDP port (falls back to `settings.json`). |

Binding a non-localhost address **without** a token prints an open-instance warning — anyone who can reach the port can view *and delete* sessions.

## Forza setup for a server

Point the game at the server instead of localhost: in **Settings → HUD and Gameplay → DATA OUT**, set the **Data Out IP Address** to the **server's IP** and the port to **20440** (Car Dash format). Open TCP `8080` and UDP `20440` in the server's firewall.

## Data & persistence

The server stores `sessions.db` and `settings.json` under the OS data directory:

| Environment | Location |
|-------------|----------|
| Windows | `%LOCALAPPDATA%\fh6-tel\` |
| Linux / macOS | `$XDG_DATA_HOME/fh6-tel/` (default `~/.local/share/fh6-tel/`) |

## Running as a service

Ready-to-edit service definitions live in [`packaging/`](../packaging/): a **systemd** unit (Linux), a **launchd** plist (macOS), and Scheduled-Task / NSSM notes (Windows).
