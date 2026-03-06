import * as THREE from 'three';
import type { SceneContext } from './scene.js';
import type { AtriumStore } from '../store.js';
import { MODE_ACTIVE_CHANNELS } from '../store.js';
import { toThree } from './coords.js';
import type { Speaker } from '../types.js';

let speakerMeshes: THREE.Group[] = [];

function createSpeakerMesh(sp: Speaker): THREE.Group {
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

  group.position.copy(toThree(sp.x, sp.y, sp.z));
  return group;
}

export function buildSpeakers(ctx: SceneContext, store: AtriumStore) {
  // Remove old
  speakerMeshes.forEach(m => ctx.scene.remove(m));

  speakerMeshes = store.speakers.map(sp => {
    const group = createSpeakerMesh(sp);
    ctx.scene.add(group);
    return group;
  });

  updateSpeakerVisibility(store);
}

export function updateSpeakerVisibility(store: AtriumStore) {
  const active = MODE_ACTIVE_CHANNELS[store.renderMode] ?? [0, 1, 2, 4, 5];
  store.speakers.forEach((sp, i) => {
    if (speakerMeshes[i]) speakerMeshes[i].visible = active.includes(sp.channel);
  });
}
