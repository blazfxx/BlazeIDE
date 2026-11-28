// BlBots avatars: a body shape, an expression, a colour, white or black
// features and an optional frame, drawn as SVG. Original artwork for Blaze IDE.
// A bot can also use a picture (uploaded or generated) instead.

export const FACE_COLORS = ['#3f9e5f', '#4a7fe0', '#d05656', '#e08a3e', '#7c5cc9', '#45a5c4', '#cf5c8e', '#d9ae3e', '#3e9e94', '#e0684d', '#a5a8b0', '#34343a'];
export const FACE_COUNT = 8;

export interface AvatarStyle {
  face?: number;
  body?: string;
  color: string;
  features?: string;
  frame?: string;
}

export const BODIES: Array<{ key: string; label: string }> = [
  { key: 'bot', label: 'Bot' }, { key: 'blob', label: 'Blob' }, { key: 'circle', label: 'Circle' }, { key: 'squircle', label: 'Square' }, { key: 'pill', label: 'Pill' },
  { key: 'drop', label: 'Drop' }, { key: 'bell', label: 'Bell' }, { key: 'hex', label: 'Hexagon' }, { key: 'diamond', label: 'Diamond' }, { key: 'star', label: 'Star' },
];
export const FRAMES: Array<{ key: string; label: string }> = [
  { key: '', label: 'Shape' }, { key: 'circle', label: 'Circle' }, { key: 'rounded', label: 'Rounded' }, { key: 'square', label: 'Square' },
];

function starPath(cx: number, cy: number, ro: number, ri: number): string {
  const pts: string[] = [];
  for (let i = 0; i < 10; i++) {
    const r = i % 2 ? ri : ro;
    const a = -Math.PI / 2 + (i * Math.PI) / 5;
    pts.push(`${(cx + r * Math.cos(a)).toFixed(1)},${(cy + r * Math.sin(a)).toFixed(1)}`);
  }
  return pts.join(' ');
}

/** Body outline + where the face sits (scale around 32,34). */
const BODY: Record<string, { el: (fill: string) => string; face: number; dy?: number }> = {
  bot: { el: (f) => `<rect x="6" y="9" width="52" height="50" rx="17" fill="${f}"/>`, face: 1 },
  blob: { el: (f) => `<path d="M33 7c12.5 0 23.5 9 23.5 22.5S47.5 58 31 58 7.5 47 7.5 32.5 19.5 7 33 7z" fill="${f}"/>`, face: 1 },
  circle: { el: (f) => `<circle cx="32" cy="33" r="25.5" fill="${f}"/>`, face: 1 },
  squircle: { el: (f) => `<rect x="8" y="9" width="48" height="48" rx="11" fill="${f}"/>`, face: 0.95 },
  pill: { el: (f) => `<rect x="13" y="5" width="38" height="55" rx="19" fill="${f}"/>`, face: 0.85 },
  drop: { el: (f) => `<path d="M32 5C32 5 54 29 54 40.5a22 22 0 0 1-44 0C10 29 32 5 32 5z" fill="${f}"/>`, face: 0.8, dy: 6 },
  bell: { el: (f) => `<path d="M32 7c-12 0-19 10-19 24v19c0 4.5 3 8 7.5 8h23c4.5 0 7.5-3.5 7.5-8V31C51 17 44 7 32 7z" fill="${f}"/>`, face: 0.85, dy: 3 },
  hex: { el: (f) => `<polygon points="32,5 56,19 56,46 32,60 8,46 8,19" fill="${f}" stroke="${f}" stroke-width="4" stroke-linejoin="round"/>`, face: 0.9 },
  diamond: { el: (f) => `<polygon points="32,5 59,32.5 32,60 5,32.5" fill="${f}" stroke="${f}" stroke-width="4" stroke-linejoin="round"/>`, face: 0.72 },
  star: { el: (f) => `<polygon points="${starPath(32, 35, 29, 14)}" fill="${f}" stroke="${f}" stroke-width="4" stroke-linejoin="round"/>`, face: 0.62, dy: 2 },
};

function features(face: number, eye: string, pupil: string): string {
  const F: Record<number, string> = {
    1: `<circle cx="23" cy="29" r="4.2" fill="${eye}"/><circle cx="41" cy="29" r="4.2" fill="${eye}"/><path d="M24 41c4.5 4.5 11.5 4.5 16 0" stroke="${eye}" stroke-width="3.4" fill="none" stroke-linecap="round"/>`,
    2: `<path d="M18 30c2.5-4 7.5-4 10 0M36 30c2.5-4 7.5-4 10 0" stroke="${eye}" stroke-width="3.4" fill="none" stroke-linecap="round"/><path d="M23 39h18c0 6-4 9.5-9 9.5S23 45 23 39z" fill="${eye}"/>`,
    3: `<circle cx="23" cy="29" r="4.2" fill="${eye}"/><path d="M36.5 29.5h9" stroke="${eye}" stroke-width="3.4" stroke-linecap="round"/><path d="M25 42c5 2.5 10 2 15-2" stroke="${eye}" stroke-width="3.4" fill="none" stroke-linecap="round"/>`,
    4: `<rect x="19.5" y="22" width="7" height="13" rx="3.5" fill="${eye}"/><rect x="37.5" y="22" width="7" height="13" rx="3.5" fill="${eye}"/><circle cx="32" cy="44" r="3.6" fill="${eye}"/>`,
    5: `<rect x="14" y="22" width="36" height="14" rx="7" fill="rgba(0,0,0,.32)"/><circle cx="24" cy="29" r="3.4" fill="${eye}"/><circle cx="40" cy="29" r="3.4" fill="${eye}"/><path d="M26 44h12" stroke="${eye}" stroke-width="3.4" stroke-linecap="round"/>`,
    6: `<path d="M18.5 30.5h9M36.5 30.5h9" stroke="${eye}" stroke-width="3.4" stroke-linecap="round"/><path d="M26 41.5c3.5 2.6 8.5 2.6 12 0" stroke="${eye}" stroke-width="3.4" fill="none" stroke-linecap="round"/>`,
    7: `<circle cx="23" cy="29" r="6.5" fill="${eye}"/><circle cx="41" cy="29" r="6.5" fill="${eye}"/><circle cx="24.5" cy="30" r="2.8" fill="${pupil}"/><circle cx="42.5" cy="30" r="2.8" fill="${pupil}"/><path d="M24 42.5c5 4 11 4 16 0" stroke="${eye}" stroke-width="3.4" fill="none" stroke-linecap="round"/>`,
    8: `<rect x="18.5" y="25" width="9" height="9" rx="2.2" fill="${eye}"/><rect x="36.5" y="25" width="9" height="9" rx="2.2" fill="${eye}"/><path d="M25 43.5h14" stroke="${eye}" stroke-width="3.4" stroke-linecap="round"/>`,
  };
  return F[face] ?? F[1];
}

function shade(hex: string, f: number): string {
  const n = parseInt(hex.slice(1), 16);
  const c = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => Math.round(f < 0 ? v * (1 + f) : v + (255 - v) * f));
  return `#${c.map((v) => v.toString(16).padStart(2, '0')).join('')}`;
}

let gid = 0;
/** SVG markup of a drawn avatar. */
export function avatarSvg(s: AvatarStyle, size: number): string {
  const c = /^#[0-9a-fA-F]{6}$/.test(s.color) ? s.color : FACE_COLORS[1];
  const face = s.face && s.face >= 1 && s.face <= FACE_COUNT ? s.face : 1;
  const bodyKey = s.body && BODY[s.body] ? s.body : 'bot';
  const body = BODY[bodyKey];
  const id = `bba${++gid}`;
  const eye = s.features === 'black' ? '#141414' : '#ffffff';
  const pupil = s.features === 'black' ? '#ffffff' : '#141414';
  const grad = `<linearGradient id="${id}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${shade(c, 0.22)}"/><stop offset="1" stop-color="${shade(c, -0.2)}"/></linearGradient>`;
  const antenna = bodyKey === 'bot' && (face === 5 || face === 8)
    ? `<path d="M32 9v-3" stroke="${shade(c, -0.25)}" stroke-width="3" stroke-linecap="round"/><circle cx="32" cy="4.5" r="3" fill="${shade(c, 0.35)}"/>` : '';
  const shine = bodyKey === 'bot' || bodyKey === 'squircle' ? '<ellipse cx="22" cy="16" rx="9" ry="3" fill="rgba(255,255,255,.18)"/>' : '';
  const k = body.face;
  const fx = `<g transform="translate(${32 - 32 * k} ${34 - 34 * k + (body.dy ?? 0)}) scale(${k})">${features(face, eye, pupil)}</g>`;
  const figure = `${antenna}${body.el(`url(#${id})`)}${shine}${fx}`;
  let inner = figure;
  if (s.frame === 'circle' || s.frame === 'rounded' || s.frame === 'square') {
    const bg = s.frame === 'circle' ? '<circle cx="32" cy="32" r="32" fill="BG"/>' : `<rect width="64" height="64" rx="${s.frame === 'rounded' ? 16 : 4}" fill="BG"/>`;
    inner = bg.replace('BG', shade(c, -0.62)) + `<g transform="translate(9.6 10.6) scale(0.7)">${figure}</g>`;
  }
  return `<svg class="bb-face" width="${size}" height="${size}" viewBox="0 0 64 64" aria-hidden="true"><defs>${grad}</defs>${inner}</svg>`;
}

/** Kept for the welcome page and templates: a default bot with this face. */
export function faceSvg(face: number, color: string, size: number): string {
  return avatarSvg({ face, color, body: 'bot' }, size);
}
