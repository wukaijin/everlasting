// vue-router — S4 pairing/PWA navigation skeleton (Step 2, design §5.1 / D1 / D6).
//
// Three routes form the PWA flow:
//   /pairing  — mobile pairing code entry (Step 4)
//   /nodes    — bound-PC node picker (Step 5)
//   /chat     — the existing chat experience (ChatView, lifted from App.vue)
//
// `beforeEach` gates pairing ONLY in the remote-served context. The
// daemon (browser-local) and Tauri Thin escape hatch never carry a
// device token AND have no `/api/v1/pairing/redeem` route on the
// daemon — a naive "no token → /pairing" guard would lock them out
// forever (P1-1, design §0 / D6). So the guard first discriminates the
// serving context via the bootstrap health body:
//
//   - `remoteId` present  → remote-served PWA → pairing gate applies.
//   - `daemonId`/undefined → daemon or Tauri context → straight to /chat.
//
// `?transport=tauri` skips the health probe entirely (`main.ts`
// bootstrap), leaving `__DAEMON_HEALTH__` undefined → treated as daemon
// context → direct /chat. This preserves the escape hatch.

import { createRouter, createWebHistory } from "vue-router";
import { hasDeviceToken } from "../transport/auth";
import { useNodesStore } from "../stores/nodes";

/** D6 (P1-1): is this SPA being served by the remote daemon?
 *  `main.ts` stores the bootstrap health body on `window.__DAEMON_HEALTH__`.
 *  The remote's health returns `remoteId` (`everlasting-remote/.../health.rs`);
 *  the daemon's returns `daemonId`. `?transport=tauri` skips the probe →
 *  undefined → treated as daemon context (no pairing needed). */
export function isRemoteContext(): boolean {
  const h = (
    window as unknown as { __DAEMON_HEALTH__?: Record<string, unknown> }
  ).__DAEMON_HEALTH__;
  return !!h && "remoteId" in h;
}

const routes = [
  {
    path: "/pairing",
    name: "pairing",
    component: () => import("../views/PairingView.vue"),
  },
  {
    path: "/nodes",
    name: "nodes",
    component: () => import("../views/NodeListView.vue"),
  },
  {
    path: "/chat",
    name: "chat",
    component: () => import("../views/ChatView.vue"),
  },
  {
    path: "/",
    redirect: () => {
      if (!isRemoteContext()) return "/chat"; // daemon/Tauri: straight into the app (current behavior)
      if (!hasDeviceToken()) return "/pairing"; // remote, unpaired
      return "/nodes"; // remote, paired, no node selected yet
    },
  },
  { path: "/:catchAll(.*)", redirect: "/" },
];

export const router = createRouter({
  history: createWebHistory(),
  routes,
});

// D6 / P1-1: pairing gate applies ONLY to the remote-served context.
// browser-local (daemon serves SPA) and Tauri Thin never have a token
// and the daemon has no redeem route — gating them to /pairing is a
// dead end. They go straight to /chat (current pre-router behavior).
//
// `useNodesStore()` is called INSIDE the guard (not at module top-level)
// so that pinia is guaranteed initialized — the guard only runs after
// `app.use(createPinia())` + `app.mount()` in main.ts. The store restores
// `selectedNodeId` from localStorage at construction time, so the guard
// doesn't need its own localStorage fallback.
router.beforeEach((to) => {
  if (to.name === "pairing") return true; // pairing page always reachable
  if (!isRemoteContext()) {
    // daemon/Tauri context: no pairing needed, chat is the app.
    return to.name === "chat" ? true : { name: "chat" };
  }
  // remote-served context: token + selected-node gating.
  if (!hasDeviceToken()) return { name: "pairing" };
  if (to.name === "chat" && !useNodesStore().selectedNodeId) {
    return { name: "nodes" };
  }
  return true;
});
