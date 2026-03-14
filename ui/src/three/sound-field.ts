import * as THREE from 'three';
import type { SceneContext } from './scene.js';
import type { AtriumStore } from '../store.js';
import { toThree } from './coords.js';

// const POINT_COUNT = 10_000;
const POINT_COUNT = 5_000; // fewer points for better perf in larger rooms

let points: THREE.Points | null = null;
let energyAttr: THREE.BufferAttribute | null = null;
let worker: Worker | null = null;
let atriumPoints: Float32Array | null = null; // Nx3 in Atrium coords (for worker)
let pendingUpdate = false;
let initialized = false;

export function buildSoundField(ctx: SceneContext, store: AtriumStore) {
  // Clean up previous
  if (points) ctx.scene.remove(points);
  if (worker) worker.terminate();
  initialized = false;

  // Scatter points within each source's audible radius (not the room box).
  // Points only exist where sound is actually present.
  const activeSources = store.sources.filter((_, i) => !store.sourceMuted[i]);
  const pointsPerSource = activeSources.length > 0
    ? Math.floor(POINT_COUNT / activeSources.length)
    : 0;

  const threePositions = new Float32Array(POINT_COUNT * 3);
  atriumPoints = new Float32Array(POINT_COUNT * 3);
  const energy = new Float32Array(POINT_COUNT); // starts at 0

  let idx = 0;
  for (const src of activeSources) {
    const radius = src.audibleR;
    const count = Math.min(pointsPerSource, POINT_COUNT - idx);
    for (let j = 0; j < count; j++) {
      // Uniform random point inside a sphere (rejection-free via cube root)
      const u = Math.random();
      const cosTheta = 2 * Math.random() - 1;
      const sinTheta = Math.sqrt(1 - cosTheta * cosTheta);
      const phi = 2 * Math.PI * Math.random();
      const r = radius * Math.cbrt(u);

      const ax = src.x + r * sinTheta * Math.cos(phi);
      const ay = src.y + r * sinTheta * Math.sin(phi);
      const az = Math.max(0, src.z + r * cosTheta); // clamp to floor

      atriumPoints[idx * 3] = ax;
      atriumPoints[idx * 3 + 1] = ay;
      atriumPoints[idx * 3 + 2] = az;

      const tv = toThree(ax, ay, az);
      threePositions[idx * 3] = tv.x;
      threePositions[idx * 3 + 1] = tv.y;
      threePositions[idx * 3 + 2] = tv.z;
      idx++;
    }
  }
  // Zero-fill any remaining slots
  for (let i = idx; i < POINT_COUNT; i++) {
    threePositions[i * 3] = 0;
    threePositions[i * 3 + 1] = -10; // offscreen
    threePositions[i * 3 + 2] = 0;
  }

  // Create geometry
  const geometry = new THREE.BufferGeometry();
  geometry.setAttribute('position', new THREE.BufferAttribute(threePositions, 3));
  energyAttr = new THREE.BufferAttribute(energy, 1);
  geometry.setAttribute('energy', energyAttr);

  // Custom shader: energy → color gradient + alpha + point size
  const material = new THREE.ShaderMaterial({
    vertexShader: `
      attribute float energy;
      varying float vEnergy;
      void main() {
        vEnergy = energy;
        vec4 mvPos = modelViewMatrix * vec4(position, 1.0);
        // Hide sub-threshold particles; tiny point for visible ones
        gl_PointSize = energy > 0.0 ? max(1.0, 8.0 / -mvPos.z) : 0.0;
        gl_Position = projectionMatrix * mvPos;
      }
    `,
    fragmentShader: `
      varying float vEnergy;
      void main() {
        float d = length(gl_PointCoord - vec2(0.5));
        if (d > 0.5) discard;

        // 3-stop gradient: dark blue → warm yellow → hot orange
        vec3 cold = vec3(0.08, 0.12, 0.35);
        vec3 warm = vec3(1.0, 0.85, 0.3);
        vec3 hot  = vec3(1.0, 0.35, 0.1);
        vec3 color = vEnergy < 0.5
          ? mix(cold, warm, vEnergy * 2.0)
          : mix(warm, hot, (vEnergy - 0.5) * 2.0);

        float alpha = smoothstep(0.0, 0.08, vEnergy) * 0.7;
        alpha *= (1.0 - 2.0 * d); // soft circle falloff

        gl_FragColor = vec4(color, alpha);
      }
    `,
    transparent: true,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
  });

  points = new THREE.Points(geometry, material);
  ctx.scene.add(points);

  // Spawn worker
  worker = new Worker(
    new URL('../workers/sound-field.worker.ts', import.meta.url),
    { type: 'module' },
  );

  worker.onmessage = (e) => {
    if (e.data.type === 'result' && energyAttr) {
      energyAttr.array = e.data.energy;
      energyAttr.needsUpdate = true;
      pendingUpdate = false;
    }
  };

  // Send static point positions to worker (once)
  const pointsCopy = new Float32Array(atriumPoints);
  worker.postMessage({ type: 'init', points: pointsCopy }, [pointsCopy.buffer]);
  initialized = true;
}

/** Called each frame from animate loop — sends update to worker on new telemetry */
export function updateSoundField(store: AtriumStore) {
  if (!initialized || !worker || !store.telemetry || pendingUpdate) return;

  const sources = store.sources.map((s, i) => {
    const t = store.telemetry![i];
    return {
      x: t?.x ?? s.x,
      y: t?.y ?? s.y,
      z: t?.z ?? s.z,
      spl: s.spl,
      refDist: s.refDist,
      orientation: 0,
      patternType: s.pattern.type,
      patternAlpha: s.pattern.alpha ?? 0,
      muted: store.sourceMuted[i] ?? false,
    };
  });

  pendingUpdate = true;
  worker.postMessage({
    type: 'update',
    sources,
    splThreshold: 20, // dB SPL hearing threshold — particles invisible below this
  });
}
