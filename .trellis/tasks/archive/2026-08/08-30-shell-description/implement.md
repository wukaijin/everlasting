# implement — 执行清单

> PR 顺序执行，每步验证过了再进下一步。PR1（后端）/ PR2（抽取+helper）/
> PR3（ShellCard）彼此独立可 revert；PR3 依赖 PR2 的 helper 与
> PermissionActions。

## PR1 — 后端 schema

- [ ] `app/src-tauri/src/tools/shell.rs::definition()`：input_schema 增加可选
      `description`（string，文案按 design §2.1）；tool description 末尾加填写
      指引；`required` 不变。
- [ ] `app/src-tauri/src/tools/run_background_shell.rs::definition()`：同上。
- [ ] 测试（`tools/tests_shell.rs` + background_shell 既有测试文件）：
      definition schema 断言；execute 带畸形 description 与不带结果一致。
- [ ] 验证：
      ```bash
      cargo test -p everlasting --lib
      cargo test -p everlasting-remote   # 快速冒烟
      ```
      permissions 模块测试零改动全绿（AC5）。

## PR2 — PermissionActions 抽取 + chip helper + drawer/审批卡接线

- [ ] 新建 `app/src/components/chat/PermissionActions.vue`：从
      `PermissionAskBody.vue:229-273` 原样搬 interactive actions 块（4 按钮 +
      showFeedback/feedback + submitFeedback/cancelFeedback +
      allowAlwaysLabel 分叉），props `{ ask, onRespond, hideAllowAlways? }`；
      纯搬运，不加新逻辑。
- [ ] `PermissionAskBody.vue`：template 改引 PermissionActions；其余结构
      （head/reason/path/outcome/historical）不动；**新增** shell 家族命令行
      + 意图行（`isShellFamilyTool(ask.toolName)` 门控，双模式同效；命令行
      pre-wrap + max-height 滚动，意图行 muted、缺失不渲染）。
- [ ] `app/src/utils/messageFormat.ts`：新增 `isShellFamilyTool(name)` +
      `toolHeaderChip(name, input)`（优先级链 design D1）。
- [ ] `DrawerToolCallCard.vue`：chip 换 `toolHeaderChip(call.name, call.input)`。
- [ ] 测试：messageFormat 优先级矩阵；PermissionAskBody 既有用例**零改动**
      全绿（AC4）+ 增补 shell 命令行/意图行分支；DrawerToolCallCard chip
      分支。
- [ ] 验证：`cd app && pnpm test`；type-check / lint。

**回滚点**：PR2 内「抽取」与「PermissionAskBody 新增行」如需拆小可分两个
commit；行为保持以既有测试为准绳。

## PR3 — ShellCard 组件 + resolver 接线

- [ ] 新建 `app/src/components/chat/ShellCard.vue`（结构对齐 EditFileCard）：
      - ToolCallHeader：chip = `toolHeaderChip`；run_background_shell 加
        background pill；statusText 增"等待审批"分支（design D3）。
      - 命令块（`$` 前缀 + pre-wrap + max-height 200px 滚动 + cwd 次行）。
      - 一体化审批：pendingAsk（接线照抄 `EditFileCard.vue:148-163`）时渲染
        风险条（`RISK_LABEL_CN` / `RISK_META`）+ `<PermissionActions>`；
        不渲染独立"需要权限"容器。
      - 输出：done → ToolOutputBody 折叠；error → 红框 pre 常显
        （`extractToolResultDisplay` + `truncateOutput(500)`）。
      - 降级：command 缺失/非 string → ToolInputBody 兜底。
      - 全 design token、0 hex；移动端 320px 复核（pre-wrap 自适应，无横向
        滚动）。
- [ ] `MessageItem.vue` resolver：`shell` / `run_background_shell` 分支 →
      ShellCard（对齐 EditFileCard 分支写法；tool name 常量放既有常量处）。
- [ ] 测试（新 `ShellCard` 测试文件，mock permStore 仿 EditFileCard 测试法）：
      chip 三级兜底 / 命令块 / cwd 行 / background pill / 等待审批态 +
      4 按钮 + onRespond / 命令不重复 / done 折叠 / error 常显 / 畸形降级
      （design §4 矩阵）。
- [ ] 验证：`cd app && pnpm test`。

## 收尾验证（最后一轮 2.2 full-scope）

- [ ] `cd app && pnpm test` 全绿。
- [ ] type-check + lint（项目既有入口）通过。
- [ ] `PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
      cargo test -p everlasting --lib` 全绿。
- [ ] 样式走查：daemon serve dist + headless 截图（或
      `scripts/ui-review.sh --screenshots-only`）人工过 ShellCard 三态
      （done/error/待审批）与 drawer 审批卡；VLM 结论仅参考，以代码为准。
- [ ] 对照 prd.md AC1-AC6 逐条勾验。
