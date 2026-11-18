// The engine's WebSocket protocol (src/server/messages.rs, src/config/scene_json.rs,
// src/engine/telemetry.rs in the Rust crate). Port 3333.

export type RenderModeName = 'world_locked' | 'vbap' | 'hrtf' | 'dbap' | 'ambisonics'
export type ChannelModeName = 'stereo' | 'quad' | '5.1'

export interface SceneSpeaker {
  label: string
  x: number
  y: number
  z: number
  channel: number
}

export interface SceneSource {
  name: string
  color: string
  spl: number
  position: [number, number, number]
  orbit_radius: number
  orbit_speed: number
  synth_kind: string | null
  emitter_kind: string
}

/** Sent once on connect and again after `reset_scene`. Positions are world coordinates. */
export interface SceneState {
  type: 'scene_state'
  room: { width: number; depth: number; height: number }
  atrium: { width: number; depth: number; height: number }
  spawn: { x: number; y: number; z: number }
  listener: { x: number; y: number; z: number; yaw: number }
  master_gain: number
  render_mode: RenderModeName
  channel_mode: ChannelModeName
  render_modes: { mode: RenderModeName; channel_modes: ChannelModeName[] }[]
  speakers: SceneSpeaker[]
  total_channels: number
  lfe_channel: number | null
  sources: SceneSource[]
  atmosphere: { temperature_c: number; humidity_pct: number; pressure_kpa: number }
}

export interface SourceTelemetry {
  x: number
  y: number
  z: number
  distance: number
  /** Level arriving at the listener, in dB relative to the source's own level. */
  db: number
  muted: boolean
  perceptual: number
}

/** Sent about 15 times a second. */
export interface Telemetry {
  type: 'telemetry'
  sources: SourceTelemetry[]
  render_mode: RenderModeName
  channel_mode: ChannelModeName
  /** Master output gain in effect, linear. */
  master_gain: number
  /** Peak of each output channel in dBFS; −120 is silence. */
  peaks_db: number[]
}

export type ServerMessage = SceneState | Telemetry

export type ClientMessage =
  | { type: 'set_render_mode'; mode: RenderModeName }
  | { type: 'set_channel_mode'; mode: ChannelModeName }
  | { type: 'set_gain'; gain: number }
  | { type: 'set_source_muted'; index: number; muted: boolean }
  | { type: 'reset_scene' }

/** Output channels a channel mode needs on the device. Quad uses channels 0, 1, 4 and 5. */
export const CHANNELS_NEEDED: Record<ChannelModeName, number> = {
  stereo: 2,
  quad: 6,
  '5.1': 6,
}

/** Highest master gain the engine accepts (`MAX_MASTER_GAIN` in messages.rs). */
export const MAX_MASTER_GAIN = 8
