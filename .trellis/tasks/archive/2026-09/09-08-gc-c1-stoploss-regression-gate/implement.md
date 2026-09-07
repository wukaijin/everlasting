# implement — 群聊止损包 + 回归闸

> 前置:prd.md(需求/AC)、design.md(技术设计)已定。执行按步序,每步带验证。

## Step 0 — R4:RULE 进 spec + 病灶清扫(commit 0,独立先行)

- [ ] `.trellis/spec/backend/quality-guidelines.md` **追加** RULE 一节(该文件已有 4 条既填约定,勿覆盖):正文/理由/三层适用/判定示例。
- [ ] 清扫:permissions/types.rs:148-155 `is_worker` 过时注释(改写为现状:worker ask 走完整往返,见 ask.rs:227-262)。~~group_chat_loop.rs round-robin 注释~~已由 `66ef6fa4` 先期修复,核对即可、无操作。
- [ ] 验证:`git diff` 仅 spec + 一处注释;cargo 编译净(Step 1 验证顺带覆盖)。

## Step 1 — R1:C1.1 ask-free(commit 1,共识顺序:先合)

- [ ] `PermissionContext` 增 `group_chat_ask_free: bool`(types.rs;注释按 R4 新 RULE 写:一句契约,细节指向 spec/测试)。
- [ ] `chat_loop/init.rs:398` 构造处从 `req.group_chat_state.is_some()` 推导;生产后台升级经 drive.rs:832 `permission_ctx.clone()` 自动继承(零改动);仅需给 `background_escalation.rs` 测试 harness(370 区,`#[cfg(test)]`)补字段。
- [ ] `permissions/ask.rs`:常量 `ASK_FREE_DENY_REASON` + 短路(design §1.2,不 register/不 emit/进审计 ToolDenied;判定可置函数头省一次 config DB 读,实现时择优)。
- [ ] 单测 `group_chat_ask_free_denies_out_of_bounds_ask_without_roundtrip`(design §3.1)。
- [ ] 验证:
  ```bash
  cargo test -p everlasting --lib "permissions::tests_ask"   # 经典聊天行为不变
  cargo test -p everlasting --lib "tests_group_chat"         # 群聊全绿 + 新用例
  ```

## Step 2 — R2 后端:C1.2 budget 机制(commit 2a)

- [ ] `GroupChatConfig.token_budget: Option<u64>`(serde default)+ `GroupChatCtx` 透传(build_group_chat_ctx)。
- [ ] `TokenTally`(AtomicU64)+ `TallySink` 装饰器(**10 方法**全转发——4 必实现 + 6 默认,`has_live_observer` 必须显式转发;Done{usage} 四计费字段累计);替换群聊三处内层调用的 sink。单测断言 `has_live_observer` 透传。
- [ ] `HaltReason::Budget` + `STOP_REASON_BUDGET`;轮头检查(design §2.3,不收束轮);finalize 落库走既有 `finalize_group_chat_lifecycle`。
- [ ] budget 三剧本单测 + None 对照(design §3.2)。
- [ ] 验证:
  ```bash
  cargo test -p everlasting --lib "tests_group_chat"
  cargo test -p everlasting --lib   # 全量门(基线 2343+)
  ```

## Step 3 — R2 前端:白名单 / notice / GUI(commit 2b)

- [ ] `streamEvents.ts` **两处**终态白名单都加 `"budget"`(151-161 handleChatEvent 早判 + 644-650 done 处理器)+ `streamController.ts` `groupChatNotice` 增 `"budget"`;可选:`scheduledStopReasonLabel` 加 budget case。
- [ ] GUI:`GroupChatConfigModal.vue` token_budget 可选输入(留空不写键)→ `chatSessionActions.ts` createNewSession metadata 加键;**`updateGroupChatConfig` 改合并保留既有 metadata 键**(防编辑名单抹掉 token_budget);vitest 三断言(建群写键/留空不写/编辑不丢键)。
- [ ] DAEMON-API §6 群聊实现要点补 `token_budget` 一条(additive 注记,非契约变更)。
- [ ] 验证:
  ```bash
  cd app && pnpm test          # 全量门(基线 1652+)
  cd app && pnpm vue-tsc --noEmit && pnpm fmt  # 或项目等价门
  ```

## Step 4 — 收尾

- [ ] clippy 净,对齐仓库既有门:`cargo clippy -p everlasting --lib`(quality-guidelines 既有约定;非 lib target 不在本任务门内)。
- [ ] AC1-AC5 逐条核对(prd.md);trellis-check。
- [ ] 不做 live 群聊验证(机制层 MockProvider 已锁;prompt/编排行为未变,不触 turn-smoke 条件)——若实现中发现动了 prompt/编排结构,补跑 `scripts/turn-smoke.sh` 一轮。

## 风险与回滚点

- 风险文件:`permissions/ask.rs`(经典聊天共用路径)——ask-free 短路必须严格限定 `group_chat_ask_free` 分支,tests_ask 全绿是回归闸。
- 回滚:两 work commit 独立 revert,无 schema/契约残留;metadata 键 additive 缺省 off。
