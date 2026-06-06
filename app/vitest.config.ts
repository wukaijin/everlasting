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
