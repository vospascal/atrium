import * as THREE from 'three';
import type { SceneContext } from './scene.js';
import type { AtriumStore } from '../store.js';
import { toThree } from './coords.js';

let sourceMeshes: THREE.Sprite[] = [];
let falloffClouds: THREE.Points[] = [];
let audibleRings: THREE.Line[] = [];
let gainLines: THREE.Line[] = [];
let distLabels: DistLabel[] = [];

interface DistLabel {
  canvas: HTMLCanvasElement;
  ctx: CanvasRenderingContext2D;
  tex: THREE.CanvasTexture;
  sprite: THREE.Sprite;
  lastText: string;
}

export function getSourceMeshes(): THREE.Sprite[] {
  return sourceMeshes;
}

function updateDistLabel(label: DistLabel, text: string, color: string) {
  if (label.lastText === text) return;
  label.lastText = text;
  const { canvas, ctx, tex } = label;
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.fillStyle = 'rgba(0,0,0,0.5)';
  ctx.beginPath();
  ctx.roundRect(0, 0, canvas.width, canvas.height, 4);
  ctx.fill();
  ctx.fillStyle = color;
  ctx.font = '15px monospace';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText(text, canvas.width / 2, canvas.height / 2);
  tex.needsUpdate = true;
}

export function buildSources(ctx: SceneContext, store: AtriumStore) {
  // Remove old
  sourceMeshes.forEach(m => ctx.scene.remove(m));
  falloffClouds.forEach(m => ctx.scene.remove(m));
  audibleRings.forEach(m => ctx.scene.remove(m));
  gainLines.forEach(m => ctx.scene.remove(m));
  distLabels.forEach(d => ctx.scene.remove(d.sprite));

  // Source meshes
  sourceMeshes = store.sources.map(s => {
    const hexColor = '#' + s.color.toString(16).padStart(6, '0');

    // Source dot — canvas-textured sprite (2 triangles vs 512 for SphereGeometry)
    const dotCanvas = document.createElement('canvas');
    dotCanvas.width = 64; dotCanvas.height = 64;
    const dctx = dotCanvas.getContext('2d')!;
    dctx.fillStyle = hexColor;
    dctx.beginPath();
    dctx.arc(32, 32, 28, 0, Math.PI * 2);
    dctx.fill();
    const dotTex = new THREE.CanvasTexture(dotCanvas);
    const dot = new THREE.Sprite(new THREE.SpriteMaterial({
      map: dotTex, transparent: true, depthTest: false,
    }));
    dot.scale.set(0.24, 0.24, 1);
    ctx.scene.add(dot);

    // Label sprite
    const canvas = document.createElement('canvas');
    canvas.width = 128; canvas.height = 32;
    const cctx = canvas.getContext('2d')!;
    cctx.fillStyle = hexColor;
    cctx.font = '20px monospace';
    cctx.fillText(s.name, 4, 22);
    const tex = new THREE.CanvasTexture(canvas);
    const label = new THREE.Sprite(new THREE.SpriteMaterial({ map: tex, transparent: true }));
    label.scale.set(1, 0.25, 1);
    label.position.y = 0.3;
    dot.add(label);
    return dot;
  });

  // Falloff clouds — disabled for now (re-enable by uncommenting)
  falloffClouds = [];

  // Audible radius rings — flat circle at the 20 dB threshold distance
  const RING_SEGMENTS = 64;
  audibleRings = store.sources.map(s => {
    const pts: THREE.Vector3[] = [];
    for (let j = 0; j <= RING_SEGMENTS; j++) {
      const angle = (j / RING_SEGMENTS) * Math.PI * 2;
      pts.push(new THREE.Vector3(
        Math.cos(angle) * s.audibleR,
        0,
        Math.sin(angle) * s.audibleR,
      ));
    }
    const geo = new THREE.BufferGeometry().setFromPoints(pts);
    const mat = new THREE.LineBasicMaterial({
      color: s.color, transparent: true, opacity: 0.25,
    });
    const ring = new THREE.Line(geo, mat);
    ctx.scene.add(ring);
    return ring;
  });

  // Gain lines
  gainLines = store.sources.map(s => {
    const geo = new THREE.BufferGeometry();
    geo.setAttribute('position', new THREE.Float32BufferAttribute([0, 0, 0, 0, 0, 0], 3));
    const mat = new THREE.LineBasicMaterial({ color: s.color, transparent: true, opacity: 0.5 });
    const line = new THREE.Line(geo, mat);
    ctx.scene.add(line);
    return line;
  });

  // Distance labels
  distLabels = store.sources.map(() => {
    const canvas = document.createElement('canvas');
    canvas.width = 128; canvas.height = 28;
    const cctx = canvas.getContext('2d')!;
    const tex = new THREE.CanvasTexture(canvas);
    tex.minFilter = THREE.LinearFilter;
    const sprite = new THREE.Sprite(new THREE.SpriteMaterial({ map: tex, transparent: true, depthTest: false }));
    sprite.scale.set(0.9, 0.2, 1);
    sprite.visible = false;
    ctx.scene.add(sprite);
    return { canvas, ctx: cctx, tex, sprite, lastText: '' };
  });
}

export function updateSources(store: AtriumStore) {
  const elapsed = performance.now() / 1000 - store.startTime;

  store.sources.forEach((s, i) => {
    if (!sourceMeshes[i]) return;

    // Position: engine telemetry is authoritative, local orbit as fallback
    // Skip stale telemetry while dragging to avoid position fighting
    let ax: number, ay: number, az: number;
    const t = store.telemetry?.[i];
    if (t && store.sourceDragging !== i) {
      ax = t.x; ay = t.y; az = t.z;
    } else if (s.r > 0 && (s.speed !== 0 || store.sourcePaused[i])) {
      const orbitAngle = store.sourcePaused[i] ? (s._frozenAngle ?? 0) : s.speed * elapsed;
      ax = s.cx + s.r * Math.cos(orbitAngle);
      ay = s.cy + s.r * Math.sin(orbitAngle);
      az = s.z;
    } else {
      ax = s.x ?? s.cx;
      ay = s.y ?? s.cy;
      az = s.z;
    }

    sourceMeshes[i].position.copy(toThree(ax, ay, az));
    if (falloffClouds[i]) {
      falloffClouds[i].position.copy(sourceMeshes[i].position);
      falloffClouds[i].visible = !store.sourceMuted[i];
    }
    if (audibleRings[i]) {
      audibleRings[i].position.copy(sourceMeshes[i].position);
      audibleRings[i].position.y = 0.01; // sit on floor
      audibleRings[i].visible = !store.sourceMuted[i];
    }

    // Gain data
    const gains = t ?? { dist: 0, emit: 0, hear: 0, total: 0, db: -999, distance: 0 };

    // Update gain line
    const srcPos = toThree(ax, ay, az);
    const lstPos = toThree(store.listener.x, store.listener.y, store.listener.z);
    const posArr = gainLines[i].geometry.attributes.position.array as Float32Array;
    posArr[0] = srcPos.x; posArr[1] = srcPos.y + 0.02; posArr[2] = srcPos.z;
    posArr[3] = lstPos.x; posArr[4] = lstPos.y + 0.02; posArr[5] = lstPos.z;
    gainLines[i].geometry.attributes.position.needsUpdate = true;
    (gainLines[i].material as THREE.LineBasicMaterial).opacity =
      store.sourceMuted[i] ? 0.05 : (0.1 + gains.total * 0.7);
    gainLines[i].visible = true;

    // Update distance label
    const dl = distLabels[i];
    const d = Math.max(1, gains.distance);
    const splAtDist = s.spl - 20 * Math.log10(d);
    const emitDb = gains.emit > 0 ? 20 * Math.log10(gains.emit) : -999;
    const hearDb = gains.hear > 0 ? 20 * Math.log10(gains.hear) : -999;
    const receivedSpl = splAtDist + emitDb + hearDb;
    const splText = isFinite(receivedSpl) ? receivedSpl.toFixed(0) : '-\u221E';
    const distText = `${gains.distance.toFixed(1)}m ${splText}dB`;
    const hexColor = '#' + s.color.toString(16).padStart(6, '0');
    updateDistLabel(dl, distText, hexColor);
    dl.sprite.position.set(
      (srcPos.x + lstPos.x) / 2,
      (srcPos.y + lstPos.y) / 2 + 0.15,
      (srcPos.z + lstPos.z) / 2,
    );
    dl.sprite.visible = !store.sourceMuted[i];
  });
}

/** Get per-source gain text for HUD readout */
export function getSourceGainText(store: AtriumStore, index: number): string {
  const s = store.sources[index];
  if (store.sourceMuted[index]) return 'muted';
  const t = store.telemetry?.[index];
  if (!t) return 'dist:- emit:- hear:- = -';
  const d = Math.max(1, t.distance);
  const splAtDist = s.spl - 20 * Math.log10(d);
  const emitDb = t.emit > 0 ? 20 * Math.log10(t.emit) : -999;
  const hearDb = t.hear > 0 ? 20 * Math.log10(t.hear) : -999;
  const receivedSpl = splAtDist + emitDb + hearDb;
  const splText = isFinite(receivedSpl) ? receivedSpl.toFixed(0) : '-\u221E';
  return `dist:${t.dist.toFixed(2)} emit:${t.emit.toFixed(2)} hear:${t.hear.toFixed(2)} = ${splText} dBSPL`;
}
