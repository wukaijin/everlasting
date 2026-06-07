# frontend polish: tool output fold, text color, scroll guard, markRaw

## 背景

单 agent chat 跑长 session 时的几处 UX/perf 收尾：用户说正文偏白、工具
output 永远展开吃掉屏幕、切 session 不自动滚底、5000 条渲染压力。5 条
需求一次过。

## 需求

1. **工具 output 可折叠，默认折叠**（含 error；header 已带红/红 icon/error
   字样就够了）
2. **`--color-text-primary` 调暗**：`#e5e7eb` → `#cbd5e1`（slate-300）
3. **切 session 自动滚到底**；同 session 流式输出只在用户靠近底部时跟滚
4. **5000 条渲染**：先做 Layer 1+2（output 折叠 + markRaw 非流式 message
   的不可变 payload 数组），Layer 3（IntersectionObserver）实测后再开
5. **3 个 atomic commit**

## 关键决策（grill 阶段敲定）

| 决策点 | 选项 | 结果 |
|---|---|---|
| 5000 条方案 | A 懒 mount / B 虚拟滚动 / C 服务端分页 | 用户否决 ABC；改 Layer 1+2 (markRaw 不可变字段) |
| Layer 3 IO 范围 | 现在做 / 复测再开 | 复测再开 |
| `--color-text-primary` 范围 | 全局 / 用户气泡 / 都不调 | 只调全局 `--color-text-primary` |
| 调暗目标 | `#cbd5e1` / `#a8b0bd` / `#b8c0cc` | `#cbd5e1` |
| Error output 状态 | 默认展开 / 全默认折叠 | 全默认折叠（红 bar + x icon + error text 已够） |
| output summary | "output" / "output · 体积" / 体积+行数 | "output · 体积" |
| input caret 同步 | 加 / 不动 | 加（input 本来就有，output 跟齐） |
| 自动滚策略 | 加 isNearBottom / 只修跨 session | 加 isNearBottom (80px) |
| 提交粒度 | 1 / 3 / 5 commit | 3 commit |

## 落地

| Commit | Hash | 内容 |
|---|---|---|
| `style(ui): dim --color-text-primary to slate-300 + density tweaks` | `dc52988` | `style.css` + 用户已改的 ChatPanel/MessageItem 密度微调 |
| `feat(chat): fold tool output by default + scroll-guard auto-follow` | `a93f417` | `ToolCallCard.vue` output 折叠 + `MessageList.vue` 2 个 watcher |
| `perf(chat): markRaw immutable deep-payload arrays on messages` | `4e502b8` | `streamController.ts` rehydrate + done/error markRaw |

## 风险 / 留作未来

- **Layer 3**（IntersectionObserver 占位）按"复测再开"决定未做。如果
  5000 条实测仍卡，再补。
- **`flush: "pre"` 的边界**：当前假定所有会让 content-hash 变化的
  reactive 写都走 pre-flush。如果未来引入批量更新 messages 的代码
  路径，要确认那个路径也走 pre 或者额外处理。
- **markRaw 再水化**：rehydrateMessages + done/error 之后 payload
  数组都 markRaw。极端情况：用户在 streaming 期间切走再切回，如果
  controller 缓存命中且还在 streaming，会出现"已 markRaw 的数组被
  push"——但当前 `last` 永远是当前正在 streaming 的 message 引用，
  markRaw 只在 `done` / `error` 之后才设，路径无交集。
- **isNearBottom (80px) 阈值**：用户若在底部但视觉上还有 80+px 间距
  （如 chat padding 较大），会判定为 "不近底部" 不滚。实测再调。

## 验证清单（手工）

1. `pnpm tauri dev` 启动
2. 工具 output 默认折叠，summary `output · 12.3K chars` 之类
3. 切 session 滚底；老消息阅读不被新流式打断
4. 助手气泡正文明显变暗（slate-300 vs 原 e5e7eb）
5. 长 session（如有）消息列表滚动流畅
