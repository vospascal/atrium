import * as THREE from 'three';
import type { SceneContext } from './scene.js';
import type { AtriumStore } from '../store.js';
import type { Speaker } from '../types.js';
import { CHANNEL_MODE_CHANNELS } from '../store.js';

let atriumGroup: THREE.Group | null = null;
let atriumWire: THREE.LineSegments | null = null;
let speakerMeshes: THREE.Group[] = [];

export function getAtriumGroup(): THREE.Group | null {
  return atriumGroup;
}

function createSpeakerMesh(sp: Speaker, spawn: { x: number; y: number }): THREE.Group {
  const group = new THREE.Group();
  const box = new THREE.Mesh(
    new THREE.BoxGeometry(0.2, 0.25, 0.15),
    new THREE.MeshStandardMaterial({ color: sp.color, emissive: sp.color, emissiveIntensity: 0.2 }),
  );
  group.add(box);

  const canvas = document.createElement('canvas');
  canvas.width = 64; canvas.height = 24;
  const ctx = canvas.getContext('2d')!;
  ctx.fillStyle = '#66bb6a';
  ctx.font = '16px monospace';
  ctx.fillText(sp.label, 4, 18);
  const tex = new THREE.CanvasTexture(canvas);
  const sprite = new THREE.Sprite(new THREE.SpriteMaterial({ map: tex, transparent: true }));
  sprite.scale.set(0.6, 0.2, 1);
  sprite.position.y = 0.25;
  group.add(sprite);

  // Speaker world position → offset from spawn (atrium center)
  // Engine X → Three.js X, Engine Y → Three.js -Z, Engine Z → Three.js Y
  const dx = sp.x - spawn.x;
  const dy = sp.y - spawn.y;
  group.position.set(dx, sp.z, -dy);

  return group;
}

export function buildAtrium(ctx: SceneContext, store: AtriumStore) {
  // Remove old
  if (atriumGroup) ctx.scene.remove(atriumGroup);

  atriumGroup = new THREE.Group();

  const { width: w, depth: d, height: h } = store.atrium;

  // Atrium wireframe (cyan, centered at group origin)
  const geo = new THREE.BoxGeometry(w, h, d);
  atriumWire = new THREE.LineSegments(
    new THREE.EdgesGeometry(geo),
    new THREE.LineBasicMaterial({ color: 0x4fc3f7 }),
  );
  atriumWire.position.set(0, h / 2, 0);
  atriumGroup.add(atriumWire);

  // Add speakers as children of atrium group
  speakerMeshes = store.speakers.map(sp => {
    const mesh = createSpeakerMesh(sp, store.spawn);
    atriumGroup!.add(mesh);
    return mesh;
  });

  ctx.scene.add(atriumGroup);

  updateAtriumPosition(store);
  updateAtriumVisibility(store);
}

/** Move atrium group to follow listener position. No rotation — fixed orientation. */
export function updateAtriumPosition(store: AtriumStore) {
  if (!atriumGroup) return;
  // Convert listener position to Three.js coords
  // Atrium X → Three.js X, Atrium Z → Three.js Y, Atrium Y → Three.js Z (flipped)
  atriumGroup.position.set(
    store.listener.x,
    store.listener.z,
    store.room.depth - store.listener.y,
  );
}

/** Show atrium in speaker modes, hide in HRTF. */
export function updateAtriumVisibility(store: AtriumStore) {
  if (!atriumGroup) return;
  const isHeadphone = store.renderMode === 'hrtf';
  atriumGroup.visible = !isHeadphone;

  // Update individual speaker visibility based on channel mode
  const active = CHANNEL_MODE_CHANNELS[store.channelMode] ?? [0, 1, 2, 4, 5];
  store.speakers.forEach((sp, i) => {
    if (speakerMeshes[i]) speakerMeshes[i].visible = active.includes(sp.channel);
  });
}
