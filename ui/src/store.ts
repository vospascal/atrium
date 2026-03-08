import type {
  RoomDef, ListenerDef, DistanceModelDef, AtmosphereDef,
  Source, Speaker, SourceTelemetry, SceneStateMessage, SpeakerDef,
  DirectivityPattern, SourceDef, RenderModeDef, ExperimentDef,
} from './types.js';

// Channel indices per channel mode (for speaker visibility in 3D view)
export const CHANNEL_MODE_CHANNELS: Record<string, number[]> = {
  stereo: [0, 1],
  quad: [0, 1, 4, 5],
  '5.1': [0, 1, 2, 4, 5],
};

function parseColor(hex: string): number {
  return parseInt(hex.replace('#', ''), 16);
}

function parsePattern(directivity: string, alpha: number): DirectivityPattern {
  if (directivity === 'omni') return { type: 'omni' };
  return { type: 'polar', alpha };
}

function parseSource(s: SourceDef): Source {
  return {
    name: s.name,
    color: parseColor(s.color),
    cx: s.position[0],
    cy: s.position[1],
    x: s.position[0],
    y: s.position[1],
    z: s.position[2],
    r: s.orbit_radius,
    speed: s.orbit_speed,
    spl: s.spl,
    refDist: s.ref_dist,
    amplitude: s.amplitude,
    audibleR: s.audible_radius,
    pattern: parsePattern(s.directivity, s.directivity_alpha),
    spread: s.spread,
  };
}

export class AtriumStore extends EventTarget {
  room: RoomDef = { width: 6, depth: 4, height: 3 };
  listener: ListenerDef = { x: 3, y: 2, z: 0, yaw: Math.PI / 2 };
  distModel: DistanceModelDef = { ref_distance: 1.0, max_distance: 20.0, rolloff: 1.0 };
  atmosphere: AtmosphereDef = { temperature_c: 20, humidity_pct: 50 };
  renderMode = 'vbap';
  channelMode = '5.1';
  renderModes: RenderModeDef[] = [];
  experiments: ExperimentDef[] = [];
  /** Active value per experiment name. */
  experimentValues: Record<string, string> = {};
  masterGain = 0.7;

  sources: Source[] = [];
  speakers: Speaker[] = [
    { label: 'FL', x: 0, y: 4, z: 0, channel: 0, color: 0x66bb6a },
    { label: 'FR', x: 6, y: 4, z: 0, channel: 1, color: 0x66bb6a },
    { label: 'C', x: 3, y: 4, z: 0, channel: 2, color: 0x66bb6a },
    { label: 'RL', x: 0, y: 0, z: 0, channel: 4, color: 0x66bb6a },
    { label: 'RR', x: 6, y: 0, z: 0, channel: 5, color: 0x66bb6a },
  ];

  telemetry: SourceTelemetry[] | null = null;
  connected = false;

  // Mute/pause state
  sourceMuted: boolean[] = [];
  sourceOrigSpeed: number[] = [];
  sourcePaused: boolean[] = [];
  sourceDragging: number | null = null;

  // WebSocket send function — injected by connection.ts
  private _send: ((msg: object) => void) | null = null;

  readonly startTime = performance.now() / 1000;

  setSendFn(fn: (msg: object) => void) {
    this._send = fn;
  }

  send(msg: object) {
    this._send?.(msg);
  }

  private emit(event: string) {
    this.dispatchEvent(new Event(event));
  }

  // === Handle messages from server ===

  handleSceneState(msg: SceneStateMessage) {
    if (msg.room) {
      this.room = msg.room;
    }
    if (msg.distance_model) {
      this.distModel = msg.distance_model;
    }
    if (msg.listener) {
      this.listener = { ...msg.listener };
    }
    if (msg.render_mode) {
      this.renderMode = msg.render_mode;
    }
    if (msg.channel_mode) {
      this.channelMode = msg.channel_mode;
    }
    if (msg.render_modes) {
      this.renderModes = msg.render_modes;
    }
    if (msg.experiments) {
      this.experiments = msg.experiments;
      // Default each experiment to its first value if not already set
      for (const exp of msg.experiments) {
        if (!this.experimentValues[exp.name] && exp.values.length > 0) {
          this.experimentValues[exp.name] = exp.values[0];
        }
      }
    }
    if (msg.atmosphere) {
      this.atmosphere = { ...msg.atmosphere };
    }
    if (msg.master_gain !== undefined) {
      this.masterGain = msg.master_gain;
    }
    if (msg.speakers) {
      this.speakers = msg.speakers.map((sp: SpeakerDef) => ({
        label: sp.label, x: sp.x, y: sp.y, z: sp.z,
        channel: sp.channel, color: 0x66bb6a,
      }));
    }
    if (msg.sources) {
      this.sources = msg.sources.map(parseSource);
      this.sourceMuted = this.sources.map(() => false);
      this.sourceOrigSpeed = this.sources.map(s => s.speed);
      this.sourcePaused = this.sources.map(() => false);
    }
    this.emit('scene-rebuild');
    this.emit('update');
  }

  handleSpeakerLayout(msg: { speakers: SpeakerDef[] }) {
    this.speakers = msg.speakers.map((sp: SpeakerDef) => ({
      label: sp.label, x: sp.x, y: sp.y, z: sp.z,
      channel: sp.channel, color: 0x66bb6a,
    }));
    this.emit('scene-rebuild');
    this.emit('update');
  }

  handleTelemetry(sources: SourceTelemetry[]) {
    this.telemetry = sources;
    this.emit('telemetry');
  }

  // === Commands ===

  setListener(x: number, y: number, z: number, yaw: number) {
    this.listener = { x, y, z, yaw };
    this.send({ type: 'set_listener', x, y, z, yaw });
    this.emit('update');
  }

  setMasterGain(gain: number) {
    this.masterGain = gain;
    this.send({ type: 'set_gain', gain });
    this.emit('update');
  }

  setRenderMode(mode: string) {
    this.renderMode = mode;
    this.send({ type: 'set_render_mode', mode });
    // Auto-select a valid channel mode if current is invalid for new render mode
    const def = this.renderModes.find(m => m.mode === mode);
    const valid = def?.channel_modes ?? [];
    if (!valid.includes(this.channelMode) && valid.length > 0) {
      this.setChannelMode(valid[valid.length - 1]);
    }
    this.emit('update');
  }

  setChannelMode(mode: string) {
    this.channelMode = mode;
    this.send({ type: 'set_channel_mode', mode });
    this.emit('update');
  }

  setSourceMuted(index: number, muted: boolean) {
    this.sourceMuted[index] = muted;
    this.send({ type: 'set_source_muted', index, muted });
    this.emit('update');
  }

  setSourcePosition(index: number, x: number, y: number, z: number) {
    const s = this.sources[index];
    s.x = x; s.y = y; s.cx = x; s.cy = y; s.r = 0; s.speed = 0;
    this.send({ type: 'set_source_position', index, x, y, z });
    this.emit('update');
  }

  setSourceOrbitRadius(index: number, radius: number) {
    this.sources[index].r = radius;
    this.send({ type: 'set_source_orbit_radius', index, radius });
  }

  setSourceOrbitSpeed(index: number, speed: number) {
    this.sources[index].speed = speed;
    this.send({ type: 'set_source_orbit_speed', index, speed });
  }

  setSourceOrbitAngle(index: number, angle: number) {
    this.sources[index]._frozenAngle = angle;
    this.send({ type: 'set_source_orbit_angle', index, angle });
  }

  toggleSourcePause(index: number) {
    const s = this.sources[index];
    this.sourcePaused[index] = !this.sourcePaused[index];
    if (this.sourcePaused[index]) {
      const elapsed = performance.now() / 1000 - this.startTime;
      s._frozenAngle = (s.speed * elapsed) % (Math.PI * 2);
      s.speed = 0;
      this.send({ type: 'set_source_orbit_speed', index, speed: 0 });
      this.send({ type: 'set_source_orbit_angle', index, angle: s._frozenAngle });
    } else {
      s.speed = this.sourceOrigSpeed[index];
      this.send({ type: 'set_source_orbit_speed', index, speed: s.speed });
    }
    this.emit('update');
  }

  setAtmosphere(temperature: number, humidity: number) {
    this.atmosphere = { temperature_c: temperature, humidity_pct: humidity };
    this.send({ type: 'set_atmosphere', temperature, humidity });
    this.emit('update');
  }

  setExperiment(name: string, value: string) {
    this.experimentValues[name] = value;
    this.send({ type: 'set_experiment', name, value });
    this.emit('update');
  }

  resetScene() {
    this.send({ type: 'reset_scene' });
  }

  sendCurrentState() {
    this.send({
      type: 'set_listener',
      x: this.listener.x, y: this.listener.y,
      z: this.listener.z, yaw: this.listener.yaw,
    });
    this.send({ type: 'set_gain', gain: this.masterGain });
    this.send({ type: 'set_render_mode', mode: this.renderMode });
  }

  setConnected(connected: boolean) {
    this.connected = connected;
    this.emit('update');
  }
}
