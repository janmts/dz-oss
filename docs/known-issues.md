# Known Bugs & Limitations

DZ-OSS is pre-1.0 and under active development. The headline feature — the drift
score — *aims to reproduce* Forza Horizon 6's hidden drift-zone scoring, inferred
from the telemetry feed and logged in-game scores rather than the game's own formula.
It tracks recorded runs to within roughly 1–2% on most runs (see
[How the Drift Scoring Works](scoring.md)), but it is an estimate, and the items below
are where it's currently weakest or where a feature isn't finished yet.

None of these stop the app from working — they're the honest edges of an in-progress
project.

## Scoring accuracy

- **It's an estimate, not the game's number.** Expect a small gap on most runs and a
  larger one in the cases below. Enter your real in-game score on each run to see the
  difference for yourself.
- **Off-road drifting outside the real scoreable area is over-credited.** The game only
  pays points inside a hidden scoreable region that is tighter than the painted flags.
  That boundary isn't mapped yet, so a slide that strays outside it still scores in
  DZ-OSS when the game would give nothing. (The per-wheel surface check catches the
  common cases; the precise spatial boundary is still to come.)
- **Summer and autumn off-road credit is assumed, not measured.** Off-tarmac scoring
  changes by festival season — spring pays for grass, winter doesn't (both confirmed).
  Summer and autumn are assumed to behave like spring until their first weeks can be
  checked against the game; if they don't, runs in those seasons may be mis-scored.
- **Entry and exit of some zones can be slightly over-credited.** DZ-OSS starts counting
  at the gate line, while the game appears to begin a short distance inside it — so the
  first and last stretch of a run can score a little high.
- **Very shallow, very fast slides are slightly under-credited.** The game rewards them
  a touch more than an angle × speed × time integral can express, and no available
  telemetry channel recovers the difference.
- **The game has its own run-to-run variance.** Two near-identical runs can post
  different in-game scores; the estimate can't reproduce noise that isn't in the data.

## App behaviour

- **No live ticking score yet.** The estimate is computed when a run finishes, not
  counted up live like the in-game HUD. The live readouts while you drive are drift
  angle, speed, and the flip counter.
- **The live view can lag when a run auto-starts.** The first time you enter a zone, the
  on-screen view may not switch to the active run until the run finishes. Recording is
  unaffected — every packet is still captured throughout — and selecting the zone in the
  app before you drive avoids the lag.
- **One telemetry app per port.** DZ-OSS binds the game's data-out UDP port, so it can't
  run at the same time as another telemetry tool unless you route the feed through a UDP
  forwarder — see the setup note in the [README](../README.md#forza-horizon-6-setup).
- **Multi-level "loop" zones use approximate height bands.** Where a course crosses over
  itself at different elevations, the bands that separate the passes can round slightly
  at the very top and bottom of the climb. This is cosmetic today (scoring is 2D) but
  worth knowing when authoring such a zone.
- **Run-viewer map → graph cursor is approximate.** Hovering the datalogger highlights
  the exact spot on the map; going the other way (hovering the map to find the graph
  point) is a close approximation, not exact.
- **Most per-zone scoring parameters have no UI.** Every scoring coefficient is
  overridable per zone through the stored zone config, but only the boundary "slack" is
  exposed as a control in the editor.
- **The car class/PI badge uses an older Forza class list.** It predates this game's
  class ranks and is missing newer ones (e.g. R), so some cars show the wrong class
  label. Purely cosmetic — scoring is car-independent, so your score is unaffected.

## Platform & distribution

- **The desktop app is Windows-only.** Forza Horizon 6 is Windows/Xbox only, so the
  telemetry originates there. Linux and macOS users can run the cross-platform
  [headless server](headless-server.md) instead.
- **Prebuilt installers are Windows-only.** Download the `.exe` from the
  [Releases](https://github.com/janmts/dz-oss/releases/latest) page. There are no prebuilt
  Linux/macOS or headless-server binaries — [build those from source](../README.md#running-from-source).

---

Found something that isn't listed here? Open an issue on GitHub.
