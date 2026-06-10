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
}

export interface DriftRunRow {
  id: number;
  /** Per-zone display number (1, 2, 3… within the zone, oldest first). Derived
   *  at query time, reflows on delete. `id` is still the global identity. */
  zoneRunNumber: number;
  zoneId: number | null;
  startedAt: number;
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

export interface DriftRunStatus {
  state: 'idle' | 'running' | 'completed' | 'invalid';
  runId: number | null;
  zoneId: number | null;
  zoneName: string | null;
  startedAt: number | null;
  endedAt: number | null;
  packetCount: number;
  invalidReason: string | null;
  /** Latest packet is earning points (drifting with a tyre on tarmac). */
  scoring: boolean;
  /** Seconds left before score-starvation aborts the run; null when scoring,
   *  idle/closed, or the starvation timer is disabled. */
  starveRemainingS: number | null;
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
  /** Seconds a drift run may go without scoring before it aborts. 0 disables. */
  driftStarveTimeoutS: number;
  /** Seconds of pre-run telemetry stored as each run's approach trail. 0 disables. */
  driftPrerollS: number;
}

export type DrivetrainLabel = 'FWD' | 'RWD' | 'AWD';
export const DRIVETRAIN_LABELS: DrivetrainLabel[] = ['FWD', 'RWD', 'AWD'];

export type CarClassLabel = 'D' | 'C' | 'B' | 'A' | 'S1' | 'S2' | 'X';
export const CAR_CLASS_LABELS: CarClassLabel[] = ['D', 'C', 'B', 'A', 'S1', 'S2', 'X'];
