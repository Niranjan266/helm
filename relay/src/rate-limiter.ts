import { DurableObject } from "cloudflare:workers";
import type { Env } from "./env";

/**
 * One instance per client IP. The token bucket lives in memory only: a restart refills it,
 * which is fine for an abuse brake and means no IP address is ever written to storage.
 */
export class RateLimiter extends DurableObject<Env> {
  private tokens = -1;
  private updated = 0;

  /** Takes one token. Returns 0 if allowed, otherwise the seconds to wait. */
  take(burst: number, perMinute: number): number {
    const now = Date.now();
    if (this.tokens < 0) {
      this.tokens = burst;
      this.updated = now;
    }
    this.tokens = Math.min(burst, this.tokens + ((now - this.updated) / 60_000) * perMinute);
    this.updated = now;
    if (this.tokens >= 1) {
      this.tokens -= 1;
      return 0;
    }
    return Math.max(1, Math.ceil(((1 - this.tokens) / perMinute) * 60));
  }
}
