import * as THREE from 'three';
import type { SceneContext } from './scene.js';
import type { AtriumStore } from '../store.js';
import { toThree } from './coords.js';
import { createGroundPie } from './directivity.js';
import type { DirectivityPattern } from '../types.js';

const HEARING_PATTERN: DirectivityPattern = {
  type: 'cone', inner: 15 * Math.PI / 180, outer: 45 * Math.PI / 180, outerGain: 0.3,
};

let listenerGroup: THREE.Group;
let listenerSphere: THREE.Mesh;
let hearingPie: THREE.Mesh;

export function getListenerGroup(): THREE.Group {
  return listenerGroup;
}

export function getListenerSphere(): THREE.Mesh {
  return listenerSphere;
}

export function buildListener(ctx: SceneContext) {
  if (listenerGroup) ctx.scene.remove(listenerGroup);
  if (hearingPie) ctx.scene.remove(hearingPie);

  listenerGroup = new THREE.Group();

  // Body sphere
  listenerSphere = new THREE.Mesh(
    new THREE.SphereGeometry(0.15, 16, 16),
    new THREE.MeshStandardMaterial({ color: 0xffffff, emissive: 0x333333 }),
  );
  listenerGroup.add(listenerSphere);

  ctx.scene.add(listenerGroup);

  // Hearing pattern — ground pie (same style as source directivity pies)
  hearingPie = createGroundPie(HEARING_PATTERN, 0x4fc3f7, 0.8, 0.15);
  ctx.scene.add(hearingPie);
}

export function updateListener(store: AtriumStore) {
  if (!listenerGroup) return;
  const pos = toThree(store.listener.x, store.listener.y, store.listener.z);
  listenerGroup.position.copy(pos);

  // Hearing pie sits on the ground, rotated to match listener yaw
  if (hearingPie) {
    hearingPie.position.set(pos.x, 0.02, pos.z);
    hearingPie.rotation.y = store.listener.yaw + Math.PI / 2;
  }
}
