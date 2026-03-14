/** Sound field energy computation — runs off main thread at telemetry rate (~15 Hz).
 *  Outputs normalised dB values: 0 = at or below hearing threshold, 1 = source SPL at ref distance. */

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
  splThreshold: number; // dB SPL below which particles are invisible (e.g. 20)
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
    const { sources, splThreshold } = msg;
    const energy = new Float32Array(pointCount);

    for (let i = 0; i < pointCount; i++) {
      const px = cachedPoints[i * 3];
      const py = cachedPoints[i * 3 + 1];
      const pz = cachedPoints[i * 3 + 2];

      // Sum pressure squared from all sources (energy addition)
      let pressureSqSum = 0;

      for (const src of sources) {
        if (src.muted) continue;
        const dx = px - src.x;
        const dy = py - src.y;
        const dz = pz - src.z;
        const dist = Math.sqrt(dx * dx + dy * dy + dz * dz);

        // Distance attenuation (inverse distance law)
        const gainDist = dist <= src.refDist ? 1.0 : src.refDist / dist;

        // Directivity
        const angle = Math.atan2(Math.sqrt(dx * dx + dz * dz), dy);
        const gainEmit = patternGain(src.patternType, src.patternAlpha, Math.abs(angle - src.orientation));

        // SPL at this point = source SPL + 20*log10(gainDist * gainEmit)
        // Pressure proportional to 10^(spl/20), so p² ∝ 10^(spl/10)
        const splAtPoint = src.spl + 20 * Math.log10(Math.max(1e-10, gainDist * gainEmit));
        const pressureSq = Math.pow(10, splAtPoint / 10);
        pressureSqSum += pressureSq;
      }

      // Convert summed pressure² back to dB SPL
      const splTotal = pressureSqSum > 0 ? 10 * Math.log10(pressureSqSum) : -Infinity;

      // Normalise: 0 at threshold, 1 at max source SPL
      // Find the dB range from threshold to the loudest source
      if (splTotal <= splThreshold) {
        energy[i] = 0;
      } else {
        // Map threshold..maxSPL → 0..1
        let maxSpl = 0;
        for (const src of sources) {
          if (!src.muted && src.spl > maxSpl) maxSpl = src.spl;
        }
        const range = maxSpl - splThreshold;
        energy[i] = range > 0
          ? Math.min(1.0, (splTotal - splThreshold) / range)
          : 0;
      }
    }

    // Transfer back (zero-copy)
    (self as unknown as Worker).postMessage({ type: 'result', energy }, [energy.buffer]);
  }
};
