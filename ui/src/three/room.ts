import * as THREE from 'three';
import type { SceneContext } from './scene.js';
import type { AtriumStore } from '../store.js';

let roomWire: THREE.LineSegments | null = null;
let gridHelper: THREE.GridHelper | null = null;

export function buildRoom(ctx: SceneContext, store: AtriumStore) {
  // Remove old
  if (roomWire) ctx.scene.remove(roomWire);
  if (gridHelper) ctx.scene.remove(gridHelper);

  const { width: w, depth: d, height: h } = store.room;

  // Room wireframe
  const geo = new THREE.BoxGeometry(w, h, d);
  roomWire = new THREE.LineSegments(
    new THREE.EdgesGeometry(geo),
    new THREE.LineBasicMaterial({ color: 0x444444 }),
  );
  roomWire.position.set(w / 2, h / 2, d / 2);
  ctx.scene.add(roomWire);

  // Floor grid
  const gridSize = 100;
  gridHelper = new THREE.GridHelper(gridSize, gridSize, 0x333333, 0x222222);
  gridHelper.position.set(w / 2, 0, d / 2);
  ctx.scene.add(gridHelper);
}
