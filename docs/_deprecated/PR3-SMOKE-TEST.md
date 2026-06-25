# PR3 Manual Smoke Test — PermissionModal + ⑨ ↔ `permission:ask` IPC

> 配套任务:`06-12-a2-b7-permission-and-mode` PR3(前端
> `PermissionModal` + `usePermissionsStore` + ⑨ ↔ `permission:ask`
> IPC 联通)。Backend ⑨ 关 5-tier 已在 PR1(commit `442fb3d`)落
> 地;PR3 只动前端 + IPC 联通。
>
> 跑通这条 case 之后,任务全部 30 条 AC(后端 10 + 前端 4 + 持久
> 化 2 + PermissionModal 14)即视为"代码侧"全部覆盖。**最终
> DoD 仍以用户在 dev 跑过的截图为准**。

## 前置

```bash
# Backend 已经在 main 上,数据库迁移跑过一遍。
cd app && pnpm tauri dev
```

- 起一个项目(任意 git repo 即可)
- 在 LLM 配置里填一个真实的 `ANTHROPIC_API_KEY` (或 `OPENAI_*`)
- 创建一个新 session(默认 mode = Chat)

## Case 1: 首次用某 tool → PermissionModal 弹出

**目的**:验证 ⑨ 关 Tier 3 路径 + frontend listener + modal mount。

1. 选 Chat 模式(默认)
2. 在输入框里让 LLM 调一个 `write_file`(例:"create a test
   file `~/Documents/test-pr3.txt` with the content 'hello PR3'")
3. 预期:
   - LLM 流式输出文字,直到 emit `tool_use` 块
   - **PermissionModal 居中弹出**:`width: min(560px, 90vw)`,
     backdrop 4px blur
   - 头部 56x56 shield icon 容器,emerald tint(因为
     `write_file` 是 `medium` risk),标题"需要权限:写文件"
   - 命令预览块带 terminal icon(左)+ `<pre>` 渲染
     `{ "path": "...", "content": "..." }` JSON + copy icon(右)
   - 副标签 "工具类别: `write_file` · 风险等级: 中"(中文字 + emerald 圆点)
   - 3 按钮等宽,顺序 "拒绝 / 仅一次 / 始终允许",始终允许
     是 `--color-accent` 背景(蓝色)
   - 默认 focus 在"仅一次"按钮(普通 risk,Enter = 仅一次)
4. 点 "仅一次"
5. 预期:
   - Modal 关闭
   - LLM 收到 `tool_result`(is_error: false),`write_file` 执
     行成功
   - 实际文件 `~/Documents/test-pr3.txt` 创建,内容 "hello PR3"
   - 该 session 的 `session_audit_events` 表里有 1 条
     `tool_permission_ask` + 1 条 `tool_allowed`

## Case 2: "始终允许" → 后续不弹窗

**目的**:验证 ⑨ 关 Tier 3 "始终允许" 写表 + 后续跳过弹窗。

1. 在 Case 1 同一 session 里,再发一条让 LLM 调 `write_file` 创
   建 `~/Documents/test-pr3-2.txt` 的消息
2. PermissionModal 再次弹出(同 Case 1)
3. 点 "始终允许"
4. 预期:
   - Modal 关闭,文件创建
   - 同一 session **再** 发 `write_file` 任务(第 3 条同 tool 调
     用)
5. 预期:
   - **不再弹窗**(直接从 `session_tool_permissions` 查到 "始终
     允许")
   - LLM 直接执行,文件创建成功
   - `session_audit_events` 多 1 条 `permission_granted` +
     多 1 条 `tool_allowed`(但没有 `tool_permission_ask` — 跳
     过了弹窗)
6. 验证 `session_tool_permissions` 表:
   ```bash
   sqlite3 ~/.local/share/everlasting/db.sqlite3 \
     "SELECT * FROM session_tool_permissions WHERE tool_name='write_file'"
   ```
   应有 1 行:`(session_id, 'write_file', 'tool', NULL, <timestamp>)`

## Case 3: 120s 不响应 → 自动拒绝 + toast

**目的**:验证 ⑨ 关 Tier 3 120s timeout 路径 + frontend 镜像 timer
+ toast。

1. 在新 session 里(避免 Case 2 留下的"始终允许"),让 LLM 调
   `write_file`
2. PermissionModal 弹出
3. **不要点任何按钮**,等 120s
4. 预期(在 2 分钟内任一时刻):
   - **不要** 关 modal,让前端 store 自己的 120s timer 先到
     (前端 + 后端 timer 都到 120s, race OK)
   - 弹 toast: "权限询问已超时,已自动拒绝"(level = warn)
   - Modal 自动关闭
   - LLM 收到 `tool_result` with content:
     `permission timed out after 120s, treat as denied`
     (`is_error: true`)
   - LLM 自我修正(可能会说"用户超时,换一种方式"或重新 plan)
   - `session_audit_events` 多 1 条 `permission_timeout` + 1 条
     `tool_denied`

(替代:如果你不想等 120s,可以临时把 `app/src/stores/permissions.ts`
里的 `ASK_TIMEOUT_MS` 改成 `10000` 测一遍再改回。)

## Case 4(可选):Critical risk 时 Enter 默认 拒绝

**目的**:验证 audit §6.2 "critical Enter 改'拒绝'"。

1. 切到 Chat 模式(默认)
2. 让 LLM 调 `shell` 跑一个 Tier 2 不命中的普通命令(例
   `echo hello`) — 这会触发 Tier 3 高 risk 弹窗(risk = high)
3. 弹窗出现,**不点按钮**
4. 让 LLM 再调 `shell` 跑一个会触发 critical 的命令(实际是
   Tier 2 命中,直接 deny,不弹窗 — 不适合此 case。**MVP 静态
   风险分级 `shell` = high 而不是 critical**,所以严格意义上
   "critical risk 弹窗" 现在不会被触发)
5. 替代:手动改后端 `agent/permissions::risk_for_tool` 把
   `shell` 改成 `Risk::Critical`,重启 dev,再让 LLM 跑
   `echo hello` 验证 Enter 焦点在"拒绝"按钮(UI 上可以看到
   "拒绝" 按钮上有 focus 描边)
6. 改回 `Risk::High`,提交验证

## Case 5(可选):Esc / X / 遮罩点击 = 拒绝

**目的**:验证 Q6 "关闭 = 拒绝"事实标准。

1. 让 LLM 触发 PermissionModal(risk = high)
2. 弹窗出现,**按 Esc**
3. 预期:Modal 关闭 + LLM 收到 `is_error: true` + audit
   `tool_denied`
4. 重新触发 modal,点右上角 X
5. 预期:同上
6. 重新触发 modal,点击 modal **外部**(backdrop)
7. 预期:同上(注意:点击 modal 卡片内部不触发 cancel — 验证
   `@click.self` 行为正确)

## AC 覆盖矩阵

| AC | Case |
|---|---|
| 后端 #1: legacy sessions backfill `mode = 'chat'` | 已有 PR1 验证 |
| 后端 #2: Plan 模式 write_file → tool_result text 错误 | PR1 + 后续(本任务不动) |
| 后端 #3: Review 模式 read_file 放行 | PR1 + 后续 |
| 后端 #4: Chat 模式 `rm -rf /` → Tier 2 deny | PR1 + 后续 |
| 后端 #5: Yolo 模式 `rm -rf /` → 静默 deny | PR1 + 后续 |
| 后端 #6: Yolo 模式 `echo hello` → 执行 | PR1 + 后续 |
| 后端 #7: 首次用 tool → emit `permission:ask` | Case 1 |
| 后端 #8: "始终允许" → 写表 + 后续不弹 | Case 2 |
| 后端 #9: "仅一次" → 不写表 + 下次再弹 | Case 1 (再发同 tool) |
| 后端 #10: "拒绝" → tool 跳过 + is_error 回 LLM | Case 5 |
| 后端 #11: root 启动 + Yolo → 拒 | PR1 + 后续(本任务不动) |
| 后端 #12: 120s 未响应 → 自动 deny | Case 3 |
| 前端 #1: ChatInput 改 flex,左侧 mode badge | PR2 已落地 |
| 前端 #2: 4 档 mode 选项可见 | PR2 已落地 |
| 前端 #3: 选 Yolo 弹 YoloConfirmModal | PR2 已落地 |
| 前端 #4: Shift+Tab 切换 mode + streaming 灰显 | PR2 已落地 |
| 持久化 #1: session 重启后 mode 保留 | PR1 + 后续 |
| 持久化 #2: 删除 session 时 cascade 清 permissions | PR1 + 后续 |
| PermissionModal #1: 居中 + 4px backdrop-blur | Case 1 |
| PermissionModal #2: 56x56 shield icon 容器 + risk tint | Case 1 |
| PermissionModal #3: Critical 3px 红左 border + shield-x | Case 4(可选) |
| PermissionModal #4: `JSON.stringify(_, null, 2)` 渲染在 `<pre>` | Case 1 |
| PermissionModal #5: terminal + copy icon + 2s toast | Case 1 |
| PermissionModal #6: 副标签 + risk 颜色点 + 中文 label | Case 1 |
| PermissionModal #7: 3 按钮等宽 33% + 顺序 | Case 1 |
| PermissionModal #8: 默认 focus + Enter = 仅一次 | Case 1(普通 risk) |
| PermissionModal #9: Esc / X / 遮罩 = 拒绝 | Case 5(可选) |
| PermissionModal #10: "始终允许" → INSERT INTO | Case 2 |
| PermissionModal #11: "仅一次" → 不写表 | Case 1 (再发同 tool) |
| PermissionModal #12: "拒绝" → 后续 tool_use 继续 | Case 5 (下一个 tool 仍按 ⑨) |
| PermissionModal #13: Stop(C1) → 整轮终止(跟 deny 区分) | C1 已验证 |
| PermissionModal #14: 不渲染 checkbox | 模板不含 checkbox,Case 1 视觉验证 |
| PermissionModal #15: 同一 turn 多 tool_use 串行 | 让 LLM 一次发 2 个 tool_use(罕见,Case 1 验证 1 个即可) |
| PermissionModal #16: reka-ui `:deep()` gotcha | DevTools 验证 `.permission-modal__*` 样式生效 |
| PermissionModal #17: 120s 超时 → 自动 deny | Case 3 |

## 已覆盖(自动化测试)

- 28 个新 vitest 测试 (PermissionModal.test.ts + permissions.test.ts)
- 20 个新 cargo 测试 (permissions module, PR1 已有)
- `pnpm build` (vue-tsc) 全过
- `cargo check` 全过

## 已知 pre-existing 问题(非本 PR 引入)

- `streamController.test.ts` 4 个 unhandled rejection 来自主仓
  pre-existing 测试 setup,跟本 PR 无关。会在 8-PR6 治理。

## 完成标准

5 个 case 跑通 + AC 矩阵全 ✓ + 28 个 vitest 全过 = PR3 任务完成。
