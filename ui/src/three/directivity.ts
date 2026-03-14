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

const PIE_STEPS = 48;

/**
 * Filled polar‐pattern "pizza pie" on the ground plane (Y = 0).
 * Forward direction = local +Z.  Caller rotates around Y to orient.
 */
export function createGroundPie(
  pattern: DirectivityPattern,
  color: number,
  radius: number = 0.6,
  opacity: number = 0.2,
): THREE.Mesh {
  // Center vertex + perimeter vertices
  const positions = new Float32Array((PIE_STEPS + 2) * 3);
  for (let j = 0; j <= PIE_STEPS; j++) {
    const theta = (j / PIE_STEPS) * Math.PI * 2 - Math.PI; // −π → π
    const gain = patternGain(pattern, Math.abs(theta));
    const r = gain * radius;
    const idx = (j + 1) * 3;
    positions[idx]     = r * Math.sin(theta); // X lateral
    positions[idx + 1] = 0;                    // Y up
    positions[idx + 2] = r * Math.cos(theta);  // Z forward
  }
  const indices: number[] = [];
  for (let j = 0; j < PIE_STEPS; j++) {
    indices.push(0, j + 1, j + 2);
  }
  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.BufferAttribute(positions, 3));
  geo.setIndex(indices);
  return new THREE.Mesh(geo, new THREE.MeshBasicMaterial({
    color, transparent: true, opacity, side: THREE.DoubleSide, depthWrite: false,
  }));
}
