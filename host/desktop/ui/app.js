// Helm desktop window. Plain JS: talks to the Rust side through Tauri commands and events.
'use strict';

(() => {
  const tauri = window.__TAURI__;
  const invoke = tauri ? tauri.core.invoke : previewInvoke;
  const listen = tauri ? tauri.event.listen : async () => () => {};

  const PAGES = ['home', 'phones', 'pair', 'activity', 'settings'];
  const SHEETS = {
    home: 'Sheet 01 · This PC',
    phones: 'Sheet 02 · Phones',
    pair: 'Sheet 03 · Pairing',
    activity: 'Sheet 04 · Activity',
    settings: 'Sheet 05 · Settings',
  };
  const FAIL_EVENTS = new Set(['pair_fail', 'auth_fail', 'handshake_timeout', 'relay_down']);

  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

  // Build elements without innerHTML (all text from the agent goes through textContent).
  function el(tag, attrs, ...children) {
    const node = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v === undefined || v === null || v === false) continue;
      if (k === 'class') node.className = v;
      else if (k === 'text') node.textContent = v;
      else if (k.startsWith('on')) node.addEventListener(k.slice(2), v);
      else node.setAttribute(k, v === true ? '' : String(v));
    }
    for (const c of children.flat()) {
      if (c === null || c === undefined || c === false) continue;
      node.append(c instanceof Node ? c : document.createTextNode(String(c)));
    }
    return node;
  }

  const state = { page: 'home', status: null, phones: [], pairing: null, pairingAt: 0, activity: [], settings: null, code: null, codeAt: 0, approvals: [] };

  // ------------------------------------------------------------------ helpers
  function toast(text, tone) {
    const t = $('#toast');
    t.textContent = text;
    t.dataset.tone = tone || '';
    t.hidden = false;
    clearTimeout(toast.timer);
    toast.timer = setTimeout(() => { t.hidden = true; }, 3200);
  }

  async function call(cmd, args) {
    try {
      return await invoke(cmd, args || {});
    } catch (e) {
      toast(String(e), 'error');
      throw e;
    }
  }

  function fmtUptime(s) {
    if (s === null || s === undefined) return '—';
    const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
    if (d > 0) return `${d}d ${h}h`;
    if (h > 0) return `${h}h ${m}m`;
    return m > 0 ? `${m} min` : `${s} s`;
  }

  function fmtClock(iso) {
    const d = new Date(iso);
    if (isNaN(d)) return iso;
    return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false });
  }

  function fmtDateTime(iso) {
    const d = new Date(iso);
    if (isNaN(d)) return iso;
    return d.toLocaleString([], { month: 'short', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
  }

  function fmtAgo(ms) {
    if (!ms) return 'never';
    const diff = Math.max(0, Date.now() - ms) / 1000;
    if (diff < 90) return 'just now';
    if (diff < 3600) return `${Math.round(diff / 60)} min ago`;
    if (diff < 86400) return `${Math.round(diff / 3600)} h ago`;
    const days = Math.round(diff / 86400);
    return days === 1 ? 'yesterday' : `${days} days ago`;
  }

  function phoneStatus(p) {
    if (p.online) return `Connected · ${p.via === 'relay' ? 'relay' : 'Wi-Fi'}`;
    return `Last seen ${fmtAgo(p.last_seen_ms)}`;
  }

  // ------------------------------------------------------------------ navigation
  function go(page) {
    if (!PAGES.includes(page)) page = 'home';
    state.page = page;
    for (const p of PAGES) $(`#page-${p}`).hidden = p !== page;
    for (const a of $$('.nav a')) {
      if (a.dataset.page === page) a.setAttribute('aria-current', 'page');
      else a.removeAttribute('aria-current');
    }
    $('#sheet').textContent = SHEETS[page];
    if (location.hash !== `#${page}`) history.replaceState(null, '', `#${page}`);
    if (page === 'phones') loadPhones();
    if (page === 'activity') loadActivity();
    if (page === 'settings') loadSettings();
    if (page === 'pair' || page === 'home') loadPairing();
    if (page === 'pair') loadCode();
  }

  document.addEventListener('click', (e) => {
    const link = e.target.closest('[data-page]');
    if (link) {
      e.preventDefault();
      go(link.dataset.page);
      return;
    }
    const btn = e.target.closest('[data-action]');
    if (btn && ACTIONS[btn.dataset.action]) {
      e.preventDefault();
      ACTIONS[btn.dataset.action](btn);
    }
  });

  // ------------------------------------------------------------------ status
  async function loadStatus() {
    const s = await invoke('get_status').catch(() => null);
    if (!s) return;
    state.status = s;
    $('#rev').textContent = s.version;
    const pill = $('#pill');
    pill.dataset.tone = s.headline.tone;
    $('#pill-text').textContent = s.headline.label;
    $('#home-badge').textContent = s.headline.badge;
    $('#host-first').textContent = s.host_first;
    $('#host-rest').textContent = s.host_rest;
    const bubble = $('#home-message');
    bubble.textContent = s.headline.message;
    bubble.dataset.tone = s.headline.tone;
    $('#home-actions').hidden = s.run === 'running' || s.run === 'starting' || s.run === 'stopping';
    $('#d-address').textContent = s.addresses.length ? s.addresses.join(', ') : '—';
    $('#d-port').textContent = s.port ?? (s.run === 'running' ? '—' : `${s.configured_port} (off)`);
    $('#d-uptime').textContent = fmtUptime(s.uptime_s);
    $('#d-phones').textContent = s.phones_summary;
    $('#d-relay').textContent = s.relay.line.label;
    $('#d-relay').title = s.relay.line.text;
    renderRelay(s.relay);
    $('#s-run').textContent = s.headline.label;
    $('#s-pause').textContent = s.run === 'running' || s.run === 'starting' ? 'Pause connections' : 'Resume connections';
    $('#a-version').textContent = s.version;
    $('#a-host').textContent = s.host_name;
    $('#a-hostid').textContent = s.host_id || '—';
    $('#a-fp').textContent = s.fingerprint || '—';
    $('#a-data').textContent = s.data_dir;
    document.title = `Helm · ${s.headline.label}`;
  }

  // "Connect from anywhere": the same switch on Home and Settings.
  function renderRelay(relay) {
    document.querySelectorAll('[data-relay-on]').forEach((box) => { box.checked = relay.enabled; });
    document.querySelectorAll('[data-relay-label]').forEach((n) => {
      n.textContent = relay.line.label;
      n.dataset.state = relay.line.state;
    });
    document.querySelectorAll('[data-relay-text]').forEach((n) => { n.textContent = relay.line.text; });
  }

  document.querySelectorAll('[data-relay-on]').forEach((box) => {
    box.addEventListener('change', async () => {
      const want = box.checked;
      const url = $('#s-relay-url').value.trim() || (state.status ? state.status.relay.url : '');
      try {
        await call('set_relay', { enabled: want, url });
        toast(want ? 'Connecting to the relay…' : 'This PC is on the local network only now.');
      } catch {
        box.checked = !want;
      }
      loadStatus();
      loadSettings();
    });
  });

  // ------------------------------------------------------------------ phones
  function tierSelect(p) {
    const sel = el('select', { 'aria-label': `Access for ${p.name}` },
      ['admin', 'remote', 'viewer'].map((t) => el('option', { value: t, selected: p.tier === t, text: t[0].toUpperCase() + t.slice(1) })));
    sel.addEventListener('change', async () => {
      try {
        await call('set_tier', { deviceId: p.device_id, tier: sel.value });
        toast(`${p.name} is now ${sel.value}.`);
      } catch {
        sel.value = p.tier;
      }
    });
    return sel;
  }

  function revokeButton(p) {
    const b = el('button', { type: 'button', class: 'btn btn-ghost btn-sm btn-danger', text: 'Revoke' });
    b.addEventListener('click', async () => {
      if (b.dataset.armed !== 'true') {
        b.dataset.armed = 'true';
        b.textContent = 'Confirm revoke';
        setTimeout(() => { b.dataset.armed = 'false'; b.textContent = 'Revoke'; }, 4000);
        return;
      }
      b.disabled = true;
      try {
        await call('revoke_phone', { deviceId: p.device_id });
        toast(`${p.name} was removed. It must pair again.`);
      } finally {
        b.disabled = false;
      }
    });
    return b;
  }

  function ringButton(p) {
    const b = el('button', { type: 'button', class: 'btn btn-ghost btn-sm', text: 'Ring this phone', title: 'Plays a loud sound on the phone so you can find it' });
    b.addEventListener('click', async () => {
      b.disabled = true;
      try {
        await call('ring_phone', { deviceId: p.device_id });
        toast(`${p.name} is ringing. Open Helm on it to stop.`);
      } catch {
        /* call() already showed the reason */
      } finally {
        b.disabled = false;
      }
    });
    return b;
  }

  function renderPhones() {
    const list = state.phones;
    const home = $('#home-phones');
    home.replaceChildren();
    if (!list.length) {
      home.append(el('div', { class: 'empty', text: 'No phones paired yet. Scan the code on the right.' }));
    }
    for (const p of list.slice(0, 4)) {
      home.append(el('div', { class: 'phone-row' },
        el('span', { class: 'st', 'data-on': String(p.online) }),
        el('span', { class: 'phone-name', text: p.name }),
        el('span', { class: 'phone-meta', text: phoneStatus(p) }),
        el('button', { type: 'button', class: 'btn btn-ghost btn-sm', 'data-page': 'phones', text: 'Manage' })));
    }
    const full = $('#phones-list');
    full.replaceChildren();
    $('#phones-count').textContent = list.length ? `${list.filter((p) => p.online).length} of ${list.length} online` : '';
    if (!list.length) {
      full.append(el('div', { class: 'empty', text: 'No phones paired yet.' }));
    }
    for (const p of list) {
      full.append(el('div', { class: 'phone-row' },
        el('span', { class: 'st', 'data-on': String(p.online) }),
        el('span', { class: 'phone-name' }, p.name,
          el('span', { class: 'muted', text: `  ·  paired ${new Date(p.paired_at_ms).toLocaleDateString()}` })),
        el('span', { class: 'phone-meta', text: phoneStatus(p), title: p.peer || '' }),
        p.online ? ringButton(p) : null,
        tierSelect(p),
        revokeButton(p)));
    }
  }

  async function loadPhones() {
    const list = await invoke('get_phones').catch(() => null);
    if (!list) return;
    state.phones = list;
    renderPhones();
  }

  // ------------------------------------------------------------------ pairing
  function renderPairing() {
    const p = state.pairing;
    const remaining = p ? Math.max(0, p.expires_in_s - Math.floor((Date.now() - state.pairingAt) / 1000)) : 0;
    const expired = !p || remaining <= 0;
    const running = state.status && state.status.run === 'running';
    for (const box of $$('[data-qr]')) {
      if (!p) {
        box.replaceChildren(el('span', { text: running ? 'Press New code to show a pairing QR.' : 'Helm is not running.' }));
        box.dataset.state = 'none';
      } else if (box.dataset.uri !== p.uri) {
        // The SVG comes from our own QR encoder (helm_agent::qr), never from a phone.
        box.innerHTML = p.svg;
        box.dataset.uri = p.uri;
      }
      if (p) box.dataset.state = expired ? 'expired' : 'live';
    }
    const label = !p ? '—' : expired ? 'Expired' : `Expires ${Math.floor(remaining / 60)}:${String(remaining % 60).padStart(2, '0')}`;
    for (const e of $$('[data-expires]')) {
      e.textContent = label;
      e.dataset.state = expired && p ? 'expired' : '';
    }
    $('#pair-uri').textContent = p && !expired ? p.uri : '';
    for (const b of $$('[data-action="copy-link"]')) b.disabled = expired;
    for (const b of $$('[data-action="new-code"]')) b.disabled = !running;
  }

  async function loadPairing() {
    const p = await invoke('get_pairing').catch(() => null);
    setPairing(p);
  }

  function setPairing(p) {
    state.pairing = p || null;
    state.pairingAt = Date.now();
    if (!p) for (const box of $$('[data-qr]')) delete box.dataset.uri;
    renderPairing();
  }

  // ------------------------------------------------------------------ 6-digit code
  function codeRemaining() {
    const c = state.code;
    return c ? Math.max(0, c.expires_in_s - Math.floor((Date.now() - state.codeAt) / 1000)) : 0;
  }

  function renderCode() {
    const c = state.code;
    const remaining = codeRemaining();
    const expired = !c || remaining <= 0;
    const running = state.status && state.status.run === 'running';
    $('#code-panel').hidden = !c;
    $('#code-panel').dataset.state = expired ? 'expired' : 'live';
    if (c) {
      $('#code-digits').textContent = c.code;
      $('#code-host').textContent = c.host_name;
    }
    const label = !c ? '—' : expired ? 'Expired' : `Expires ${Math.floor(remaining / 60)}:${String(remaining % 60).padStart(2, '0')}`;
    for (const e of $$('[data-code-expires]')) {
      e.textContent = label;
      e.dataset.state = expired && c ? 'expired' : '';
    }
    $('#code-btn').textContent = c ? 'New code' : 'Use a 6-digit code instead';
    $('#code-btn').disabled = !running;
  }

  function setCode(c) {
    state.code = c || null;
    state.codeAt = Date.now();
    renderCode();
  }

  async function loadCode() {
    setCode(await invoke('get_code_pairing').catch(() => null));
  }

  // ------------------------------------------------------------------ approve new phones
  function approvalCard(a) {
    const left = () => Math.max(0, Math.round((a.expires_unix_ms - Date.now()) / 1000));
    const timer = el('span', { class: 'expires', text: `${left()} s left` });
    const answer = async (allow, btns) => {
      btns.forEach((b) => { b.disabled = true; });
      try {
        const done = await call('resolve_pairing', { requestId: a.request_id, allow });
        toast(!done ? 'That request already ended.' : allow ? `${a.device_name} was allowed.` : `${a.device_name} was denied.`);
      } catch {
        btns.forEach((b) => { b.disabled = false; });
      }
      loadApprovals();
    };
    const allow = el('button', { type: 'button', class: 'btn btn-primary', text: 'Allow' });
    const deny = el('button', { type: 'button', class: 'btn btn-ghost btn-danger', text: 'Deny' });
    allow.addEventListener('click', () => answer(true, [allow, deny]));
    deny.addEventListener('click', () => answer(false, [allow, deny]));
    timer.dataset.expiresAt = String(a.expires_unix_ms);
    const how = a.method === 'code' ? 'with the 6-digit code' : 'with the QR code';
    return el('div', { class: 'approval-card' },
      el('div', { class: 'card-head' }, el('span', { text: 'A new phone wants to pair' }), timer),
      el('div', { class: 'approval-body' },
        el('div', { class: 'approval-text' },
          el('strong', { text: a.device_name || 'A phone' }),
          el('span', { class: 'muted', text: `Pairing ${how} from ${(a.peer || '').startsWith('via=relay') ? 'outside this network, through the relay' : (a.peer || '').replace(/:\d+$/, '') || 'the network'}. Only allow a phone you hold.` })),
        el('div', { class: 'actions-row' }, allow, deny)));
  }

  function renderApprovals() {
    const box = $('#approvals');
    box.replaceChildren(...state.approvals.map(approvalCard));
    box.hidden = state.approvals.length === 0;
  }

  function tickApprovals() {
    for (const t of $$('#approvals [data-expires-at]')) {
      const s = Math.max(0, Math.round((Number(t.dataset.expiresAt) - Date.now()) / 1000));
      t.textContent = `${s} s left`;
    }
  }

  async function loadApprovals() {
    const list = await invoke('get_pending_pairings').catch(() => null);
    if (!list) return;
    state.approvals = list;
    renderApprovals();
  }

  // ------------------------------------------------------------------ activity
  function renderActivity() {
    const rows = state.activity;
    const log = $('#home-log');
    log.replaceChildren();
    if (!rows.length) log.append(el('span', { text: 'Nothing yet.' }));
    for (const r of rows.slice(0, 6)) {
      log.append(el('span', { class: FAIL_EVENTS.has(r.event) ? 'fail' : '' },
        el('span', { class: 't', text: fmtClock(r.time) }), r.text + (r.peer && !r.peer.startsWith('desktop') ? ` · ${r.peer.replace(/:\d+$/, '')}` : '')));
    }
    const body = $('#activity-rows');
    body.replaceChildren();
    if (!rows.length) body.append(el('tr', {}, el('td', { colspan: 5, text: 'The audit log is empty.' })));
    for (const r of rows) {
      body.append(el('tr', { class: FAIL_EVENTS.has(r.event) ? 'fail' : '' },
        el('td', { text: fmtDateTime(r.time) }),
        el('td', { class: 'ev', text: r.text }),
        el('td', { text: r.device_name || (r.device_id ? r.device_id.slice(0, 12) : '—'), title: r.device_id || '' }),
        el('td', { text: r.peer || '—' }),
        el('td', { text: r.detail || '' })));
    }
  }

  async function loadActivity() {
    const rows = await invoke('get_activity', { limit: 200 }).catch(() => null);
    if (!rows) return;
    state.activity = rows;
    renderActivity();
  }

  // ------------------------------------------------------------------ settings
  async function loadSettings() {
    const s = await invoke('get_settings').catch(() => null);
    if (!s) return;
    state.settings = s;
    $('#s-autostart').checked = s.autostart;
    $('#s-autostart').closest('label').title = s.autostart_key;
    if (document.activeElement !== $('#s-port')) $('#s-port').value = s.port;
    if (document.activeElement !== $('#s-relay-url')) $('#s-relay-url').value = s.relay_url;
    $('#s-approval').checked = s.require_approval;
  }

  $('#s-approval').addEventListener('change', async (e) => {
    const want = e.target.checked;
    try {
      const now = await call('set_require_approval', { on: want });
      e.target.checked = now;
      toast(now ? 'Helm will ask before a new phone pairs.' : 'New phones will pair without asking.');
    } catch {
      e.target.checked = !want;
    }
  });

  $('#s-autostart').addEventListener('change', async (e) => {
    const want = e.target.checked;
    try {
      const now = await call('set_autostart', { on: want });
      e.target.checked = now;
      toast(now ? 'Helm will start with Windows.' : 'Helm will not start with Windows.');
    } catch {
      e.target.checked = !want;
    }
  });

  // ------------------------------------------------------------------ actions
  const ACTIONS = {
    'new-code': async () => {
      const p = await call('new_pairing');
      setPairing(p);
    },
    'new-pair-code': async () => {
      setCode(await call('new_code_pairing'));
    },
    'copy-link': async () => {
      const p = state.pairing;
      if (!p) return;
      try {
        await navigator.clipboard.writeText(p.uri);
      } catch {
        const ta = el('textarea', {});
        ta.value = p.uri;
        document.body.append(ta);
        ta.select();
        document.execCommand('copy');
        ta.remove();
      }
      toast('Pairing link copied. It works once.');
    },
    resume: async () => { await call('set_paused', { paused: false }); },
    'toggle-pause': async () => {
      const running = state.status && (state.status.run === 'running' || state.status.run === 'starting');
      await call('set_paused', { paused: running });
    },
    'save-port': async () => {
      const port = Number($('#s-port').value);
      if (!Number.isInteger(port)) return toast('Enter a whole number.', 'error');
      await call('set_port', { port });
      toast('Port saved.');
      loadSettings();
    },
    'save-relay': async () => {
      await call('set_relay', { enabled: $('#s-relay-on').checked, url: $('#s-relay-url').value.trim() });
      toast('Relay address saved.');
      $('#s-relay-url').blur();
      loadStatus();
      loadSettings();
    },
    'check-updates': async (btn) => {
      btn.disabled = true;
      $('#u-summary').textContent = 'Checking…';
      try {
        const u = await call('check_updates');
        $('#u-summary').textContent = u.summary;
        $('#u-download').hidden = u.state !== 'available';
      } catch {
        $('#u-summary').textContent = 'Update check failed.';
      } finally {
        btn.disabled = false;
      }
    },
    'download-update': async (btn) => {
      btn.disabled = true;
      $('#u-summary').textContent = 'Downloading and verifying…';
      try {
        const path = await call('download_update');
        $('#u-summary').textContent = `Verified installer saved to ${path}. Run it to update Helm.`;
      } finally {
        btn.disabled = false;
      }
    },
    'refresh-activity': () => loadActivity(),
    'open-data': () => call('open_data_dir'),
    quit: () => call('quit_app'),
  };

  // ------------------------------------------------------------------ live updates
  listen('helm://changed', (e) => {
    const kind = e.payload && e.payload.kind;
    if (kind === 'status') { loadStatus().then(renderPairing); }
    if (kind === 'phones') loadPhones();
    if (kind === 'activity') loadActivity();
    if (kind === 'pairing') loadPairing();
    if (kind === 'code') loadCode();
    if (kind === 'approvals') loadApprovals();
  });
  listen('helm://navigate', (e) => go(String(e.payload)));

  setInterval(() => { renderPairing(); renderCode(); tickApprovals(); }, 1000);
  setInterval(() => { if (!document.hidden) loadStatus(); }, 5000);
  setInterval(() => { if (!document.hidden && state.page !== 'activity') renderPhones(); }, 60000);

  (async () => {
    await loadStatus();
    await Promise.all([loadPhones(), loadActivity(), loadPairing(), loadSettings(), loadCode(), loadApprovals()]);
    go(location.hash.slice(1) || 'home');
  })();

  // ------------------------------------------------------------------ browser preview
  // Opening ui/index.html in a normal browser (no Tauri) shows sample data for design work.
  async function previewInvoke(cmd) {
    const now = Date.now();
    const sample = {
      get_status: {
        run: 'running', version: '0.1.0', host_name: 'DESKTOP-PREVIEW', host_first: 'DESKTOP-', host_rest: 'PREVIEW',
        host_id: 'preview', fingerprint: 'AB:CD:EF', port: 47820, configured_port: 47820, addresses: ['192.168.0.173'],
        uptime_s: 754, phones_online: 1, phones_total: 2, phones_summary: '1 of 2 online',
        relay: { enabled: false, url: 'wss://relay.example.com', connected: false, last_error: null, line: { state: 'off', label: 'Off', text: 'Off. Phones reach this PC on the same network only.' } }, error: null, data_dir: 'C:\\Users\\you\\AppData\\Roaming\\Helm',
        headline: { tone: 'ok', label: 'Running', badge: 'Connected', message: 'Pixel 8 is connected. Go steer!' },
      },
      get_phones: [
        { device_id: 'a', name: 'Pixel 8', tier: 'admin', paired_at_ms: now - 9e8, last_seen_ms: now, online: true, via: 'lan', peer: '192.168.0.42:5555' },
        { device_id: 'b', name: 'Galaxy Tab', tier: 'remote', paired_at_ms: now - 2e9, last_seen_ms: now - 9e7, online: false, via: null, peer: null },
      ],
      get_pairing: null,
      get_code_pairing: null,
      get_pending_pairings: [],
      get_activity: [
        { time: new Date(now - 6e4).toISOString(), event: 'auth_ok', device_id: 'a', device_name: 'Pixel 8', peer: '192.168.0.42:5555', detail: '', text: 'Pixel 8 connected' },
        { time: new Date(now - 3e6).toISOString(), event: 'pair_fail', device_id: null, device_name: null, peer: '192.168.0.50:4000', detail: 'bad_token', text: 'Pairing refused (bad token)' },
      ],
      get_settings: { require_approval: false, autostart: false, autostart_key: 'HKCU\\…\\Run', port: 47820, relay_enabled: false, relay_url: 'wss://relay.example.com', data_dir: '', version: '0.1.0' },
    };
    if (cmd in sample) return sample[cmd];
    throw new Error('Preview only: not connected to Helm.');
  }
})();
