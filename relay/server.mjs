// BlazeAPI relay for Blaze IDE "Continue on mobile" (m.blazeapi.org).
//
// A desktop (Blaze IDE or `blaze serve --relay`) long-polls POST /connect/poll
// with its BlazeAPI key and a slot id; a phone opens https://<host>/r/<slot>/.
// The relay serves the phone app itself (static files shipped with the relay)
// and forwards ONLY /r/<slot>/api/* and /r/<slot>/healthz to the desktop that
// owns the slot. Desktops never get to serve HTML or script on this shared
// origin, and their cookies are pinned to their own /r/<slot>/ path.
// Nothing about message contents is logged.
import http from 'node:http';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const PORT = Number(process.env.PORT || 4891);
const BACKEND = process.env.BLAZE_BACKEND || 'http://127.0.0.1:7173';
const BACKEND_HOST = process.env.BLAZE_BACKEND_HOST || 'api.blazeapi.org';
const DATA = process.env.STATE_DIRECTORY || process.env.DATA_DIR || './data';
const PUBLIC = process.env.PUBLIC_DIR || new URL('./public/', import.meta.url).pathname;
const MAX_BODY = 2 * 1024 * 1024;
const POLL_HOLD = 25_000;
const REPLY_WAIT = 45_000;
const OFFLINE_AFTER = 70_000;
const MAX_SLOTS_PER_USER = 3;
const MAX_INFLIGHT = 32;
const SLOT_RE = /^[a-z0-9]{16}$/;

fs.mkdirSync(DATA, { recursive: true });
const REG_FILE = path.join(DATA, 'slots.json');
let registry = {};
try { registry = JSON.parse(fs.readFileSync(REG_FILE, 'utf8')); } catch { registry = {}; }
function saveRegistry() {
  const tmp = REG_FILE + '.tmp';
  fs.writeFileSync(tmp, JSON.stringify(registry), { mode: 0o600 });
  fs.renameSync(tmp, REG_FILE);
}

const STATIC = {
  '': ['index.html', 'text/html; charset=utf-8'], 'index.html': ['index.html', 'text/html; charset=utf-8'],
  'app.js': ['app.js', 'text/javascript; charset=utf-8'], 'app.css': ['app.css', 'text/css; charset=utf-8'],
  'manifest.json': ['manifest.json', 'application/manifest+json'], 'icon.svg': ['icon.svg', 'image/svg+xml'],
};
const files = {};
for (const [k, [f]] of Object.entries(STATIC)) files[k] = fs.readFileSync(path.join(PUBLIC, f));

const APP_CSP = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; manifest-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";
function secure(res, csp) {
  res.setHeader('content-security-policy', csp);
  res.setHeader('x-content-type-options', 'nosniff');
  res.setHeader('referrer-policy', 'no-referrer');
  res.setHeader('x-frame-options', 'DENY');
  res.setHeader('strict-transport-security', 'max-age=31536000');
}
function json(res, status, obj) {
  secure(res, "sandbox; default-src 'none'");
  res.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' });
  res.end(JSON.stringify(obj));
}
function readBody(req, limit = MAX_BODY) {
  return new Promise((resolve, reject) => {
    const chunks = []; let n = 0;
    req.on('data', (c) => { n += c.length; if (n > limit) { reject(new Error('too large')); req.destroy(); } else chunks.push(c); });
    req.on('end', () => resolve(Buffer.concat(chunks)));
    req.on('error', reject);
  });
}

// --- desktop auth: BlazeAPI key -> username (cached 5 min) ---
const authCache = new Map();
async function userFor(req) {
  const m = /^Bearer\s+(\S{10,200})$/.exec(req.headers.authorization || '');
  if (!m) return null;
  const h = crypto.createHash('sha256').update(m[1]).digest('hex');
  const c = authCache.get(h);
  if (c && c.exp > Date.now()) return c.user;
  try {
    const r = await fetch(BACKEND + '/paid/v1/usage', { headers: { authorization: 'Bearer ' + m[1], host: BACKEND_HOST }, signal: AbortSignal.timeout(8000) });
    if (!r.ok) { authCache.set(h, { user: null, exp: Date.now() + 60_000 }); return null; }
    const d = await r.json();
    const user = typeof d.username === 'string' && d.username ? d.username : null;
    authCache.set(h, { user, exp: Date.now() + 300_000 });
    if (authCache.size > 5000) authCache.delete(authCache.keys().next().value);
    return user;
  } catch { return null; }
}

// --- live slots ---
const slots = new Map(); // slot -> { queue, waiters, inflight: Map(id -> res), lastPoll }
function live(slot) {
  let s = slots.get(slot);
  if (!s) { s = { queue: [], waiters: [], inflight: new Map(), lastPoll: 0 }; slots.set(slot, s); }
  return s;
}
function dispatch(s) {
  while (s.queue.length && s.waiters.length) {
    const w = s.waiters.shift();
    clearTimeout(w.timer);
    const items = s.queue.splice(0, 8);
    json(w.res, 200, items);
  }
}

async function claimSlot(req, res) {
  const user = await userFor(req);
  if (!user) { json(res, 401, { error: 'Sign in to BlazeAPI again (the key was not accepted).' }); return null; }
  const slot = String(req.headers['x-blaze-slot'] || '');
  if (!SLOT_RE.test(slot)) { json(res, 400, { error: 'Bad slot id' }); return null; }
  const reg = registry[slot];
  if (reg && reg.user !== user) { json(res, 403, { error: 'This relay address belongs to another account.' }); return null; }
  if (!reg) {
    const mine = Object.values(registry).filter((r) => r.user === user).length;
    if (mine >= MAX_SLOTS_PER_USER) { json(res, 403, { error: `An account can use the relay from ${MAX_SLOTS_PER_USER} computers. Turn it off on one first.` }); return null; }
    registry[slot] = { user, created: Date.now() };
    saveRegistry();
  }
  const name = String(req.headers['x-blaze-name'] || '').slice(0, 80);
  if (name && registry[slot].name !== name) { registry[slot].name = name; saveRegistry(); }
  return slot;
}

async function onPoll(req, res) {
  const slot = await claimSlot(req, res);
  if (!slot) return;
  const s = live(slot);
  s.lastPoll = Date.now();
  if (s.waiters.length >= 4) { const old = s.waiters.shift(); clearTimeout(old.timer); json(old.res, 200, []); }
  const w = { res, timer: setTimeout(() => { s.waiters = s.waiters.filter((x) => x !== w); json(res, 200, []); }, POLL_HOLD) };
  req.on('close', () => { if (!res.writableEnded) { clearTimeout(w.timer); s.waiters = s.waiters.filter((x) => x !== w); } });
  s.waiters.push(w);
  dispatch(s);
}

async function onReply(req, res) {
  const slot = await claimSlot(req, res);
  if (!slot) return;
  let r;
  try { r = JSON.parse((await readBody(req, 6 * 1024 * 1024)).toString('utf8')); } catch { return json(res, 400, { error: 'Bad reply' }); }
  const s = live(slot);
  s.lastPoll = Date.now();
  const phone = s.inflight.get(String(r.id || ''));
  if (!phone) return json(res, 200, { ok: false });
  s.inflight.delete(r.id);
  clearTimeout(phone.timer);
  const out = phone.res;
  secure(out, "sandbox; default-src 'none'");
  const headers = { 'cache-control': 'no-store', 'content-type': 'application/json' };
  for (const [k, v] of Array.isArray(r.headers) ? r.headers : []) {
    const key = String(k).toLowerCase();
    if (key === 'set-cookie') {
      // Pin every cookie to this slot's path, whatever the desktop said.
      const pinned = String(v).replace(/;\s*Path=[^;]*/i, '') + `; Path=/r/${slot}/`;
      headers['set-cookie'] = (headers['set-cookie'] || []).concat(pinned.includes('Secure') ? pinned : pinned + '; Secure');
    }
  }
  const status = Number(r.status) >= 200 && Number(r.status) < 600 ? Number(r.status) : 502;
  out.writeHead(status, headers);
  out.end(Buffer.from(String(r.body || ''), 'base64'));
  json(res, 200, { ok: true });
}

const ipHits = new Map();
function rateOk(ip) {
  const now = Date.now();
  const e = ipHits.get(ip) || { n: 0, t: now };
  if (now - e.t > 60_000) { e.n = 0; e.t = now; }
  e.n++;
  ipHits.set(ip, e);
  if (ipHits.size > 20000) ipHits.clear();
  return e.n <= 600;
}

async function onPhoneApi(req, res, slot, rest) {
  const ip = String(req.headers['cf-connecting-ip'] || req.socket.remoteAddress || '');
  if (!rateOk(ip)) return json(res, 429, { error: 'Too many requests' });
  const s = slots.get(slot);
  if (!registry[slot] || !s || Date.now() - s.lastPoll > OFFLINE_AFTER) {
    return json(res, 503, { error: 'Your computer is offline or Continue on mobile is off. Open Blaze IDE (or run blaze serve --relay) and try again.' });
  }
  if (s.inflight.size >= MAX_INFLIGHT) return json(res, 429, { error: 'Too many requests at once' });
  let body;
  try { body = await readBody(req); } catch { return json(res, 413, { error: 'Too large' }); }
  const id = crypto.randomBytes(9).toString('hex');
  const keep = ['cookie', 'content-type', 'x-blaze', 'origin', 'user-agent'];
  const headers = Object.entries(req.headers).filter(([k]) => keep.includes(k)).map(([k, v]) => [k, Array.isArray(v) ? v.join('; ') : String(v)]);
  headers.push(['host', String(req.headers.host || '')]);
  const timer = setTimeout(() => { if (s.inflight.delete(id)) json(res, 504, { error: 'Your computer did not answer in time.' }); }, REPLY_WAIT);
  s.inflight.set(id, { res, timer });
  req.on('close', () => { if (!res.writableEnded && s.inflight.delete(id)) clearTimeout(timer); });
  s.queue.push({ id, method: req.method, path: rest, headers, body: body.toString('base64'), ip });
  dispatch(s);
}

const server = http.createServer(async (req, res) => {
  const t0 = Date.now();
  res.on('finish', () => {
    if (process.env.RELAY_LOG) console.log(`${req.method} ${(req.url || '').startsWith('/connect') ? req.url.split('?')[0] : '/r/…'} ${res.statusCode} ${Date.now() - t0}ms`);
  });
  try {
    const u = new URL(req.url || '/', 'http://x');
    if (u.pathname === '/healthz') return json(res, 200, { ok: true, slots: slots.size });
    if (req.method === 'POST' && u.pathname === '/connect/poll') return await onPoll(req, res);
    if (req.method === 'POST' && u.pathname === '/connect/reply') return await onReply(req, res);
    const m = /^\/r\/([a-z0-9]{16})(\/.*)?$/.exec(u.pathname);
    if (!m) {
      if (u.pathname === '/') { secure(res, APP_CSP); res.writeHead(200, { 'content-type': 'text/plain; charset=utf-8' }); return res.end('Blaze relay. Open the address shown in Blaze IDE > Settings > Continue on mobile.'); }
      return json(res, 404, { error: 'Not found' });
    }
    const slot = m[1];
    const rest = m[2] || '';
    if (rest === '') { res.writeHead(301, { location: `/r/${slot}/` }); return res.end(); }
    if (rest.startsWith('/api/') || rest === '/healthz') return await onPhoneApi(req, res, slot, rest + u.search);
    const f = STATIC[rest.slice(1)];
    if (f && (req.method === 'GET' || req.method === 'HEAD')) {
      secure(res, APP_CSP);
      res.writeHead(200, { 'content-type': f[1], 'cache-control': 'no-cache' });
      return res.end(req.method === 'HEAD' ? undefined : files[rest.slice(1)]);
    }
    return json(res, 404, { error: 'Not found' });
  } catch (e) {
    if (!res.headersSent) json(res, 500, { error: 'Relay error' });
  }
});
server.requestTimeout = 60_000;
server.headersTimeout = 20_000;
server.keepAliveTimeout = 30_000;
server.listen(PORT, '127.0.0.1', () => console.log(`blaze relay on 127.0.0.1:${PORT}`));
