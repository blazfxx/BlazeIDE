// Tab strip: supports file tabs (Monaco models) and custom-DOM tabs (settings,
// diff review). Pure DOM + callbacks; the editor module owns the semantics.

import { el, escapeHtml } from '../state';
import { icon, iconClose } from '../icons/index';

export interface Tab {
  id: string; // file path for file tabs, arbitrary id for custom tabs
  title: string;
  kind: 'file' | 'custom';
  dirty: boolean;
  customEl?: HTMLElement;
}

export interface TabStripCallbacks {
  onActivate(id: string): void;
  onClose(id: string): void;
  onContextMenu(id: string, e: MouseEvent): void;
}

export class TabStrip {
  private tabs: Tab[] = [];
  private activeId: string | null = null;
  // Per-file diagnostics counts (path → counts), drives the tab error badge.
  private markers = new Map<string, { errors: number; warnings: number }>();

  constructor(private host: HTMLElement, private cbs: TabStripCallbacks) {}

  list(): readonly Tab[] {
    return this.tabs;
  }

  get(id: string): Tab | undefined {
    return this.tabs.find((t) => t.id === id);
  }

  getActive(): Tab | undefined {
    return this.activeId !== null ? this.get(this.activeId) : undefined;
  }

  activeTabId(): string | null {
    return this.activeId;
  }

  add(tab: Tab): void {
    if (!this.get(tab.id)) this.tabs.push(tab);
    this.activeId = tab.id;
    this.render();
  }

  setDirty(id: string, dirty: boolean): void {
    const tab = this.get(id);
    if (tab && tab.dirty !== dirty) {
      tab.dirty = dirty;
      this.render();
    }
  }

  /** Update the error/warning counts shown on a file tab. */
  setMarkers(id: string, errors: number, warnings: number): void {
    const prev = this.markers.get(id);
    if (prev && prev.errors === errors && prev.warnings === warnings) return;
    if (errors === 0 && warnings === 0) this.markers.delete(id);
    else this.markers.set(id, { errors, warnings });
    if (this.get(id)) this.render();
  }

  activate(id: string): void {
    if (this.get(id) && this.activeId !== id) {
      this.activeId = id;
      this.render();
    }
  }

  /** Remove a tab; returns the id of the tab to activate next, if any. */
  remove(id: string): string | null {
    const idx = this.tabs.findIndex((t) => t.id === id);
    if (idx < 0) return this.activeId;
    this.tabs.splice(idx, 1);
    if (this.activeId === id) {
      const next = this.tabs[Math.min(idx, this.tabs.length - 1)];
      this.activeId = next ? next.id : null;
    }
    this.render();
    return this.activeId;
  }

  isEmpty(): boolean {
    return this.tabs.length === 0;
  }

  private render(): void {
    this.host.textContent = '';
    for (const tab of this.tabs) {
      const elTab = el('div', `tab${tab.id === this.activeId ? ' active' : ''}`);
      const errs = tab.kind === 'file' ? this.markers.get(tab.id) : undefined;
      const label = el('span', `tab-label${errs && errs.errors > 0 ? ' has-errors' : errs && errs.warnings > 0 ? ' has-warnings' : ''}`, escapeHtml(tab.title));
      elTab.appendChild(label);
      // Diagnostics count badge (errors take precedence over warnings).
      if (errs && (errs.errors > 0 || errs.warnings > 0)) {
        const n = errs.errors > 0 ? errs.errors : errs.warnings;
        const badge = el('span', `tab-diag ${errs.errors > 0 ? 'err' : 'warn'}`, String(n));
        badge.title = `${errs.errors} error${errs.errors === 1 ? '' : 's'}, ${errs.warnings} warning${errs.warnings === 1 ? '' : 's'}`;
        elTab.appendChild(badge);
      }
      if (tab.dirty) elTab.appendChild(el('span', 'dirty'));
      const close = el('span', 'close icon-12', icon(iconClose));
      close.title = 'Close';
      close.addEventListener('click', (e) => {
        e.stopPropagation();
        this.cbs.onClose(tab.id);
      });
      elTab.appendChild(close);
      elTab.addEventListener('click', () => this.cbs.onActivate(tab.id));
      elTab.addEventListener('auxclick', (e) => {
        if (e.button === 1) this.cbs.onClose(tab.id);
      });
      elTab.addEventListener('contextmenu', (e) => {
        e.preventDefault();
        this.cbs.onContextMenu(tab.id, e);
      });
      this.host.appendChild(elTab);
    }
  }
}
