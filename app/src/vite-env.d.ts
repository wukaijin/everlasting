/// <reference types="vite/client" />

// P2.4 D3.3: build-time injected app version (vite `define` in
// vite.config.ts). Used by `transport/health.ts` for the Q5
// daemon/frontend build-drift warn-only check. Always a string at
// runtime; declared `const` so narrowing works.
declare const __APP_VERSION__: string;
