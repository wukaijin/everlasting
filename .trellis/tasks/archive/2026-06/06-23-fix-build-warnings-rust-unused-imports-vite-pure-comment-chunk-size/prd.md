# 修复 build warnings — Rust 4 unused + Vite 2 pure comment + 1 chunk size

## Goal

`pnpm build` + `cargo check` 现在各自有 warning 输出,清掉它们保持 CI / 本地构建输出干净。

| Warning 来源 | 数量 | 类型 | 修法 |
|---|---|---|---|
| `app/src-tauri/src/agent/permissions/mod.rs` | 4 | `unused_imports` | 删 3 个真没用的 re-export,留 1 行(其余 re-export 仍被外部消费) |
| `@vueuse/core@14.3.0` 库内 `/* #__PURE__ */` 注释位置 | 2 | Rollup `PARSER_ERROR` / annotation 警告 | `vite.config.ts` 加 `onwarn` 过滤 |
| 主 bundle 745 kB > 500 kB 阈值 | 1 | Vite chunk size 提示 | 调 `chunkSizeWarningLimit: 800` + 留 follow-up TODO |

合计 **7 条 warning 全部清掉**,不引入新依赖、不改公共 API、不改构建产物行为。

## What I already know

### 现状(`pnpm build` + `cargo check` 实测)

```
warning: unused import: `ask::ASK_TIMEOUT`         mod.rs:97
warning: unused import: `AuditKind`                mod.rs:98
warning: unused imports: `PendingAsk` and `register_ask`  mod.rs:102
warning: unused imports: `Risk` and `risk_for_tool`       mod.rs:103
```

> 注:line 98 `record_message_resend_audit` + `record_tool_executed_audit` 实际**有**外部使用(line 警告只挑了 `AuditKind`),不删;
> line 102 `cancel_session_asks` / `new_permission_store` / `resolve_ask` / `PermissionStore` 实际有外部使用,不删;
> line 103 `Decision` / `PermissionContext` / `PermissionResponse` 实际有外部使用,不删。

### 外部消费清单(决定哪些 re-export 要留)

| Re-export | 外部使用? | 证据 |
|---|---|---|
| `ASK_TIMEOUT` | ❌ 不用 | `permissions/ask.rs:20` 定义;`ask.rs:234/414` 内部用 `use super::ASK_TIMEOUT`-free 直接 `use ask::ASK_TIMEOUT`(其实在同 module 内,所以 `mod.rs` 的 re-export 路径上没人走)|
| `AuditKind` | ✅ 用 | `permissions/tests_audit.rs:3` `use crate::agent::permissions::AuditKind` |
| `register_ask` | ❌ 不用 | `permissions/ask.rs` 走 `use super::store::register_ask`(同 module 内,不走 mod.rs re-export) |
| `PendingAsk` | ❌ 不用 | `permissions/store.rs:50/74` 内部用;无外部消费 |
| `Risk` | ✅ 用 | `subagent/sink.rs:1082/1296` `crate::agent::permissions::Risk::High`;`tests_check.rs:11` |
| `risk_for_tool` | ✅ 用 | `tests_types.rs:3` + `tests_check.rs:10` `use crate::agent::permissions::risk_for_tool` |

**结论**:只删 `ASK_TIMEOUT` / `PendingAsk` / `register_ask` 三处,保留 `AuditKind` / `Risk` / `risk_for_tool`。

### Vite 现状

- `app/vite.config.ts` 27 行,极简,无 `build` 段配置
- 主 bundle 745.61 kB(单 chunk,无 vendor 拆分)
- @vueuse/core 是 14.3.0,已知 PURE 注释位置 bug(本项目无法控制上游)

## Requirements

### 必改

| 改动 | 文件 | 范围 |
|---|---|---|
| 删 3 个 unused re-export | `app/src-tauri/src/agent/permissions/mod.rs` | line 97 删 `pub use ask::ASK_TIMEOUT;`;line 102 删 `PendingAsk,` 与 `register_ask,` |
| 加 `build.rollupOptions.onwarn` 过滤 @vueuse/core PURE 注释 | `app/vite.config.ts` | 写个 `onwarn(warning, defaultHandler)` 函数,匹配 `code === 'PARSER_ERROR' / 'INVALID_ANNOTATION'` 且 warning 文本含 `@vueuse/core` 时不调 defaultHandler(走静默),其他照旧 |
| `chunkSizeWarningLimit: 800` + follow-up TODO 注释 | `app/vite.config.ts` | 加到 `build` 段,跟一条 `// TODO: code-split vendor (vue / @vueuse / pinia) — see ROADMAP.md V2-档2-?` |

### 必不动

- 任何 Rust 模块的 `pub use` 中**实际有外部消费的项**(`AuditKind` / `Risk` / `risk_for_tool` / `cancel_session_asks` / `new_permission_store` / `resolve_ask` / `PermissionStore` / `Decision` / `PermissionContext` / `PermissionResponse` / `record_message_resend_audit` / `record_tool_executed_audit` / `mode::filter_tools_for_mode` / `mode::mode_system_prefix` / `payload::PermissionAskPayload` / `check::check`)
- `permissions/mod.rs` 的其他 re-export line(`audit::` / `check::` / `mode::` / `payload::` 四行)
- `permissions/ask.rs` 内部 `use super::store::register_ask` 不变(已走的 `super::` 路径不依赖 mod.rs re-export)
- 后端 IPC / Tauri command / 数据库 / store / 任何 Vue 组件
- `package.json` 依赖列表(不升级 @vueuse/core — 该 warning 在 14.x 多版本都存在,留给上游)

## Acceptance Criteria

- [ ] `pnpm build` 输出 0 warning(不计 `pnpm` 自身 scaffold 提示)
- [ ] `cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check` 输出 0 warning
- [ ] `cargo test --lib` 全绿(确认删 re-export 没破坏测试的 `use crate::agent::permissions::AuditKind` 等路径)
- [ ] `mod.rs` 删后剩余 re-export 共 13 行(原 16 行),compile 通过
- [ ] `vite.config.ts` 加 `onwarn` 过滤函数 + `chunkSizeWarningLimit: 800` + TODO 注释
- [ ] 不引入新依赖
- [ ] 不改 `package.json` / `Cargo.toml`

## Definition of Done

- 4 个 Rust unused warning + 2 个 @vueuse pure warning + 1 个 chunk size warning 共 7 条全部消失
- `cargo check` / `pnpm build` / `cargo test --lib` 全绿
- commit message 形如 `chore: fix build warnings (rust unused re-exports + vite onwarn + chunk size)`
- ROADMAP / DEBT.md / 任何 spec 不动(本次纯杂务)

## Technical Approach

### Rust 改法(`permissions/mod.rs`)

```rust
// line 97:删
- pub use ask::ASK_TIMEOUT;
// line 98:保留
  pub use audit::{record_message_resend_audit, record_tool_executed_audit, AuditKind};
// line 99-101:保留
  pub use check::check;
  pub use mode::{filter_tools_for_mode, mode_system_prefix};
  pub use payload::PermissionAskPayload;
// line 102:删 PendingAsk, register_ask
- pub use store::{cancel_session_asks, new_permission_store, register_ask, resolve_ask, PendingAsk, PermissionStore};
+ pub use store::{cancel_session_asks, new_permission_store, resolve_ask, PermissionStore};
// line 103:保留
  pub use types::{risk_for_tool, Decision, PermissionContext, PermissionResponse, Risk};
```

### Vite 改法(`vite.config.ts`)

```ts
import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

/**
 * 过滤 @vueuse/core 14.x 内部 `/* #__PURE__ *\/` 注释位置警告
 * (上游 rollup 注释解析问题,见 https://github.com/vueuse/vueuse/issues/xxxx)。
 * 命中条件:warning.code 为 PARSER_ERROR / INVALID_ANNOTATION 且路径含 @vueuse/core。
 * 其他 warning 一律走 defaultHandler。
 */
function viteOnwarn(warning: any, defaultHandler: (w: any) => void) {
  const code = warning?.code;
  const id: string = warning?.id ?? "";
  if (
    (code === "PARSER_ERROR" || code === "INVALID_ANNOTATION") &&
    id.includes("@vueuse/core")
  ) {
    return; // 静默
  }
  defaultHandler(warning);
}

export default defineConfig(async () => ({
  plugins: [vue(), tailwindcss()],

  clearScreen: false,
  server: { /* 保持 */ },

  build: {
    // TODO: code-split vendor chunk (vue / @vueuse / pinia) — see ROADMAP V2-档2-?
    chunkSizeWarningLimit: 800,
    rollupOptions: {
      onwarn: viteOnwarn,
    },
  },
}));
```

## Decision (ADR-lite)

**Context**:用户选了「提阈值 + 留 follow-up TODO」,不立即做 manualChunks;@vueuse pure warning 用 onwarn 过滤。

**Decision**:

* **ADR-1 chunk size 修复档位 = 提阈值 + TODO(不立即拆)** — 主 bundle 745 kB 主要来自 vue + @vueuse + pinia + Tauri API,manualChunks 需要把 vendor 边界 / cache 策略想清楚,不适合塞在杂务任务里;留 follow-up TODO + ROADMAP 链接
* **ADR-2 @vueuse pure warning = onwarn 过滤** — 上游库 bug,本项目无法修;只静默来自 `@vueuse/core` 的 `PARSER_ERROR` / `INVALID_ANNOTATION`,其他 warning 照旧走 defaultHandler(避免误屏蔽真问题)
* **ADR-3 Rust re-export 删除 = 3 个真没用** — 只删 `ASK_TIMEOUT` / `PendingAsk` / `register_ask`(无外部消费);其余 13 行 re-export 全部保留(测试代码 + 跨模块 import 大量依赖 `crate::agent::permissions::Xxx` 短路径)

**Consequences**:

- (+) 7 条 warning 全部消失
- (+) Rust 端 0 改动测试代码(测试仍走 `crate::agent::permissions::AuditKind` 等路径)
- (+) Vite 配置改 1 文件,改动局部
- (-) chunk size 真问题留 follow-up(用户已 confirm)
- (-) @vueuse 上游 bug 仍存在,版本升级时这个 onwarn 过滤可能需要调整

## Open Questions

*(无 — 3 项决策已与用户 confirm)*

## Out of Scope

- 改 `package.json` 升级 @vueuse/core(留给上游修复)
- 改 Vite 配置做 manualChunks vendor 拆分(留 follow-up)
- 改任何后端 Rust 模块(只动 `permissions/mod.rs` re-export 行)
- 改 `docs/ROADMAP.md` / `.trellis/reviews/DEBT.md` / 任何 spec(本次纯杂务)
- 改前端 Vue 组件 / store / IPC
- 改 `pnpm tauri dev` / `pnpm tauri build` 脚本

## Technical Notes

### 文件位置

```
app/
├── src-tauri/src/agent/permissions/mod.rs    (改:删 3 个 re-export token)
└── vite.config.ts                            (改:加 build 段 + onwarn 函数)
```

### cargo check 提醒(WSL)

`cargo check` / `cargo test` 需 `PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"`,见 `CLAUDE.md` Common Commands 节。

### 不需要新增 .test.ts

本次纯杂务,删除的是无外部消费的 re-export,既无新增逻辑也无回归风险。
