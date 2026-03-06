import * as THREE from 'three';
import type { SceneContext } from './scene.js';
import type { AtriumStore } from '../store.js';
import { toThree } from './coords.js';
import { createPatternMesh } from './directivity.js';
import type { DirectivityPattern } from '../types.js';

const HEARING_PATTERN: DirectivityPattern = {
  type: 'cone', inner: 15 * Math.PI / 180, outer: 45 * Math.PI / 180, outerGain: 0.3,
};

let listenerGroup: THREE.Group;
let listenerSphere: THREE.Mesh;

export function getListenerGroup(): THREE.Group {
  return listenerGroup;
}

export function getListenerSphere(): THREE.Mesh {
  return listenerSphere;
}

export function buildListener(ctx: SceneContext) {
  if (listenerGroup) ctx.scene.remove(listenerGroup);

  listenerGroup = new THREE.Group();

  // Body sphere
  listenerSphere = new THREE.Mesh(
    new THREE.SphereGeometry(0.15, 16, 16),
    new THREE.MeshStandardMaterial({ color: 0xffffff, emissive: 0x333333 }),
  );
  listenerGroup.add(listenerSphere);

  // Direction arrow
  const arrowGeo = new THREE.ConeGeometry(0.08, 0.3, 8);
  arrowGeo.rotateX(Math.PI / 2);
  arrowGeo.translate(0, 0, 0.25);
  const arrowMesh = new THREE.Mesh(
    arrowGeo,
    new THREE.MeshStandardMaterial({ color: 0x4fc3f7, emissive: 0x1a6080 }),
  );
  listenerGroup.add(arrowMesh);

  // Hearing pattern wireframe
  const hearingMesh = createPatternMesh(HEARING_PATTERN, 0x4fc3f7);
  listenerGroup.add(hearingMesh);

  ctx.scene.add(listenerGroup);
}

export function updateListener(store: AtriumStore) {
  if (!listenerGroup) return;
  listenerGroup.position.copy(toThree(store.listener.x, store.listener.y, store.listener.z));
  listenerGroup.rotation.y = store.listener.yaw + Math.PI / 2;
}
