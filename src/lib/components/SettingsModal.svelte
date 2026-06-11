<script lang="ts">
  import { settings, saveSettings } from '$lib/stores/sessions';
  import MapCalibrator from './MapCalibrator.svelte';
  import type { AppSettings } from '$lib/types';

  let { onClose }: { onClose: () => void } = $props();

  let draft = $state<AppSettings | null>(null);
  let showCalibrator = $state(false);

  // The calibrator persists cal fields straight to settings; resync the draft
  // so the modal reflects them when it returns.
  function onCalibratorClose() {
    showCalibrator = false;
    if ($settings && draft) {
      draft = {
        ...draft,
        mapCalAWorld: $settings.mapCalAWorld,
        mapCalAPix: $settings.mapCalAPix,
        mapCalBWorld: $settings.mapCalBWorld,
        mapCalBPix: $settings.mapCalBPix,
      };
    }
  }

  $effect(() => {
    if ($settings && !draft) {
      // Defensive: guarantee map fields exist even if a very old settings
      // object somehow reaches the UI without them.
      const mapDefaults = {
        mapEnabled: false,
        mapOverride: false,
        mapTileUrl: '',
        mapMinZoom: 0,
        mapMaxZoom: 5,
        mapTileSize: 256,
        mapCalAWorld: [0, 0] as [number, number],
        mapCalAPix: [0, 0] as [number, number],
        mapCalBWorld: [0, 0] as [number, number],
        mapCalBPix: [0, 0] as [number, number],
        mapViewMaxZoom: 0,
        mapDefaultZoom: 0,
        mapDefaultCenter: [0, 0] as [number, number],
        tiresVisible: true,
        driftStarveTimeoutS: 5,
        driftPrerollS: 10,
      };
      draft = { ...mapDefaults, ...$settings };
    }
  });

  async function save() {
    if (!draft) return;
    await saveSettings(draft);
    onClose();
  }
</script>

{#if draft}
  <div class="overlay" role="dialog" aria-modal="true">
    <div class="modal">
      <h2>Settings</h2>

      <label>
        UDP Port
        <input type="number" bind:value={draft.port} min="1024" max="65535" />
        <span class="hint">Port changes take effect after restarting the app.</span>
      </label>

      <label>
        Units
        <select bind:value={draft.useMph}>
          <option value={true}>mph</option>
          <option value={false}>kph</option>
        </select>
      </label>

      <label class="checkbox-label">
        <input type="checkbox" bind:checked={draft.autoRecord} />
        Auto-record sessions
      </label>

      <fieldset>
        <legend>Drift Runs</legend>
        <label>
          Score-starvation timeout (s)
          <input
            type="number"
            min="0"
            step="0.5"
            bind:value={draft.driftStarveTimeoutS}
          />
          <span class="hint">
            A run aborts after this many seconds without earning drift points
            (the in-game fail condition). <strong>0 disables</strong> — a run then
            ends only at the finish gate. The default <strong>5 s matches FH6</strong>;
            the live run box turns yellow while the timer counts if you want to
            see it in action.
          </span>
        </label>
        <label>
          Pre-roll trail (s)
          <input
            type="number"
            min="0"
            step="1"
            bind:value={draft.driftPrerollS}
          />
          <span class="hint">
            Seconds of telemetry kept from <strong>before</strong> each run
            starts, stored alongside it — shows how the car approached the
            start gate (e.g. how long the drift was already established, which
            changes when the game starts crediting). Analysis-only: never
            affects scores. <strong>0 disables</strong>.
          </span>
        </label>
      </fieldset>

      <fieldset>
        <legend>Tire Temp Thresholds (°C)</legend>
        <label>Cold below <input type="number" bind:value={draft.tireTempCold} /></label>
        <label>Optimal up to <input type="number" bind:value={draft.tireTempOptimal} /></label>
        <label>Hot above <input type="number" bind:value={draft.tireTempHot} /></label>
      </fieldset>

      <fieldset>
        <legend>Track Map</legend>
        <label class="checkbox-label">
          <input type="checkbox" bind:checked={draft.mapEnabled} />
          Show track map
        </label>
        <span class="hint">
          Default: <strong>Forza Horizon 6: Japan</strong> — bundled tiles
        </span>
        <button class="cal-btn" onclick={() => (showCalibrator = true)}>
          Calibrate map…
        </button>
        <label class="checkbox-label">
          <input type="checkbox" bind:checked={draft.mapOverride} />
          Override map configuration
        </label>

        {#if draft.mapOverride}
          <label>
            Tile URL template
            <input
              type="text"
              placeholder="https://host/tiles/{'{z}'}/{'{x}'}/{'{y}'}.png"
              bind:value={draft.mapTileUrl}
            />
            <span class="hint">
              Blank = bundled tiles
              (<code>/maptiles/{'{z}'}/{'{y}'}/{'{x}'}.jpg</code>). Set an XYZ
              URL to use a remote source instead.
            </span>
          </label>
          <div class="row3">
            <label>Min zoom <input type="number" bind:value={draft.mapMinZoom} /></label>
            <label>Tile max zoom <input type="number" bind:value={draft.mapMaxZoom} /></label>
            <label>Tile px <input type="number" bind:value={draft.mapTileSize} /></label>
          </div>
          <div class="row3">
            <label>View max zoom <input type="number" bind:value={draft.mapViewMaxZoom} /></label>
            <label>Default zoom <input type="number" bind:value={draft.mapDefaultZoom} /></label>
            <span></span>
          </div>
          <span class="hint">
            View max zoom may exceed tile max zoom (tiles upscale). 0 = preset.
            Set the default centre with “Save current view as default” in the
            calibrator.
          </span>
          <span class="hint">
            Calibration: two reference points — a known game world (X, Z) and
            its pixel (X, Y) on the full-resolution map. Two distinct points
            define scale/rotation. Leave A = B to skip (auto-fit instead).
          </span>
          <div class="cal-grid">
            <span class="cal-head"></span>
            <span class="cal-head">World X</span>
            <span class="cal-head">World Z</span>
            <span class="cal-head">Pixel X</span>
            <span class="cal-head">Pixel Y</span>

            <span class="cal-head">A</span>
            <input type="number" bind:value={draft.mapCalAWorld[0]} />
            <input type="number" bind:value={draft.mapCalAWorld[1]} />
            <input type="number" bind:value={draft.mapCalAPix[0]} />
            <input type="number" bind:value={draft.mapCalAPix[1]} />

            <span class="cal-head">B</span>
            <input type="number" bind:value={draft.mapCalBWorld[0]} />
            <input type="number" bind:value={draft.mapCalBWorld[1]} />
            <input type="number" bind:value={draft.mapCalBPix[0]} />
            <input type="number" bind:value={draft.mapCalBPix[1]} />
          </div>
        {/if}
      </fieldset>

      <div class="actions">
        <button onclick={onClose}>Cancel</button>
        <button class="primary" onclick={save}>Save</button>
      </div>
    </div>
  </div>
{/if}

{#if showCalibrator}
  <MapCalibrator onClose={onCalibratorClose} />
{/if}

<style>
  .overlay {
    position: fixed; inset: 0; background: rgba(0,0,0,0.65);
    display: flex; align-items: center; justify-content: center;
    /* Above leaflet panes/controls (which z-index up to ~1000). */
    z-index: 1200;
  }
  .modal {
    background: var(--bg-panel); border: 1px solid var(--bd-muted); border-radius: var(--r-lg);
    padding: 1.5rem; width: 440px; max-height: 88vh; overflow-y: auto;
    display: flex; flex-direction: column; gap: 1rem;
    box-shadow: 0 16px 60px rgba(0, 0, 0, 0.55);
  }
  h2 { margin: 0; color: var(--tx-hi); font-size: 1rem; font-weight: 650; }
  label { display: flex; flex-direction: column; gap: 0.3rem; color: var(--tx-mid); font-size: 0.82rem; }
  .checkbox-label { flex-direction: row; align-items: center; gap: 0.5rem; accent-color: var(--ac); }
  input[type="number"], input[type="text"], select {
    background: var(--bg-card); border: 1px solid var(--bd-muted); border-radius: var(--r-sm);
    color: var(--tx-hi); padding: 0.4rem; font-size: 0.85rem; width: 100%; font-family: inherit;
  }
  input[type="number"] { font-family: var(--font-mono); }
  input:focus, select:focus {
    outline: none; border-color: color-mix(in srgb, var(--ac) 60%, var(--bg-panel));
  }
  .row3 { display: grid; grid-template-columns: repeat(3, 1fr); gap: 0.5rem; }
  .row3 label { font-size: 0.75rem; }
  .cal-grid {
    display: grid; grid-template-columns: 1.2rem repeat(4, 1fr);
    gap: 0.3rem; align-items: center;
  }
  .cal-grid input { padding: 0.3rem; font-size: 0.78rem; }
  .cal-head { color: var(--tx-dim); font-size: 0.66rem; text-align: center; }
  code { font-size: 0.68rem; color: var(--tx-lo); }
  .cal-btn {
    align-self: flex-start; background: var(--ac-wash);
    border: 1px solid color-mix(in srgb, var(--ac) 55%, var(--bg-panel)); color: var(--ac);
    padding: 0.35rem 0.8rem; border-radius: var(--r-sm); font-size: 0.78rem; cursor: pointer;
    font-family: inherit;
  }
  .cal-btn:hover { border-color: var(--ac); }
  fieldset { border: 1px solid var(--bd-subtle); border-radius: var(--r-md); padding: 0.75rem; display: flex; flex-direction: column; gap: 0.5rem; }
  legend {
    color: var(--tx-dim); font-size: 0.62rem; font-weight: 600;
    letter-spacing: 0.12em; text-transform: uppercase; padding: 0 0.3rem;
  }
  .actions { display: flex; justify-content: flex-end; gap: 0.5rem; }
  button {
    padding: 0.4rem 1rem; border-radius: var(--r-sm); border: 1px solid var(--bd-muted);
    background: var(--bg-elevated); color: var(--tx-mid); cursor: pointer; font-size: 0.82rem;
    font-family: inherit;
  }
  button:hover { border-color: var(--bd-strong); color: var(--tx-hi); }
  button.primary { background: var(--ac); border-color: var(--ac); color: var(--bg-body); font-weight: 650; }
  button.primary:hover { background: var(--ac-bright); border-color: var(--ac-bright); color: var(--bg-body); }
  .hint { font-size: 0.7rem; color: var(--tx-dim); margin-top: 0.15rem; }
</style>
