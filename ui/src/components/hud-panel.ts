import { LitElement, html, css } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import type { AtriumStore } from '../store.js';
import { StoreController } from '../store-controller.js';

import './listener-info.js';
import './gain-slider.js';
import './source-list.js';
import './render-mode.js';
import './atmosphere-panel.js';
import './status-indicator.js';

@customElement('hud-panel')
export class HudPanel extends LitElement {
  static styles = css`
    :host {
      position: absolute; top: 16px; left: 16px;
      background: rgba(0,0,0,0.7); padding: 12px 16px; border-radius: 8px;
      font-size: 12px; line-height: 1.6; pointer-events: auto;
      min-width: 220px;
      font-family: 'SF Mono', 'Fira Code', monospace;
      color: #ccc;
    }
    h2 { font-size: 14px; color: #fff; margin-bottom: 6px; }
    .section { margin-top: 8px; border-top: 1px solid #333; padding-top: 6px; }
    button.reset {
      background: none; border: 1px solid #555; color: #999; font-size: 11px;
      padding: 4px 10px; border-radius: 4px; cursor: pointer;
      font-family: inherit; width: 100%;
    }
    button.reset:hover { border-color: #888; color: #ccc; }
  `;

  private _ctrl = new StoreController(this);

  @property({ attribute: false })
  get store(): AtriumStore { return this._ctrl.store!; }
  set store(s: AtriumStore) { this._ctrl.store = s; }

  render() {
    if (!this.store) return html``;
    return html`
      <h2>Atrium Control</h2>
      <listener-info .store=${this.store}></listener-info>
      <gain-slider .store=${this.store}></gain-slider>
      <source-list .store=${this.store}></source-list>
      <render-mode .store=${this.store}></render-mode>
      <atmosphere-panel .store=${this.store}></atmosphere-panel>
      <status-indicator .store=${this.store}></status-indicator>
      <div class="section">
        <button class="reset" @click=${() => this.store.resetScene()}>Reset Scene</button>
      </div>
    `;
  }
}
