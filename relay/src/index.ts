import { limits, type Env } from "./env";
import { turnCredentials } from "./turn";

export { HostRelay } from "./host-relay";
export { RateLimiter } from "./rate-limiter";

const HOST_ID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const WS_ROUTE_RE = /^\/(host|connect)\/([^/]+)$/;

function text(status: number, body: string, headers: Record<string, string> = {}): Response {
  return new Response(body, {
    status,
    headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store", ...headers },
  });
}

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });
}

function parseHostId(raw: string | null | undefined): string | null {
  const id = (raw ?? "").toLowerCase();
  return HOST_ID_RE.test(id) ? id : null;
}

/** Per-IP token bucket. Returns null if allowed, else a 429 response. */
async function rateLimit(request: Request, env: Env): Promise<Response | null> {
  const ip = request.headers.get("CF-Connecting-IP") ?? "unknown";
  const lim = limits(env);
  const stub = env.RATE_LIMITER.get(env.RATE_LIMITER.idFromName(ip));
  const wait = await stub.take(lim.connectBurst, lim.connectPerMinute);
  return wait > 0 ? text(429, "rate limited", { "retry-after": String(wait) }) : null;
}

function hostStub(env: Env, hostId: string) {
  return env.HOST_RELAY.get(env.HOST_RELAY.idFromName(hostId));
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (request.method !== "GET") return text(405, "method not allowed", { allow: "GET" });
    if (url.pathname === "/health") return text(200, "ok");

    const route = WS_ROUTE_RE.exec(url.pathname);
    if (route) {
      const hostId = parseHostId(route[2]);
      if (!hostId) return text(400, "bad host_id");
      if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket") {
        return text(426, "websocket upgrade required", { upgrade: "websocket" });
      }
      const limited = await rateLimit(request, env);
      if (limited) return limited;
      return hostStub(env, hostId).fetch(`https://relay.internal/${route[1]}`, {
        headers: { Upgrade: "websocket", "x-helm-host-id": hostId },
      });
    }

    if (url.pathname === "/turn") {
      const hostId = parseHostId(url.searchParams.get("host_id"));
      if (!hostId) return text(400, "bad host_id");
      const limited = await rateLimit(request, env);
      if (limited) return limited;
      if (!(env.TURN_URLS ?? "").trim() || !env.TURN_SHARED_SECRET) return text(501, "turn not configured");
      if (!(await hostStub(env, hostId).isHostOnline())) return text(404, "host offline");
      const creds = await turnCredentials(env, hostId);
      return creds ? json(200, creds) : text(501, "turn not configured");
    }

    return text(404, "not found");
  },
} satisfies ExportedHandler<Env>;
