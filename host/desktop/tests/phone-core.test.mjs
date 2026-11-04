// The Phone page's pure logic. Run: node --test host/desktop/tests/phone-core.test.mjs (cargo test runs it too).
import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const core = require('../ui/phone-core.js');

function frame(kind, pts, payload) {
  const out = new Uint8Array(9 + payload.length);
  out[0] = kind;
  new DataView(out.buffer).setBigUint64(1, BigInt(pts), false);
  out.set(payload, 9);
  return out;
}

test('frames: the layout of section 10', () => {
  const bytes = new Uint8Array([0x02, 0, 0, 0, 0, 0, 1, 0, 0, 0xaa, 0xbb]);
  const f = core.parseFrame(bytes.buffer);
  assert.equal(f.key, true);
  assert.equal(f.ptsUs, 65536);
  assert.deepEqual([...f.data], [0xaa, 0xbb]);
  const d = core.parseFrame(frame(1, 7, [1]));
  assert.equal(d.key, false);
  assert.equal(d.ptsUs, 7);
});

test('frames: junk and other kinds are ignored', () => {
  assert.equal(core.parseFrame(new Uint8Array([1, 2, 3])), null);
  assert.equal(core.parseFrame(frame(1, 7, [])), null, 'empty access unit');
  assert.equal(core.parseFrame(frame(0x06, 7, [1])), null, 'parts are joined before this');
  assert.equal(core.parseFrame(frame(0x03, 7, [1])), null, 'no audio here');
});

test('frames: a view into a larger buffer', () => {
  const big = new Uint8Array(30);
  big.set(frame(2, 9, [5, 6]), 10);
  const f = core.parseFrame(big.subarray(10, 21));
  assert.equal(f.ptsUs, 9);
  assert.deepEqual([...f.data], [5, 6]);
});

test('codec string comes from the SPS', () => {
  const au = new Uint8Array([0, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f, 0xda, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 1, 0x65, 0x88]);
  assert.equal(core.codecFromKeyframe(au), 'avc1.42E01F');
  const high = new Uint8Array([0, 0, 1, 0x27, 0x64, 0x00, 0x28, 0xac]);
  assert.equal(core.codecFromKeyframe(high), 'avc1.640028', 'nal_ref_idc does not matter');
  assert.equal(core.codecFromKeyframe(new Uint8Array([0, 0, 1, 0x65, 0x88])), null);
  assert.equal(core.codecFromKeyframe(new Uint8Array([0, 0, 1, 0x67, 0x42])), null, 'cut short');
});

const box = { left: 100, top: 50, width: 800, height: 800 };

test('coordinates: a portrait phone in a square box has bars left and right', () => {
  // 400 x 800 fits as 400 wide, 800 tall: 200 px bars each side.
  assert.deepEqual(core.pictureRect(box, 400, 800), { left: 300, top: 50, width: 400, height: 800 });
  assert.deepEqual(core.toPhone(300, 50, box, 400, 800, false), { x: 0, y: 0 });
  assert.deepEqual(core.toPhone(700, 850, box, 400, 800, false), { x: 1, y: 1 });
  assert.deepEqual(core.toPhone(500, 450, box, 400, 800, false), { x: 0.5, y: 0.5 });
  assert.equal(core.toPhone(299, 450, box, 400, 800, false), null, 'on the bar');
  assert.equal(core.toPhone(701, 450, box, 400, 800, false), null, 'on the bar');
});

test('coordinates: rotating the phone just changes the picture shape', () => {
  // 800 x 400 fits as 800 wide, 400 tall: 200 px bars top and bottom.
  assert.deepEqual(core.pictureRect(box, 800, 400), { left: 100, top: 250, width: 800, height: 400 });
  assert.deepEqual(core.toPhone(100, 250, box, 800, 400, false), { x: 0, y: 0 });
  assert.deepEqual(core.toPhone(500, 450, box, 800, 400, false), { x: 0.5, y: 0.5 });
  assert.deepEqual(core.toPhone(900, 650, box, 800, 400, false), { x: 1, y: 1 });
  assert.equal(core.toPhone(500, 249, box, 800, 400, false), null);
  // The same spot on the screen is a different spot on the phone.
  assert.deepEqual(core.toPhone(400, 300, box, 400, 800, false), { x: 0.25, y: 0.3125 });
  assert.deepEqual(core.toPhone(400, 300, box, 800, 400, false), { x: 0.375, y: 0.125 });
});

test('coordinates: scaled up and down, aspect is kept', () => {
  const r = core.pictureRect({ left: 0, top: 0, width: 300, height: 400 }, 1080, 2400);
  assert.equal(r.height, 400);
  assert.ok(Math.abs(r.width - 180) < 1e-9);
  assert.ok(Math.abs(r.left - 60) < 1e-9);
  const tiny = core.toPhone(150, 200, { left: 0, top: 0, width: 300, height: 400 }, 64, 64, false);
  assert.deepEqual(tiny, { x: 0.5, y: 0.5 });
});

test('coordinates: drags clamp to the edge; no picture means no point', () => {
  assert.deepEqual(core.toPhone(0, 450, box, 400, 800, true), { x: 0, y: 0.5 });
  assert.deepEqual(core.toPhone(5000, 5000, box, 400, 800, true), { x: 1, y: 1 });
  assert.equal(core.toPhone(500, 450, box, 0, 0, true), null);
  assert.equal(core.toPhone(500, 450, { left: 0, top: 0, width: 0, height: 0 }, 400, 800, true), null);
});

test('gesture: a quick click is a tap at the press position', () => {
  const g = new core.PointerGesture();
  const p = { x: 0.5, y: 0.5 };
  assert.deepEqual(g.press(p, 1000), []);
  assert.deepEqual(g.move({ x: 0.505, y: 0.5 }), [], 'within the slop');
  assert.deepEqual(g.release({ x: 0.505, y: 0.5 }, 1100), [{ k: 'tap', x: 0.5, y: 0.5 }]);
  assert.equal(g.state, 'idle');
});

test('gesture: moving makes a drag from where it started', () => {
  const g = new core.PointerGesture();
  g.press({ x: 0.5, y: 0.5 }, 1000);
  assert.deepEqual(g.move({ x: 0.6, y: 0.5 }), [
    { k: 'down', x: 0.5, y: 0.5 },
    { k: 'move', x: 0.6, y: 0.5 },
  ]);
  assert.deepEqual(g.move({ x: 0.7, y: 0.5 }), [{ k: 'move', x: 0.7, y: 0.5 }]);
  assert.deepEqual(g.tick(9999), [], 'a drag never turns into a long press');
  assert.deepEqual(g.release({ x: 0.8, y: 0.5 }, 9999), [{ k: 'up', x: 0.8, y: 0.5 }]);
});

test('gesture: holding still is one long press', () => {
  const g = new core.PointerGesture(500);
  g.press({ x: 0.2, y: 0.3 }, 1000);
  assert.deepEqual(g.tick(1499), []);
  assert.deepEqual(g.tick(1500), [{ k: 'long', x: 0.2, y: 0.3 }]);
  assert.deepEqual(g.tick(2000), [], 'fires once');
  assert.deepEqual(g.move({ x: 0.9, y: 0.9 }), [], 'no drag after it');
  assert.deepEqual(g.release({ x: 0.2, y: 0.3 }, 2000), [], 'no tap after it');
});

test('gesture: a slow release with no tick in between is still a long press', () => {
  const g = new core.PointerGesture(500);
  g.press({ x: 0.2, y: 0.3 }, 1000);
  assert.deepEqual(g.release({ x: 0.2, y: 0.3 }, 1600), [{ k: 'long', x: 0.2, y: 0.3 }]);
});

test('gesture: cancelling lifts the finger only if it was down', () => {
  const g = new core.PointerGesture();
  g.press({ x: 0.5, y: 0.5 }, 0);
  assert.deepEqual(g.cancel({ x: 0.5, y: 0.5 }), []);
  g.press({ x: 0.5, y: 0.5 }, 0);
  g.move({ x: 0.9, y: 0.5 });
  assert.deepEqual(g.cancel({ x: 0.9, y: 0.5 }), [{ k: 'up', x: 0.9, y: 0.5 }]);
  assert.deepEqual(g.release({ x: 0, y: 0 }, 1), [], 'nothing pressed any more');
});

test('wheel: collected into notches', () => {
  const w = new core.WheelBatch();
  assert.equal(w.take(), null);
  const p = { x: 0.5, y: 0.25 };
  w.add(0, 100, 0, p);
  w.add(0, 50, 0, p);
  assert.deepEqual(w.take(), { k: 'scroll', x: 0.5, y: 0.25, dy: 1.5, dx: 0 });
  assert.equal(w.take(), null, 'nothing left');
  w.add(0, -3, 1, p);
  assert.deepEqual(w.take(), { k: 'scroll', x: 0.5, y: 0.25, dy: -1, dx: 0 });
  w.add(100, 0, 0, p);
  assert.equal(w.take().dx, 1);
  w.add(0, 1, 2, p);
  assert.equal(w.take().dy, 3, 'one page');
  w.add(0, 1, 0, p);
  assert.equal(w.take(), null, 'too little to move');
});

test('keys: text, edit keys, and what is left alone', () => {
  assert.deepEqual(core.keyAction({ key: 'a' }), { text: 'a' });
  assert.deepEqual(core.keyAction({ key: 'é' }), { text: 'é' });
  assert.deepEqual(core.keyAction({ key: '😀' }), { text: '😀' });
  assert.deepEqual(core.keyAction({ key: ' ' }), { text: ' ' });
  assert.deepEqual(core.keyAction({ key: 'Backspace' }), { key: 'backspace' });
  assert.deepEqual(core.keyAction({ key: 'Enter' }), { key: 'enter' });
  assert.deepEqual(core.keyAction({ key: 'PageDown' }), { swipe: 'down' });
  assert.equal(core.keyAction({ key: 'a', ctrlKey: true }), null, 'shortcuts stay on the PC');
  assert.equal(core.keyAction({ key: 'a', altKey: true }), null);
  assert.equal(core.keyAction({ key: 'Tab' }), null);
  assert.equal(core.keyAction({ key: 'Shift' }), null);
});

test('page keys: down means the finger moves up', () => {
  const s = core.pageSwipe('down');
  assert.equal(s.k, 'swipe');
  assert.ok(s.y1 > s.y2);
  const u = core.pageSwipe('up');
  assert.ok(u.y1 < u.y2);
});

test('frame gate: nothing before the first keyframe', () => {
  const g = new core.FrameGate(3);
  assert.deepEqual(g.offer(false, 0), { decode: false, requestKey: false });
  assert.deepEqual(g.offer(true, 0), { decode: true, requestKey: false });
  assert.deepEqual(g.offer(false, 0), { decode: true, requestKey: false });
  assert.equal(g.dropped, 1);
});

test('frame gate: a slow decoder drops deltas until a keyframe, asking once', () => {
  const g = new core.FrameGate(3);
  g.offer(true, 0);
  assert.deepEqual(g.offer(false, 3), { decode: true, requestKey: false });
  assert.deepEqual(g.offer(false, 4), { decode: false, requestKey: true });
  assert.deepEqual(g.offer(false, 0), { decode: false, requestKey: false }, 'broken reference');
  assert.deepEqual(g.offer(true, 9), { decode: true, requestKey: false }, 'keys always go');
  assert.deepEqual(g.offer(false, 0), { decode: true, requestKey: false });
});

test('frame gate: a decoder reset waits for a keyframe', () => {
  const g = new core.FrameGate(3);
  g.offer(true, 0);
  g.reset();
  assert.deepEqual(g.offer(false, 0), { decode: false, requestKey: false });
  assert.deepEqual(g.offer(true, 0), { decode: true, requestKey: false });
});

test('black check: protected screens are shown after a while', () => {
  assert.equal(core.isBlack(new Uint8ClampedArray([0, 0, 0, 255, 8, 8, 8, 255])), true);
  assert.equal(core.isBlack(new Uint8ClampedArray([0, 0, 0, 255, 0, 9, 0, 255])), false);
  assert.equal(core.isBlack(new Uint8ClampedArray([])), false);
  const w = new core.BlackWatch(2000);
  assert.equal(w.sample(true, 1000), false);
  assert.equal(w.sample(true, 2999), false);
  assert.equal(w.sample(true, 3000), true);
  assert.equal(w.sample(false, 3100), false);
  assert.equal(w.sample(true, 3200), false, 'starts over');
});

test('error states read clearly', () => {
  assert.equal(core.endTitle('declined', false), 'Declined');
  assert.equal(core.endTitle('capture_denied', false), 'Declined');
  assert.equal(core.endTitle('timeout', false), 'Not started');
  assert.equal(core.endTitle('screen_locked', true), 'Session ended');
  assert.match(core.endText('declined'), /declined/i);
  assert.match(core.endText('screen_locked'), /locked/i);
  assert.match(core.endText('idle'), /10 minutes/);
  assert.equal(core.endText('brand_new_reason'), 'Ended (brand new reason).');
});
