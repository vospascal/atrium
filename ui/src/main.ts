import { AtriumStore } from './store.js';
import { WebSocketConnection } from './ws/connection.js';
import { initCoords } from './three/coords.js';
import { createScene } from './three/scene.js';
import { buildEnvironment } from './three/environment.js';
import { buildListener } from './three/listener.js';
import { buildAtrium, updateAtriumVisibility } from './three/atrium.js';
import { buildSources } from './three/sources.js';
import { buildSoundField } from './three/sound-field.js';
import { setupInteractions } from './three/interactions.js';
import { startAnimationLoop } from './three/animate.js';

// Import components (registers custom elements)
import './components/atrium-app.js';

// Boot
const store = new AtriumStore();
initCoords(store);

// Three.js scene
const ctx = createScene(store);
buildEnvironment(ctx, store);
buildListener(ctx);
buildAtrium(ctx, store);
setupInteractions(ctx, store);

// Mount Lit shell
const app = document.querySelector('atrium-app')!;
(app as any).setStore(store);

// Rebuild scene on server state updates
store.addEventListener('scene-rebuild', () => {
  buildEnvironment(ctx, store);
  buildListener(ctx);
  buildAtrium(ctx, store);
  buildSources(ctx, store);
  buildSoundField(ctx, store);
  updateAtriumVisibility(store);

  // Re-center camera + target on listener after coordinate system changes
  const listenerX = store.listener.x;
  const listenerY = store.listener.z;
  const listenerZ = store.room.depth - store.listener.y;
  ctx.controls.target.set(listenerX, listenerY, listenerZ);
  ctx.camera.position.set(listenerX, listenerY + 8, listenerZ + 8);
  ctx.camera.lookAt(listenerX, listenerY, listenerZ);
});

// WebSocket
const ws = new WebSocketConnection(store);
ws.connect();

// Start render loop
startAnimationLoop(ctx, store);
