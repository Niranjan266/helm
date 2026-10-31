import type { HostRelay } from "./host-relay";
import type { RateLimiter } from "./rate-limiter";

export interface Env {
  HOST_RELAY: DurableObjectNamespace<HostRelay>;
  RATE_LIMITER: DurableObjectNamespace<RateLimiter>;
  MAX_PHONES_PER_HOST?: string;
  MAX_FRAME_BYTES?: string;
  HOST_AUTH_TIMEOUT_MS?: string;
  HOST_IDLE_TIMEOUT_MS?: string;
  PHONE_IDLE_TIMEOUT_MS?: string;
  MAX_PENDING_HOSTS?: string;
  CONNECT_BURST?: string;
  CONNECT_PER_MINUTE?: string;
  TURN_URLS?: string;
  TURN_SHARED_SECRET?: string;
}

export interface Limits {
  maxPhones: number;
  maxFrame: number;
  authTimeoutMs: number;
  hostIdleMs: number;
  phoneIdleMs: number;
  maxPendingHosts: number;
  connectBurst: number;
  connectPerMinute: number;
}

function int(value: string | undefined, fallback: number): number {
  const n = Number.parseInt(value ?? "", 10);
  return Number.isFinite(n) && n > 0 ? n : fallback;
}

export function limits(env: Env): Limits {
  return {
    maxPhones: int(env.MAX_PHONES_PER_HOST, 4),
    maxFrame: int(env.MAX_FRAME_BYTES, 65536),
    authTimeoutMs: int(env.HOST_AUTH_TIMEOUT_MS, 10_000),
    hostIdleMs: int(env.HOST_IDLE_TIMEOUT_MS, 90_000),
    phoneIdleMs: int(env.PHONE_IDLE_TIMEOUT_MS, 120_000),
    maxPendingHosts: int(env.MAX_PENDING_HOSTS, 2),
    connectBurst: int(env.CONNECT_BURST, 10),
    connectPerMinute: int(env.CONNECT_PER_MINUTE, 20),
  };
}
