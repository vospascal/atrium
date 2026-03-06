import { LitElement, html, css } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import type { AtriumStore } from '../store.js';
import { StoreController } from '../store-controller.js';

import './source-row.js';

@customElement('source-list')
export class SourceList extends LitElement {
  static styles = css`
    :host { display: block; margin-top: 8px; border-top: 1px solid #333; padding-top: 6px; }
    h3 { font-size: 12px; color: #999; margin-bottom: 4px; }
  `;

  private _ctrl = new StoreController(this);

  @property({ attribute: false })
  get store(): AtriumStore { return this._ctrl.store!; }
  set store(s: AtriumStore) { this._ctrl.store = s; }

  render() {
    if (!this.store?.sources.length) return html``;
    return html`
      <h3>Sources</h3>
      ${this.store.sources.map((_s, i) => html`
        <source-row .store=${this.store} .index=${i}></source-row>
      `)}
    `;
  }
}
