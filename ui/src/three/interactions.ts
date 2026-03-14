import * as THREE from 'three';
import type { SceneContext } from './scene.js';
import type { AtriumStore } from '../store.js';
import { getListenerSphere } from './listener.js';
import { getSourceMeshes } from './sources.js';

let dragTarget: 'listener' | number | null = null;
let spaceHeld = false;

/** Keyboard state for WASD movement (read by animation loop). */
export const keys: Record<string, boolean> = {};

export function setupInteractions(ctx: SceneContext, store: AtriumStore) {
  const raycaster = new THREE.Raycaster();
  const mouse = new THREE.Vector2();
  const dragPlane = new THREE.Plane(new THREE.Vector3(0, 1, 0), 0);
  const intersection = new THREE.Vector3();

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return;

    mouse.x = (e.clientX / window.innerWidth) * 2 - 1;
    mouse.y = -(e.clientY / window.innerHeight) * 2 + 1;
    raycaster.setFromCamera(mouse, ctx.camera);

    // Check listener
    if (raycaster.intersectObject(getListenerSphere(), true).length > 0) {
      dragTarget = 'listener';
      ctx.controls.enabled = false;
      return;
    }

    // Check sources
    const meshes = getSourceMeshes();
    for (let i = 0; i < meshes.length; i++) {
      if (raycaster.intersectObject(meshes[i]).length > 0) {
        dragTarget = i;
        store.sourceDragging = i;
        ctx.controls.enabled = false;
        return;
      }
    }
  }

  function onPointerMove(e: PointerEvent) {
    if (dragTarget === null) return;

    mouse.x = (e.clientX / window.innerWidth) * 2 - 1;
    mouse.y = -(e.clientY / window.innerHeight) * 2 + 1;
    raycaster.setFromCamera(mouse, ctx.camera);

    if (!raycaster.ray.intersectPlane(dragPlane, intersection)) return;

    if (dragTarget === 'listener') {
      const margin = 0.2;
      const x = Math.max(margin, Math.min(store.room.width - margin, intersection.x));
      const y = Math.max(margin, Math.min(store.room.depth - margin, store.room.depth - intersection.z));
      store.setListener(x, y, store.listener.z, store.listener.yaw);
    } else {
      const i = dragTarget;
      const ax = intersection.x;
      const ay = store.room.depth - intersection.z;
      store.setSourcePosition(i, ax, ay, store.sources[i].z);
    }
  }

  function onPointerUp() {
    if (dragTarget !== null) {
      store.sourceDragging = null;
      dragTarget = null;
      ctx.controls.enabled = true;
    }
  }

  function onWheel(e: WheelEvent) {
    if (e.target !== ctx.renderer.domElement) return;
    if (!spaceHeld) return;
    let yaw = store.listener.yaw - e.deltaY * 0.003;
    yaw = ((yaw % (Math.PI * 2)) + Math.PI * 2) % (Math.PI * 2);
    store.setListener(store.listener.x, store.listener.y, store.listener.z, yaw);
    e.preventDefault();
  }

  ctx.renderer.domElement.addEventListener('pointerdown', onPointerDown);
  ctx.renderer.domElement.addEventListener('pointermove', onPointerMove);
  ctx.renderer.domElement.addEventListener('pointerup', onPointerUp);
  ctx.renderer.domElement.addEventListener('wheel', onWheel, { passive: false });

  window.addEventListener('keydown', (e) => {
    keys[e.code] = true;
    if (e.code === 'Space') {
      spaceHeld = true;
      ctx.controls.enableZoom = false;
      ctx.controls.mouseButtons.RIGHT = THREE.MOUSE.PAN;
      e.preventDefault();
    }
  });
  window.addEventListener('keyup', (e) => {
    keys[e.code] = false;
    if (e.code === 'Space') {
      spaceHeld = false;
      ctx.controls.enableZoom = true;
      ctx.controls.mouseButtons.RIGHT = THREE.MOUSE.ROTATE;
    }
  });
}
