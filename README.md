# DZ-OSS

[![License: MIT](https://img.shields.io/badge/license-MIT-green)](LICENSE)
![Platform](https://img.shields.io/badge/platform-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-0078d4)

**DZ-OSS** (*Drift Zone Original Scoring System*) is a real-time telemetry app for **Forza Horizon 6**, built around a **drift-scoring workbench** that aims to reproduce the game's own drift-zone scoring live from the UDP feed — plus a full gauge cluster and automatic session recording. Desktop app (Tauri + Svelte); [build and run from source](#running-from-source).

![The DZ-OSS Drift workbench — live signed drift-angle gauge, the zone map with the driven line, and computed-vs-actual scoring](docs/img/drift.png)

## Drift Workbench

The headline feature. Draw a drift zone on the calibrated track map, then drive it — the app detects each run, estimates a score live, and keeps a per-zone history you can check against the game's actual score.

- **Define zones** — draw a boundary and its two gates on the map. Runs are bidirectional: enter either gate, exit the other.
- **Live instruments** — a signed drift-angle needle gauge, speed, and a live flip counter while you're sliding.
- **Estimated score** — a per-wheel slip/surface model that *aims to mirror* FH6's drift scoring, running on every 64 Hz packet to produce a score the moment a run ends.
- **Run history** — every run is stored per zone. Enter the game's actual score to compare it against the estimate, with a breakdown, notes, and one-click recompute.

## Other Features

- **Live gauges** — speed, RPM, gear, compass heading, attitude indicator, G-meter, steering + pedal inputs, lap times, and tire temps.
- **Session recording** — timed events (races, Rivals, Time Trial) auto-record every packet to SQLite, with a replay + analysis viewer (input / speed / G-force / tire charts and the driven line). Survives in-race rewinds.
- **Track map** — the driven racing line over the *Forza Horizon 6: Japan* map, calibrated to world coordinates from two landmarks.
- **Headless web server** *(optional)* — serve the same dashboard over HTTP to any browser on your network. See [docs/headless-server.md](docs/headless-server.md).

## Forza Horizon 6 Setup

1. In FH6, open **Settings → HUD and Gameplay**.
2. Under **DATA OUT**, set **Data Out** to **On**.
3. Set **Data Out IP Address** to `127.0.0.1` and **Data Out IP Port** to `20440`.

A green dot in the app's top-left confirms packets are arriving. Sessions are stored at `%LOCALAPPDATA%\fh6-tel\sessions.db`.

## Running from Source

Prerequisites: Rust 1.75+ and Node.js 18+ (Windows also needs WebView2, pre-installed on Windows 10/11).

```bash
npm install
npm run tauri dev      # hot-reloading dev app
npm run tauri build    # production installer (output under src-tauri/target/release/bundle/)
```

## License

DZ-OSS is MIT-licensed — see [LICENSE](LICENSE). It began as a fork of [fh6-tel by BanHammer](https://github.com/TheBanHammer/fh6-tel) — the telemetry foundation it's built on — and has since grown into its own project, maintained independently. Thanks to BanHammer for the original work.
