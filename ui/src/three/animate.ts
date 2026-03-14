import * as THREE from 'three';
import type { SceneContext } from './scene.js';
import type { AtriumStore } from '../store.js';
import { updateListener } from './listener.js';
import { updateSources } from './sources.js';
import { updateSoundField } from './sound-field.js';
import { updateAtriumPosition } from './atrium.js';
import { keys } from './interactions.js';

const MOVE_SPEED = 3.0; // meters per second

/** Extract camera yaw in engine convention (yaw=0 → +X, yaw=π/2 → +Y).
 *  Three.js camera direction (dx, dy, dz) maps to Atrium direction (dx, -dz). */
function getCameraYaw(camera: THREE.Camera): number {
  const cameraDirection = new THREE.Vector3();
  camera.getWorldDirection(cameraDirection);
  return Math.atan2(-cameraDirection.z, cameraDirection.x);
}

function updateWASD(store: AtriumStore, camera: THREE.Camera, dt: number) {
  let dx = 0;
  let dy = 0;

  if (keys['KeyW']) dy += 1;
  if (keys['KeyS']) dy -= 1;
  if (keys['KeyA']) dx -= 1;
  if (keys['KeyD']) dx += 1;

  if (dx === 0 && dy === 0) return;

  // Normalize diagonal movement
  const length = Math.sqrt(dx * dx + dy * dy);
  dx /= length;
  dy /= length;

  const cameraYaw = getCameraYaw(camera);

  // Rotate movement into Atrium coords using engine yaw convention
  // Forward = (cos(yaw), sin(yaw)), Right = (sin(yaw), -cos(yaw))
  const cos = Math.cos(cameraYaw);
  const sin = Math.sin(cameraYaw);
  const moveX = (dy * cos + dx * sin) * MOVE_SPEED * dt;
  const moveY = (dy * sin - dx * cos) * MOVE_SPEED * dt;

  const newX = store.listener.x + moveX;
  const newY = store.listener.y + moveY;

  store.setListener(newX, newY, store.listener.z, cameraYaw);
}

export function startAnimationLoop(ctx: SceneContext, store: AtriumStore) {
  let lastTime = performance.now();

  function animate() {
    requestAnimationFrame(animate);

    const now = performance.now();
    const dt = Math.min((now - lastTime) / 1000, 0.1); // cap at 100ms
    lastTime = now;

    updateWASD(store, ctx.camera, dt);

    // Always sync listener yaw with camera facing direction —
    // the hearing cone rotates as you orbit the view
    const cameraYaw = getCameraYaw(ctx.camera);
    if (Math.abs(cameraYaw - store.listener.yaw) > 0.001) {
      store.setListener(store.listener.x, store.listener.y, store.listener.z, cameraYaw);
    }

    updateListener(store);
    updateAtriumPosition(store);
    updateSources(store);
    updateSoundField(store);

    // Keep OrbitControls target on listener when WASD is active
    if (keys['KeyW'] || keys['KeyS'] || keys['KeyA'] || keys['KeyD']) {
      ctx.controls.target.set(
        store.listener.x,
        store.listener.z,
        store.room.depth - store.listener.y,
      );
    }

    ctx.controls.update();
    ctx.renderer.render(ctx.scene, ctx.camera);
  }

  animate();
}
