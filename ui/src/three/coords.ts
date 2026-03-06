import * as THREE from 'three';
import type { AtriumStore } from '../store.js';

let store: AtriumStore;

export function initCoords(s: AtriumStore) {
  store = s;
}

/** Atrium (Z-up) → Three.js (Y-up), with y-flip for natural map view */
export function toThree(ax: number, ay: number, az: number): THREE.Vector3 {
  return new THREE.Vector3(ax, az, store.room.depth - ay);
}

/** Three.js → Atrium coordinates */
export function toAtrium(tv: THREE.Vector3): { x: number; y: number; z: number } {
  return { x: tv.x, y: store.room.depth - tv.z, z: tv.y };
}
