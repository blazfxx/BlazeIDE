// Settings > Continue on mobile: let a phone continue chats on this computer.
// Dynamic text is always set with textContent (el() writes innerHTML).

import { el } from '../state';
import { settingBlock, settingRow, toggle } from './controls';
import type { MobileStatus } from '../../shared/ipc';

function text(tag: 'div' | 'span' | 'a' | 'code', cls: string, t: string): HTMLElement {
  const e = el(tag, cls);
  e.textContent = t;
  return e;
}

export function buildMobileSection(): HTMLElement[] {
  const rows: HTMLElement[] = [];
  const statusBox = el('div', 'mobile-status');
  const pwRow = el('div', 'mobile-pw');
  const waysBox = el('div', 'mobile-ways');
  const urlsBox = el('div', 'mobile-urls');
  const devBox = el('div', 'mobile-devices');
  let st: MobileStatus | null = null;
  let busy = false;
  let timer = 0;

  // ---- password ----
  const pw = el('input') as HTMLInputElement;
  pw.type = 'password';
  pw.placeholder = 'At least 10 characters';
  pw.autocomplete = 'new-password';
  const pw2 = el('input') as HTMLInputElement;
  pw2.type = 'password';
  pw2.placeholder = 'Type it again';
  pw2.autocomplete = 'new-password';
  const pwBtn = el('button', 'primary', 'Save password') as HTMLButtonElement;
  const pwMsg = el('div', 'set-inline-note mobile-msg');
  pwRow.append(pw, pw2, pwBtn, pwMsg);
  pwBtn.addEventListener('click', () => {
    pwMsg.textContent = '';
    if (pw.value.length < 10) { pwMsg.textContent = 'Use at least 10 characters.'; return; }
    if (pw.value !== pw2.value) { pwMsg.textContent = 'The two passwords are different.'; return; }
    pwBtn.disabled = true;
    void window.void
      .mobileSetPassword(pw.value)
      .then((s) => { pw.value = ''; pw2.value = ''; pwMsg.textContent = 'Saved. Every phone was signed out.'; render(s); })
      .catch((e: unknown) => { pwMsg.textContent = String(e instanceof Error ? e.message : e); })
      .finally(() => { pwBtn.disabled = false; });
  });

  // ---- ways in ----
  const enabled = { v: false };
  const lan = el('input') as HTMLInputElement; lan.type = 'checkbox';
  const quick = el('input') as HTMLInputElement; quick.type = 'checkbox';
  const relay = el('input') as HTMLInputElement; relay.type = 'checkbox';
  const useDomain = el('input') as HTMLInputElement; useDomain.type = 'checkbox';
  const domain = el('input') as HTMLInputElement; domain.placeholder = 'https://ide.example.com';
  const viaTunnel = el('input') as HTMLInputElement; viaTunnel.type = 'checkbox';
  const token = el('input') as HTMLInputElement; token.type = 'password'; token.placeholder = 'Cloudflare tunnel token (kept in your keychain)';
  const port = el('input') as HTMLInputElement; port.type = 'number'; port.min = '1024'; port.max = '65535';
  const applyBtn = el('button', 'primary', 'Apply') as HTMLButtonElement;
  const applyMsg = el('div', 'set-inline-note mobile-msg');

  const way = (box: HTMLInputElement, title: string, desc: string, extra?: HTMLElement): HTMLElement => {
    const row = el('label', 'mobile-way');
    const body = el('div', 'mobile-way-text');
    body.append(text('div', 'mobile-way-title', title), text('div', 'set-row-desc', desc));
    if (extra) body.appendChild(extra);
    row.append(box, body);
    return row;
  };
  const domainExtra = el('div', 'mobile-domain');
  const tunnelLine = el('label', 'mobile-inline');
  tunnelLine.append(viaTunnel, text('span', '', 'It reaches this computer through my Cloudflare tunnel (paste its token):'));
  domainExtra.append(domain, tunnelLine, token);
  const portLine = el('div', 'mobile-inline');
  portLine.append(text('span', 'set-row-desc', 'Port on this computer'), port);
  waysBox.append(
    way(relay, 'BlazeAPI relay (works anywhere)', 'Your phone opens a fixed blazeapi.org address from any network, with no setup. This computer connects out to BlazeAPI; BlazeAPI can technically see this traffic (like Cloudflare can for a tunnel).'),
    way(quick, 'Free quick tunnel', 'A random https://…trycloudflare.com address from Cloudflare, no account needed. The address changes every time Blaze IDE starts. Downloads cloudflared (checked against its published checksum) the first time.'),
    way(useDomain, 'Your own domain', 'Point a domain you own at http://localhost:<port> on this computer, for example with your own Cloudflare tunnel or reverse proxy. Blaze checks that the domain really reaches this computer.', domainExtra),
    way(lan, 'Same Wi-Fi (IP address)', 'Your phone opens http://<this computer\'s IP>:<port> on the same network. Plain http: use it only on a network you trust, or forward the port on your router at your own risk.'),
    portLine,
  );
  const applyRow = el('div', 'mobile-inline');
  applyRow.append(applyBtn, applyMsg);
  waysBox.appendChild(applyRow);

  const save = (on: boolean): void => {
    applyMsg.textContent = on ? 'Starting…' : 'Stopping…';
    applyBtn.disabled = true;
    busy = true;
    const p = Number(port.value) || undefined;
    const go = async (): Promise<MobileStatus> => {
      if (useDomain.checked && viaTunnel.checked && token.value.trim()) {
        await window.void.mobileSetTunnelToken(token.value.trim());
        token.value = '';
      }
      return window.void.mobileConfigure({
        enabled: on, lan: lan.checked, quickTunnel: quick.checked, relay: relay.checked,
        domain: useDomain.checked ? domain.value.trim() : '', domainTunnel: useDomain.checked && viaTunnel.checked, port: p,
      });
    };
    void go()
      .then((s) => { applyMsg.textContent = s.error ? '' : on ? 'Running.' : 'Off.'; render(s); })
      .catch((e: unknown) => { applyMsg.textContent = String(e instanceof Error ? e.message : e); })
      .finally(() => { applyBtn.disabled = false; busy = false; });
  };
  applyBtn.addEventListener('click', () => save(true));
  const onToggle = toggle(false, (v) => { enabled.v = v; save(v); });

  // ---- render ----
  function render(s: MobileStatus): void {
    st = s;
    enabled.v = s.enabled;
    onToggle.classList.toggle('on', s.enabled);
    statusBox.textContent = '';
    const line = !s.hasPassword ? 'Choose a password below to start.'
      : !s.enabled ? 'Off. Phones cannot reach this computer.'
      : s.running ? `On. ${s.devices.length} phone(s) signed in.${s.activeRuns ? ` ${s.activeRuns} chat(s) working from a phone.` : ''}`
      : 'Not running.';
    statusBox.appendChild(text('div', 'set-row-desc', line));
    if (s.error) statusBox.appendChild(text('div', 'setting-error mobile-left', s.error));
    if (s.relay && s.relayState && s.relayState !== 'online') statusBox.appendChild(text('div', 'set-row-desc', `Relay: ${s.relayState}`));
    pwBtn.textContent = s.hasPassword ? 'Change password' : 'Save password';
    if (!busy && document.activeElement?.closest('.mobile-ways') == null) {
      lan.checked = s.lan; quick.checked = s.quickTunnel; relay.checked = s.relay;
      useDomain.checked = !!s.domain; domain.value = s.domain ?? ''; viaTunnel.checked = s.domainTunnel;
      port.value = String(s.port);
      token.placeholder = s.hasTunnelToken ? 'Token saved (paste a new one to replace it)' : 'Cloudflare tunnel token (kept in your keychain)';
    }
    urlsBox.textContent = '';
    if (!s.urls.length) urlsBox.appendChild(text('div', 'set-row-desc', s.enabled ? 'No address yet.' : 'Turn it on to get an address.'));
    for (const u of s.urls) {
      const row = el('div', 'mobile-url');
      const a = text('code', 'mobile-url-text', u.url);
      const label = text('div', 'set-row-desc', u.label);
      const qrBtn = el('button', '', 'Show QR code') as HTMLButtonElement;
      const copy = el('button', '', 'Copy') as HTMLButtonElement;
      const qrBox = el('div', 'mobile-qr');
      qrBtn.addEventListener('click', () => {
        void window.void.mobilePairQr(u.url).then((q) => {
          qrBox.textContent = '';
          const img = el('img') as HTMLImageElement;
          img.alt = 'QR code to sign your phone in';
          img.src = URL.createObjectURL(new Blob([q.svg], { type: 'image/svg+xml' }));
          qrBox.append(img, text('div', 'set-row-desc', 'Scan with your phone\'s camera within 5 minutes: it signs in without the password (works once).'));
        }).catch((e: unknown) => { qrBox.textContent = String(e instanceof Error ? e.message : e); });
      });
      copy.addEventListener('click', () => { void navigator.clipboard.writeText(u.url); copy.textContent = 'Copied'; setTimeout(() => { copy.textContent = 'Copy'; }, 1200); });
      const btns = el('div', 'mobile-inline');
      btns.append(qrBtn, copy);
      row.append(label, a, btns, qrBox);
      urlsBox.appendChild(row);
    }
    devBox.textContent = '';
    if (!s.devices.length) devBox.appendChild(text('div', 'set-row-desc', 'No phones signed in.'));
    for (const d of s.devices) {
      const row = el('div', 'mobile-inline mobile-device');
      const mins = Math.max(0, Math.round((Date.now() / 1000 - d.lastSeen) / 60));
      const out = el('button', '', 'Sign out') as HTMLButtonElement;
      out.addEventListener('click', () => { void window.void.mobileRevoke(d.id).then(render); });
      row.append(text('span', '', d.name), text('span', 'set-row-desc', mins < 2 ? 'active now' : `last seen ${mins} min ago`), out);
      devBox.appendChild(row);
    }
  }

  const refresh = (): void => {
    if (!statusBox.isConnected && st) { window.clearInterval(timer); return; }
    void window.void.mobileStatus().then(render).catch(() => undefined);
  };
  refresh();
  timer = window.setInterval(refresh, 4000);

  rows.push(settingRow('Continue on mobile', 'Open your Blaze chats on your phone, send messages and approve the agent\'s actions while away from this computer. Turns run here, on your BlazeAPI account. Phones can only chat and answer approvals in projects you opened here: no files, terminal or settings.', onToggle));
  rows.push(settingBlock('Status', '', statusBox));
  rows.push(settingBlock('Phone password', 'Phones sign in with this password (or a one-time QR code). Only a hash is stored. Changing it signs every phone out.', pwRow));
  rows.push(settingBlock('How your phone reaches this computer', 'Pick one or more, then Apply.', waysBox));
  rows.push(settingBlock('Addresses', 'Open one on your phone, or show its QR code.', urlsBox));
  rows.push(settingBlock('Signed-in phones', '', devBox));
  return rows;
}
