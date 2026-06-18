import type {
  SessionRow,
  TelemetryPacket,
  AppSettings,
  SessionLap,
  DriftRunRow,
  DriftRunPackets,
  DriftRunStatus,
  DriftZoneInput,
  DriftZoneRow,
} from '$lib/types';

const isTauri =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export const isDesktop = isTauri;

export interface TelemetryHandlers {
  onTick: (p: TelemetryPacket) => void;
  onBindFailed?: (msg: string) => void;
  onError?: (msg: string) => void;
}

export interface DriftZoneCaptureShortcut {
  side: 'left' | 'right' | null;
}

// ---------- Tauri implementation ----------
async function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<T>(cmd, args);
}

async function tauriSubscribe(h: TelemetryHandlers): Promise<() => void> {
  const { listen } = await import('@tauri-apps/api/event');
  const un1 = await listen<TelemetryPacket>('telemetry_tick', (e) => h.onTick(e.payload));
  const un2 = await listen<string>('udp_bind_failed', (e) => h.onBindFailed?.(e.payload));
  const un3 = await listen<string>('session_error', (e) => h.onError?.(e.payload));
  return () => { un1(); un2(); un3(); };
}

async function tauriVersion(): Promise<string> {
  const { getVersion } = await import('@tauri-apps/api/app');
  return getVersion();
}

// ---------- HTTP implementation ----------
async function http<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, { credentials: 'include', ...init });
  if (res.status === 401) {
    window.location.href = '/login';
    throw new Error('unauthorized');
  }
  if (!res.ok) {
    let detail = res.statusText;
    try { detail = ((await res.json()) as { error?: string }).error ?? detail; } catch { /* noop */ }
    throw new Error(detail);
  }
  if (res.status === 204) return undefined as T;
  return res.json() as Promise<T>;
}

function httpSubscribe(h: TelemetryHandlers): () => void {
  const es = new EventSource('/events', { withCredentials: true });
  es.addEventListener('telemetry_tick', (e) => h.onTick(JSON.parse((e as MessageEvent).data)));
  es.addEventListener('udp_bind_failed', (e) => h.onBindFailed?.((e as MessageEvent).data));
  es.addEventListener('session_error', (e) => h.onError?.((e as MessageEvent).data));
  return () => es.close();
}

// ---------- Public API ----------
export const ipc = {
  getSessions: (): Promise<SessionRow[]> =>
    isTauri ? tauriInvoke('get_sessions') : http('/api/sessions'),

  getSessionPackets: (sessionId: number): Promise<TelemetryPacket[]> =>
    isTauri ? tauriInvoke('get_session_packets', { sessionId })
            : http(`/api/sessions/${sessionId}/packets`),

  getSessionLaps: (sessionId: number): Promise<SessionLap[]> =>
    isTauri ? tauriInvoke('get_session_laps', { sessionId })
            : http(`/api/sessions/${sessionId}/laps`),

  deleteSession: (sessionId: number): Promise<void> =>
    isTauri ? tauriInvoke('delete_session', { sessionId })
            : http(`/api/sessions/${sessionId}`, { method: 'DELETE' }),

  clearAllSessions: (): Promise<void> =>
    isTauri ? tauriInvoke('clear_all_sessions') : http('/api/sessions', { method: 'DELETE' }),

  renameSession: (sessionId: number, name: string | null): Promise<void> =>
    isTauri ? tauriInvoke('rename_session', { sessionId, name })
            : http(`/api/sessions/${sessionId}/rename`, {
                method: 'POST', headers: { 'content-type': 'application/json' },
                body: JSON.stringify({ name }),
              }),

  setSessionBookmark: (sessionId: number, bookmarked: boolean): Promise<void> =>
    isTauri ? tauriInvoke('set_session_bookmark', { sessionId, bookmarked })
            : http(`/api/sessions/${sessionId}/bookmark`, {
                method: 'POST', headers: { 'content-type': 'application/json' },
                body: JSON.stringify({ bookmarked }),
              }),

  getDriftRuns: (): Promise<DriftRunRow[]> =>
    isTauri ? tauriInvoke('get_drift_runs') : http('/api/drift-runs'),

  getDriftRunPackets: (runId: number): Promise<DriftRunPackets> =>
    isTauri ? tauriInvoke('get_drift_run_packets', { runId })
            : http(`/api/drift-runs/${runId}/packets`),

  recomputeDriftScores: (): Promise<number> =>
    isTauri ? tauriInvoke('recompute_drift_scores')
            : http('/api/drift-runs/recompute', { method: 'POST' }),

  getDriftRunStatus: (): Promise<DriftRunStatus> =>
    isTauri ? tauriInvoke('get_drift_run_status') : http('/api/drift-runs/status'),

  abortDriftRun: (): Promise<DriftRunStatus> =>
    isTauri ? tauriInvoke('abort_drift_run')
            : http('/api/drift-runs/abort', { method: 'POST' }),

  setDriftRunManualScore: (
    runId: number,
    manualScore: number,
    notes: string | null
  ): Promise<void> =>
    isTauri ? tauriInvoke('set_drift_run_manual_score', { runId, manualScore, notes })
            : http(`/api/drift-runs/${runId}/manual-score`, {
                method: 'POST', headers: { 'content-type': 'application/json' },
                body: JSON.stringify({ manualScore, notes }),
              }),

  deleteDriftRun: (runId: number): Promise<void> =>
    isTauri ? tauriInvoke('delete_drift_run', { runId })
            : http(`/api/drift-runs/${runId}`, { method: 'DELETE' }),

  deleteInvalidDriftRuns: (): Promise<number> =>
    isTauri ? tauriInvoke('delete_invalid_drift_runs')
            : http('/api/drift-runs/purge-invalid', { method: 'POST' }),

  getDriftZones: (): Promise<DriftZoneRow[]> =>
    isTauri ? tauriInvoke('get_drift_zones') : http('/api/drift-zones'),

  saveDriftZone: (zone: DriftZoneInput): Promise<DriftZoneRow> => {
    if (isTauri) return tauriInvoke('save_drift_zone', { zone });
    const path = zone.id ? `/api/drift-zones/${zone.id}` : '/api/drift-zones';
    return http(path, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(zone),
    });
  },

  deleteDriftZone: (zoneId: number): Promise<void> =>
    isTauri ? tauriInvoke('delete_drift_zone', { zoneId })
            : http(`/api/drift-zones/${zoneId}`, { method: 'DELETE' }),

  getSettings: (): Promise<AppSettings> =>
    isTauri ? tauriInvoke('get_settings') : http('/api/settings'),

  saveSettings: (s: AppSettings): Promise<void> =>
    isTauri ? tauriInvoke('save_settings', { newSettings: s })
            : http('/api/settings', {
                method: 'POST', headers: { 'content-type': 'application/json' },
                body: JSON.stringify(s),
              }),

  getAppVersion: (): Promise<string> =>
    isTauri ? tauriVersion() : http<{ version: string }>('/api/version').then((v) => v.version),

  subscribeTelemetry: (h: TelemetryHandlers): Promise<() => void> | (() => void) =>
    isTauri ? tauriSubscribe(h) : httpSubscribe(h),

  subscribeDriftRunStatus: async (
    handler: (event: DriftRunStatus) => void
  ): Promise<() => void> => {
    if (isTauri) {
      const { listen } = await import('@tauri-apps/api/event');
      return listen<DriftRunStatus>('drift_run_status', (e) => handler(e.payload));
    }
    const es = new EventSource('/events', { withCredentials: true });
    es.addEventListener('drift_run_status', (e) => handler(JSON.parse((e as MessageEvent).data)));
    return () => es.close();
  },

  subscribeDriftZoneCapture: async (
    handler: (event: DriftZoneCaptureShortcut) => void
  ): Promise<() => void> => {
    if (!isTauri) return () => {};
    const { listen } = await import('@tauri-apps/api/event');
    return listen<DriftZoneCaptureShortcut>('drift_zone_capture', (e) => handler(e.payload));
  },
};
