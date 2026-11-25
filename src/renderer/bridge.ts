// window.void: the UI's only door to the system. Each method maps onto one
// Tauri command implemented in Rust (src-tauri/src/commands.rs). Imported
// first by index.ts so the global exists before any module touches it.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { MenuAction, VoidAPI } from '../shared/ipc';
import type { AIStreamEvent, AppSettings } from '../shared/types';
import { DEFAULT_SETTINGS } from '../shared/types';

function detectPlatform(): 'mac' | 'windows' | 'linux' {
  const ua = navigator.userAgent;
  if (/Mac OS X|Macintosh/.test(ua)) return 'mac';
  if (/Windows/.test(ua)) return 'windows';
  return 'linux';
}
const PLATFORM = detectPlatform();

/** Subscribe to a Rust event with a synchronous unsubscribe, which is what the
 *  UI modules expect. Unsubscribing before `listen` resolves still works. */
function on<T>(event: string, cb: (payload: T) => void): () => void {
  let unlisten: UnlistenFn | null = null;
  let cancelled = false;
  void listen<T>(event, (e) => cb(e.payload)).then((fn) => {
    if (cancelled) fn();
    else unlisten = fn;
  });
  return () => {
    cancelled = true;
    unlisten?.();
  };
}

/** Each path segment is percent-encoded but slashes are kept, so relative
 *  links inside a previewed page resolve to sibling files. */
function mediaUrl(path: string): string {
  const segs = path.replace(/\\/g, '/').split('/').map((s, i) => (i === 0 && /^[A-Za-z]:$/.test(s) ? s : encodeURIComponent(s)));
  const p = segs.join('/');
  return PLATFORM === 'windows' ? `http://blazemedia.localhost/${p.replace(/^\/+/, '')}` : `blazemedia://localhost${p.startsWith('/') ? p : '/' + p}`;
}

const api: VoidAPI = {
  authGetSession: () => invoke('auth_get_session'),
  authSignIn: () => invoke('auth_sign_in'),
  authCancel: () => invoke('auth_cancel'),
  authLogout: () => invoke('auth_logout'),
  authGetUsage: () => invoke('auth_get_usage'),
  checkVersion: () => invoke('check_version'),
  installUpdate: () => invoke('install_update'),
  autoUpdate: () => invoke('auto_update'),
  applyUpdate: () => invoke('apply_update'),
  onUpdateProgress: (cb) => on<{ done: number; total: number | null }>('update-progress', (p) => cb(p.done, p.total)),

  fsOpenFolder: () => invoke('fs_open_folder'),
  fsReadTree: (root) => invoke('fs_read_tree', { root }),
  fsReadFile: (path) => invoke('fs_read_file', { path }),
  fsTryReadFile: (path) => invoke('fs_try_read_file', { path }),
  fsWriteFile: (path, content) => invoke('fs_write_file', { path, content }),
  fsCreateFile: (path) => invoke('fs_create_file', { path }),
  fsCreateFolder: (path) => invoke('fs_create_folder', { path }),
  fsRename: (oldPath, newPath) => invoke('fs_rename', { oldPath, newPath }),
  fsDelete: (path) => invoke('fs_delete', { path }),
  fsSearch: (root, query) => invoke('fs_search', { root, query }),
  fsReplace: (root, query, replacement) => invoke('fs_replace', { root, query, replacement }),
  fsWatch: (root) => invoke('fs_watch', { root }),
  fsUnwatch: () => invoke('fs_unwatch'),
  onFsChanged: (cb) => on<null>('fs-changed', () => cb()),
  getRecentProjects: () => invoke('get_recent_projects'),
  mediaUrl,

  termCreate: (cwd) => invoke('term_create', { cwd }),
  termWrite: (id, data) => void invoke('term_write', { id, data }),
  termResize: (id, cols, rows) => void invoke('term_resize', { id, cols: Math.max(2, Math.floor(cols)), rows: Math.max(1, Math.floor(rows)) }),
  termKill: (id) => void invoke('term_kill', { id }),
  onTermData: (cb) => on<{ id: string; data: string }>('term-data', (p) => cb(p.id, p.data)),
  onTermExit: (cb) => on<string>('term-exit', (id) => cb(id)),

  aiSend: (req) => invoke('ai_send', { req }),
  aiCancel: (requestId) => void invoke('ai_cancel', { requestId }),
  onAIChunk: (cb) => on<AIStreamEvent>('ai-chunk', cb),
  aiGetModels: () => invoke('ai_get_models'),
  aiRefreshModels: () => invoke('ai_refresh_models'),
  aiRunTool: (name, args, projectRoot) => invoke('ai_run_tool', { name, args, projectRoot }),
  aiGenerateMedia: (req) => invoke('ai_generate_media', { req }),
  aiImageSuppliers: (model) => invoke('ai_image_suppliers', { model }),
  aiComplete: (req) => invoke('ai_complete', { req }),

  providersList: () => invoke('providers_list'),
  providersSave: (input) => invoke('providers_save', { input }),
  providersDelete: (id) => invoke('providers_delete', { id }),
  providersFetchModels: (baseUrl, apiKey, id) => invoke('providers_fetch_models', { baseUrl, apiKey, id }),

  extSearch: (query, category, offset) => invoke('ext_search', { query, category, offset }),
  extDetails: (namespace, name) => invoke('ext_details', { namespace, name }),
  extIconTheme: (extId, themeId) => invoke('ext_icon_theme', { extId, themeId }),
  extInstall: (namespace, name) => invoke('ext_install', { namespace, name }),
  extUninstall: (id) => invoke('ext_uninstall', { id }),
  extList: () => invoke('ext_list'),
  extContributions: () => invoke('ext_contributions'),

  chatList: (projectRoot) => invoke('chat_list', { projectRoot }),
  chatLoad: (projectRoot, id) => invoke('chat_load', { projectRoot, id }),
  chatSave: (projectRoot, chat) => invoke('chat_save', { projectRoot, chat }),
  chatDelete: (projectRoot, id) => invoke('chat_delete', { projectRoot, id }),

  // Rust stores only what the user changed; defaults live here, so a new
  // setting needs no migration.
  settingsGet: async () => ({ ...DEFAULT_SETTINGS, ...(await invoke<Partial<AppSettings>>('settings_get')) }),
  settingsSet: (patch) => invoke('settings_set', { patch }),

  openExternal: (url) => invoke('open_external', { url }),
  pickMedia: (kind) => invoke('pick_media', { kind }),
  cliStatus: () => invoke('cli_status'),
  cliInstall: () => invoke('cli_install'),
  presenceSet: (what) => invoke('presence_set', { what }),
  mobileStatus: () => invoke('mobile_status'),
  mobileSetPassword: (password) => invoke('mobile_set_password', { password }),
  mobileConfigure: (cfg) => invoke('mobile_configure', { cfg }),
  mobileSetTunnelToken: (token) => invoke('mobile_set_tunnel_token', { token }),
  mobilePairQr: (url) => invoke('mobile_pair_qr', { url }),
  mobileRevoke: (id) => invoke('mobile_revoke', { id }),
  botsList: () => invoke('bots_list'),
  botsSave: (bot) => invoke('bots_save', { bot }),
  botsDelete: (id) => invoke('bots_delete', { id }),
  botsDuplicate: (id) => invoke('bots_duplicate', { id }),
  botsThreads: (bot) => invoke('bots_threads', { bot }),
  botsThread: (bot, id) => invoke('bots_thread', { bot, id }),
  botsSetMemory: (bot, text) => invoke('bots_set_memory', { bot, text }),
  botsSend: (bot, thread, text) => invoke('bots_send', { bot, thread, text }),
  botsCancel: (runId) => invoke('bots_cancel', { runId }),
  botsAnswer: (id, allow, answers) => invoke('bots_answer', { id, allow, answers }),
  botsComputer: (bot) => invoke('bots_computer', { bot }),
  roomsSave: (room) => invoke('rooms_save', { room }),
  roomsDelete: (id) => invoke('rooms_delete', { id }),
  roomsGet: (id) => invoke('rooms_get', { id }),
  roomsSend: (room, text) => invoke('rooms_send', { room, text }),
  onBotEvent: (cb) => on('bot-event', cb),
  onBotPending: (cb) => on('bot-pending', cb),
  onBotRunEnd: (cb) => on('bot-run-end', cb),
  onBotSched: (cb) => on('bot-sched', cb),
  platform: () => PLATFORM,

  winNew: () => invoke('win_new'),
  winOpenFile: (path, projectRoot) => invoke('win_open_file', { path, projectRoot }),

  onMenu: (cb) => on<MenuAction>('menu', cb),
};

window.void = api;
document.documentElement.dataset.platform = PLATFORM;
