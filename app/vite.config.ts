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
 *
 * Also filtered: the `dynamic import will not move module into
 * another chunk` warning (emitted by the `vite:reporter` plugin).
 * A few store modules use `await import(...)` as an intentional
 * cycle-breaker at module-init time (`questionCards` ↔ `chat` /
 * `chatModeActions` — see those files). Because those modules are
 * also statically imported by components, Rollup correctly notes the
 * dynamic import can't produce a separate chunk; that's fine here —
 * the dynamic import exists to defer evaluation, not to code-split.
 * This is a build-time informational notice, not a defect.
 */
function viteOnwarn(warning: unknown, defaultHandler: (w: unknown) => void) {
  const w = warning as {
    code?: string;
    id?: string;
    plugin?: string;
    message?: string;
  };
  const code = w?.code;
  const id = w?.id ?? "";
  const msg = w?.message ?? "";
  if (
    code === "PLUGIN_WARNING" &&
    w?.plugin === "vite:reporter" &&
    msg.includes("will not move module into another chunk")
  ) {
    return;
  }
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

  // P2.4 D3.3: inject the app build version so `health.ts` can
  // warn on daemon/frontend build drift (Q5 warn-only check). Read
  // from package.json — vite replaces this statically at build time,
  // so the prod bundle carries a string literal (no runtime cost).
  // Dev server gets the same value (no separate dev build version).
  define: {
    __APP_VERSION__: JSON.stringify(
      // @ts-expect-error injected by the vite define at build; the
      // fallback is defensive for non-build contexts.
      (await import("./package.json", { with: { type: "json" } })).default
        .version,
    ),
  },

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
