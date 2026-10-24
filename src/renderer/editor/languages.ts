// File-extension → Monaco language id mapping.

const EXT_MAP: Record<string, string> = {
  ts: 'typescript',
  tsx: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  js: 'javascript',
  jsx: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  py: 'python',
  go: 'go',
  rs: 'rust',
  css: 'css',
  scss: 'scss',
  less: 'less',
  html: 'html',
  htm: 'html',
  json: 'json',
  md: 'markdown',
  markdown: 'markdown',
  sh: 'shell',
  zsh: 'shell',
  bash: 'shell',
  yaml: 'yaml',
  yml: 'yaml',
  toml: 'ini',
  c: 'c',
  h: 'c',
  cpp: 'cpp',
  hpp: 'cpp',
  cc: 'cpp',
  java: 'java',
  swift: 'swift',
  xml: 'xml',
  svg: 'xml',
  sql: 'sql',
};

export function languageForPath(path: string): string {
  const name = path.split('/').pop() ?? '';
  if (name === 'Dockerfile') return 'dockerfile';
  if (name === 'Makefile') return 'makefile';
  const ext = name.includes('.') ? name.split('.').pop()!.toLowerCase() : '';
  return EXT_MAP[ext] ?? 'plaintext';
}
