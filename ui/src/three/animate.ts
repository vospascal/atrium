import type { SceneContext } from './scene.js';
import type { AtriumStore } from '../store.js';
import { updateListener } from './listener.js';
import { updateSources } from './sources.js';
import { updateSoundField } from './sound-field.js';

export function startAnimationLoop(ctx: SceneContext, store: AtriumStore) {
  function animate() {
    requestAnimationFrame(animate);

    updateListener(store);
    updateSources(store);
    updateSoundField(store);

    ctx.controls.update();
    ctx.renderer.render(ctx.scene, ctx.camera);
  }

  animate();
}
