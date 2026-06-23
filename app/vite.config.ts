import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

/**
 * Silence @vueuse/core 14.x's `/* #__PURE__ *\/` annotation warnings.
 *
 * Rollup only recognizes a `/* #__PURE__ *\/` comment when it sits
 * directly in front of a call / member expression. @vueuse/core 14.x
 * has a few cases where the comment is positioned in a way Rollup
 * cannot interpret, so it emits `PARSER_ERROR` / `INVALID_ANNOTATION`
 * warnings during bundle. This is an upstream issue, not something
 * we can fix from the consumer side — filter the noisy warnings
 * here so they don't drown real build errors. All other warnings
 * pass through to the default handler unchanged.
 */
function viteOnwarn(warning: unknown, defaultHandler: (w: unknown) => void) {
  const w = warning as { code?: string; id?: string };
  const code = w?.code;
  const id = w?.id ?? "";
  if (
    (code === "PARSER_ERROR" || code === "INVALID_ANNOTATION") &&
    id.includes("@vueuse/core")
  ) {
    return;
  }
  defaultHandler(warning);
}

export default defineConfig(async () => ({
  plugins: [vue(), tailwindcss()],

  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },

  build: {
    // TODO(follow-up): code-split vendor chunk (vue / @vueuse / pinia).
    // Main bundle is 745 kB — the proper fix is `manualChunks` to
    // split vendor from app code (better caching + faster TTI).
    // Tracked in ROADMAP V2-档2 code-splitting item.
    chunkSizeWarningLimit: 800,
    rollupOptions: {
      onwarn: viteOnwarn,
    },
  },
}));
