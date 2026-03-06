import { LitElement, html, css } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import type { AtriumStore } from '../store.js';
import { StoreController } from '../store-controller.js';

@customElement('listener-info')
export class ListenerInfo extends LitElement {
  static styles = css`
    :host { display: block; }
    .row { display: flex; justify-content: space-between; align-items: center; }
    label { color: #999; }
    .val { color: #4fc3f7; }
  `;

  private _ctrl = new StoreController(this);

  @property({ attribute: false })
  get store(): AtriumStore { return this._ctrl.store!; }
  set store(s: AtriumStore) { this._ctrl.store = s; }

  render() {
    const l = this.store?.listener;
    if (!l) return html``;
    return html`
      <div class="row">
        <label>Listener</label>
        <span class="val">${l.x.toFixed(1)}, ${l.y.toFixed(1)}, ${l.z.toFixed(1)}</span>
      </div>
      <div class="row">
        <label>Yaw</label>
        <span class="val">${(l.yaw * 180 / Math.PI).toFixed(0)}&deg;</span>
      </div>
    `;
  }
}
