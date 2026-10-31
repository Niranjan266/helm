import { bytesToBase64 } from "./crypto";
import type { Env } from "./env";

export const TURN_TTL_S = 3600;

export interface TurnCredentials {
  urls: string[];
  username: string;
  credential: string;
  ttl: number;
}

/**
 * Stub for the WebRTC fallback path: time-limited credentials in the shared-secret
 * "TURN REST API" scheme (username = "<expiry>:<id>", credential = base64 HMAC-SHA1).
 * Returns null when no provider is configured.
 */
export async function turnCredentials(
  env: Env,
  hostId: string,
  nowMs = Date.now(),
): Promise<TurnCredentials | null> {
  const urls = (env.TURN_URLS ?? "")
    .split(",")
    .map((u) => u.trim())
    .filter((u) => u.length > 0);
  const secret = env.TURN_SHARED_SECRET ?? "";
  if (urls.length === 0 || secret.length === 0) return null;
  const username = `${Math.floor(nowMs / 1000) + TURN_TTL_S}:${hostId}`;
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-1" },
    false,
    ["sign"],
  );
  const mac = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(username));
  return { urls, username, credential: bytesToBase64(new Uint8Array(mac)), ttl: TURN_TTL_S };
}
