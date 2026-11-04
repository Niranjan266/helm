// The pure logic of the Phone page: frames, the codec string, coordinate mapping, gestures, the
// decoder queue and the black-picture check. No DOM here, so tests/phone-core.test.mjs runs it
// under Node; the same file is loaded by the window (window.HelmPhoneCore).
'use strict';

(function (root, factory) {
  const core = factory();
  if (typeof module === 'object' && module.exports) module.exports = core;
  else root.HelmPhoneCore = core;
})(typeof self !== 'undefined' ? self : this, () => {
  const HEADER_LEN = 9;
  const KIND_VIDEO = 0x01;
  const KIND_VIDEO_KEY = 0x02;

  // ---------------------------------------------------------------- frames

  /** `[kind u8][pts_us u64 BE][payload]` (PROTOCOL §10) from an ArrayBuffer / Uint8Array. */
  function parseFrame(buffer) {
    const bytes = buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer);
    if (bytes.length < HEADER_LEN) return null;
    const kind = bytes[0];
    if (kind !== KIND_VIDEO && kind !== KIND_VIDEO_KEY) return null;
    const view = new DataView(bytes.buffer, bytes.byteOffset, HEADER_LEN);
    const ptsUs = Number(view.getBigUint64(1, false));
    const data = bytes.subarray(HEADER_LEN);
    if (data.length === 0) return null;
    return { key: kind === KIND_VIDEO_KEY, ptsUs, data };
  }

  /** Start offsets of the NAL units in an Annex-B buffer (just after each start code). */
  function nalOffsets(data) {
    const out = [];
    for (let i = 0; i + 2 < data.length; i++) {
      if (data[i] === 0 && data[i + 1] === 0 && data[i + 2] === 1) {
        out.push(i + 3);
        i += 2;
      }
    }
    return out;
  }

  function hex2(n) {
    return n.toString(16).toUpperCase().padStart(2, '0');
  }

  /**
   * WebCodecs codec string from the SPS in a keyframe, e.g. `avc1.42E01F`: profile_idc,
   * constraint flags, level_idc are the three bytes after the NAL header. Null without an SPS.
   */
  function codecFromKeyframe(data) {
    for (const at of nalOffsets(data)) {
      if ((data[at] & 0x1f) === 7 && at + 3 < data.length) {
        return `avc1.${hex2(data[at + 1])}${hex2(data[at + 2])}${hex2(data[at + 3])}`;
      }
    }
    return null;
  }

  // ---------------------------------------------------------------- coordinates

  /**
   * Where the picture sits in its element: fitted whole, centred, aspect ratio kept.
   * `box` = the element's on-screen rectangle.
   */
  function pictureRect(box, picWidth, picHeight) {
    if (!(picWidth > 0 && picHeight > 0 && box.width > 0 && box.height > 0)) return null;
    const scale = Math.min(box.width / picWidth, box.height / picHeight);
    const width = picWidth * scale;
    const height = picHeight * scale;
    return {
      left: box.left + (box.width - width) / 2,
      top: box.top + (box.height - height) / 2,
      width,
      height,
    };
  }

  /**
   * A mouse position to normalised 0..1 on the picture. Null on the bars around the picture,
   * unless `clamp` (a drag that leaves the picture keeps going along its edge).
   * The picture already has the phone's current rotation, so nothing else is needed.
   */
  function toPhone(clientX, clientY, box, picWidth, picHeight, clamp) {
    const r = pictureRect(box, picWidth, picHeight);
    if (!r) return null;
    const x = (clientX - r.left) / r.width;
    const y = (clientY - r.top) / r.height;
    const inside = x >= 0 && x <= 1 && y >= 0 && y <= 1;
    if (!inside && !clamp) return null;
    return { x: Math.min(1, Math.max(0, x)), y: Math.min(1, Math.max(0, y)) };
  }

  // ---------------------------------------------------------------- mouse to gestures

  /**
   * Press and release to `phone.in` events. Time is passed in.
   *  - press + release, little movement, under the hold time: `tap`
   *  - press held still for the hold time: `long` (see `tick`)
   *  - press + movement past the slop: `down`, `move`..., `up`
   */
  class PointerGesture {
    constructor(holdMs = 500, slop = 0.012) {
      this.holdMs = holdMs;
      this.slop = slop;
      this.reset();
    }

    reset() {
      this.state = 'idle'; // idle | pressed | dragging | held
      this.start = null;
      this.at = 0;
    }

    press(p, now) {
      this.state = 'pressed';
      this.start = p;
      this.at = now;
      return [];
    }

    move(p) {
      if (this.state === 'dragging') return [{ k: 'move', x: p.x, y: p.y }];
      if (this.state !== 'pressed') return [];
      const far = Math.hypot(p.x - this.start.x, p.y - this.start.y) > this.slop;
      if (!far) return [];
      this.state = 'dragging';
      return [{ k: 'down', x: this.start.x, y: this.start.y }, { k: 'move', x: p.x, y: p.y }];
    }

    /** Call while pressed; the long press fires when the hold time passes. */
    tick(now) {
      if (this.state !== 'pressed' || now - this.at < this.holdMs) return [];
      this.state = 'held';
      return [{ k: 'long', x: this.start.x, y: this.start.y }];
    }

    release(p, now) {
      const state = this.state;
      const start = this.start;
      const fired = this.tick(now);
      this.reset();
      if (fired.length) return fired;
      if (state === 'dragging') return [{ k: 'up', x: p.x, y: p.y }];
      if (state === 'pressed') return [{ k: 'tap', x: start.x, y: start.y }];
      return [];
    }

    /** The mouse left without a release (window lost focus, session ended). */
    cancel(p) {
      const dragging = this.state === 'dragging';
      this.reset();
      return dragging && p ? [{ k: 'up', x: p.x, y: p.y }] : [];
    }
  }

  /** Wheel events collected into `scroll` events, a few per second: gestures take their time. */
  class WheelBatch {
    constructor(lineHeight = 100) {
      this.lineHeight = lineHeight;
      this.dy = 0;
      this.dx = 0;
      this.p = null;
    }

    add(deltaX, deltaY, deltaMode, p) {
      // deltaMode: 0 = pixels, 1 = lines, 2 = pages.
      const unit = deltaMode === 1 ? 1 / 3 : deltaMode === 2 ? 3 : 1 / this.lineHeight;
      this.dy += deltaY * unit;
      this.dx += deltaX * unit;
      this.p = p;
    }

    take() {
      if (!this.p || (Math.abs(this.dy) < 0.05 && Math.abs(this.dx) < 0.05)) return null;
      const ev = { k: 'scroll', x: this.p.x, y: this.p.y, dy: this.dy, dx: this.dx };
      this.dy = 0;
      this.dx = 0;
      return ev;
    }
  }

  // ---------------------------------------------------------------- keyboard

  /** A key press to what it means on the phone: text to buffer, an edit key, or nothing. */
  function keyAction(e) {
    if (e.ctrlKey || e.metaKey || e.altKey) return null;
    if (e.key === 'Backspace') return { key: 'backspace' };
    if (e.key === 'Enter') return { key: 'enter' };
    if (e.key === 'PageDown') return { swipe: 'down' };
    if (e.key === 'PageUp') return { swipe: 'up' };
    if (typeof e.key === 'string' && [...e.key].length === 1) return { text: e.key };
    return null;
  }

  /** A page's worth of scrolling. `down` = show what is below: the finger moves up. */
  function pageSwipe(direction) {
    const [from, to] = direction === 'down' ? [0.8, 0.2] : [0.2, 0.8];
    return { k: 'swipe', x1: 0.5, y1: from, x2: 0.5, y2: to, ms: 300 };
  }

  // ---------------------------------------------------------------- decoding

  /**
   * Which frames go to the decoder. A delta needs the picture before it, so after dropping one
   * (decoder behind) everything waits for a keyframe, which is requested once.
   */
  class FrameGate {
    constructor(maxQueue = 3) {
      this.maxQueue = maxQueue;
      this.waitingKey = true;
      this.dropped = 0;
    }

    /** `{decode, requestKey}` for one frame. */
    offer(key, decodeQueueSize) {
      if (key) {
        this.waitingKey = false;
        return { decode: true, requestKey: false };
      }
      if (this.waitingKey) {
        this.dropped++;
        return { decode: false, requestKey: false };
      }
      if (decodeQueueSize > this.maxQueue) {
        this.waitingKey = true;
        this.dropped++;
        return { decode: false, requestKey: true };
      }
      return { decode: true, requestKey: false };
    }

    /** The decoder was reset: nothing can be decoded until a keyframe. */
    reset() {
      this.waitingKey = true;
    }
  }

  /** True when an RGBA sample has no light in it (phones show secure windows as pure black). */
  function isBlack(rgba, threshold = 8) {
    for (let i = 0; i + 3 < rgba.length; i += 4) {
      if (rgba[i] > threshold || rgba[i + 1] > threshold || rgba[i + 2] > threshold) return false;
    }
    return rgba.length >= 4;
  }

  /** Says "protected screen" after a black picture has lasted a while. Time is passed in. */
  class BlackWatch {
    constructor(afterMs = 2000) {
      this.afterMs = afterMs;
      this.since = null;
    }

    sample(black, now) {
      if (!black) {
        this.since = null;
        return false;
      }
      if (this.since === null) this.since = now;
      return now - this.since >= this.afterMs;
    }
  }

  // ---------------------------------------------------------------- what to say

  const END_TEXT = {
    declined: 'The phone declined.',
    capture_denied: "The phone declined at Android's capture prompt.",
    disabled: '"Let PC see this phone" is off on the phone.',
    busy: 'The phone is already in a session.',
    timeout: 'No answer from the phone in time.',
    open_timeout: 'The phone accepted but its video never arrived.',
    stopped_on_phone: 'Stopped on the phone.',
    stopped_by_pc: 'Stopped.',
    screen_locked: 'The phone was locked, which ends the session.',
    idle: 'Ended after 10 minutes without input from this PC.',
    capture_stopped: 'Android ended the capture.',
    encoder_failed: "The phone's video encoder failed.",
    unsupported: 'The phone cannot do this.',
    disconnected: 'The connection to the phone was lost.',
    agent_stopping: 'Helm stopped.',
  };

  function endText(reason) {
    return END_TEXT[reason] || `Ended (${String(reason).replace(/_/g, ' ')}).`;
  }

  /** A declined or never-started request reads as a refusal; the rest as an ended session. */
  function endTitle(reason, wasActive) {
    if (wasActive) return 'Session ended';
    return reason === 'declined' || reason === 'capture_denied' ? 'Declined' : 'Not started';
  }

  return {
    HEADER_LEN,
    parseFrame,
    codecFromKeyframe,
    pictureRect,
    toPhone,
    PointerGesture,
    WheelBatch,
    keyAction,
    pageSwipe,
    FrameGate,
    isBlack,
    BlackWatch,
    endText,
    endTitle,
  };
});
