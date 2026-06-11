# Design System & Styling Guidelines

The UI follows a single design language: **modern data-logger** (MoTeC/AiM
lineage) with clean-minimal restraint — flat layered graphite surfaces,
hairline borders, monospace numerals, and one quiet amber accent. This document
is the reference for keeping new UI consistent with it.

## Source of truth

All tokens live in [`src/app.css`](src/app.css) as CSS custom properties on
`:root`. **Never hardcode a colour hex in a component** — every colour must
reference a token, either via `var(--token)` in CSS or via
[`themeColor()`](src/lib/theme.ts) when a JS API needs a colour string
(leaflet polylines, uPlot axes, generated SVG markup).

```ts
import { themeColor } from '$lib/theme';
L.polyline(points, { color: themeColor('--map-left', '#84b577') });
```

`themeColor(name, fallback)` resolves and caches the computed value; the
fallback is only used during SSR and must match the token.

## Tokens

### Surfaces (graphite ramp, deepest → most elevated)

| Token | Use |
|-------|-----|
| `--bg-body` | App background, input wells |
| `--bg-panel` | Rails, bars, modal bodies |
| `--bg-card` | Cards inside rails |
| `--bg-elevated` | Hovers, toolbar buttons, panel headers |
| `--bg-track` | Gauge/inactive track fills |

### Borders (hairlines)

`--bd-dim` (card edges) → `--bd-subtle` (rail/section dividers) →
`--bd-muted` (interactive borders) → `--bd-strong` (hover emphasis).

### Text ramp

`--tx-hi` (primary values) → `--tx-mid` (body) → `--tx-lo` (supplemental
data) → `--tx-dim` (labels/captions) → `--tx-xdim` (faint chrome) →
`--tx-ghost` (disabled/idle).

Readability rule of thumb: data a user actually reads (angles, speeds,
timestamps) sits at `--tx-lo` or brighter and ≥ 0.67rem; only labels and
chrome may use `--tx-dim` and below.

### Accent — restrained amber

`--ac` (#d2a24c) with `--ac-bright` for hover and `--ac-wash` for selected
backgrounds. The accent is deliberately muted and **used sparingly**: active
tab underline, selected rows (2px inset bar + wash), primary buttons, the
gauge needle. Large amber fills are off-brand — if a surface starts looking
"amber-themed", dial it back.

### Status colours (desaturated on purpose)

| Token | Meaning |
|-------|---------|
| `--ok` / `--ok-bright` | Healthy / **live** highlight (the bright variant exists for corner-of-the-eye glanceability — live run ring, LIVE label) |
| `--warn` | Attention: starvation timer, handbrake, over-estimates |
| `--bad` / `--bad-tx` | Errors, invalid runs, deletes (`--bad-tx` is the readable text variant) |
| `--info` | Cool informational (under-estimates, cold tires) |
| `--violet` | Best-lap / boost / clutch family |

### Map semantics

`--map-left` / `--map-right` (zone boundaries), `--gate-a` / `--gate-b` /
`--gate-split` (gates), `--live-dot` (player marker). Per-lap traces use
`LAP_PALETTE` from `src/lib/theme.ts` — never invent chart colours.

### The one exception: car class colours

The class + PI pill in the TopBar uses Forza's own class hues (D blue,
C yellow, B orange, A red, S1 purple, S2 cyan, X green) at game-like
vividness, because players read those colours at a glance in-game. Keep that
mapping; don't desaturate it to match the system.

## Typography

- UI text: `--font-ui` (Inter Variable, bundled via `@fontsource-variable` —
  no network fetches; the Tauri app must work offline).
- **Every numeral and data readout** uses `--font-mono` (JetBrains Mono
  Variable) with tabular figures. Use the global `.mono` utility class.
- Section/field labels use the global `.cap` utility: small uppercase,
  0.12em tracking, `--tx-dim`.
- Weights stay in the 550–750 range; avoid 800+.

## Shape & depth

- Radii via tokens only: `--r-xs` 2px (chips) → `--r-sm` 3px (buttons/inputs)
  → `--r-md` 4px (cards) → `--r-lg` 6px (modals). Data-logger sharp; nothing
  rounder.
- Depth comes from the surface ramp + hairline borders, not shadows. Shadows
  are reserved for floating panels and modals.

## Layering (z-index map)

Leaflet's internal panes go up to ~1000, so any inline map container **must**
declare `isolation: isolate` (see `.map-shell`, `.zone-map`, `.map-wrap`,
`.map-host`, `.fp`), and app overlays sit above 1000:

| Layer | z-index |
|-------|---------|
| Floating panels | 50 |
| Replay bar | 110 |
| Settings modal | 1200 |
| Map calibrator | 1300 |
| Toasts | 1400 |
| Update bar | 1500 |

## Conventions

- Selected list rows: amber wash + `inset 2px 0 0 var(--ac)` bar, never a
  filled background.
- Invalid drift runs keep identical row geometry and signal failure with a
  dull red outline + faint red wash (`color-mix` against `--bad`). This is a
  load-bearing at-a-glance indicator — preserve it in any restyle.
- Status messages attach to a card's bottom edge as tinted strips
  (`color-mix` wash + matching border-top), not floating mid-card.
- State classes on rows/cards must be namespaced if a sibling style could
  collide (see the `.invalid` → `.invalid-msg` fix — a message rule once
  leaked padding onto `.run-row-wrap.invalid`).
- Buttons: `primary` = solid amber with `--bg-body` text; `ghost` = hairline
  uppercase micro-button; destructive hover = `--bad` border + `--bad-tx`
  text.
