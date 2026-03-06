import { LitElement, html, css } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import type { AtriumStore } from '../store.js';
import { StoreController } from '../store-controller.js';

const MODES = [
  { mode: 'world_locked', label: 'WorldLocked' },
  { mode: 'vbap', label: 'VBAP' },
  { mode: 'stereo', label: 'Stereo' },
  { mode: 'binaural', label: 'Binaural' },
];

@customElement('render-mode')
export class RenderMode extends LitElement {
  static styles = css`
    :host { display: block; margin-top: 8px; border-top: 1px solid #333; padding-top: 6px; }
    h3 { font-size: 12px; color: #999; margin-bottom: 4px; }
    .mode-btns { display: flex; gap: 4px; margin-top: 4px; flex-wrap: wrap; }
    .mode-btn {
      background: none; border: 1px solid #555; color: #999; font-size: 10px;
      padding: 3px 8px; border-radius: 4px; cursor: pointer; font-family: inherit;
    }
    .mode-btn:hover { border-color: #888; color: #ccc; }
    .mode-btn.active { border-color: #4fc3f7; color: #4fc3f7; background: rgba(79,195,247,0.1); }
  `;

  private _ctrl = new StoreController(this);

  @property({ attribute: false })
  get store(): AtriumStore { return this._ctrl.store!; }
  set store(s: AtriumStore) { this._ctrl.store = s; }

  private _onClick(mode: string) {
    this.store.setRenderMode(mode);
  }

  render() {
    return html`
      <h3>Render Mode</h3>
      <div class="mode-btns">
        ${MODES.map(m => html`
          <button class="mode-btn ${this.store?.renderMode === m.mode ? 'active' : ''}"
            @click=${() => this._onClick(m.mode)}>${m.label}</button>
        `)}
      </div>
    `;
  }
}
