import type { ReactiveController, ReactiveControllerHost } from 'lit';
import type { AtriumStore } from './store.js';

/**
 * Reactive controller that subscribes a Lit element to AtriumStore events.
 * Triggers host re-render on update, telemetry, and scene-rebuild.
 *
 * Usage:
 *   private _store = new StoreController(this);
 *   // then set: this._store.value = store;
 *   // or use with @property and updated():
 */
export class StoreController implements ReactiveController {
  private _host: ReactiveControllerHost;
  private _store: AtriumStore | null = null;
  private _handler = () => this._host.requestUpdate();

  private static EVENTS = ['update', 'telemetry', 'scene-rebuild'] as const;

  constructor(host: ReactiveControllerHost) {
    this._host = host;
    host.addController(this);
  }

  set store(s: AtriumStore | null) {
    if (s === this._store) return;
    this._unsubscribe();
    this._store = s;
    this._subscribe();
  }

  get store(): AtriumStore | null {
    return this._store;
  }

  hostConnected() {
    this._subscribe();
  }

  hostDisconnected() {
    this._unsubscribe();
  }

  private _subscribe() {
    if (!this._store) return;
    for (const evt of StoreController.EVENTS) {
      this._store.addEventListener(evt, this._handler);
    }
  }

  private _unsubscribe() {
    if (!this._store) return;
    for (const evt of StoreController.EVENTS) {
      this._store.removeEventListener(evt, this._handler);
    }
  }
}
