// The single source of truth for the bridge between the UI and the Rust core.
// The renderer only ever talks to `window.void` (typed as VoidAPI); the
// implementation in src/renderer/bridge.ts maps each method onto a Tauri
// command defined in src-tauri/src/commands/. If a method isn't here, it
// doesn't exist.

import type {
  ExtContributions,
  InstalledExt,
  MarketItem,
  AIRequest,
  AIStreamEvent,
  AppSettings,
  ChatSummary,
  CustomProvider,
  FileNode,
  ModelEntry,
  ProviderInput,
  SearchHit,
  SessionPublic,
  StoredChat,
  UsageInfo,
  VersionInfo,
} from './types';

export interface VoidAPI {
  // --- auth (BlazeAPI account, browser sign-in) ---
  authGetSession(): Promise<SessionPublic | null>;
  /** Open BlazeAPI's sign-in page in the browser. Resolves once the browser
   *  hands the session back to the app (or rejects on cancel/timeout). */
  authSignIn(): Promise<SessionPublic>;
  /** Stop waiting for an in-progress browser sign-in. */
  authCancel(): Promise<void>;
  authLogout(): Promise<void>;
  authGetUsage(): Promise<UsageInfo | null>;
  /** Check for a newer release (drives the update banner). */
  checkVersion(): Promise<VersionInfo>;
  /** Download and install the available update, then restart. */
  installUpdate(): Promise<void>;
  /** Download progress of installUpdate (bytes; total may be unknown). */
  onUpdateProgress(cb: (done: number, total: number | null) => void): () => void;

  // --- fs ---
  fsOpenFolder(): Promise<string | null>; // native dialog; returns root path
  fsReadTree(root: string): Promise<FileNode>;
  fsReadFile(path: string): Promise<string>;
  /** Like fsReadFile but resolves to null (no throw) when the file is missing. */
  fsTryReadFile(path: string): Promise<string | null>;
  fsWriteFile(path: string, content: string): Promise<void>;
  fsCreateFile(path: string): Promise<void>;
  fsCreateFolder(path: string): Promise<void>;
  fsRename(oldPath: string, newPath: string): Promise<void>;
  fsDelete(path: string): Promise<void>;
  fsSearch(root: string, query: string): Promise<SearchHit[]>;
  /** Replace every literal occurrence of `query` with `replacement` across the
   *  project's text files. Returns counts of files changed + replacements. */
  fsReplace(root: string, query: string, replacement: string): Promise<{ files: number; replacements: number }>;
  fsWatch(root: string): Promise<void>;
  fsUnwatch(): Promise<void>;
  onFsChanged(cb: () => void): () => void;
  getRecentProjects(): Promise<string[]>;
  /** URL the webview can load a local file from (images, media, preview).
   *  Only files inside an open project or a file the user picked are served. */
  mediaUrl(path: string): string;

  // --- terminal ---
  termCreate(cwd: string): Promise<string>; // returns terminal id
  termWrite(id: string, data: string): void;
  termResize(id: string, cols: number, rows: number): void;
  termKill(id: string): void;
  onTermData(cb: (id: string, data: string) => void): () => void;
  onTermExit(cb: (id: string) => void): () => void;

  // --- ai ---
  aiSend(req: AIRequest): Promise<void>; // stream arrives via onAIChunk
  aiCancel(requestId: string): void;
  onAIChunk(cb: (ev: AIStreamEvent) => void): () => void;
  aiGetModels(): Promise<ModelEntry[]>;
  /** Force-refresh the model list from BlazeAPI, bypassing the cache. */
  aiRefreshModels(): Promise<ModelEntry[]>;
  aiRunTool(name: string, args: string, projectRoot: string): Promise<string>;
  /** Generate an image with a BlazeAPI image model and save it into the
   *  project. Returns a human/tool-result string (path written, or an error). */
  aiGenerateMedia(req: {
    kind: 'image';
    prompt: string;
    path: string;
    aspectRatio?: string;
    projectRoot: string;
  }): Promise<string>;

  /** Ghost-text completion for the cursor position ('' when none). */
  aiComplete(req: { prefix: string; suffix: string; filename?: string }): Promise<string>;

  // --- custom providers (OpenAI-compatible endpoints; keys stay in the keychain) ---
  providersList(): Promise<CustomProvider[]>;
  providersSave(input: ProviderInput): Promise<CustomProvider>;
  providersDelete(id: string): Promise<void>;
  /** Ask an endpoint which models it serves. */
  providersFetchModels(baseUrl: string, apiKey?: string, id?: string): Promise<string[]>;

  // --- VS Code extensions from Open VSX (themes and snippets; no extension host) ---
  extSearch(query: string, category?: string, offset?: number): Promise<{ items: MarketItem[]; total: number }>;
  extInstall(namespace: string, name: string): Promise<InstalledExt>;
  extUninstall(id: string): Promise<void>;
  extList(): Promise<InstalledExt[]>;
  extContributions(): Promise<ExtContributions>;

  // --- chats (persisted under <project>/.blazeide/chats) ---
  chatList(projectRoot: string): Promise<ChatSummary[]>;
  chatLoad(projectRoot: string, id: string): Promise<StoredChat | null>;
  chatSave(projectRoot: string, chat: StoredChat): Promise<void>;
  chatDelete(projectRoot: string, id: string): Promise<void>;

  // --- settings ---
  settingsGet(): Promise<AppSettings>;
  settingsSet(patch: Partial<AppSettings>): Promise<void>;

  // --- shell ---
  /** Open an http(s) URL in the default browser. Other schemes are refused. */
  openExternal(url: string): Promise<void>;
  /** Native picker for a background image/video or ambient audio file. */
  pickMedia(kind: 'image' | 'video' | 'audio'): Promise<string | null>;
  /** 'mac' | 'windows' | 'linux' — for platform-specific labels and chords. */
  platform(): 'mac' | 'windows' | 'linux';

  // --- windows ---
  winNew(): Promise<void>; // blank workbench window
  winOpenFile(path: string, projectRoot: string | null): Promise<void>; // pop a file out

  // --- menu events (native menu -> renderer) ---
  onMenu(cb: (action: MenuAction) => void): () => void;
}

export type MenuAction =
  | 'open-folder'
  | 'new-file'
  | 'new-window'
  | 'save'
  | 'close-tab'
  | 'toggle-sidebar'
  | 'toggle-ai-panel'
  | 'toggle-terminal'
  | 'command-palette'
  | 'settings'
  | 'account';
