import * as THREE from 'three';
import type { DirectivityPattern } from '../types.js';

/** Evaluate directivity gain at angle (mirrors Rust DirectivityPattern::gain_at_angle) */
export function patternGain(pattern: DirectivityPattern, angle: number): number {
  if (pattern.type === 'omni') return 1.0;
  if (pattern.type === 'polar') {
    return Math.max(0, (pattern.alpha ?? 0) + (1 - (pattern.alpha ?? 0)) * Math.cos(angle));
  }
  if (pattern.type === 'cone') {
    if (angle <= (pattern.inner ?? 0)) return 1.0;
    if (angle >= (pattern.outer ?? Math.PI)) return pattern.outerGain ?? 0;
    const t = (angle - (pattern.inner ?? 0)) / ((pattern.outer ?? Math.PI) - (pattern.inner ?? 0));
    return 1.0 + t * ((pattern.outerGain ?? 0) - 1.0);
  }
  return 1.0;
}

const POLAR_STEPS = 48;
const RING_STEPS = 24;
const PATTERN_SCALE = 0.8;

/** Generate a 3D directivity pattern as a wireframe surface of revolution */
export function createPatternMesh(pattern: DirectivityPattern, color: number): THREE.LineSegments {
  const vertices: number[] = [];
  const indices: number[] = [];

  for (let p = 0; p <= POLAR_STEPS; p++) {
    const theta = (p / POLAR_STEPS) * Math.PI;
    const gain = patternGain(pattern, theta);
    const r = gain * PATTERN_SCALE;
    const rPerp = r * Math.sin(theta);
    const zAlongFwd = r * Math.cos(theta);

    for (let a = 0; a <= RING_STEPS; a++) {
      const phi = (a / RING_STEPS) * Math.PI * 2;
      vertices.push(
        rPerp * Math.cos(phi),
        rPerp * Math.sin(phi),
        zAlongFwd,
      );
    }
  }

  for (let p = 0; p < POLAR_STEPS; p++) {
    for (let a = 0; a < RING_STEPS; a++) {
      const curr = p * (RING_STEPS + 1) + a;
      const next = curr + 1;
      const below = curr + (RING_STEPS + 1);
      indices.push(curr, next);
      indices.push(curr, below);
    }
  }

  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.Float32BufferAttribute(vertices, 3));
  geo.setIndex(indices);

  return new THREE.LineSegments(geo, new THREE.LineBasicMaterial({
    color, transparent: true, opacity: 0.15,
  }));
}
