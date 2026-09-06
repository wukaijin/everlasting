# Implement — GCE-M3 控制面暴露层

> 前置:prd.md(三决策已定)+ design.md 已阅;spec 消费 `pattern-group-chat-preempt-inject.md`(P0 机制)与 `backend/` 相关规范经 trellis-before-dev 注入。

## 顺序清单

### Phase A — JS 共享层 + 纯函数(TDD 先行)

1. `scripts/group-chat-run.mjs`:新增 `preemptGroupChat(base, sessionId)`(cancel 路由域,session 域 vs rid 域注释)。
2. `scripts/group-chat-run.mjs`:新增纯函数 `interpretAcceptance(acceptance, ownRequestId)`(injected / misfire 分派,misfire 带止损动作语义)。
3. `scripts/group-chat-run.test.mjs`:两纯函数用例(preempt helper 的 body 形状 / acceptance 三态分派)。
   - 验证:`node --test scripts/group-chat-run.test.mjs`

### Phase B — MCP 工具 ×2

4. `scripts/group-chat-mcp.mjs`:`TOOLS` 增 `interrupt_discussion` / `inject_message` + zod shapes + handlers + `coreInterrupt` / `coreInject`;`coreInject` 含**前置 busy guard**(复用 `findSession`:非 busy / 已收官 → 不 fireChat 直接语义报错;fireChat 后 acceptance 非 `Injected` → 自有 rid cancelChat 竞态兜底 + 语义报错);`TOOLS_BUDGET_CHARS` 2300 → 3200。
5. `scripts/group-chat-mcp.test.mjs`:新工具用例(成功 / **已收官群聊注入 → 报错且零 fireChat 调用、mock 断言** / guard 通过后误发竞态兜底 / 预算锁文案 AC4 同步)+ InMemoryTransport 全链一例。
6. `scripts/group-chat-mcp-smoke.mjs`:tools/list 期望六工具 + 预算锁 3200(`:20` 常量;test 侧 import 自动跟随)。
   - 验证:`node --test scripts/group-chat-mcp.test.mjs` && `node scripts/group-chat-mcp-smoke.mjs`

### Phase C — GUI 打断入口

7. store 层 action `preemptGroupChat()`(invoke `{"sessionId"}`,两 transport 通用)+ toast 分支;vitest 单测(调用形状 + 返回分派)。
8. `ChatPanel.vue` 群聊 indicator 区「打断」按钮(可见性 `sending && isGroupChatSession`,无确认弹层)。
   - 验证:`cd app && pnpm test`(全量回归);Playwright 用例实现期裁定(确定性门槛:route-mock 可驱动的最小 preempt 交互可加,否则 local-only)。

### Phase D — 文档

9. `docs/DAEMON-API.md`:§6.1 工具清单补两工具(受 AC4 预算约束的描述同源);新增 SSE follow 消费专章(design §5 六项清单)。
10. `docs/GROUP-CHAT-API-ROADMAP.md` §4:M3 置 ✅(收官时)+ M4 条目记「打断权限粒度随认证一体议」(Q2 决议落账)。

### Phase E — live 验收

11. daemon 重启新前端 dist;`node scripts/group-chat-mcp-smoke.mjs --live` 或 M1 脚本起一场真讨论,依 AC1/AC2/AC5 全链走:inject(busy)→ SSE follow 按文档消费 → interrupt → 轮询终态(`preempted`,若恰逢自然收官则 `group_chat_end`)+ summary → result;另取已收官 session 注入 → 报错且旧 `stop_reason`/`summary` 原样。
12. 收官:PRD AC 勾选、roadmap 状态行、journal。

## 验证命令汇总

```bash
node --test scripts/group-chat-run.test.mjs
node --test scripts/group-chat-mcp.test.mjs
node scripts/group-chat-mcp-smoke.mjs
cd app && pnpm test
# live(Phase E):
node scripts/group-chat-mcp-smoke.mjs --live
```

## 风险与回滚点

- **唯一动存量行为的点**:smoke 预算断言(2300→3200)——有意变更,review 时盯两侧同步。
- GUI 按钮误触面:无确认弹层是有意决策(收束不毁场);若实现中发现误触代价超预期(如收束轮消耗大),回退为加轻确认,回炉 PRD 记账。
- 回滚:单 commit revert;无 schema/无持久化/daemon 零改动。
- Rust 侧出现任何 diff = 违反 design 基线,停下回炉。
