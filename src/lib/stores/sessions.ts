import { writable } from 'svelte/store';
import { ipc } from '$lib/ipc';
import type {
  SessionRow,
  TelemetryPacket,
  AppSettings,
  SessionLap,
  DriftRunRow,
  DriftRunStatus,
  DriftZoneInput,
  DriftZoneRow,
} from '$lib/types';

export const sessions = writable<SessionRow[]>([]);
export const driftRuns = writable<DriftRunRow[]>([]);
export const driftRunStatus = writable<DriftRunStatus>({
  state: 'idle',
  runId: null,
  zoneId: null,
  zoneName: null,
  startedAt: null,
  endedAt: null,
  packetCount: 0,
  invalidReason: null,
});
export const driftZones = writable<DriftZoneRow[]>([]);
export const settings = writable<AppSettings | null>(null);

export async function loadSessions() { sessions.set(await ipc.getSessions()); }
export async function loadSessionPackets(sessionId: number): Promise<TelemetryPacket[]> { return ipc.getSessionPackets(sessionId); }
export async function loadSessionLaps(sessionId: number): Promise<SessionLap[]> { return ipc.getSessionLaps(sessionId); }
export async function deleteSession(sessionId: number) { await ipc.deleteSession(sessionId); await loadSessions(); }
export async function clearAllSessions() { await ipc.clearAllSessions(); await loadSessions(); }
export async function renameSession(sessionId: number, name: string | null) { await ipc.renameSession(sessionId, name); await loadSessions(); }
export async function setSessionBookmark(sessionId: number, bookmarked: boolean) { await ipc.setSessionBookmark(sessionId, bookmarked); await loadSessions(); }
export async function loadDriftRuns() { driftRuns.set(await ipc.getDriftRuns()); }
export async function recomputeDriftScores(): Promise<number> {
  const n = await ipc.recomputeDriftScores();
  await loadDriftRuns();
  return n;
}
export async function loadDriftRunStatus() { driftRunStatus.set(await ipc.getDriftRunStatus()); }
export async function startDriftRunStatusListener(): Promise<() => void> {
  await loadDriftRunStatus();
  return ipc.subscribeDriftRunStatus((status) => {
    driftRunStatus.set(status);
    if (status.state === 'completed' || status.state === 'invalid') void loadDriftRuns();
  });
}
export async function setDriftRunManualScore(runId: number, manualScore: number, notes: string | null) {
  await ipc.setDriftRunManualScore(runId, manualScore, notes);
  await loadDriftRuns();
}
export async function deleteDriftRun(runId: number) {
  await ipc.deleteDriftRun(runId);
  await loadDriftRuns();
}
export async function deleteInvalidDriftRuns(): Promise<number> {
  const n = await ipc.deleteInvalidDriftRuns();
  await loadDriftRuns();
  return n;
}
export async function loadDriftZones() { driftZones.set(await ipc.getDriftZones()); }
export async function saveDriftZone(zone: DriftZoneInput): Promise<DriftZoneRow> {
  const saved = await ipc.saveDriftZone(zone);
  await loadDriftZones();
  return saved;
}
export async function deleteDriftZone(zoneId: number) {
  await ipc.deleteDriftZone(zoneId);
  await loadDriftZones();
}
export async function loadSettings(): Promise<AppSettings> { const s = await ipc.getSettings(); settings.set(s); return s; }
export async function saveSettings(s: AppSettings) { await ipc.saveSettings(s); settings.set(s); }
