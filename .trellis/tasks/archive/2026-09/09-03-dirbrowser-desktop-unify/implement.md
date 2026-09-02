# implement.md — 09-03-dirbrowser-desktop-unify

> 执行计划。PRD 见同目录 `prd.md`(需求 / AC / 取证事实)。
> 设计无架构决策(双注册基建已在 `7a747379` 交付),不单开 design.md;
> 键盘导航交互契约在 PRD R4,细化注意点见步骤 5。

## 步骤(建议提交切分:PR1 前端切换+Rust 删除 / PR2 键盘导航+测试 / PR3 e2e+文档)

### PR1 — 切换 + 整链删除(核心)

1. [ ] `app/src/stores/projects.ts`:
   - 删 `isPickUnavailable` helper(:25-33)与 `TransportError` import
     (确认文件内无其它使用);
   - `addProject()`(:200-233)重命名为 `openDirBrowser()`,体 = `dirBrowserOpen.value = true`
     (返回 void;两个消费方都忽略返回值);
   - 头部 Q8v2 pick 语义注释(:7-12)+ store 内 P2.4 D6 相关注释改写为
     「全模式统一入口」叙述。
2. [ ] 消费方改名:`ProjectTabs.vue:53`、`EmptyProjectState.vue:33`(含各自
   周边「browser degrade」文案注释)。
3. [ ] Rust 整链删除(R2 清单,按 PRD Confirmed Facts 的 file:line):
   - `commands/projects.rs`:pick_project_dir fn + `use tauri_plugin_dialog::DialogExt` + 模块 doc :9 提及;
   - `lib.rs`::455 命令注册、:143 插件注册、:457 相邻注释修正;
   - `commands/mod.rs`::128 all_command_names 条目、:16 doc;
   - `daemon/routes/mod.rs:34`:路由清单去掉 pick_project_dir(它从未有 route——stale 修正);
   - `daemon/routes/projects.rs:139` doc 注释同步;
   - `Cargo.toml:36` 删依赖(cargo check 收敛 Cargo.lock);
   - `capabilities/default.json:27` 删 `dialog:default`。
4. [ ] 注释收口(R3):DirBrowserModal.vue 头注释「browser-mode degrade」→ 全模式统一。

### PR2 — 键盘导航(R4)+ 测试对齐(R5 前半)

5. [ ] `DirBrowserModal.vue` 键盘导航:roving tabindex——行(.. + entries)中恰一个
   `tabindex=0`(选中行),其余 `-1`;ArrowDown/ArrowUp 移动 DOM focus(钳边不环绕);
   Enter 走 button 原生 click;输入框聚焦时方向键不劫持;列表发起的 navigate 完成
   后焦点复位新列表首行,路径直达/前往/上一步按钮发起的不抢焦点。
   注意:reka-ui Dialog 已有焦点陷阱与 Esc,不要绕;busy/error 行 disabled 不落焦点。
6. [ ] `projects.test.ts`:改写 pick 相关 ~6 用例 → openDirBrowser 断言
   (翻 dirBrowserOpen、零 pick IPC);registerPickedPath 三路径回归用例保留
   (改经 addProjectByPath 或直接调 openDirBrowser 后的路径流程)。
7. [ ] `DirBrowserModal.test.ts`:补键盘用例(方向键移动焦点 / Enter 触发
   browse_dir / 输入框聚焦方向键不移动)。jsdom focus 语义注意
   `test-environment.md` gotchas。

### PR3 — e2e + 文档(R5 后半 + R6)

8. [ ] 新 e2e spec `app/e2e/projects-add-dirbrowser.spec.ts`:route-mock
   `get_home_dir` / `browse_dir` / `create_project` / `list_projects`,
   断言:空态「添加项目」→ 模态框 → 点目录行 → 「选择此目录」→ create_project
   请求体正确 + Tab 出现;至少一条键盘路径(方向键 + Enter 进入)。fixture 契约
   见 `browser-regression.md`(catch-all miss 500 fail-loud、fake EventSource)。
9. [ ] 文档:BACKLOG §5.3 状态改已落地(引用本任务);ROADMAP §1.2 加一行
   (一句做了什么 + 时间 + 链接)。

## 验证命令

```bash
# Rust(根 workspace;PKG_CONFIG_PATH 必需)
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo check -p everlasting
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test -p everlasting --lib

# 前端
cd app && pnpm test && npx vue-tsc --noEmit && pnpm build && pnpm test:e2e

# 残留扫(生产代码零命中;spec/任务归档/docs 允许)
grep -rn "pick_project_dir" app/src app/src-tauri/src
grep -rn "tauri_plugin_dialog\|tauri-plugin-dialog" app/src-tauri
```

## 风险点 / 回滚

- 回滚单元 = 三个 PR 各自独立可 revert;PR1 是行为切换点(revert 即回 native picker)。
- `capabilities/default.json` 删权限后 Tauri 构建若报缺 permission(理论上删的是
  无消费方权限),以 `pnpm build`/tauri build 校验为准——只跑过 `cargo check` 不覆盖
  capabilities 校验面,vitest/build 不触发 tauri 打包;至少确认 `cargo check` +
  前端 build 绿,打包验证留桌面手测(AC1 同批)。
- transport-parity.test.ts 若解析 all_command_names 源码,删条目后应仍一致(守卫方向
  是 daemon↔CMD_TO_DOMAIN);红了先读测试再动。

## start 前检查

- [x] prd.md 收敛(无 open questions)
- [ ] implement.jsonl / check.jsonl 策展真实条目
- [ ] 用户评审通过 → `task.py start 09-03-dirbrowser-desktop-unify`
