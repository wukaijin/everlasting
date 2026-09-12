# Implement: GCE-P2(执行计划)

> 前置阅读:prd.md / design.md / research/engine-visibility.md。
> 验证速查:`node --test scripts/group-chat-run.test.mjs`(基线 16)/
> `node --test scripts/group-chat-mcp.test.mjs`(基线 19)/
> `node scripts/group-chat-mcp-smoke.mjs`(非 live)/ live 零 LLM 链见 Phase 5。

## Phase 1 引擎核心(group-chat-run.mjs)

- [x] 1.1 抽 `composePersonaMd(file, kind)` 导出;composePresets 改调它(行为零变,
      既有「PRESETS 单一事实源」单测即回归锁)。
- [x] 1.2 `mergePresets(builtin, rows, file)` 纯函数(design §2.2 规则 + 防御 throw)。
- [x] 1.3 `listUserPresets(base)` 拉取封装(api 同模块直用)。
- [x] 1.4 `loadEffectivePresets(rowsProvider)` 降级层(返回 {presets, degraded,
      detail?})。
- [x] 1.5 `resolveParticipants` 增可选 `presets` 参数 + 三趟 key 解析(display_name
      精确 → 忽略大小写;miss 报可用清单;多命中歧义防御)。
- [x] 1.6 run():resolveParticipants 前插 loadEffectivePresets;degraded 警告 +
      preset miss 降级提示;moderator 默认改吃 eff.presets;dry-run help 文案更新。
- [x] 1.7 cmdPresets 异步化(base 参数):合并视图三段输出(内置+已覆盖标记 /
      用户档 / footer 增引用说明);main 分发同步改。
- [x] 1.8 printRunHelp / usage 文案:`--preset` 说明加用户预设(UUID/名称)。

## Phase 2 MCP(group-chat-mcp.mjs)

- [x] 2.1 realDeps 加 `listPresets`;import { listUserPresets, loadEffectivePresets }。
- [x] 2.2 coreStart:eff = loadEffectivePresets(deps);resolveParticipants 传
      presets;moderator 取 eff.presets 三趟解析后定义;degraded+miss 报错增提示。
- [x] 2.3 buildToolShapes:preset → z.string() + describe;加 list_presets shape;
      TOOLS 加条目;start_discussion description 四档摘要句改「see list_presets」。
- [x] 2.4 corePresets(deps) + handler 接线(list_models 同款 textResult)。
- [x] 2.5 实测八工具 wire 字符 → 定预算锁(目标 3500,超则最小升档)→ 四处同步
      (常量注释 / mcp.test / smoke BUDGET / spec 提及处)。

## Phase 3 测试

- [x] 3.1 run.test.mjs:mergePresets 六臂 + resolveParticipants(merged)五臂 +
      loadEffectivePresets 两臂(设计 §5 表;基线 16 递增)。
- [x] 3.2 mcp.test.mjs:makeMockDeps 默认 listPresets→[];coreStart 用户预设/
      覆盖档/降级三臂;corePresets 两臂;八工具名 + preset schema string +
      预算实测(基线 19 递增)。
- [x] 3.3 smoke:八工具名;list_presets callTool 内置四 key 恒在断言(daemon
      两态确定性);BUDGET 同步。

## Phase 4 文档

- [x] 4.1 AGENTS.md:GCE-P1 段「M1/MCP 暂只认内置(P2)」句改定;GCE-M2 段
      七工具→八工具。
- [x] 4.2 docs/DAEMON-API.md §6.4:引擎侧消费注记(M1/MCP 运行时拉取 + 降级语义)。
- [x] 4.3 .trellis/spec/backend/group-chat-presets.md:边界(P2 未做)改「P2 已收」
      + 引擎合流契约小节(mergePresets 镜像规则 / 三趟解析 / 降级两层分工)。
- [x] 4.4 .agents/skills/group-chat/SKILL.md:preset 引用说明(内置 key / 用户
      UUID/名称)如有硬编码四档处,更新。
- [x] 4.5 .trellis/spec/scripts/group-chat-mcp-deploy.md(若提及 presets 烤入):
      补「运行时增量拉取,bin 免重部署」一句。

## Phase 5 门禁 + live 零 LLM 验证

- [x] 5.1 `node --test scripts/group-chat-run.test.mjs` 全绿(16+N)。
- [x] 5.2 `node --test scripts/group-chat-mcp.test.mjs` 全绿(19+N)。
- [x] 5.3 `node scripts/group-chat-mcp-smoke.mjs` 非 live 全绿(八工具)。
- [x] 5.4 live 零 LLM 链(daemon 在跑):经 HTTP 建临时用户预设行 + 覆盖行 →
      `presets` 子命令合并视图 → `run --dry-run --preset <用户档id>` 全链 →
      MCP list_presets callTool 见行 → 删测试行清场。
- [x] 5.5 Rust / 前端 / routes-sync 零改动确认(git status 只含 scripts/ + 文档)。

## 回滚点

- 每 Phase 一次可独立 revert;scripts/ 无 schema/wire 变更,单 commit 粒度回滚安全。
