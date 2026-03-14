import * as THREE from 'three';
import type { SceneContext } from './scene.js';
import type { AtriumStore } from '../store.js';

let environmentWire: THREE.LineSegments | null = null;
let gridHelper: THREE.GridHelper | null = null;

export function buildEnvironment(ctx: SceneContext, store: AtriumStore) {
  // Remove old
  if (environmentWire) ctx.scene.remove(environmentWire);
  if (gridHelper) ctx.scene.remove(gridHelper);

  const { width: w, depth: d, height: h } = store.room;

  // Environment wireframe (dark gray, fixed at world origin)
  // Only show when environment differs from atrium (otherwise it overlaps)
  const atrium = store.atrium;
  const sameAsAtrium = w === atrium.width && d === atrium.depth && h === atrium.height;
  if (!sameAsAtrium) {
    const geo = new THREE.BoxGeometry(w, h, d);
    environmentWire = new THREE.LineSegments(
      new THREE.EdgesGeometry(geo),
      new THREE.LineBasicMaterial({ color: 0x444444 }),
    );
    environmentWire.position.set(w / 2, h / 2, d / 2);
    ctx.scene.add(environmentWire);
  }

  // Floor grid
  const gridSize = 100;
  gridHelper = new THREE.GridHelper(gridSize, gridSize, 0x333333, 0x222222);
  gridHelper.position.set(w / 2, 0, d / 2);
  ctx.scene.add(gridHelper);
}
