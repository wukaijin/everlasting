# z-index 阶梯 token 化:13 档语义 token 收编 41 处跨组件散值

## Goal

把散在 35 个文件、21 个不同值的 72 处 z-index 里**跨组件生效的 41 处**
收编为 style.css 的 13 档 `--z-*` 语义 token(值=现网事实,零行为变更);
组件内部的局部微层(2~70)留裸值但补契约注释;修正 design-tokens.md
里已与现实脱节的 modal z 约定(9998/9999/10000 → 实际 2000/2001/3000/5500)。

## 现状分带(2026-08-23 grep 实证,72 处)

| 带 | 值 | 站点 | 语义 |
|---|---|---|---|
| 局部微层 | 2/5/10/20/50/60/70 | 10 处 | 组件内/聊天浮动卡家族,跨文件契约靠注释 |
| 面板带 | 100 | ×4 | TracePanel、Mode/ModelSelect、WorktreeChip |
| 抽屉带 | 105/110 | ×2 | 移动抽屉遮罩 / Sidebar 抽屉 |
| 输入弹层 | 200 | ×3 | latency / token usage / TriggerMenu |
| 重覆盖面 | 999/1000 | ×3 | SubagentDrawer 遮罩+体、DiffModal |
| 确认弹窗 | 1100/1200 | ×3 | ConfirmDialog、DeleteWorktree / Yolo |
| Modal 家族 | 2000/2001 | ×16 | 8 个 modal 的 overlay/content 对 |
| portal 层 | 3000 | ×8 | modal 内 reka Select(6 处 !important)+ msg 菜单/tooltip |
| toast | 5500 | ×1 | ToastProvider |
| 顶层 | 9999 | ×2 | SessionList ctx 菜单、AppShell 旧 toast |

已知坏味(记录不处理):AppShell 旧 toast(9999)与 ToastProvider(5500)
两套并存;design-tokens.md "Modal Tokens" 表写的 backdrop 9998/content
9999/toast 10000 与现实全面脱节。

## Requirements

- **R1 阶梯落地(style.css @theme)**:13 档 `--z-*`,值=现网事实:
  `--z-raised:100`、`--z-drawer-overlay:105`、`--z-drawer:110`、
  `--z-input-pop:200`、`--z-sheet-overlay:999`、`--z-sheet:1000`、
  `--z-confirm:1100`、`--z-confirm-critical:1200`、`--z-modal-overlay:2000`、
  `--z-modal:2001`、`--z-over-modal:3000`、`--z-toast:5500`、`--z-top:9999`。
  注释写明"值即契约,新层级先查表再落位;禁止发明新散值"。
- **R2 扫编 41 处跨组件站点**为 `var(--z-*)`;`!important` 原样保留;
  值一个都不许变(纯重命名,零渲染差异)。
- **R3 局部微层 10 处**留裸值,逐处有契约注释(缺的补一行:盖过谁/被谁盖)。
- **R4 design-tokens.md**:新增 "Z-Index Ladder" 小节;修正 Modal Tokens
  表的 backdrop/content/toast 行为实际值;记录双 toast 坏味为后续输入。

## Acceptance Criteria

- [ ] `grep -rn "z-index: *(100|105|110|200|999|1000|1100|1200|2000|2001|3000|5500|9999)" app/src --include="*.vue"`
      仅剩 style.css 的 token 定义行,组件里零裸值命中;
- [ ] 41 处替换后 `pnpm test` 全绿 + `pnpm build`(vue-tsc+vite)零错;
- [ ] 零行为变更的自证:替换前后 `git diff` 中不允许出现数值变化
      (只允许 `数字;` → `var(--z-*)`;`!important` 保留);
- [ ] 运行时探针:开一个 modal(设置)读 overlay/content computed
      z-index = 2000/2001,证明 token 级联生效;
- [ ] 局部微层 10 处每处有注释;design-tokens.md 无 9998/10000 残留。

## Notes

- 不合并双 toast、不改任何层级关系——那是行为变更,另立任务。
- reka-ui portal 的 3000 !important 6 处:portal 内容挂 body 下,须压过
  2001 的 modal 内容;!important 是现状的一部分,原样保留。
