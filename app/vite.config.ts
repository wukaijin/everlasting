import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";
import { VitePWA } from "vite-plugin-pwa";

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
  plugins: [
    vue(),
    tailwindcss(),
    // S4 PWA shell (design §4): manifest + app-shell precache SW.
    // - registerType autoUpdate: new SW takes over on controllerchange.
    // - injectRegister auto: vite-plugin-pwa injects the SW registration
    //   script into index.html at build time (no manual register code).
    // - workbox precaches static assets (js/css/html/fonts/icons) for
    //   offline app-shell load; data (API/SSE) is always network-only
    //   (runtimeCaching empty — the app is fundamentally online).
    // - navigateFallbackDenylist excludes /api so 404s return real
    //   errors instead of being silently rewritten to index.html.
    // - devOptions disabled: dev server must not register a SW (would
    //   cache-break HMR). SW only exists in production builds.
    VitePWA({
      registerType: "autoUpdate",
      injectRegister: "auto",
      manifest: {
        name: "Everlasting",
        short_name: "Everlasting",
        description: "远程遥控你的 AI agent",
        theme_color: "#0a0e14",
        background_color: "#0a0e14",
        display: "standalone",
        start_url: "/",
        icons: [
          { src: "/icons/192.png", sizes: "192x192", type: "image/png" },
          { src: "/icons/512.png", sizes: "512x512", type: "image/png" },
          {
            src: "/icons/512-maskable.png",
            sizes: "512x512",
            type: "image/png",
            purpose: "maskable",
          },
        ],
      },
      workbox: {
        globPatterns: ["**/*.{js,css,html,woff2,png,svg}"],
        navigateFallback: "/index.html",
        navigateFallbackDenylist: [/^\/api\//],
        runtimeCaching: [],
      },
      devOptions: { enabled: false },
    }),
  ],

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
    // Main app chunk was ~1.08 MB. Split stable third-party code into
    // vendor chunks so the app chunk stays small: better HTTP caching
    // (vendor rarely changes), faster TTI for the critical path.
    // The chunks below are grouped by runtime role:
    //   - vendor-vue:      framework + state (vue, @vue/*, pinia)
    //   - vendor-reka:     reka-ui component library (heavy)
    //   - vendor-editor:   CodeMirror editor core (ChatInput)
    //   - vendor-markdown: markdown rendering + sanitize
    //   - vendor-icons:    icon sets (heroicons / lucide)
    //   - vendor-misc:     everything else (diff / fuzzysort / tauri api)
    // A module is matched via its node_modules package root so pnpm
    // symlink paths (node_modules/.pnpm/<pkg>/node_modules/<pkg>) work.
    chunkSizeWarningLimit: 800,
    rollupOptions: {
      output: {
        manualChunks(id: string) {
          if (!id.includes("node_modules")) return;
          const pkg = id.split("node_modules/").pop() ?? "";
          if (
            pkg.startsWith("vue") ||
            pkg.startsWith("@vue/") ||
            pkg.startsWith("pinia")
          )
            return "vendor-vue";
          if (pkg.startsWith("reka-ui")) return "vendor-reka";
          if (pkg.startsWith("@codemirror")) return "vendor-editor";
          if (
            pkg.startsWith("marked") ||
            pkg.startsWith("highlight.js") ||
            pkg.startsWith("dompurify")
          )
            return "vendor-markdown";
          if (pkg.startsWith("@heroicons") || pkg.startsWith("@lucide"))
            return "vendor-icons";
          return "vendor-misc";
        },
      },
      onwarn: viteOnwarn,
    },
  },
}));
