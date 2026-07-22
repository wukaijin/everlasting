// Vitest config for the frontend.
//
// We use the jsdom env (instead of happy-dom) for one reason:
// DOMPurify wants a real-ish DOM to run its hooks against, and
// jsdom has the broader feature surface. Test runtime is Node, NOT
// the Tauri webview — so this config is for unit tests of pure
// utilities and components that don't touch Tauri APIs. Tests that
// need Tauri (invoke / event) should mock them with vi.mock at the
// top of the test file.

import { defineConfig } from "vitest/config";
import vue from "@vitejs/plugin-vue";

export default defineConfig({
  plugins: [vue()],
  // P2.4 D3.3: mirror the vite build-time `define` of `__APP_VERSION__`
  // so `transport/health.ts`'s build-drift check has a real value in
  // tests (matches package.json version "0.1.0"). Without this, the
  // constant is `undefined` in vitest and the drift branch silently
  // no-ops, defeating the "build drift warns" test.
  define: {
    __APP_VERSION__: JSON.stringify("0.1.0"),
  },
  test: {
    environment: "jsdom",
    globals: true,
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    // Don't try to load .vue files as test files — they don't have
    // describe/it at the top level and would just fail to parse.
    exclude: ["**/node_modules/**", "**/dist/**"],
  },
  resolve: {
    alias: {
      // Match the `@` alias pattern other Vue/Tauri projects use;
      // the path resolves to `./src` so imports like
      // `import { foo } from "@/utils/x"` work.
      "@": new URL("./src", import.meta.url).pathname,
    },
  },
});
