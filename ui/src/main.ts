import { AtriumStore } from './store.js';
import { WebSocketConnection } from './ws/connection.js';
import { initCoords } from './three/coords.js';
import { createScene } from './three/scene.js';
import { buildRoom } from './three/room.js';
import { buildListener } from './three/listener.js';
import { buildSpeakers, updateSpeakerVisibility } from './three/speakers.js';
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
buildRoom(ctx, store);
buildListener(ctx);
buildSpeakers(ctx, store);
setupInteractions(ctx, store);

// Mount Lit shell
const app = document.querySelector('atrium-app')!;
(app as any).setStore(store);

// Rebuild scene on server state updates
store.addEventListener('scene-rebuild', () => {
  buildRoom(ctx, store);
  buildListener(ctx);
  buildSpeakers(ctx, store);
  buildSources(ctx, store);
  buildSoundField(ctx, store);
  updateSpeakerVisibility(store);
});

// WebSocket
const ws = new WebSocketConnection(store);
ws.connect();

// Start render loop
startAnimationLoop(ctx, store);
