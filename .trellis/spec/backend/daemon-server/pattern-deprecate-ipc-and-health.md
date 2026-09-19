<!-- Moved from daemon-server.md 2026-09-19 (doc-split) -->

## Pattern: 下线一个弃用 IPC 命令(RULE-SHIM-001 闭合 2026-08-30)

「新增 4 处注册」的镜像清单 —— 删一个命令同样缺一不可,且比新增多两处
测试侧清单。全链(以 `get_pending_question` 下线为参照):

1. 命令本体:`commands/<domain>.rs` 的 `#[tauri::command]` 壳 + `_inner`
   (inner 若因此失去最后一个调用方则同删;daemon 路由若只剩它是消费者,
   路由 handler + `Get*Request` 结构体一并删)。
2. Tauri 注册表:`lib.rs` `generate_handler!` 列表 + 相邻 `#[allow(deprecated)]`
   与说明注释。
3. daemon 路由:`daemon/routes/<domain>.rs` 的 `.route("/<cmd>", ...)`。
4. 命令清单:`commands/mod.rs::all_command_names()`(文档型 inventory,
   漏删不报错,只会留漂移)。
5. 前端映射:`transport/http.ts` 的 cmd→domain 表 + `questionCards.types.ts`
   类 `*_CMD` 常量及其消费方。
6. **测试侧**(新增没有的):`tests/e2e.rs` 路由 inventory 列表 +
   专测该命令的用例(改名/删除/移植到现代等价物)。

前置条件:全仓 grep 命令名确认前端/脚本零消费(含 `"<cmd>"` 字符串字面量);
历史叙事文档(docs/HACKING-*、_history)中的提及是事实记录,不算消费方,不追改。

## Contract: `/api/v1/health` 保持 stateless(RULE-HEALTH-001 闭合 2026-08-30)

- handler **不接 `State<Arc<AppState>>`**:Q1 端口冲突探测在新 daemon 加载
  AppState **之前**就要打到这个端点(问的是占端口的旧 daemon),stateless
  顶层 router 挂载是它的前提。需要带状态的体检,另立
  `/api/v1/health/detailed`,不动本端点。
- wire 形状:`{daemonId, daemonVersion, apiVersions, uptimeSeconds}`(camelCase)。
  `sessionCount` 字段已删除 —— 曾是 `-1` 哨兵,核实零消费方(仅 TS 接口声明
  + 测试 fixture)后移除;JSON 删字段对旧消费者向后兼容(absent ≠ 哨兵语义)。
  单测断言 `sessionCount` **缺席**(防止无意识回填)。
