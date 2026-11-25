// Sidebar: project header, file tree, search, context-menu file operations.

import type { FileNode, SearchHit } from '../../shared/types';
import { bus, el, escapeHtml, state } from '../state';
import { icon, iconFilePlus, iconFolderPlus, iconSearch } from '../icons/index';
import { FileTree } from './tree';
import { showContextMenu } from './contextMenu';
import type { CtxItem } from './contextMenu';
import { confirmModal } from './confirm';

export function initSidebar(host: HTMLElement): void {
  const header = el('div', 'sidebar-header');
  const title = el('span', 'sidebar-title', 'no folder open');
  const actions = el('div', 'sidebar-actions');
  const btnNewFile = iconBtn(iconFilePlus, 'New File');
  const btnNewFolder = iconBtn(iconFolderPlus, 'New Folder');
  const btnSearch = iconBtn(iconSearch, 'Search');
  actions.append(btnNewFile, btnNewFolder, btnSearch);
  header.append(title, actions);

  const errorLine = el('div', 'sidebar-error');
  errorLine.style.display = 'none';

  const searchWrap = el('div', 'sidebar-search');
  searchWrap.style.display = 'none';
  const searchInput = el('input', 'search-input');
  searchInput.placeholder = 'search project';
  searchInput.spellcheck = false;
  searchWrap.appendChild(searchInput);
  // Replace row: a field + Replace-all button, applied across the project.
  const replaceRow = el('div', 'search-replace-row');
  const replaceInput = el('input', 'search-input') as HTMLInputElement;
  replaceInput.placeholder = 'replace with…';
  replaceInput.spellcheck = false;
  const replaceBtn = el('button', 'search-replace-btn', 'Replace all');
  replaceRow.append(replaceInput, replaceBtn);
  searchWrap.appendChild(replaceRow);
  const searchResults = el('div', 'search-results');
  searchWrap.appendChild(searchResults);

  const openBtnWrap = el('div', 'sidebar-open-wrap');
  const openBtn = el('button', 'primary', 'Open Folder');
  openBtnWrap.appendChild(openBtn);

  const treeHost = el('div', 'tree-host');
  host.append(header, errorLine, searchWrap, openBtnWrap, treeHost);

  const tree = new FileTree(treeHost, {
    onContextMenu: (e, node) => openNodeMenu(e, node),
    onMove: (src, destDir) => void moveEntry(src, destDir),
    onRename: (node) => void renameEntry(node),
    onDelete: (node) => void deleteEntry(node),
  });

  let searchTimer: number | null = null;

  function iconBtn(svg: string, tip: string): HTMLElement {
    const b = el('button', 'icon-btn', icon(svg));
    b.title = tip;
    return b;
  }

  function showError(msg: string): void {
    errorLine.textContent = msg;
    errorLine.style.display = 'block';
    window.setTimeout(() => {
      errorLine.style.display = 'none';
    }, 4000);
  }

  async function refresh(): Promise<void> {
    if (!state.projectRoot) return;
    try {
      const root = await window.void.fsReadTree(state.projectRoot);
      tree.setRoot(root);
    } catch (err) {
      showError(err instanceof Error ? err.message : 'Failed to read folder');
    }
  }

  async function openFolder(): Promise<void> {
    const root = await window.void.fsOpenFolder();
    if (!root) return;
    const name = root.split('/').pop() ?? root;
    state.projectRoot = root;
    state.projectName = name;
    await window.void.fsWatch(root);
    bus.emit('project-opened', { root, name });
  }

  function dirOf(node: FileNode): string {
    return node.isDirectory ? node.path : node.path.slice(0, node.path.lastIndexOf('/'));
  }

  async function createEntry(parent: string, kind: 'file' | 'folder'): Promise<void> {
    const name = await tree.promptInline(parent, kind);
    if (!name) return;
    // A trailing "/" forces a folder regardless of which action started the
    // prompt; intermediate slashes create nested paths ("a/b/c.ts").
    const wantsFolder = kind === 'folder' || name.endsWith('/');
    const cleaned = name.replace(/\/+$/, '');
    if (!cleaned) return;
    const path = `${parent}/${cleaned}`;
    try {
      if (wantsFolder) {
        await window.void.fsCreateFolder(path);
      } else {
        await window.void.fsCreateFile(path);
        bus.emit('open-file', { path });
      }
      await refresh();
    } catch (err) {
      showError(err instanceof Error ? err.message : 'Operation failed');
    }
  }

  // Move (drag-drop): rename src into destDir, keeping its basename.
  async function moveEntry(src: string, destDir: string): Promise<void> {
    const base = src.slice(src.lastIndexOf('/') + 1);
    try {
      await window.void.fsRename(src, `${destDir}/${base}`);
      await refresh();
    } catch (err) {
      showError(err instanceof Error ? err.message : 'Move failed');
    }
  }

  async function renameEntry(node: FileNode): Promise<void> {
    const parent = node.path.slice(0, node.path.lastIndexOf('/'));
    const name = await tree.promptInline(parent, node.isDirectory ? 'folder' : 'file', node);
    if (!name || name === node.name) return;
    try {
      await window.void.fsRename(node.path, `${parent}/${name}`);
      await refresh();
    } catch (err) {
      showError(err instanceof Error ? err.message : 'Rename failed');
    }
  }

  async function deleteEntry(node: FileNode): Promise<void> {
    await deleteEntries([node]);
  }

  /** Delete one or more entries with a single confirmation for the batch. */
  async function deleteEntries(nodes: FileNode[]): Promise<void> {
    if (nodes.length === 0) return;
    if (state.settings.confirmDelete) {
      const multi = nodes.length > 1;
      const result = await confirmModal({
        title: multi ? `Delete ${nodes.length} items` : (nodes[0].isDirectory ? 'Delete folder' : 'Delete file'),
        message: multi
          ? `Delete these ${nodes.length} items? This cannot be undone.`
          : `Delete ${nodes[0].name}${nodes[0].isDirectory ? ' and everything inside it' : ''}? This cannot be undone.`,
        confirmLabel: 'Delete',
        danger: true,
        showDontAskAgain: true,
      });
      if (!result.confirmed) return;
      if (result.dontShowAgain) {
        state.settings.confirmDelete = false;
        void window.void.settingsSet({ confirmDelete: false });
      }
    }
    try {
      for (const node of nodes) await window.void.fsDelete(node.path);
      await refresh();
    } catch (err) {
      showError(err instanceof Error ? err.message : 'Delete failed');
    }
  }

  function openNodeMenu(e: MouseEvent, node: FileNode): void {
    const items: CtxItem[] = [
      { label: 'New File', action: () => void createEntry(dirOf(node), 'file') },
      { label: 'New Folder', action: () => void createEntry(dirOf(node), 'folder') },
      { label: 'Rename', action: () => void renameEntry(node) },
      { label: 'Delete', danger: true, action: () => void deleteEntry(node) },
    ];
    // Add file(s) to the AI chat as context (@-mention pins). Uses the full
    // multi-selection if there is one, else just this node.
    const selected = tree.getSelectedNodes().filter((n) => !n.isDirectory).map((n) => n.path);
    const toPin = selected.length > 0 && selected.includes(node.path) ? selected : (!node.isDirectory ? [node.path] : []);
    if (toPin.length > 0) {
      const label = toPin.length > 1 ? `Add ${toPin.length} files to chat` : 'Add to chat';
      items.unshift({ label, action: () => bus.emit('ai-pin-files', { paths: toPin }) });
    }
    // HTML files can be opened in the live web preview (select + comment there).
    if (!node.isDirectory && /\.html?$/i.test(node.name)) {
      items.unshift({ label: 'Open in Preview', action: () => bus.emit('open-preview', { url: node.path }) });
    }
    showContextMenu(e.clientX, e.clientY, items);
  }

  function renderSearchHits(hits: SearchHit[]): void {
    searchResults.textContent = '';
    if (hits.length === 0) {
      searchResults.appendChild(el('div', 'search-empty', 'No results'));
      return;
    }
    for (const hit of hits) {
      const relPath = state.projectRoot ? hit.path.slice(state.projectRoot.length + 1) : hit.path;
      const row = el('div', 'search-hit');
      row.appendChild(el('div', 'hit-file', escapeHtml(relPath) + (hit.line > 0 ? `:${hit.line}` : '')));
      if (hit.preview) row.appendChild(el('div', 'hit-preview', highlight(hit.preview, searchInput.value)));
      row.addEventListener('click', () => bus.emit('open-file', { path: hit.path }));
      searchResults.appendChild(row);
    }
  }

  function highlight(text: string, query: string): string {
    const safe = escapeHtml(text);
    if (!query) return safe;
    const idx = safe.toLowerCase().indexOf(escapeHtml(query).toLowerCase());
    if (idx < 0) return safe;
    const end = idx + escapeHtml(query).length;
    return `${safe.slice(0, idx)}<span class="match">${safe.slice(idx, end)}</span>${safe.slice(end)}`;
  }

  function toggleSearch(show?: boolean): void {
    const visible = show ?? searchWrap.style.display === 'none';
    searchWrap.style.display = visible ? 'block' : 'none';
    treeHost.style.display = visible ? 'none' : 'block';
    if (visible) searchInput.focus();
    else searchResults.textContent = '';
  }

  btnNewFile.addEventListener('click', () => {
    const target = tree.createTarget();
    if (target) void createEntry(target, 'file');
  });
  btnNewFolder.addEventListener('click', () => {
    const target = tree.createTarget();
    if (target) void createEntry(target, 'folder');
  });
  btnSearch.addEventListener('click', () => toggleSearch());
  openBtn.addEventListener('click', () => void openFolder());

  async function doReplaceAll(): Promise<void> {
    const query = searchInput.value;
    const replacement = replaceInput.value;
    if (!query || !state.projectRoot) return;
    const result = await confirmModal({
      title: 'Replace across project',
      message: `Replace every "${query}" with "${replacement}" in all project files? This rewrites files on disk and cannot be undone.`,
      confirmLabel: 'Replace all',
      danger: true,
    });
    if (!result.confirmed) return;
    try {
      const { files, replacements } = await window.void.fsReplace(state.projectRoot, query, replacement);
      showError(`Replaced ${replacements} occurrence${replacements === 1 ? '' : 's'} in ${files} file${files === 1 ? '' : 's'}.`);
      await refresh();
    } catch (err) {
      showError(err instanceof Error ? err.message : 'Replace failed');
    }
  }
  replaceBtn.addEventListener('click', () => void doReplaceAll());
  replaceInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') void doReplaceAll();
    if (e.key === 'Escape') toggleSearch(false);
  });

  searchInput.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') toggleSearch(false);
  });
  searchInput.addEventListener('input', () => {
    if (searchTimer !== null) window.clearTimeout(searchTimer);
    searchTimer = window.setTimeout(() => {
      const q = searchInput.value.trim();
      if (!q || !state.projectRoot) {
        searchResults.textContent = '';
        return;
      }
      window.void
        .fsSearch(state.projectRoot, q)
        .then(renderSearchHits)
        .catch(() => renderSearchHits([]));
    }, 250);
  });

  treeHost.addEventListener('contextmenu', (e) => {
    if (e.target === treeHost && state.projectRoot) {
      e.preventDefault();
      showContextMenu(e.clientX, e.clientY, [
        { label: 'New File', action: () => void createEntry(state.projectRoot!, 'file') },
        { label: 'New Folder', action: () => void createEntry(state.projectRoot!, 'folder') },
      ]);
    }
  });

  bus.on('sidebar-search', () => toggleSearch(true));
  bus.on('project-opened', ({ name }) => {
    title.textContent = name;
    openBtnWrap.style.display = 'none';
    void refresh();
  });
  // Cmd+Backspace: delete the file(s)/folder(s) selected in the tree.
  bus.on('delete-selected-file', () => {
    const nodes = tree.getSelectedNodes();
    if (nodes.length > 0) void deleteEntries(nodes);
  });
  // Enter / F2: rename the selected file/folder.
  bus.on('rename-selected-file', () => {
    const node = tree.getSelectedNode();
    if (node) void renameEntry(node);
  });
  window.void.onFsChanged(() => void refresh());
}
