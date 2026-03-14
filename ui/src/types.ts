// === Wire protocol types (mirroring Rust JSON messages) ===

export interface RoomDef {
  width: number;
  depth: number;
  height: number;
}

export interface ListenerDef {
  x: number;
  y: number;
  z: number;
  yaw: number;
}

export interface SourceDef {
  name: string;
  color: string; // "#rrggbb"
  position: [number, number, number];
  orbit_radius: number;
  orbit_speed: number;
  spl: number;
  ref_dist: number;
  amplitude: number;
  audible_radius: number;
  directivity: string;
  directivity_alpha: number;
  spread: number;
}

export interface SpeakerDef {
  label: string;
  x: number;
  y: number;
  z: number;
  channel: number;
}

export interface DistanceModelDef {
  ref_distance: number;
  max_distance: number;
  rolloff: number;
}

export interface AtmosphereDef {
  temperature_c: number;
  humidity_pct: number;
}

export interface NormalizationDef {
  spl_threshold: number;
  target_rms: number;
}

export interface RenderModeDef {
  mode: string;
  channel_modes: string[];
}

export interface ExperimentDef {
  name: string;
  values: string[];
}

export interface SceneStateMessage {
  type: 'scene_state';
  room: RoomDef;
  listener: ListenerDef;
  sources: SourceDef[];
  speakers: SpeakerDef[];
  render_mode: string;
  channel_mode: string;
  render_modes: RenderModeDef[];
  experiments?: ExperimentDef[];
  atmosphere: AtmosphereDef;
  master_gain: number;
  distance_model: DistanceModelDef;
  normalization: NormalizationDef;
}

export interface SpeakerLayoutMessage {
  type: 'speaker_layout';
  speakers: SpeakerDef[];
}

export interface SourceTelemetry {
  x: number;
  y: number;
  z: number;
  distance: number;
  dist: number;
  emit: number;
  hear: number;
  total: number;
  db: number;
  muted: boolean;
  perceptual: number;
}

export interface TelemetryMessage {
  type: 'telemetry';
  sources: SourceTelemetry[];
}

export type ServerMessage = SceneStateMessage | SpeakerLayoutMessage | TelemetryMessage;

// === Local state types ===

export interface DirectivityPattern {
  type: 'omni' | 'polar' | 'cone';
  alpha?: number;
  inner?: number;
  outer?: number;
  outerGain?: number;
}

export interface Source {
  name: string;
  color: number;
  cx: number;
  cy: number;
  x: number;
  y: number;
  z: number;
  r: number;
  speed: number;
  spl: number;
  refDist: number;
  amplitude: number;
  audibleR: number;
  pattern: DirectivityPattern;
  spread: number;
  _frozenAngle?: number;
}

export interface Speaker {
  label: string;
  x: number;
  y: number;
  z: number;
  channel: number;
  color: number;
}
