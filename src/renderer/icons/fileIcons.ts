// Per-file-type and per-folder-type icons for the tree and tabs.
// Stroke-based, currentColor — color comes from the fi-* CSS class.

import { iconFile, iconFolder, iconFolderOpen, iconTerminal, iconLock, iconKey, iconBranch } from './index';

export interface FileIconSpec {
  svg: string;
  cls: string;
}

const svg = (inner: string): string =>
  `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" width="16" height="16">${inner}</svg>`;

// ---- file glyphs ----

const glyphBraces = svg(
  '<path d="M5.8 3c-1.3 0-2 .7-2 1.8v1.4c0 .9-.6 1.5-1.6 1.8 1 .3 1.6.9 1.6 1.8v1.4c0 1.1.7 1.8 2 1.8"/><path d="M10.2 3c1.3 0 2 .7 2 1.8v1.4c0 .9.6 1.5 1.6 1.8-1 .3-1.6.9-1.6 1.8v1.4c0 1.1-.7 1.8-2 1.8"/>'
);

const glyphJson = svg(
  '<path d="M5.8 3c-1.3 0-2 .7-2 1.8v1.4c0 .9-.6 1.5-1.6 1.8 1 .3 1.6.9 1.6 1.8v1.4c0 1.1.7 1.8 2 1.8"/><path d="M10.2 3c1.3 0 2 .7 2 1.8v1.4c0 .9.6 1.5 1.6 1.8-1 .3-1.6.9-1.6 1.8v1.4c0 1.1-.7 1.8-2 1.8"/><path d="M8 6.2v.01M8 9.8v.01"/>'
);

const glyphPython = svg(
  '<path d="M5.2 7V4.6A2.6 2.6 0 0 1 7.8 2h.4a2.6 2.6 0 0 1 2.6 2.6V7"/><path d="M10.8 9v2.4A2.6 2.6 0 0 1 8.2 14h-.4a2.6 2.6 0 0 1-2.6-2.6V9"/><path d="M5.2 7h5.6M5.2 9h5.6"/><path d="M7 4.4v.01M9 11.6v.01"/>'
);

const glyphCode = svg('<path d="M5.5 5 2.8 8l2.7 3M10.5 5l2.7 3-2.7 3"/>');

const glyphHtml = svg('<path d="M5 5.2 2.4 8 5 10.8M11 5.2 13.6 8 11 10.8M9.2 4 6.8 12"/>');

const glyphHash = svg('<path d="M6.4 3.8 5.4 12.2M10.6 3.8 9.6 12.2M3.9 6.3h8.6M3.5 9.7h8.6"/>');

const glyphMarkdown = svg(
  '<rect x="1.5" y="4" width="13" height="8" rx="1"/><path d="M3.8 10V6.6l1.9 1.9 1.9-1.9V10"/><path d="M10.9 6.6v3.2M9.5 8.4l1.4 1.5 1.4-1.5"/>'
);

const glyphList = svg(
  '<path d="M3 4.6h1.6M6.2 4.6H13M3 8h1.6M6.2 8h4.6M3 11.4h1.6M6.2 11.4H13"/>'
);

const glyphGear = svg(
  '<circle cx="8" cy="8" r="2.1"/><path d="M8 3.2v1.7M8 11.1v1.7M3.2 8h1.7M11.1 8h1.7M4.6 4.6l1.2 1.2M10.2 10.2l1.2 1.2M11.4 4.6l-1.2 1.2M5.8 10.2l-1.2 1.2"/>'
);

const glyphImage = svg(
  '<rect x="2" y="3.5" width="12" height="9" rx="1"/><circle cx="5.6" cy="6.4" r="1.1"/><path d="M4 12.5 7.6 9l2.1 2 2.2-2.2 2.1 2.1"/>'
);

const glyphDocker = svg(
  '<path d="M3 7.2h2.4v2.2H3zM6.8 7.2h2.4v2.2H6.8zM10.6 7.2H13v2.2h-2.4zM6.8 4.4h2.4v2.2H6.8z"/><path d="M2 11.8h12"/>'
);

const glyphDb = svg(
  '<ellipse cx="8" cy="4.3" rx="4.8" ry="1.9"/><path d="M3.2 4.3v7.2c0 1.05 2.15 1.9 4.8 1.9s4.8-.85 4.8-1.9V4.3"/><path d="M3.2 8c0 1.05 2.15 1.9 4.8 1.9S12.8 9.05 12.8 8"/>'
);

// ---- folder glyphs (closed outline + small mark; open shared) ----

const FOLDER_OUTLINE =
  '<path d="M1.5 3.5A.5.5 0 0 1 2 3h4l1.5 2H14a.5.5 0 0 1 .5.5V12a.5.5 0 0 1-.5.5H2a.5.5 0 0 1-.5-.5V3.5z"/>';

const folderWith = (mark: string): string => svg(FOLDER_OUTLINE + mark);

const folderCode = folderWith('<path d="M6.3 8.3 5 9.6l1.3 1.3M9.7 8.3 11 9.6l-1.3 1.3"/>');
const folderTest = folderWith('<path d="M6 9.7l1.4 1.4 2.8-2.8"/>');
const folderConfig = folderWith('<circle cx="8" cy="9.7" r="1.2"/><path d="M8 7.6v.9M8 10.8v.9M5.9 9.7h.9M9.2 9.7h.9"/>');
const folderAssets = folderWith('<circle cx="6.3" cy="8.8" r=".9"/><path d="M5.6 11.5l2-2 1.3 1.3 1.5-1.5"/>');
const folderPkg = folderWith('<rect x="6.6" y="8.3" width="2.8" height="2.8"/>');
const folderDocs = folderWith('<path d="M5.8 8.6h4.4M5.8 10.4h3"/>');
const folderScripts = folderWith('<path d="M5.8 8.4 7.2 9.7 5.8 11M8.8 11h2.4"/>');

// ---- maps ----

const EXT_ICONS: Record<string, FileIconSpec> = {
  ts: { svg: glyphBraces, cls: 'fi-accent' },
  tsx: { svg: glyphBraces, cls: 'fi-accent' },
  mts: { svg: glyphBraces, cls: 'fi-accent' },
  cts: { svg: glyphBraces, cls: 'fi-accent' },
  js: { svg: glyphBraces, cls: 'fi-amber' },
  jsx: { svg: glyphBraces, cls: 'fi-amber' },
  mjs: { svg: glyphBraces, cls: 'fi-amber' },
  cjs: { svg: glyphBraces, cls: 'fi-amber' },
  py: { svg: glyphPython, cls: 'fi-green' },
  go: { svg: glyphCode, cls: 'fi-bright' },
  rs: { svg: glyphCode, cls: 'fi-amber' },
  c: { svg: glyphCode, cls: 'fi-accent' },
  h: { svg: glyphCode, cls: 'fi-accent' },
  cpp: { svg: glyphCode, cls: 'fi-accent' },
  hpp: { svg: glyphCode, cls: 'fi-accent' },
  cc: { svg: glyphCode, cls: 'fi-accent' },
  java: { svg: glyphCode, cls: 'fi-red' },
  swift: { svg: glyphCode, cls: 'fi-amber' },
  rb: { svg: glyphCode, cls: 'fi-red' },
  php: { svg: glyphCode, cls: 'fi-bright' },
  html: { svg: glyphHtml, cls: 'fi-amber' },
  htm: { svg: glyphHtml, cls: 'fi-amber' },
  xml: { svg: glyphHtml, cls: 'fi-dim' },
  vue: { svg: glyphHtml, cls: 'fi-green' },
  svelte: { svg: glyphHtml, cls: 'fi-red' },
  css: { svg: glyphHash, cls: 'fi-accent' },
  scss: { svg: glyphHash, cls: 'fi-red' },
  less: { svg: glyphHash, cls: 'fi-accent' },
  json: { svg: glyphJson, cls: 'fi-amber' },
  jsonc: { svg: glyphJson, cls: 'fi-amber' },
  md: { svg: glyphMarkdown, cls: 'fi-bright' },
  markdown: { svg: glyphMarkdown, cls: 'fi-bright' },
  sh: { svg: iconTerminal, cls: 'fi-green' },
  zsh: { svg: iconTerminal, cls: 'fi-green' },
  bash: { svg: iconTerminal, cls: 'fi-green' },
  fish: { svg: iconTerminal, cls: 'fi-green' },
  yml: { svg: glyphList, cls: 'fi-red' },
  yaml: { svg: glyphList, cls: 'fi-red' },
  toml: { svg: glyphGear, cls: 'fi-dim' },
  ini: { svg: glyphGear, cls: 'fi-dim' },
  conf: { svg: glyphGear, cls: 'fi-dim' },
  cfg: { svg: glyphGear, cls: 'fi-dim' },
  plist: { svg: glyphGear, cls: 'fi-dim' },
  png: { svg: glyphImage, cls: 'fi-green' },
  jpg: { svg: glyphImage, cls: 'fi-green' },
  jpeg: { svg: glyphImage, cls: 'fi-green' },
  gif: { svg: glyphImage, cls: 'fi-green' },
  webp: { svg: glyphImage, cls: 'fi-green' },
  ico: { svg: glyphImage, cls: 'fi-green' },
  svg: { svg: glyphImage, cls: 'fi-amber' },
  sql: { svg: glyphDb, cls: 'fi-accent' },
  db: { svg: glyphDb, cls: 'fi-dim' },
  sqlite: { svg: glyphDb, cls: 'fi-dim' },
  txt: { svg: iconFile, cls: 'fi-dim' },
  log: { svg: iconFile, cls: 'fi-dim' },
};

const NAME_ICONS: Array<[RegExp, FileIconSpec]> = [
  [/^\.env(\..*)?$/, { svg: iconKey, cls: 'fi-amber' }],
  [/^(package-lock\.json|yarn\.lock|pnpm-lock\.yaml|cargo\.lock|poetry\.lock|gemfile\.lock|composer\.lock)$/, { svg: iconLock, cls: 'fi-dim' }],
  [/^\.git(ignore|attributes|modules)$/, { svg: iconBranch, cls: 'fi-red' }],
  [/^(dockerfile|containerfile)(\..*)?$/, { svg: glyphDocker, cls: 'fi-accent' }],
  [/^docker-compose(\..*)?\.(yml|yaml)$/, { svg: glyphDocker, cls: 'fi-accent' }],
  [/^makefile$/, { svg: iconTerminal, cls: 'fi-dim' }],
  [/^license(\..*)?$/, { svg: iconFile, cls: 'fi-dim' }],
  [/^\..*rc(\.(json|js|cjs|yml|yaml))?$/, { svg: glyphGear, cls: 'fi-dim' }],
];

const DEFAULT_FILE: FileIconSpec = { svg: iconFile, cls: 'fi-dim' };

const FOLDER_KINDS: Array<[RegExp, FileIconSpec]> = [
  [/^(src|source|lib|app|core|main)$/, { svg: folderCode, cls: 'fi-accent' }],
  [/^(test|tests|__tests__|spec|specs|e2e|fixtures)$/, { svg: folderTest, cls: 'fi-green' }],
  [/^(config|\.config|settings|conf|\.vscode|\.claude)$/, { svg: folderConfig, cls: 'fi-dim' }],
  [/^(assets|images|img|icons|public|static|media|fonts)$/, { svg: folderAssets, cls: 'fi-amber' }],
  [/^(vendor|packages|deps|third_party|build|out|target|bin|release)$/, { svg: folderPkg, cls: 'fi-dim' }],
  [/^(docs?|documentation)$/, { svg: folderDocs, cls: 'fi-bright' }],
  [/^(scripts?|tools?|utils?)$/, { svg: folderScripts, cls: 'fi-green' }],
];

const DEFAULT_FOLDER: FileIconSpec = { svg: iconFolder, cls: 'fi-folder' };

export function fileIcon(name: string): FileIconSpec {
  const lower = name.toLowerCase();
  for (const [pattern, spec] of NAME_ICONS) {
    if (pattern.test(lower)) return spec;
  }
  const ext = lower.includes('.') ? lower.split('.').pop()! : '';
  return EXT_ICONS[ext] ?? DEFAULT_FILE;
}

export function folderIcon(name: string, open: boolean): FileIconSpec {
  const kind = FOLDER_KINDS.find(([pattern]) => pattern.test(name.toLowerCase()));
  const cls = kind ? kind[1].cls : DEFAULT_FOLDER.cls;
  if (open) return { svg: iconFolderOpen, cls };
  return kind ? { svg: kind[1].svg, cls } : DEFAULT_FOLDER;
}

/** Color class only — used for the small type dot on editor tabs. */
export function fileColorClass(name: string): string {
  return fileIcon(name).cls;
}
