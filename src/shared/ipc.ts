// The single source of truth for the bridge between the UI and the Rust core.
// The renderer only ever talks to `window.void` (typed as VoidAPI); the
// implementation in src/renderer/bridge.ts maps each method onto a Tauri
// command defined in src-tauri/src/commands/. If a method isn't here, it
// doesn't exist.

import type {
  SupplierEntry,
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
  /** Background update: check + download (+ install in place on macOS). */
  autoUpdate(): Promise<{ status: 'off' | 'current' | 'installed' | 'ready'; version?: string }>;
  /** Restart into the new version (installs a waiting Windows update first). */
  applyUpdate(): Promise<void>;

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
    kind: 'image' | 'video';
    prompt: string;
    path: string;
    aspectRatio?: string;
    /** video: the project image to animate */
    image?: string;
    projectRoot: string;
  }): Promise<string>;

  /** BlazeAPI suppliers that serve an image model (for Settings). */
  aiImageSuppliers(model: string): Promise<SupplierEntry[]>;

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
  /** The bundled `blaze` CLI: is it in this build, and on the PATH? */
  cliStatus(): Promise<{ available: boolean; installed: boolean; path: string }>;
  /** Put `blaze` on the PATH (macOS may ask for an administrator password). */
  cliInstall(): Promise<string>;
  // Continue on mobile
  mobileStatus(): Promise<MobileStatus>;
  mobileSetPassword(password: string): Promise<MobileStatus>;
  mobileConfigure(cfg: { enabled: boolean; lan: boolean; quickTunnel: boolean; relay: boolean; domain: string; domainTunnel: boolean; port?: number }): Promise<MobileStatus>;
  mobileSetTunnelToken(token: string): Promise<MobileStatus>;
  mobilePairQr(url: string): Promise<{ link: string; svg: string }>;
  mobileRevoke(id: string): Promise<MobileStatus>;
  // BlBots
  botsList(): Promise<BotsOverview>;
  botsSave(bot: BotProfile): Promise<BotProfile>;
  botsDelete(id: string): Promise<void>;
  botsDuplicate(id: string): Promise<BotProfile>;
  botsThreads(bot: string): Promise<{ threads: Array<{ id: string; title: string; updatedAt: number; messageCount: number }>; memory: string }>;
  botsThread(bot: string, id: string): Promise<{ id: string; title: string; messages: BotMsg[]; pending: BotPending[] }>;
  botsSetMemory(bot: string, text: string): Promise<void>;
  botsSend(bot: string, thread: string | null, text: string): Promise<{ runId: string; chatId: string }>;
  botsCancel(runId: string): Promise<void>;
  botsAnswer(id: string, allow: boolean | null, answers: string[] | null): Promise<void>;
  botsComputer(bot: string): Promise<string>;
  roomsSave(room: { id: string; name: string; members: string[]; lead: string; instructions: string; folder: string }): Promise<{ id: string }>;
  roomsDelete(id: string): Promise<void>;
  roomsGet(id: string): Promise<{ id: string; name: string; messages: BotMsg[]; pending: BotPending[] }>;
  roomsSend(room: string, text: string): Promise<{ runId: string; chatId: string }>;
  onBotEvent(cb: (e: { runId: string; chatId: string; bot: string; ev: any }) => void): () => void;
  onBotPending(cb: (p: BotPending) => void): () => void;
  onBotRunEnd(cb: (e: { runId: string; chatId: string; error: string | null; stopped: boolean }) => void): () => void;
  onBotSched(cb: (e: { kind: 'started' | 'finished'; bot: string; thread: string | null; title: string; error?: string | null }) => void): () => void;
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

export interface MobileStatus {
  enabled: boolean;
  hasPassword: boolean;
  running: boolean;
  port: number;
  lan: boolean;
  quickTunnel: boolean;
  relay: boolean;
  domain: string | null;
  domainTunnel: boolean;
  hasTunnelToken: boolean;
  urls: Array<{ label: string; url: string }>;
  relayState: string;
  activeRuns: number;
  devices: Array<{ id: string; name: string; lastSeen: number; created: number }>;
  error: string | null;
}

export type BotSchedule = { kind: 'daily' | 'weekdays'; time: string } | { kind: 'weekly'; days: number[]; time: string } | { kind: 'every'; hours: number } | { kind: 'cron'; expr: string };
export interface BotRoutine { id: string; title: string; prompt: string; schedule: BotSchedule; enabled: boolean; catchUp: boolean; lastRun: number; thread: string | null }
export interface BotProfile {
  id: string; name: string; avatar: string; color: string; title: string; soul: string;
  engine: { kind: 'blaze' } | { kind: 'claude' | 'codex' | 'grok'; bin: string | null };
  model: string; approval: 'ask' | 'edits' | 'full'; folder: string; mode: string;
  routines: BotRoutine[]; checkBacks: Array<{ id: string; at: number; thread: string; note: string }>;
  pinned: boolean; hidden: boolean; created: number; computer: boolean;
}
export interface BotsOverview {
  bots: BotProfile[];
  rooms: Array<{ id: string; name: string; members: string[]; lead: string | null; instructions: string; folder: string; updated: number }>;
  engines: Array<{ name: string; path: string | null }>;
}
export interface BotMsg { role: string; text: string; tools?: string[]; author?: string }
export interface BotPending { id: string; kind: string; runId: string; chatId: string; summary?: string; questions?: Array<{ question: string; options: string[] }> }
