export interface TelemetryPacket {
  isRaceOn: boolean;
  timestampMs: number;
  engineMaxRpm: number;
  engineIdleRpm: number;
  currentEngineRpm: number;
  accelX: number;
  accelY: number;
  accelZ: number;
  velX: number;
  velY: number;
  velZ: number;
  angularVelocityX: number;
  angularVelocityY: number;
  angularVelocityZ: number;
  positionX: number;
  positionY: number;
  positionZ: number;
  tireSlipRatioFl: number;
  tireSlipRatioFr: number;
  tireSlipRatioRl: number;
  tireSlipRatioRr: number;
  tireSlipAngleFl: number;
  tireSlipAngleFr: number;
  tireSlipAngleRl: number;
  tireSlipAngleRr: number;
  tireCombinedSlipFl: number;
  tireCombinedSlipFr: number;
  tireCombinedSlipRl: number;
  tireCombinedSlipRr: number;
  /** Per-wheel surface roughness: 0 on smooth tarmac, >0 on grass/dirt/gravel.
   *  The off-tarmac / scoring tarmac-gate signal. */
  surfaceRumbleFl: number;
  surfaceRumbleFr: number;
  surfaceRumbleRl: number;
  surfaceRumbleRr: number;
  carOrdinal: number;
  carClass: number;
  carPi: number;
  drivetrainType: number;
  numCylinders: number;
  carGroup: number;
  smashableVelDiff: number;
  smashableMass: number;
  speedMs: number;
  power: number;
  torque: number;
  tireTempFl: number;
  tireTempFr: number;
  tireTempRl: number;
  tireTempRr: number;
  boost: number;
  fuel: number;
  distanceTraveled: number;
  bestLap: number;
  lastLap: number;
  currentLap: number;
  currentRaceTime: number;
  lapNumber: number;
  racePosition: number;
  throttle: number;
  brake: number;
  clutch: number;
  handbrake: number;
  gear: number;
  steer: number;
  yaw: number;
  pitch: number;
  roll: number;
  suspensionFl: number;
  suspensionFr: number;
  suspensionRl: number;
  suspensionRr: number;
  tireWearFl: number | null;
  tireWearFr: number | null;
  tireWearRl: number | null;
  tireWearRr: number | null;
}

export interface SessionRow {
  id: number;
  startedAt: number;
  endedAt: number | null;
  carOrdinal: number;
  carClass: number;
  carPi: number;
  bestLap: number | null;
  packetCount: number;
  name: string | null;
  bookmarked: boolean;
}

export interface SessionLap {
  lapNumber: number;
  lapTime: number;
}

/** One sector's slice of a run (mirrors scoring::SectorScore). Index-aligned to
 *  the zone's `sectorNames` in A→B order (N splits ⇒ N+1 sectors). Summing
 *  `points` across sectors reproduces the run score. */
export interface SectorScore {
  /** Scaled points banked while in this sector. */
  points: number;
  /** Packets recorded in this sector (drifting or not). */
  sampleCount: number;
  /** Seconds spent drifting in this sector. */
  driftTimeS: number;
}

/** One detected in-game rewind (mirrors scoring::RewindEvent). A rewind reads as a
 *  telemetry gap (UDP stops) with a large backward position jump on resume; the
 *  abandoned attempt `(targetIndex, gapIndex]` is replayed, so the score
 *  double-counts it. Runs with any rewind are flagged for exclusion from the fit. */
export interface RewindEvent {
  /** Last packet index before the rewind gap (rewound FROM). */
  gapIndex: number;
  /** First packet index after the gap (the resume / rewound-TO position). */
  resumeIndex: number;
  /** Telemetry gap across the rewind (ms). */
  gapMs: number;
  /** Planar (x,z) jump across the gap (m) — large and backward for a rewind. */
  jumpM: number;
  /** Recorded packet nearest (3D) the resume — the rewind target. */
  targetIndex: number;
  /** 3D distance (m) from the resume to that nearest earlier packet. */
  resumePathDistM: number;
  /** True when the resume landed back on the recorded path (in-zone rewind); false
   *  when it left the zone (e.g. rewound out the start gate → fail/re-trigger). */
  onPath: boolean;
}

/** Per-run scoring breakdown (mirrors scoring::RunScore in the backend). */
export interface DriftScoreBreakdown {
  score: number;
  sampleCount: number;
  driftTimeS: number;
  totalTimeS: number;
  avgAngleDeg: number;
  maxAngleDeg: number;
  avgSpeedMs: number;
  maxMultiplier: number;
  transitions: number;
  /** Signed-direction reversals between scoring packets (weave jitter counts).
   *  Absent on breakdowns stored before the field existed — Recompute backfills. */
  directionFlips?: number;
  /** Points re-paid through brief sub-gate dips by the low-speed transit term
   *  (0 for normal-speed runs). Absent pre-v0.4.0 — Recompute backfills. */
  transitPts?: number;
  /** Points withheld by the low-speed flip-pause term (0 for normal-speed
   *  runs). Absent pre-v0.4.0 — Recompute backfills. */
  flipPausePts?: number;
  /** Per-sector point rollup in A→B order. Only the run-viewer re-scoring path
   *  fills this (it needs the zone split geometry); empty/absent for zones with
   *  no splits and for breakdowns stored by the live scorer. */
  sectors?: SectorScore[];
  /** In-game rewinds detected in this run (empty/absent = none). A non-empty list
   *  means the score double-counts the replayed stretch(es); the run is flagged for
   *  exclusion from the fit. Absent on breakdowns stored before the field existed —
   *  Recompute backfills it. */
  rewinds?: RewindEvent[];
}

export interface DriftRunRow {
  id: number;
  /** Per-zone display number (1, 2, 3… within the zone, oldest first). Derived
   *  at query time, reflows on delete. `id` is still the global identity. */
  zoneRunNumber: number;
  zoneId: number | null;
  startedAt: number;
  /** In-game season the run was driven in, derived server-side from startedAt
   *  against the weekly schedule. Scoring is season-aware: outside winter,
   *  off-tarmac surfaces pay. */
  season: 'spring' | 'summer' | 'autumn' | 'winter';
  endedAt: number | null;
  carOrdinal: number;
  carClass: number;
  carPi: number;
  drivetrainType: number;
  carGroup: number;
  valid: boolean;
  invalidReason: string | null;
  computedScore: number | null;
  manualScore: number | null;
  manualNotes: string | null;
  packetCount: number;
  scoreBreakdown: DriftScoreBreakdown | null;
}

/** Authoritative per-packet scoring diagnostics (mirrors scoring::TickScore).
 *  One entry per packet in `DriftRunPackets.packets`, same order. Drives the
 *  run-viewer tick colouring — scoring (green), drifting-but-unpaid (red),
 *  idle (grey) — without re-deriving the season/tarmac gate on the frontend. */
export interface TickScore {
  isDrifting: boolean;
  isScoring: boolean;
  /** Wheels on tarmac this tick (0–4). */
  tarmacWheels: number;
  /** Scaled points banked at this tick; summing all ticks reproduces the score. */
  points: number;
  /** Sector this tick falls in: splits crossed since the entry gate, monotonic
   *  (furthest-reached) and A→B-canonical, so `sectorNames[sector]` names it
   *  regardless of entry direction. 0 when the zone has no splits. */
  sector: number;
}

/** A marker for one in-game rewind (mirrors api::RewindMarker), placed where it
 *  landed on the cleaned trace. The viewer strips the replayed packets so a rewound
 *  run reads as one clean piece; this dot is the only trace of the rewind. */
export interface RewindMarker {
  positionX: number;
  positionZ: number;
  /** How far back the rewind jumped (m) — shown in the tooltip. */
  jumpM: number;
  /** False when the rewind left the zone (rewound out a gate → re-triggered). */
  onPath: boolean;
}

/** One drift run's telemetry + scoring diagnostics for the run viewer, returned by
 *  `getDriftRunPackets`. `packets` and `ticks` are index-aligned. For a rewound run
 *  these are the CLEANED one-piece run (replayed stretch removed); the dropped
 *  stretch survives only as a `rewindMarkers` entry. */
export interface DriftRunPackets {
  runId: number;
  zoneId: number | null;
  season: 'spring' | 'summer' | 'autumn' | 'winter';
  packets: TelemetryPacket[];
  ticks: TickScore[];
  score: DriftScoreBreakdown;
  /** One marker per detected rewind (empty for a clean run). */
  rewindMarkers: RewindMarker[];
}

export interface DriftRunStatus {
  state: 'idle' | 'running' | 'completed' | 'invalid';
  runId: number | null;
  zoneId: number | null;
  zoneName: string | null;
  startedAt: number | null;
  endedAt: number | null;
  packetCount: number;
  invalidReason: string | null;
  /** Latest packet is earning points (drifting with a tyre on tarmac). A live
   *  banking instrument only — it no longer gates validity (the kill is
   *  forward-progress / out-of-bounds, not scoring). */
  scoring: boolean;
  /** Seconds left before the forward-progress stall aborts the run, while it is
   *  not advancing; null when advancing, idle/closed, or the kill is disabled.
   *  The live "death timer". (Field name kept from the old score-starvation timer.) */
  starveRemainingS: number | null;
  /** Live signed drift angle (°) of the latest packet; null when idle/closed. */
  angleDeg: number | null;
  /** Live speed (m/s) of the latest packet; null when idle/closed. */
  speedMs: number | null;
  /** Direction flips so far in this run; null when idle. */
  directionFlips: number | null;
}

export interface ZonePoint {
  x: number;
  z: number;
}

export interface DriftZoneInput {
  id: number | null;
  name: string;
  description: string | null;
  active: boolean;
  leftBoundary: ZonePoint[];
  rightBoundary: ZonePoint[];
  startGate: ZonePoint[];
  finishGate: ZonePoint[];
  splitGates: ZonePoint[][];
  scoringConfig: Record<string, unknown>;
}

export interface DriftZoneRow extends DriftZoneInput {
  id: number;
  createdAt: number;
  updatedAt: number;
}

export interface AppSettings {
  port: number;
  useMph: boolean;
  tireTempCold: number;
  tireTempOptimal: number;
  tireTempHot: number;
  autoRecord: boolean;
  theme: 'dark' | 'cobalt2' | 'purple';
  mapEnabled: boolean;
  mapOverride: boolean;
  mapTileUrl: string;
  mapMinZoom: number;
  mapMaxZoom: number;
  mapTileSize: number;
  mapCalAWorld: [number, number];
  mapCalAPix: [number, number];
  mapCalBWorld: [number, number];
  mapCalBPix: [number, number];
  mapViewMaxZoom: number;
  mapDefaultZoom: number;
  mapDefaultCenter: [number, number];
  tiresVisible: boolean;
  /** Run auto-fail on/off: >0 enables the measured forward-progress / out-of-bounds
   *  kill, 0 disables it (run ends only at the finish gate or a manual abort). The
   *  fail timing is per-zone, not this value. (Name kept for settings-file compat.) */
  driftStarveTimeoutS: number;
  /** Seconds of pre-run telemetry stored as each run's approach trail. 0 disables. */
  driftPrerollS: number;
}

export type DrivetrainLabel = 'FWD' | 'RWD' | 'AWD';
export const DRIVETRAIN_LABELS: DrivetrainLabel[] = ['FWD', 'RWD', 'AWD'];

export type CarClassLabel = 'D' | 'C' | 'B' | 'A' | 'S1' | 'S2' | 'X';
export const CAR_CLASS_LABELS: CarClassLabel[] = ['D', 'C', 'B', 'A', 'S1', 'S2', 'X'];
