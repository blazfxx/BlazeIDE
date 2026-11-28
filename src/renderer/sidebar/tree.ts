// File tree rendering with expansion state preserved across refreshes.
// Rows carry per-type icons (see icons/fileIcons.ts) and indent guides.

import type { FileNode } from '../../shared/types';
import { bus, el, escapeHtml } from '../state';
import { iconChevronDown, iconChevronRight } from '../icons/index';
import { fileIcon, folderIcon } from '../icons/fileIcons';

/** Escape a string for safe use inside a CSS attribute selector (paths contain
 *  /, ., spaces, etc.). Uses native CSS.escape where available. */
function cssEscape(s: string): string {
  const fn = (window as unknown as { CSS?: { escape?: (v: string) => string } }).CSS?.escape;
  return fn ? fn(s) : s.replace(/["\\]/g, '\\$&');
}

export interface TreeCallbacks {
  /** Open the file/folder's context menu. */
  onContextMenu(e: MouseEvent, node: FileNode): void;
  /** Move srcPath into destDir (a directory). */
  onMove(srcPath: string, destDir: string): void;
  /** Rename this node (F2 / Enter / double-click). */
  onRename(node: FileNode): void;
  /** Delete this node (Delete / Cmd+Backspace while the tree is focused). */
  onDelete(node: FileNode): void;
}

export class FileTree {
  private expanded = new Set<string>();
  private selectedPath: string | null = null;
  /** Full multi-selection (Cmd/Shift+click). Always includes selectedPath when
   *  non-null. Plain click collapses this back to a single path. */
  private selectedPaths = new Set<string>();
  /** Anchor row for Shift+click range selection. */
  private anchorPath: string | null = null;
  /** Folder the user last clicked — the target for toolbar create actions. */
  private targetDir: string | null = null;
  private root: FileNode | null = null;
  /** Path of a row currently showing an inline input (create/rename). */
  private inline: { parentPath: string; kind: 'file' | 'folder'; renameOf?: FileNode } | null = null;
  private inlineResolve: ((name: string | null) => void) | null = null;

  constructor(private host: HTMLElement, private cbs: TreeCallbacks) {
    bus.on('active-file-changed', ({ path }) => {
      if (path) {
        this.selectedPath = path;
        // Opening a file elsewhere collapses a stale multi-selection, unless
        // this path is already part of the current selection.
        if (!this.selectedPaths.has(path)) {
          this.selectedPaths = new Set([path]);
          this.anchorPath = path;
        }
      }
      if (this.inline) return; // don't destroy an open create/rename input
      // If the row already exists, just move the highlight in place — a full
      // re-render would replace the row element and break a pending dblclick.
      if (path && this.host.querySelector(`.tree-item[data-path="${cssEscape(path)}"]`)) {
        this.highlightSelection();
      } else {
        this.render();
      }
    });
    this.setupRootDrop();
    this.setupKeys();
  }

  /** Folders being read right now (big trees load a folder when it is opened). */
  private loading = new Set<string>();

  private load(node: FileNode): void {
    if (this.loading.has(node.path)) return;
    this.loading.add(node.path);
    void window.void
      .fsReadTree(node.path)
      .then((t) => { node.children = t.children ?? []; })
      .catch(() => { node.children = []; })
      .finally(() => {
        node.truncated = false;
        this.loading.delete(node.path);
        if (!this.inline) this.render();
      });
  }

  setRoot(root: FileNode | null): void {
    this.root = root;
    if (root) this.expanded.add(root.path);
    // A folder refresh (e.g. fs watcher) must not nuke an open inline input.
    if (!this.inline) this.render();
  }

  getRoot(): FileNode | null {
    return this.root;
  }

  /** The node currently selected in the tree (follows the active file), or
   *  null. Used by the delete-file keybinding. */
  getSelectedNode(): FileNode | null {
    if (!this.selectedPath || !this.root) return null;
    return this.findNode(this.selectedPath);
  }

  /** All currently selected nodes (multi-select). Falls back to the single
   *  active node. Used so delete can remove every selected file at once. */
  getSelectedNodes(): FileNode[] {
    const paths = this.effectiveSelection();
    const nodes: FileNode[] = [];
    for (const p of paths) {
      const n = this.findNode(p);
      if (n) nodes.push(n);
    }
    return nodes;
  }

  private findNode(path: string): FileNode | null {
    const find = (node: FileNode): FileNode | null => {
      if (node.path === path) return node;
      for (const child of node.children ?? []) {
        const hit = find(child);
        if (hit) return hit;
      }
      return null;
    };
    return this.root ? find(this.root) : null;
  }

  /** Directory new files/folders should be created in: the last-clicked
   *  folder (or, for a clicked file, its parent), else the project root. */
  createTarget(): string | null {
    return this.targetDir ?? this.root?.path ?? null;
  }

  /** Prompt for a name with an inline input row under parentPath. */
  promptInline(parentPath: string, kind: 'file' | 'folder', renameOf?: FileNode): Promise<string | null> {
    this.inlineResolve?.(null);
    this.inline = { parentPath, kind, renameOf };
    this.expanded.add(parentPath);
    this.render();
    return new Promise((resolve) => {
      this.inlineResolve = resolve;
    });
  }

  render(): void {
    this.host.textContent = '';
    // Make the tree focusable so it can own keyboard ops (rename/delete) when
    // the user is working in the file list rather than the editor.
    if (this.host.tabIndex < 0) this.host.tabIndex = 0;
    if (!this.root) return;
    // Render the root's children at depth 0 (the project name lives in the header).
    for (const child of this.root.children ?? []) this.renderNode(child, 0);
    // The standalone "create" input only renders for a NEW file/folder — never
    // during a rename (rename draws its input in renderNode at the node's row).
    if (this.inline && !this.inline.renameOf && this.inline.parentPath === this.root.path) this.renderInline(0);
  }

  /** Update just the `.selected` highlight in place — no DOM rebuild, so live
   *  rows (and their click/dblclick targets) survive. */
  private highlightSelection(): void {
    const sel = this.effectiveSelection();
    for (const row of this.host.querySelectorAll<HTMLElement>('.tree-item[data-path]')) {
      const p = row.dataset.path ?? '';
      row.classList.toggle('selected', sel.has(p));
      row.classList.toggle('selected-primary', p === this.selectedPath);
    }
  }

  /** The set of paths to highlight: the multi-selection if any, else just the
   *  single active path. */
  private effectiveSelection(): Set<string> {
    if (this.selectedPaths.size > 0) return this.selectedPaths;
    return this.selectedPath ? new Set([this.selectedPath]) : new Set();
  }

  /** Flatten currently-visible rows (respecting expansion) into display order. */
  private visiblePaths(): string[] {
    const out: string[] = [];
    const walk = (node: FileNode): void => {
      for (const child of node.children ?? []) {
        out.push(child.path);
        if (child.isDirectory && this.expanded.has(child.path)) walk(child);
      }
    };
    if (this.root) walk(this.root);
    return out;
  }

  /** Select every visible row between two paths (inclusive). */
  private selectRange(from: string, to: string): void {
    const order = this.visiblePaths();
    const a = order.indexOf(from);
    const b = order.indexOf(to);
    if (a === -1 || b === -1) { this.selectedPaths = new Set([to]); return; }
    const [lo, hi] = a <= b ? [a, b] : [b, a];
    this.selectedPaths = new Set(order.slice(lo, hi + 1));
  }

  /** Indent guides: one thin vertical line per ancestor level. */
  private indentEl(depth: number): HTMLElement {
    const indent = el('span', 'tree-indent');
    for (let i = 0; i < depth; i++) indent.appendChild(el('span', 'indent-guide'));
    return indent;
  }

  private renderNode(node: FileNode, depth: number): void {
    const isTarget = node.isDirectory && node.path === this.targetDir;
    const sel = this.effectiveSelection();
    const cls = `tree-item${sel.has(node.path) ? ' selected' : ''}` +
      `${node.path === this.selectedPath ? ' selected-primary' : ''}${isTarget ? ' target-dir' : ''}`;
    const row = el('div', cls);
    row.dataset.path = node.path;
    const isOpen = this.expanded.has(node.path);

    row.appendChild(this.indentEl(depth));

    if (node.isDirectory) {
      row.appendChild(el('span', 'icon icon-12 chevron', isOpen ? iconChevronDown : iconChevronRight));
      const spec = folderIcon(node.name, isOpen);
      row.appendChild(el('span', `icon tree-icon ${spec.cls}`, spec.svg));
      row.classList.add('is-dir');
    } else {
      row.appendChild(el('span', 'chevron-spacer'));
      const spec = fileIcon(node.name);
      row.appendChild(el('span', `icon tree-icon ${spec.cls}`, spec.svg));
    }
    row.appendChild(el('span', 'tree-name', escapeHtml(node.name)));

    if (this.inline?.renameOf?.path === node.path) {
      this.host.appendChild(this.makeInlineInput(depth, node.name));
      return;
    }

    row.addEventListener('click', (e) => {
      // Cmd/Ctrl+click toggles an individual row in/out of the selection.
      // Shift+click selects the range from the anchor to this row.
      const multi = e.metaKey || e.ctrlKey;
      const range = e.shiftKey;
      if (range && this.anchorPath) {
        this.selectRange(this.anchorPath, node.path);
        this.selectedPath = node.path;
        this.targetDir = node.isDirectory ? node.path : node.path.slice(0, node.path.lastIndexOf('/'));
        this.highlightSelection();
        this.host.focus({ preventScroll: true });
        return;
      }
      if (multi) {
        if (this.selectedPaths.has(node.path)) this.selectedPaths.delete(node.path);
        else this.selectedPaths.add(node.path);
        this.selectedPath = node.path;
        this.anchorPath = node.path;
        this.targetDir = node.isDirectory ? node.path : node.path.slice(0, node.path.lastIndexOf('/'));
        this.highlightSelection();
        this.host.focus({ preventScroll: true });
        return;
      }
      // Plain click: single selection + open.
      this.selectedPath = node.path;
      this.selectedPaths = new Set([node.path]);
      this.anchorPath = node.path;
      // Track the folder context for toolbar create actions.
      this.targetDir = node.isDirectory ? node.path : node.path.slice(0, node.path.lastIndexOf('/'));
      if (node.isDirectory) {
        if (isOpen) this.expanded.delete(node.path);
        else this.expanded.add(node.path);
        this.render();
      } else {
        bus.emit('open-file', { path: node.path, preserveFocus: true });
        // Update the highlight in place (NOT a full re-render) so this row's
        // element survives — otherwise a follow-up double-click sees a fresh
        // element and never registers as a dblclick.
        this.highlightSelection();
      }
      // Keep keyboard focus on the tree so Enter/F2/Delete work after a click.
      this.host.focus({ preventScroll: true });
    });
    // Double-click a row to rename it (VS Code-style).
    row.addEventListener('dblclick', (e) => {
      e.preventDefault();
      this.selectedPath = node.path;
      this.cbs.onRename(node);
    });
    row.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      this.selectedPath = node.path;
      this.targetDir = node.isDirectory ? node.path : node.path.slice(0, node.path.lastIndexOf('/'));
      this.cbs.onContextMenu(e, node);
    });
    this.wireDrag(row, node);
    this.host.appendChild(row);

    if (node.isDirectory && isOpen) {
      if (node.truncated) {
        this.load(node);
        const wait = el('div', 'tree-item tree-loading');
        wait.appendChild(this.indentEl(depth + 1));
        wait.appendChild(el('span', 'tree-name', 'Loading…'));
        this.host.appendChild(wait);
      }
      for (const child of node.children ?? []) this.renderNode(child, depth + 1);
      if (this.inline && !this.inline.renameOf && this.inline.parentPath === node.path) {
        this.renderInline(depth + 1);
      }
    }
  }

  /** Make a row draggable and (for folders) a drop target for moves. */
  private wireDrag(row: HTMLElement, node: FileNode): void {
    row.draggable = true;
    row.addEventListener('dragstart', (e) => {
      e.dataTransfer?.setData('text/void-path', node.path);
      if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
      row.classList.add('dragging');
    });
    row.addEventListener('dragend', () => row.classList.remove('dragging'));

    // Drop target = the directory the item lands in (folder itself, or a
    // file's parent so dropping onto a file drops into its folder).
    const destDir = node.isDirectory ? node.path : node.path.slice(0, node.path.lastIndexOf('/'));
    row.addEventListener('dragover', (e) => {
      if (!e.dataTransfer?.types.includes('text/void-path')) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
      row.classList.add('drop-into');
    });
    row.addEventListener('dragleave', () => row.classList.remove('drop-into'));
    row.addEventListener('drop', (e) => {
      row.classList.remove('drop-into');
      const src = e.dataTransfer?.getData('text/void-path');
      if (!src) return;
      e.preventDefault();
      e.stopPropagation();
      this.tryMove(src, destDir);
    });
  }

  /** Drops onto empty tree space move the item to the project root. */
  private setupRootDrop(): void {
    this.host.addEventListener('dragover', (e) => {
      if (e.target !== this.host || !e.dataTransfer?.types.includes('text/void-path')) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
    });
    this.host.addEventListener('drop', (e) => {
      if (e.target !== this.host) return;
      const src = e.dataTransfer?.getData('text/void-path');
      if (!src || !this.root) return;
      e.preventDefault();
      this.tryMove(src, this.root.path);
    });
  }

  /** Keyboard ops are handled globally (see bindKeys in index.ts) via the
   *  rename-selected-file / delete-selected-file bus events, so they work
   *  regardless of exactly where focus landed. Nothing to wire on the host. */
  private setupKeys(): void {
    // Rename/delete are driven by global keybinds. Arrow keys navigate the tree
    // when it has focus (and no inline input is open).
    this.host.addEventListener('keydown', (e: KeyboardEvent) => {
      if (this.inline) return;
      if (!['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight'].includes(e.key)) return;
      e.preventDefault();
      const order = this.visiblePaths();
      if (order.length === 0) return;
      const cur = this.selectedPath && order.includes(this.selectedPath) ? order.indexOf(this.selectedPath) : -1;
      const node = this.selectedPath ? this.findNode(this.selectedPath) : null;

      if (e.key === 'ArrowDown') {
        this.moveTo(order[Math.min(order.length - 1, cur + 1)] ?? order[0]);
      } else if (e.key === 'ArrowUp') {
        this.moveTo(cur <= 0 ? order[0] : order[cur - 1]);
      } else if (e.key === 'ArrowRight') {
        // Expand a closed folder; if already open (or a file), move to next row.
        if (node?.isDirectory && !this.expanded.has(node.path)) {
          this.expanded.add(node.path);
          this.render();
        } else {
          this.moveTo(order[Math.min(order.length - 1, cur + 1)]);
        }
      } else if (e.key === 'ArrowLeft') {
        // Collapse an open folder; else jump to the parent folder.
        if (node?.isDirectory && this.expanded.has(node.path)) {
          this.expanded.delete(node.path);
          this.render();
        } else if (this.selectedPath) {
          const parent = this.selectedPath.slice(0, this.selectedPath.lastIndexOf('/'));
          if (parent && parent !== this.root?.path && order.includes(parent)) this.moveTo(parent);
        }
      }
    });
  }

  /** Move the single selection to a path: select it, open files, scroll to it. */
  private moveTo(path: string | undefined): void {
    if (!path) return;
    this.selectedPath = path;
    this.selectedPaths = new Set([path]);
    this.anchorPath = path;
    const node = this.findNode(path);
    if (node && !node.isDirectory) bus.emit('open-file', { path, preserveFocus: true });
    this.highlightSelection();
    const row = this.host.querySelector<HTMLElement>(`.tree-item[data-path="${cssEscape(path)}"]`);
    row?.scrollIntoView({ block: 'nearest' });
    this.host.focus({ preventScroll: true });
  }

  private tryMove(src: string, destDir: string): void {
    const srcParent = src.slice(0, src.lastIndexOf('/'));
    if (srcParent === destDir) return;
    if (destDir === src || destDir.startsWith(src + '/')) return;
    this.cbs.onMove(src, destDir);
  }

  private renderInline(depth: number): void {
    this.host.appendChild(this.makeInlineInput(depth));
  }

  private makeInlineInput(depth: number, initial = ''): HTMLElement {
    const wrap = el('div', 'tree-item tree-inline');
    const kind = this.inline?.kind ?? 'file';
    wrap.appendChild(this.indentEl(depth));
    wrap.appendChild(el('span', 'chevron-spacer'));
    const spec = kind === 'folder' ? folderIcon(initial || 'folder', false) : fileIcon(initial || 'file');
    wrap.appendChild(el('span', `icon tree-icon ${spec.cls}`, spec.svg));
    const input = el('input', 'tree-input');
    input.value = initial;
    input.spellcheck = false;
    wrap.appendChild(input);

    const finish = (commit: boolean): void => {
      const name = input.value.trim();
      const resolve = this.inlineResolve;
      const isRename = !!this.inline?.renameOf;
      this.inline = null;
      this.inlineResolve = null;
      // Creation accepts slashes ("hi/" makes a folder, "a/b.ts" nests);
      // rename stays single-segment. Absolute paths and ".." are rejected.
      const valid =
        name.length > 0 &&
        !name.startsWith('/') &&
        !name.split('/').some((seg, i, all) => (seg === '' && i < all.length - 1) || seg === '..') &&
        (!isRename || !name.includes('/'));
      if (commit && valid) resolve?.(name);
      else resolve?.(null);
      this.render();
    };
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') finish(true);
      else if (e.key === 'Escape') finish(false);
      e.stopPropagation();
    });
    input.addEventListener('blur', () => finish(false));
    window.setTimeout(() => {
      input.focus();
      input.select();
    }, 0);
    return wrap;
  }
}
