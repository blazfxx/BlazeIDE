// Injected at the top of HTML pages served to the live preview (before the
// page's own scripts) so the AI agent's browser tool can see and use the page:
// console capture, screenshots (via the bundled html-to-image, __blazeShot),
// click and scroll. Talks to the IDE via postMessage; the IDE checks the
// message source is this preview frame.
(() => {
  if (window.__voidAgent) return; window.__voidAgent = true;
  var logs = [];
  function text(a) {
    if (a instanceof Error) return a.message;
    if (a && typeof a === 'object') { try { return JSON.stringify(a).slice(0, 400); } catch (e) { return String(a); } }
    return String(a);
  }
  function push(level, args) {
    try {
      logs.push('[' + level + '] ' + Array.prototype.map.call(args, text).join(' ').slice(0, 800));
      if (logs.length > 120) logs.shift();
    } catch (e) { /* never break the page */ }
  }
  ['log', 'info', 'warn', 'error'].forEach(function (l) {
    var orig = console[l];
    console[l] = function () { push(l, arguments); return orig && orig.apply(console, arguments); };
  });
  window.addEventListener('error', function (e) {
    var t = e.target;
    if (t && t !== window && (t.src || t.href)) push('error', ['Failed to load ' + (t.src || t.href)]);
    else push('error', [(e.message || 'Script error') + (e.filename ? ' (' + String(e.filename).split('/').pop() + ':' + e.lineno + ')' : '')]);
  }, true);
  window.addEventListener('unhandledrejection', function (e) {
    push('error', ['Unhandled promise rejection: ' + ((e.reason && e.reason.message) || e.reason)]);
  });

  function reply(id, ok, data) { try { parent.postMessage({ type: 'void:agent-result', id: id, ok: ok, data: data }, '*'); } catch (e) { /* ignore */ } }
  function bg() {
    var c = getComputedStyle(document.body || document.documentElement).backgroundColor;
    if (!c || c === 'rgba(0, 0, 0, 0)' || c === 'transparent') c = getComputedStyle(document.documentElement).backgroundColor;
    return (!c || c === 'rgba(0, 0, 0, 0)' || c === 'transparent') ? '#ffffff' : c;
  }

  window.addEventListener('message', function (ev) {
    var d = ev.data;
    if (!d || d.type !== 'void:agent' || ev.source !== parent) return;
    var id = d.id;
    try {
      if (d.op === 'console') {
        return reply(id, true, { logs: logs.slice(-60), title: document.title, url: location.href, errors: logs.filter(function (l) { return l.indexOf('[error]') === 0; }).length });
      }
      if (d.op === 'click') {
        var el = document.querySelector(d.selector);
        if (!el) return reply(id, false, 'No element matches ' + d.selector);
        el.scrollIntoView({ block: 'center' });
        el.click();
        return setTimeout(function () { reply(id, true, 'Clicked ' + d.selector + ' (' + el.tagName.toLowerCase() + ')'); }, 500);
      }
      if (d.op === 'scroll') {
        window.scrollTo(0, Number(d.y) || 0);
        return setTimeout(function () { reply(id, true, 'Scrolled to y=' + Math.round(window.scrollY) + ' of a ' + document.documentElement.scrollHeight + ' px page'); }, 350);
      }
      if (d.op === 'screenshot') {
        if (!window.__blazeShot) return reply(id, false, 'the screenshot helper is not loaded in this page');
        var root = document.documentElement;
        var full = !!d.full;
        var w = root.clientWidth;
        var pageH = Math.max(root.scrollHeight, document.body ? document.body.scrollHeight : 0);
        var h = full ? Math.min(pageH, 4000) : window.innerHeight;
        var sy = full ? 0 : window.scrollY;
        var scale = w > 1280 ? 1280 / w : 1;
        window.__blazeShot.toJpeg(root, {
          width: w, height: h, quality: 0.78, pixelRatio: scale, backgroundColor: bg(),
          // A broken image must not fail the whole shot (it is exactly what the agent should see).
          imagePlaceholder: 'data:image/gif;base64,R0lGODlhAQABAIAAAP///wAAACH5BAEAAAAALAAAAAABAAEAAAICRAEAOw==',
          style: { transform: 'translateY(' + (-sy) + 'px)', transformOrigin: 'top left', margin: '0' },
          filter: function (n) { return !(n && n.getAttribute && n.getAttribute('data-blaze-ui')); },
        }).then(function (url) {
          reply(id, true, { url: url, width: w, height: h, scrollY: Math.round(sy), pageHeight: pageH, errors: logs.filter(function (l) { return l.indexOf('[error]') === 0; }).length });
        }, function (e) { reply(id, false, 'screenshot failed: ' + ((e && e.message) || e)); });
        return;
      }
      reply(id, false, 'unknown action ' + d.op);
    } catch (e) { reply(id, false, String((e && e.message) || e)); }
  });
})();
