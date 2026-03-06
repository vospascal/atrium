/** Sound field energy computation — runs off main thread at telemetry rate (~15 Hz) */

interface SourceData {
  x: number;
  y: number;
  z: number;
  spl: number;
  refDist: number;
  orientation: number;
  patternType: 'omni' | 'polar' | 'cone';
  patternAlpha: number;
  muted: boolean;
}

interface ComputeMessage {
  type: 'init';
  points: Float32Array; // Nx3 flat array
}

interface UpdateMessage {
  type: 'update';
  sources: SourceData[];
  splReference: number;
}

function patternGain(patternType: string, alpha: number, angle: number): number {
  if (patternType === 'omni') return 1.0;
  if (patternType === 'polar') {
    return Math.max(0, alpha + (1 - alpha) * Math.cos(angle));
  }
  return 1.0;
}

let cachedPoints: Float32Array | null = null;
let pointCount = 0;

self.onmessage = (e: MessageEvent<ComputeMessage | UpdateMessage>) => {
  const msg = e.data;

  if (msg.type === 'init') {
    cachedPoints = msg.points;
    pointCount = msg.points.length / 3;
    return;
  }

  if (msg.type === 'update' && cachedPoints) {
    const { sources, splReference } = msg;
    const energy = new Float32Array(pointCount);

    // For each sample point, sum energy contributions from all sources
    for (let i = 0; i < pointCount; i++) {
      const px = cachedPoints[i * 3];
      const py = cachedPoints[i * 3 + 1];
      const pz = cachedPoints[i * 3 + 2];
      let totalEnergy = 0;

      for (const src of sources) {
        if (src.muted) continue;
        const dx = px - src.x;
        const dy = py - src.y;
        const dz = pz - src.z;
        const dist = Math.sqrt(dx * dx + dy * dy + dz * dz);

        // Distance attenuation (inverse model)
        const gainDist = dist <= src.refDist ? 1.0 : src.refDist / dist;

        // Directivity: angle from source forward to point
        const angle = Math.atan2(Math.sqrt(dx * dx + dz * dz), dy);
        const gainEmit = patternGain(src.patternType, src.patternAlpha, Math.abs(angle - src.orientation));

        // Energy contribution (proportional to amplitude squared)
        const amplitude = src.spl / splReference;
        totalEnergy += amplitude * amplitude * gainDist * gainDist * gainEmit;
      }

      energy[i] = totalEnergy;
    }

    // Normalize against fixed reference (energy at refDist from loudest source)
    // This makes colors represent absolute energy, not relative
    let refEnergy = 0;
    for (const src of sources) {
      if (src.muted) continue;
      const a = src.spl / splReference;
      refEnergy += a * a; // energy at refDist where gainDist = 1.0
    }
    if (refEnergy > 0) {
      const inv = 1.0 / refEnergy;
      for (let i = 0; i < pointCount; i++) {
        energy[i] = Math.min(1.0, energy[i] * inv);
      }
    }

    // Transfer back (zero-copy)
    (self as unknown as Worker).postMessage({ type: 'result', energy }, [energy.buffer]);
  }
};
