// Bundled into src-tauri/src/preview_shot.js (npm run build:shot) and injected
// into previewed project pages, so the agent's browser tool can screenshot them.
import { toJpeg } from 'html-to-image';
window.__blazeShot = { toJpeg };
