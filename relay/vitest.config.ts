import { cloudflareTest } from "@cloudflare/vitest-pool-workers";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [
    cloudflareTest({
      wrangler: { configPath: "./wrangler.toml" },
      miniflare: {
        // Small limits so the tests can hit them quickly.
        bindings: {
          MAX_PHONES_PER_HOST: "2",
          MAX_FRAME_BYTES: "1024",
          HOST_AUTH_TIMEOUT_MS: "800",
          HOST_IDLE_TIMEOUT_MS: "2000",
          PHONE_IDLE_TIMEOUT_MS: "1500",
          MAX_PENDING_HOSTS: "2",
          CONNECT_BURST: "5",
          CONNECT_PER_MINUTE: "5",
          TURN_URLS: "turn:turn.test:3478?transport=udp,turns:turn.test:5349",
          TURN_SHARED_SECRET: "test-only-secret",
        },
      },
    }),
  ],
  test: {
    testTimeout: 15_000,
  },
});
