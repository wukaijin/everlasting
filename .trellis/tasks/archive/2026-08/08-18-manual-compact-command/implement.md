# Implement — 手动 /compact 命令入口

> 有序清单;每步含验证。回滚点:各步独立可摘除(design §7)。

## 后端

- [x] B1 `build_compaction_prompt` 加 `focus: Option<&str>`(auto 调用点传 None)+ focus 注入单测
  - 文件:`agent/compaction.rs`(模板 ~:533)+ `agent/chat_loop/drive.rs` 调用点
  - 验证:`cargo test -p everlasting --lib "compaction"`
- [x] B2 提取 `find_latest_summary_anchor(&[MessageRow])`(apply_compaction_watermark 第 1-2 步共用)+ 单测(命中/未命中)
  - 验证:同上
- [x] B3 `run_manual_compaction` 编排(`agent/compaction.rs` pub(crate)):gate → in-flight → provider → 行加载 → anchor → 保留区 → prompt → LLM → 落库(trigger=manual/focus/prior_summary_seq)→ 熔断记账 → 响应载荷
  - 注意:cutoff 精确值语义(C3+ §4.3 修订);空待压区 Err;失败零写入
  - 验证:新增 mock 测试(低阈值成功/focus/增量/失败/in-flight/空区/群聊/tripped)
- [x] B4 `compact_session` 命令:`commands/sessions.rs` `_inner` + tauri 壳;`lib.rs` 注册;`daemon/routes/sessions.rs` handler + 路由 + oneshot 冒烟;`transport/http.ts` CMD_TO_DOMAIN
  - 验证:`cargo test -p everlasting --lib "sessions"` + daemon 冒烟测试

## 前端

- [x] F1 `BUILTIN_COMMANDS` + compact 条目;`BuiltinCommand.argument_hint` 字段 + panel.rs builtin hint 映射改读字段
- [x] F2 `ChatInput.vue`:builtin switch 提取 `executeBuiltin(name, focus)`;`case "compact"`(残留文本作 focus、toast 中/成/败、成功后 reload 消息流)
- [x] F3 `app/src/utils` 新 `matchBuiltinCommandInput(text, names)` 纯函数 + 提交路径拦截(palette 打开时不冲突)+ vitest
  - 验证:`cd app && pnpm test`

## 收尾

- [x] C1 全量:`cargo test -p everlasting --lib`(PKG_CONFIG_PATH 见 AGENTS.md)+ `cd app && pnpm test`
- [x] C2 mock 端到端(AC7 前半,design §6)
- [x] C3 live:`scripts/turn-smoke.sh` 扩展 `--compact`(idle 后调 compact_session → 断言摘要行 + trigger=manual → 再跑一轮 context_input 下降)
- [x] C4 spec 回写:pattern-llm-compaction.md 补 manual 入口契约(trigger=manual/focus/熔断语义/命令注册链)
- [ ] C5 trellis-check 质量核验

## 风险文件

- `agent/chat_loop/drive.rs`(attempt_summary_compaction 调用点穿参——只加 None 实参,不动逻辑)
- `agent/compaction.rs`(水位函数重构提取——保持 apply_compaction_watermark 行为不变,既有测试罩)
- `app/src/components/chat/ChatInput.vue`(builtin switch 重构——/help /clear /new 行为不变,面板路径回归)
