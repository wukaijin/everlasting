# Implement:执行顺序与验证

> 每步验证命令;全量门槛 = `cargo test -p everlasting --lib`(WSL 需
> PKG_CONFIG_PATH,见 AGENTS.md)。clippy:`cargo clippy -p everlasting -- -D warnings`。

## Step 1 协议层骨架(routes/mcp.rs + mod.rs 挂载)

- [x] mcp.rs:router(POST/GET/DELETE /mcp)+ JSON-RPC 解析/分派骨架
      (initialize/ping/notifications/tools 占位)+ 状态码矩阵
- [x] routes/mod.rs 挂载(merge 模式 + 注释)
- [x] oneshot 测试:initialize echo(五值参数化)/未知版本回退 2025-03-26/
      通知 202/ping/未知方法/406/415/-32700/GET 405/DELETE 200/批处理
- 验证:`cargo test -p everlasting --lib daemon::routes::mcp::`

## Step 2 预设嵌入 + 模型/项目解析(纯逻辑)

- [x] include_str! + OnceLock 解析(persona 展开/compose,坏 JSON fail-loud)
- [x] merge(builtin ⊕ GcPresetRow:覆盖/追加/脏跳过)+ lookup 三趟
- [x] normalize_model_ref(id/精确/忽略大小写/miss 清单)
- [x] resolve_project_by_path(list+hidden 物理比对,miss create)
- [x] 单测:compose 确定性(锁 JSON 唯一事实源)/merge 三分支/lookup 三趟
      + 歧义/normalize 撞车用例(GLM-5.3 大小写)/slug 规则
- 验证:同上 + `node --test scripts/group-chat-run.test.mjs` 不受影响

## Step 3 tools/list wire schema + AC4 预算锁

- [x] 8 工具 inputSchema(json! 手写,逐字段平移 JS buildToolShapes 含描述)
- [x] AC4 断言:tools/list 序列化 ≤4200 字符(实测 3600;SDK 客户端口径 3609)
- 验证:单测红绿;实测字符数写注释(对齐 JS TOOLS_BUDGET_CHARS 注释惯例)

## Step 4 工具实现(编排层)

- [x] ToolError{Semantic,Infra} + textResult/errorResult 同构
- [x] start_discussion(校验→预设→模型→项目→建群→chat_inner/HttpSseSink)
- [x] discussion_status(busy 真相源/wait 长轮询/detail/惰性转录触发)
- [x] discussion_result(终态 guard/detail 降级/roster/tokens/warnings)
- [x] export_mcp_transcript(render_scheduled_transcript 复用 + out/ 落点)
- [x] cancel(session_active_request rid / already_finished)/interrupt(preempt)/
      inject(busy guard + acceptance 判定 + 止损)
- [x] list_models/list_presets(合并视图,degraded 恒 false)
- [x] 工具链 oneshot:错误文案/校验路径(不进 LLM)
- 验证:`cargo test -p everlasting --lib daemon::routes::mcp::`(27 用例)

## Step 5 全量门槛 + clippy

- [x] `cargo test -p everlasting --lib` 全绿(2440 passed / 0 failed,基线 2413)
- [x] `cargo clippy -p everlasting -- -D warnings` 干净(main 基线 26 条既有
      报错为 clippy 1.96 新增于存量代码,git stash 对照确认与本任务无关)
- [x] 根 workspace:`cargo test -p everlasting-remote` 不受影响

## Step 6 P2 冒烟脚本 + live 验证

- [x] scripts/group-chat-mcp-http-smoke.mjs(SDK StreamableHTTPClientTransport;
      非 live:initialize/tools/list+预算/未知工具 405 链;--live 全链)
- [x] 手动起 daemon(`scripts/daemon.sh`)跑非 live 冒烟(09-14 实跑全绿:握手 +
      ping + tools/list 8 工具 wire 3609 + 未知工具 -32602 + session 不存在错误链 +
      wait_seconds 越界拒 + list_presets degraded=false + list_models 5 + GET 405 /
      DELETE 200 / 406 / 415 传输探针)
- [x] live 冒烟:start(arch 小阵容)→ status wait → result(烧真 token,一场)
      —— 09-14 实跑通过:session 3ee8ba4a,19 条消息,161s 自然收官
      `stop_reason=group_chat_end`,转录落 `out/group-chat-冒烟验收-…-20260914125150.md`
      (阵容经 normalize_model_ref 正确解析:MiniMax-M3 主持 + glm-5.3 + deepseek-flash);
      wait_seconds=30 长轮询两态都验证到(有变化即返 + 无变化 30s 到点 wait_timed_out
      再续)。已知外观瑕疵(P4 打磨候选):转录头行沿用 render_scheduled_transcript
      的「定时审议」字样(D2 复用的既知代价)。live 后 daemon RSS 113 MB —— 系真跑
      一场讨论的编排/DB/SSE 常态开销(任何入口等价),非 MCP 层成本;MCP 层成本以
      非 live 冒烟增量 1068 KB 计。
- [x] AC5:ps 观察 daemon RSS 增量 <10MB,无 MCP 子进程(实测:基线 58252 KB →
      冒烟后 59320 KB,增量 1068 KB;children 前后均空 = 零子进程)

## Step 7 文档回填 + 宿主双挂载

- [x] docs/DAEMON-API.md 新 §6.5(/mcp 契约矩阵)+ §6.1 指针 + §7 路由清单
- [x] docs/GROUP-CHAT-API-ROADMAP.md §5 path③ 回填(✅ 2026-09-14 落地记录)
- [x] .trellis/spec/backend/daemon-server.md Signatures 表补 /mcp 例外注记
      (绝对路径 merge、不进 CMD_TO_DOMAIN/routes-sync 守卫;实测守卫测试仍绿)
- [x] AGENTS.md MCP 段补 daemon 原生 endpoint(切换指引,P3 待办)
- [ ] 双挂载实测(~/.zcode/cli/config.json 加 everlasting-group-chat-http,
      本会话或新会话验 mcp__everlasting-group-chat-http__* 可用)—— **P3,另行
      确认后做**(本任务边界:不动宿主配置)
- [ ] git commit(勿切主配置;P3 切换另行确认)
