// Shared types between the Rust core and the renderer. No runtime imports here:
// types only, except small const objects safe in any context.

export type AIMode = 'ask' | 'plan' | 'agent';

/** How hard the model should reason. 'off' disables thinking where supported;
 *  the rest map to provider effort levels. */
export type ReasoningEffort = 'off' | 'low' | 'medium' | 'high' | 'max';

export const REASONING_LEVELS: ReasoningEffort[] = ['off', 'low', 'medium', 'high', 'max'];

/** The signed-in BlazeAPI account, as shown in the UI. The API key itself
 *  never leaves the Rust side (it lives in the OS keychain). */
export interface SessionPublic {
  userEmail: string;
  username: string;
  /** Owner account: sees the "Cheapest" routing choice. */
  admin?: boolean;
}

/** BlazeAPI routing for chat: '' = the account's own setting. */
/** 'direct' is the retired Direct routing; it now means 'trusted'. */
export type AIRouting = '' | 'normal' | 'trusted' | 'direct' | 'cheapest' | 'fastest';

/** A BlazeAPI supplier (codename) that serves a model. */
export interface SupplierEntry {
  id: string;
  status: string;
  price: number;
}

export type ModelKind = 'chat' | 'image' | 'video';

/** A BlazeAPI model offered in the picker. Prices are USD per 1M tokens
 *  (chat) or per image (image models); 0 when unknown. */
export interface ModelEntry {
  id: string;
  displayName: string;
  provider: string;
  contextWindow: number;
  kind: ModelKind;
  vision: boolean;
  /** Free tier: no cost to call. */
  free: boolean;
  priceIn: number;
  priceOut: number;
  tags: string[];
}

/** A user-added OpenAI-compatible endpoint (its API key stays in the OS keychain). */
export interface CustomProvider {
  id: string;
  name: string;
  baseUrl: string;
  models: string[];
  /** Endpoint: '' (find out), 'chat', 'responses' or 'anthropic'. */
  wire?: string;
  /** The models that can see images. */
  vision?: string[];
  hasKey: boolean;
}

export interface ProviderInput {
  id?: string;
  name: string;
  baseUrl: string;
  models: string[];
  wire?: string;
  /** The user's own "sees images" choices, by model. */
  vision?: Record<string, boolean>;
  /** Omit to keep the stored key; empty string removes it. */
  apiKey?: string;
}

export interface ChatMessage {
  role: 'system' | 'user' | 'assistant' | 'tool';
  content: string;
  toolCallId?: string;
  toolCalls?: ToolCall[];
  /** The model's thinking for a tool-call turn; sent back on the next round
   *  (thinking models refuse the request without it). */
  reasoning?: string;
  /** Images on a user message (data: URLs): pasted pictures, browser screenshots. */
  images?: string[];
}

export interface ToolCall {
  id: string;
  name: AgentToolName;
  arguments: string; // raw JSON string from the model
  /** Opaque provider-specific data that must be echoed back on later turns.
   *  Gemini puts a required `extra_content.google.thought_signature` here; if
   *  it isn't sent back with the assistant tool_call, Gemini rejects the turn. */
  extra?: Record<string, unknown>;
}

/** A question the AI asks the user via the ask_question tool. */
export interface AskQuestion {
  question: string;
  options?: string[];
}

/** A comment the user attached to an element in the web preview, queued as
 *  context for the next AI message. */
export interface ElementComment {
  id: string;
  comment: string;
  selector: string;   // a CSS-ish selector to locate the element
  tag: string;        // tag name, e.g. "button"
  text: string;       // trimmed text content snippet
  html: string;       // outerHTML excerpt (capped)
  pageUrl: string;    // the preview URL the element was on
}

export type AgentToolName =
  | 'read_file'
  | 'write_file'
  | 'apply_diff'
  | 'list_directory'
  | 'run_command'
  | 'search_files'
  | 'generate_image'
  | 'generate_video'
  | 'browser'
  | 'write_plan'
  | 'use_skill'
  | 'web_search'
  | 'web_fetch'
  | 'web_browser'
  | 'web_interact'
  | 'ask_question';

export interface AIRequest {
  requestId: string;
  model: string;
  messages: ChatMessage[];
  mode: AIMode;
  filename?: string;
  /** How hard the model should reason; defaults to 'medium'. */
  reasoningEffort?: ReasoningEffort;
  /** Whether the model may use the image generation tool this request. */
  mediaGen?: boolean;
}

/** Streamed chunks pushed main -> renderer over `ai:chunk`. */
export type AIStreamEvent =
  | { requestId: string; type: 'text'; text: string }
  | { requestId: string; type: 'reasoning'; text: string }
  | { requestId: string; type: 'tool_call'; toolCall: ToolCall }
  | { requestId: string; type: 'tool_delta'; index: number; id?: string; name?: string; argsDelta: string }
  | { requestId: string; type: 'usage'; inputTokens: number; outputTokens: number }
  | { requestId: string; type: 'balance'; usd?: number; tokensRemaining?: number; tokenLimit?: number }
  | { requestId: string; type: 'done' }
  | { requestId: string; type: 'error'; message: string; status?: number };

export interface FileNode {
  name: string;
  path: string;
  isDirectory: boolean;
  children?: FileNode[];
  /** A folder whose contents were not read yet (a big tree): opening it loads them. */
  truncated?: boolean;
}

/** A saved AI conversation, persisted as JSON under <project>/.blazeide/chats. */
export interface StoredChat {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  mode: AIMode;
  model: string;
  messages: ChatMessage[];
  /** Per user message: the files the agent changed after it, with their earlier
   *  content (null = the file did not exist), so the chat can be reverted. */
  checkpoints?: Checkpoint[];
  /** Set while this IDE's agent works on the chat (ms), so a phone shows it
   *  as busy. Missing once it is done. */
  live?: number;
}

export interface Checkpoint {
  /** Index in `messages` of the user message this turn started with. */
  userIndex: number;
  files: Array<{ rel: string; before: string | null }>;
}

/** Lightweight chat row for the history list (no message bodies). */
export interface ChatSummary {
  id: string;
  title: string;
  updatedAt: number;
  messageCount: number;
}

export interface SearchHit {
  path: string;
  line: number;
  preview: string;
}

/** The account's BlazeAPI pay-as-you-go balance. */
export interface UsageInfo {
  balanceUsd: number;
}

/** Backend version gate: drives the force-update screen. */
export interface VersionInfo {
  latest: string;
  min: string;
  current: string | null;
  mustUpdate: boolean;
  downloadUrl: string;
}

export type Theme = 'dark' | 'light';

export interface AppSettings {
  editorFontSize: number;
  editorMinimap: boolean;
  editorRelativeLineNumbers: boolean;
  /** Auto-save edits shortly after typing. Off = save manually with Cmd+S. */
  editorAutosave: boolean;
  editorWordWrap: boolean;
  terminalFontSize: number;
  /** Old "approve everything" switch; read only until aiApproval is set. */
  aiAutoApprove: boolean;
  /** What the chat agent may do without asking ('' = not chosen yet). */
  aiApproval?: ApprovalLevel | '';
  /** Web search and browsing for the AI chat, CLI and phone chats. */
  webAccess?: boolean;
  /** What turns sent from a phone may do without asking. */
  mobileApproval?: ApprovalLevel | '';
  /** Allow the AI to generate images (the generate_image tool). Off = that tool
   *  isn't offered, so no image generation happens. */
  aiMediaGen: boolean;
  /** Model used when the AI generates an image; '' = automatic. */
  imageModel: string;
  /** BlazeAPI supplier codename for the chosen image model; '' = automatic. */
  imageSupplier: string;
  /** Allow the agent to make video clips (generate_video). Costs per clip; off by default. */
  aiVideoGen: boolean;
  /** Video model for generate_video; '' = automatic. */
  videoModel: string;
  /** Chat routing: '' = account setting, 'normal', 'direct', 'cheapest' (owner). */
  aiRouting: AIRouting;
  /** Inline ghost-text completions while typing. */
  aiAutocomplete: boolean;
  /** Model for autocomplete; '' = a fast free model when available. */
  aiAutocompleteModel: string;
  /** Download and install new versions in the background (default on). */
  autoUpdate: boolean;
  /** Anonymous usage counts (opt-in; see Settings > Privacy). */
  analytics: boolean;
  /** Discord status "Working on <file> - BlazeCLI - blazeapi.org". */
  discordStatus: boolean;
  /** Editor colour theme from an installed extension (`ext:<id>/<label>`); '' = the app theme. */
  editorTheme: string;
  /** File icon theme from an extension, "<extId>/<themeId>"; "" = Blaze's own icons. */
  iconTheme: string;
  /** Let an extension colour theme recolour the whole window, not just the editor. */
  themeWorkbench: boolean;
  selectedModel: string;
  aiMode: AIMode;
  confirmDelete: boolean;
  sidebarWidth: number;
  aiPanelWidth: number;
  terminalHeight: number;
  reasoningEffort: ReasoningEffort;
  theme: Theme;
  /** data URL or file URL for a custom workbench background; null = none. */
  backgroundImage: string | null;
  /** Blur applied to the custom background, in px (0 = sharp). */
  backgroundBlur: number;
  /** Opacity of the custom background overlay (0–1; lower = more subtle). */
  backgroundOpacity: number;
  /** Horizontal focal point of the background, 0–100% (50 = centered). */
  backgroundPosX: number;
  /** Vertical focal point of the background, 0–100% (50 = centered). */
  backgroundPosY: number;
  /** Background zoom, as a percentage of the base size. 100 = base (cover or
   *  contain per backgroundFit); below 100 zooms out, above zooms in. */
  backgroundScale: number;
  /** How the image is sized to the window: 'cover' fills it (may crop),
   *  'contain' shows the whole image without upscaling past its size. */
  backgroundFit: 'cover' | 'contain';
  /** Background rotation in degrees. */
  backgroundRotation: number;
  /** File path to a looping video background; null = none. Takes precedence
   *  over backgroundImage when set. */
  backgroundVideo: string | null;
  /** Sharpen/enhance the background (crisp scaling + contrast/saturation boost). */
  backgroundSharpen: boolean;
  /** Strength of the sharpen/enhance effect, 0–100. */
  backgroundSharpenAmount: number;
  /** Auto-adjust text colour for contrast against the custom background's
   *  brightness (light text on dark images, dark text on light). Off = theme. */
  backgroundAutoContrast: boolean;
  /** Custom keybindings: action id → chord string (e.g. "Cmd+B"). Missing
   *  entries fall back to DEFAULT_KEYBINDINGS. */
  keybindings: Record<string, string>;
  /** Set once the post-sign-in onboarding has been completed/skipped. */
  onboarded: boolean;
  /** Play a looping ambient background sound while using the IDE. */
  ambientEnabled: boolean;
  /** Selected ambient source: a preset id (e.g. 'babft') or 'custom'. */
  ambientSource: string;
  /** Local file path for a user-uploaded ambient sound (when source = custom). */
  ambientCustomPath: string | null;
  /** Ambient volume, 0–1. */
  ambientVolume: number;
}

/** Actions a keybinding can trigger. Kept in sync with the dispatcher. */
export type KeybindAction =
  | 'toggle-sidebar'
  | 'toggle-ai-panel'
  | 'toggle-terminal'
  | 'command-palette'
  | 'file-palette'
  | 'save'
  | 'close-tab'
  | 'delete-file'
  | 'settings'
  | 'account'
  | 'open-folder';

export interface KeybindDef {
  action: KeybindAction;
  label: string;
  /** Default chord, in the app's normalized form (e.g. "Cmd+Shift+A"). */
  default: string;
}

/** The canonical action list + their default chords + human labels. */
export const KEYBIND_DEFS: KeybindDef[] = [
  { action: 'toggle-sidebar', label: 'Toggle Sidebar', default: 'Cmd+B' },
  { action: 'toggle-ai-panel', label: 'Toggle AI Panel', default: 'Cmd+J' },
  { action: 'toggle-terminal', label: 'Toggle Terminal', default: 'Ctrl+`' },
  { action: 'command-palette', label: 'Command Palette', default: 'Cmd+K' },
  { action: 'file-palette', label: 'Open File', default: 'Cmd+P' },
  { action: 'save', label: 'Save', default: 'Cmd+S' },
  { action: 'close-tab', label: 'Close Tab', default: 'Cmd+W' },
  { action: 'delete-file', label: 'Delete File', default: 'Cmd+Backspace' },
  { action: 'settings', label: 'Settings', default: 'Cmd+,' },
  { action: 'account', label: 'Account', default: 'Cmd+Shift+A' },
  { action: 'open-folder', label: 'Open Folder', default: 'Cmd+O' },
];

export const DEFAULT_KEYBINDINGS: Record<string, string> = Object.fromEntries(
  KEYBIND_DEFS.map((d) => [d.action, d.default])
);

export const DEFAULT_SETTINGS: AppSettings = {
  editorFontSize: 13,
  editorMinimap: false,
  editorRelativeLineNumbers: false,
  editorAutosave: false,
  editorWordWrap: false,
  terminalFontSize: 13,
  aiAutoApprove: false,
  aiApproval: '',
  webAccess: true,
  aiMediaGen: false,
  imageModel: '',
  imageSupplier: '',
  aiVideoGen: false,
  videoModel: '',
  aiRouting: '',
  aiAutocomplete: true,
  aiAutocompleteModel: '',
  autoUpdate: true,
  analytics: false,
  discordStatus: true,
  editorTheme: '',
  iconTheme: '',
  themeWorkbench: true,
  selectedModel: 'claude-sonnet-5',
  aiMode: 'ask',
  confirmDelete: true,
  sidebarWidth: 220,
  aiPanelWidth: 320,
  terminalHeight: 200,
  reasoningEffort: 'medium',
  theme: 'dark',
  backgroundImage: null,
  backgroundBlur: 0,
  backgroundOpacity: 0.5,
  backgroundPosX: 50,
  backgroundPosY: 50,
  backgroundScale: 100,
  backgroundFit: 'cover',
  backgroundRotation: 0,
  backgroundVideo: null,
  backgroundSharpen: false,
  backgroundSharpenAmount: 50,
  backgroundAutoContrast: false,
  keybindings: {},
  onboarded: false,
  ambientEnabled: false,
  ambientSource: 'babft',
  ambientCustomPath: null,
  ambientVolume: 0.4,
};

/** One result from the extension marketplace (Open VSX). */
export interface MarketItem {
  id: string;
  namespace: string;
  name: string;
  displayName: string;
  description: string;
  version: string;
  downloads: number;
  rating: number | null;
  verified: boolean;
  iconUrl: string | null;
}

export interface ExtDetails {
  id: string;
  namespace: string;
  name: string;
  displayName: string;
  description: string;
  version: string;
  downloads: number;
  rating: number | null;
  reviewCount: number;
  verified: boolean;
  iconUrl: string | null;
  homepage: string;
  repository: string;
  bugs: string;
  license: string;
  categories: string[];
  tags: string[];
  updated: string;
  size: number;
  versions: number;
  galleryColor: string;
  publisher: {
    name: string;
    namespace: string;
    login: string;
    fullName: string;
    avatar: string | null;
    homepage: string;
    verified: boolean;
    extensions: number;
  };
  readme: string;
  changelog: string;
  /** Image URL as written in the readme -> data: URL. */
  images: Record<string, string>;
  themes: string[];
  snippets: string[];
  iconThemes: string[];
  unsupported: string[];
}

/** A file icon theme: VS Code's mappings plus each icon as a data: URL. */
export interface IconThemeData {
  extId: string;
  id: string;
  label: string;
  map: Record<string, unknown>;
  icons: Record<string, string>;
}

export interface InstalledExt {
  id: string;
  displayName: string;
  description: string;
  version: string;
  themes: Array<{ label: string; base: string; path: string }>;
  snippets: Array<{ language: string; path: string }>;
  /** File icon themes (older installs: missing). */
  iconThemes?: Array<{ id: string; label: string; path: string }>;
  /** Parts of the extension that cannot run here (its own code, grammars, commands…). */
  unsupported: string[];
  /** The extension's own icon (data: URL), when it has one. */
  icon?: string | null;
}

export interface ExtContributions {
  themes: Array<{ extId: string; label: string; base: string; json: Record<string, unknown> }>;
  snippets: Array<{ extId: string; language: string; json: Record<string, unknown> }>;
}

export type ApprovalLevel = 'ask' | 'edits' | 'auto' | 'full';
