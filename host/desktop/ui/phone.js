// The Phone page: pick a connected phone, ask it, decode its H.264 with WebCodecs onto a canvas,
// and turn mouse, wheel and keyboard into input for it. The rules live on the phone and in the
// agent (Docs/protocol/phone-control.md); this page only shows what they decide.
'use strict';

(() => {
  const core = window.HelmPhoneCore;
  const tauri = window.__TAURI__;
  // Opening ui/index.html in a normal browser shows the layout with a sample phone.
  const invoke = tauri ? tauri.core.invoke : async (cmd) => {
    if (cmd === 'phone_targets') return [{ device_id: 'a', name: 'Pixel 8', via: 'lan', supported: true, enabled: true, accessibility: true }];
    throw new Error('Preview only: not connected to Helm.');
  };
  const listen = tauri ? tauri.event.listen : async () => () => {};

  const $ = (id) => document.getElementById(id);
  const MAX_FRAME_WAIT_MS = 8000;
  const WHEEL_EVERY_MS = 220;
  const KEYS_EVERY_MS = 60;
  const BLACK_CHECK_MS = 500;

  const page = {
    targets: [],
    session: null, // {id, device_id, relayed, ...} from the agent
    phase: 'idle', // idle | waiting | starting | live
    decoder: null,
    codec: null,
    gate: new core.FrameGate(),
    gesture: new core.PointerGesture(),
    wheel: new core.WheelBatch(),
    black: new core.BlackWatch(),
    typed: '',
    frames: 0,
    lastFrameAt: 0,
    protectedShown: false,
    lastPoint: null,
  };

  // ------------------------------------------------------------------ messages

  function setStatus(title, text, tone) {
    $('phone-status-title').textContent = title;
    $('phone-status-text').textContent = text;
    $('phone-status').dataset.tone = tone || '';
  }

  function setNotice(id, text) {
    const el = $(id);
    el.hidden = !text;
    el.textContent = text || '';
  }

  function toast(text) {
    const t = $('toast');
    t.textContent = text;
    t.dataset.tone = 'error';
    t.hidden = false;
    clearTimeout(toast.timer);
    toast.timer = setTimeout(() => { t.hidden = true; }, 3200);
  }

  function render() {
    const live = page.phase === 'live';
    const running = page.phase !== 'idle';
    $('phone-request').hidden = running;
    $('phone-stop').hidden = !running;
    $('phone-select').disabled = running;
    $('phone-side').hidden = !live;
    $('phone-empty').hidden = live;
    $('phone-canvas').hidden = !live;
    const control = Boolean(page.session && page.session.control);
    const badge = $('phone-badge');
    badge.hidden = !live;
    badge.textContent = control ? 'Controlling' : 'View only';
    badge.dataset.on = String(control);
    for (const b of document.querySelectorAll('[data-phone-global]')) b.disabled = !control;
    setNotice('phone-relay', page.session && page.session.relayed
      ? 'This phone is connected through the relay, so this will be slow, and the picture is limited to 720p at 15 fps.'
      : '');
    setNotice('phone-viewonly', live && !control
      ? (page.session.accessibility
        ? 'View only: the phone chose not to allow control this time.'
        : 'View only: the Helm accessibility service is off on the phone. Clicks and keys are not sent.')
      : '');
    setNotice('phone-secure', secureText());
    if (live && page.frames > 0) {
      $('phone-size').textContent = `${page.session.width} × ${page.session.height}`;
    } else {
      $('phone-size').textContent = running ? 'Waiting' : '—';
    }
  }

  function secureText() {
    if (page.phase !== 'live' || !page.session) return '';
    if (page.session.secure === 'password') {
      return 'Password field: typing from the PC is blocked. Type it on the phone.';
    }
    if (page.protectedShown) {
      return 'The picture is black. Android hides protected apps (banking, DRM video) from capture, and Helm does not get around that. It may also just be a dark screen. Use the phone itself for this one.';
    }
    return '';
  }

  // ------------------------------------------------------------------ phones

  function targetLabel(t) {
    const base = `${t.name} · ${t.via === 'relay' ? 'relay' : 'Wi-Fi'}`;
    if (!t.supported) return `${base} · update its app`;
    if (!t.enabled) return `${base} · off on the phone`;
    return t.accessibility ? `${base} · can be controlled` : `${base} · view only`;
  }

  async function loadTargets() {
    const list = await invoke('phone_targets').catch(() => null);
    if (!list) return;
    page.targets = list;
    const select = $('phone-select');
    const had = select.value;
    select.replaceChildren(...list.map((t) => {
      const o = document.createElement('option');
      o.value = t.device_id;
      o.textContent = targetLabel(t);
      return o;
    }));
    if (list.some((t) => t.device_id === had)) select.value = had;
    const none = list.length === 0;
    select.hidden = none;
    $('phone-none').hidden = !none;
    $('phone-request').disabled = none;
    explainSelected();
  }

  function explainSelected() {
    const t = page.targets.find((x) => x.device_id === $('phone-select').value);
    if (page.phase !== 'idle') return;
    if (!t) {
      setStatus('Connect a phone', 'Open Helm on the phone and connect it to this PC first.', 'wait');
    } else if (!t.supported) {
      setStatus('Update the phone', 'Helm on that phone is too old for this. Update the app there.', 'wait');
    } else if (!t.enabled) {
      setStatus('Off on the phone', 'On the phone, open Helm, then "Let PC see this phone", and turn it on. It is off by default.', 'wait');
    } else {
      setStatus('Ready', 'The phone will ask its owner. You see nothing until they accept there and in Android\'s prompt.', '');
    }
  }

  // ------------------------------------------------------------------ decoding

  function closeDecoder() {
    if (page.decoder && page.decoder.state !== 'closed') {
      try { page.decoder.close(); } catch { /* already gone */ }
    }
    page.decoder = null;
    page.codec = null;
  }

  function drawFrame(frame) {
    const canvas = $('phone-canvas');
    const w = frame.displayWidth;
    const h = frame.displayHeight;
    // A rotation arrives as a new SPS, so the picture simply changes shape here.
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    canvas.getContext('2d').drawImage(frame, 0, 0);
    frame.close();
    page.frames++;
    page.lastFrameAt = performance.now();
    if (page.phase === 'starting') goLive();
  }

  async function ensureDecoder(codec) {
    if (page.decoder && page.codec === codec && page.decoder.state === 'configured') return true;
    closeDecoder();
    const config = { codec, optimizeForLatency: true };
    const check = await VideoDecoder.isConfigSupported(config).catch(() => ({ supported: false }));
    if (!check.supported) {
      failDecoding(`This PC cannot decode ${codec}.`);
      return false;
    }
    page.decoder = new VideoDecoder({
      output: drawFrame,
      error: () => {
        // A broken stream or a driver hiccup: start over from the next keyframe.
        closeDecoder();
        page.gate.reset();
        invoke('phone_keyframe').catch(() => {});
      },
    });
    page.decoder.configure(config);
    page.codec = codec;
    return true;
  }

  function failDecoding(text) {
    setStatus('Cannot show the phone', text, 'error');
    invoke('phone_stop').catch(() => {});
  }

  let decodeChain = Promise.resolve();

  /** One frame from Rust. Acknowledged at once: Rust holds back at most a few frames for us. */
  function onFrame(buffer) {
    invoke('phone_ack').catch(() => {});
    const frame = core.parseFrame(buffer);
    if (!frame) return;
    // Configuring is asynchronous; frames stay in order behind it.
    decodeChain = decodeChain.then(() => decode(frame)).catch(() => {});
  }

  async function decode(frame) {
    if (page.phase === 'idle') return;
    if (frame.key) {
      const codec = core.codecFromKeyframe(frame.data);
      if (codec && !(await ensureDecoder(codec))) return;
    }
    const decoder = page.decoder;
    const queued = decoder ? decoder.decodeQueueSize : 0;
    const verdict = page.gate.offer(frame.key && Boolean(decoder), queued);
    if (verdict.requestKey) invoke('phone_keyframe').catch(() => {});
    if (!verdict.decode || !decoder || decoder.state !== 'configured') return;
    decoder.decode(new EncodedVideoChunk({
      type: frame.key ? 'key' : 'delta',
      timestamp: frame.ptsUs,
      data: frame.data,
    }));
  }

  function goLive() {
    page.phase = 'live';
    setStatus('Live', 'Stop here, or on the phone at any time. Locking the phone ends it too.', 'ok');
    render();
    $('phone-canvas').focus({ preventScroll: true });
  }

  // ------------------------------------------------------------------ session

  async function request() {
    const device = $('phone-select').value;
    if (!device) return;
    if (!tauri) throw new Error('Preview only: not connected to Helm.');
    page.phase = 'waiting';
    page.frames = 0;
    page.protectedShown = false;
    page.black = new core.BlackWatch();
    page.gate = new core.FrameGate();
    decodeChain = Promise.resolve();
    setStatus('Waiting for the phone', 'Accept on the phone, then in Android\'s prompt.', 'wait');
    render();
    const channel = new tauri.core.Channel();
    channel.onmessage = onFrame;
    try {
      const info = await invoke('phone_request', { deviceId: device, onFrame: channel });
      // The phone may have answered already; the events know better then.
      if (page.phase === 'waiting') page.session = info;
      render();
    } catch (e) {
      page.phase = 'idle';
      page.session = null;
      setStatus('Not started', String(e), 'error');
      render();
    }
  }

  function finish(title, text, tone) {
    page.phase = 'idle';
    page.session = null;
    page.gesture.reset();
    page.typed = '';
    closeDecoder();
    setStatus(title, text, tone);
    render();
    loadTargets();
  }

  function onEvent(ev) {
    switch (ev.event) {
      case 'targets':
        loadTargets();
        break;
      case 'waiting':
        break;
      case 'accepted':
        page.session = ev.info;
        page.phase = 'starting';
        setStatus('Starting', 'The phone accepted. The first picture is on its way.', 'wait');
        render();
        break;
      case 'active':
        page.session = ev.info;
        render();
        break;
      case 'state':
        page.session = ev.info;
        render();
        break;
      case 'ended':
        finish(core.endTitle(ev.reason, ev.was_active), core.endText(ev.reason),
          ev.was_active && ev.reason === 'stopped_by_pc' ? '' : 'error');
        break;
      default:
        break;
    }
  }

  // ------------------------------------------------------------------ input

  function canControl() {
    return page.phase === 'live' && page.session && page.session.control;
  }

  function send(events) {
    if (!canControl()) return;
    for (const ev of events) invoke('phone_input', { event: ev }).catch(() => {});
  }

  function point(e, clamp) {
    const canvas = $('phone-canvas');
    return core.toPhone(e.clientX, e.clientY, canvas.getBoundingClientRect(), canvas.width, canvas.height, clamp);
  }

  function flushTyped() {
    if (!page.typed) return;
    const text = page.typed;
    page.typed = '';
    send([{ k: 'text', text }]);
  }

  function wireInput() {
    const canvas = $('phone-canvas');
    canvas.addEventListener('contextmenu', (e) => e.preventDefault());
    canvas.addEventListener('pointerdown', (e) => {
      canvas.focus({ preventScroll: true });
      const p = point(e, false);
      if (!p || !canControl()) return;
      if (e.button === 2) {
        // Right click is the quick way to a long press.
        send([{ k: 'long', x: p.x, y: p.y }]);
        return;
      }
      if (e.button !== 0) return;
      canvas.setPointerCapture(e.pointerId);
      page.lastPoint = p;
      send(page.gesture.press(p, performance.now()));
    });
    canvas.addEventListener('pointermove', (e) => {
      if (page.gesture.state === 'idle') return;
      const p = point(e, true);
      if (!p) return;
      page.lastPoint = p;
      send(page.gesture.move(p));
    });
    canvas.addEventListener('pointerup', (e) => {
      if (e.button !== 0 || page.gesture.state === 'idle') return;
      const p = point(e, true) || page.lastPoint;
      send(page.gesture.release(p, performance.now()));
    });
    canvas.addEventListener('pointercancel', () => {
      send(page.gesture.cancel(page.lastPoint));
    });
    canvas.addEventListener('lostpointercapture', () => {
      send(page.gesture.cancel(page.lastPoint));
    });
    canvas.addEventListener('wheel', (e) => {
      e.preventDefault();
      const p = point(e, true);
      if (p && canControl()) page.wheel.add(e.deltaX, e.deltaY, e.deltaMode, p);
    }, { passive: false });
    canvas.addEventListener('keydown', (e) => {
      const action = core.keyAction(e);
      if (!action) return;
      e.preventDefault();
      if (!canControl()) return;
      if (action.text) {
        page.typed += action.text;
      } else if (action.key) {
        flushTyped();
        send([{ k: 'key', key: action.key }]);
      } else if (action.swipe) {
        send([core.pageSwipe(action.swipe)]);
      }
    });
    canvas.addEventListener('paste', (e) => {
      const text = e.clipboardData && e.clipboardData.getData('text');
      if (text && canControl()) {
        e.preventDefault();
        flushTyped();
        // Longer pastes go in pieces the agent accepts (8 KiB of UTF-8 at most).
        const chars = [...text];
        for (let i = 0; i < chars.length; i += 2000) {
          send([{ k: 'text', text: chars.slice(i, i + 2000).join('') }]);
        }
      }
    });

    // A held press becomes a long press without waiting for the release.
    setInterval(() => send(page.gesture.tick(performance.now())), 50);
    setInterval(() => {
      const ev = page.wheel.take();
      if (ev) send([ev]);
    }, WHEEL_EVERY_MS);
    setInterval(flushTyped, KEYS_EVERY_MS);
  }

  // Protected screens arrive as pure black: look at a tiny copy now and then.
  function wireBlackCheck() {
    const small = document.createElement('canvas');
    small.width = 16;
    small.height = 16;
    const ctx = small.getContext('2d', { willReadFrequently: true });
    setInterval(() => {
      if (page.phase !== 'live' || page.frames === 0) return;
      ctx.drawImage($('phone-canvas'), 0, 0, 16, 16);
      const black = core.isBlack(ctx.getImageData(0, 0, 16, 16).data);
      const show = page.black.sample(black, performance.now());
      if (show !== page.protectedShown) {
        page.protectedShown = show;
        render();
      }
    }, BLACK_CHECK_MS);
  }

  // A session that never delivers a first picture should not sit there silently.
  function wireStartWatch() {
    let startedAt = 0;
    setInterval(() => {
      if (page.phase !== 'starting') {
        startedAt = 0;
        return;
      }
      if (!startedAt) startedAt = performance.now();
      if (performance.now() - startedAt > MAX_FRAME_WAIT_MS) {
        startedAt = performance.now();
        invoke('phone_keyframe').catch(() => {});
      }
    }, 1000);
  }

  // ------------------------------------------------------------------ wiring

  function start() {
    if (typeof VideoDecoder === 'undefined') {
      setStatus('Cannot show phones here', 'This window has no WebCodecs video decoder. Update the WebView2 runtime.', 'error');
      $('phone-request').disabled = true;
    }
    $('phone-request').addEventListener('click', () => {
      request().catch((e) => toast(String(e)));
    });
    $('phone-stop').addEventListener('click', () => {
      invoke('phone_stop').catch((e) => toast(String(e)));
    });
    $('phone-select').addEventListener('change', explainSelected);
    for (const b of document.querySelectorAll('[data-phone-global]')) {
      b.addEventListener('click', () => send([{ k: 'global', action: b.dataset.phoneGlobal }]));
    }
    wireInput();
    wireBlackCheck();
    wireStartWatch();
    listen('helm://phone', (e) => onEvent(e.payload));
    listen('helm://changed', (e) => {
      if (e.payload && e.payload.kind === 'phones') loadTargets();
    });
    render();
    loadTargets();
  }

  window.HelmPhone = { onShow: loadTargets };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
  else start();
})();
