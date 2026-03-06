import { LitElement, html, css } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import type { AtriumStore } from '../store.js';
import { StoreController } from '../store-controller.js';

@customElement('status-indicator')
export class StatusIndicator extends LitElement {
  static styles = css`
    :host { display: block; margin-top: 8px; }
    .dot {
      display: inline-block; width: 8px; height: 8px;
      border-radius: 50%; margin-right: 6px; vertical-align: middle;
    }
    .connected { background: #4caf50; }
    .disconnected { background: #f44336; }
  `;

  private _ctrl = new StoreController(this);

  @property({ attribute: false })
  get store(): AtriumStore { return this._ctrl.store!; }
  set store(s: AtriumStore) { this._ctrl.store = s; }

  render() {
    const connected = this.store?.connected ?? false;
    return html`
      <span class="dot ${connected ? 'connected' : 'disconnected'}"></span>
      <span>${connected ? 'Connected' : 'Disconnected'}</span>
    `;
  }
}
