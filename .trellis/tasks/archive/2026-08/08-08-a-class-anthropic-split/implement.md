# A类单体重构:anthropic provider 拆分 — Implement

## 执行策略

同 batch1:先"提取"(anthropic.rs 内部函数化,每阶段独立 commit + 中间验证)→ 再"拆分"(文件移动 + 测试迁出,单 commit)→ 文档 sweep。`stream!` 宏体含 yield 的骨架保留,提取严格限于无 yield 代码。

## 执行结果(2026-08-08 完成)

- Commit 1-6(提取):`6300f0c` request_log_fields → `1933688` send_request → `f5e6283` content_block_start → `fc7bdd4` content_block_delta → `6b4da17` content_block_stop → `0df7451` message_delta+start;每步 1657 全绿。宏体从 ~430 行降至 **93 行**(AC1 ≤220 ✓)。
- Commit 7(拆分):`3948897` hub(584 行)+ `anthropic/events.rs`(345)+ `anthropic/transport.rs`(103)+ 测试迁出 `tests_anthropic.rs`(590);clippy/fmt 零警告。
- Commit 8(handler 测试):`f7bd5e6` + `c995635` 新增 5 个事件 handler 测试(AC7 ≥4 ✓;1662 = 1657 + 5)。
- Commit 9(文档 sweep):`c34ccd7` spec/docs 9 处 `anthropic.rs:LINE` 行号引用改符号;源码注释零残留。
- **spec 沉淀**:`pattern-large-function-split.md` 追加 "Variant:stream!/闭包生成器(无 yield 纯函数提取)" + gotcha 5(测试迁出可见性)。
- **与 design 的偏差**:handler 提取为 impl 方法(`Self::handle_*` 调用)再于拆分时转自由函数;`handle_message_start`/`handle_message_delta` 提取顺序与 design 表列序不同(无影响)。

## 验证命令

```bash
cd /usr/local/code/github/everlasting/app/src-tauri
export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"
cargo check --lib                          # 每个提取 commit 后
cargo test --lib                           # 每个提取 commit 后:全量 1657 基线
cargo fmt                                  # 每 commit 前
cargo clippy --lib --tests && cargo fmt --check   # 拆分 commit 后:零警告终验
```

## Ordered Checklist

### Phase A:提取(anthropic.rs 内部,每步独立 commit)

1. **[commit] 提取 `request_log_fields`** — 阶段 0(L149–165):log_model / log_tools_count / log_has_system → `(String, usize, bool)`;宏体调用点解构回同名局部。
2. **[commit] 提取 `send_request`** — 阶段 A-D(L181–227):client 构建 + `→ LLM request` 日志 + HTTP post + 非 2xx 状态检查(`classify_error_response`)。`async fn -> Result<reqwest::Response, LlmError>`;宏体 `match { Err(e) => { yield Err(e); return; } }`。
3. **[commit] 提取 `handle_content_block_start`** — L268–339:纯状态转换(tool_use/thinking/redacted_thinking/默认 Text → 对应 BlockState)。
4. **[commit] 提取 `handle_content_block_delta`** — L342–414:`-> Option<ChatEvent>`(Delta/ThinkingDelta yield 点收敛);input_json/signature 累积不 yield。
5. **[commit] 提取 `handle_content_block_stop`** — L417–490:`mem::replace` 终结 → `Option<ChatEvent>`(ToolCall / SignatureDelta / RedactedThinkingDelta)。
6. **[commit] 提取 `handle_message_delta` + `handle_message_start`** — L493–556:stop_reason / usage 更新(语义:delta 覆盖、start 仅 None 时写)。
   - **Gate**:宏体应只剩骨架(~190 行):client/HTTP match、初始化、chunk 循环、事件分发(每类一个 handler 调用 + 统一 yield)、Done。`cargo test --lib` 全量全绿。

### Phase B:拆分 + 测试迁出(单 commit)

7. **[commit] 子模块化** — 建 `anthropic/{events,transport}.rs`:
   - events.rs:`BlockState`(pub(crate))+ 5 个 handler
   - transport.rs:`request_log_fields` + `send_request`
   - hub:re-export + `LlmConfig` / `DEFAULT_MAX_TOKENS` / `AnthropicProvider` / `impl Provider` / 3 个已有纯函数保留
   - 内联 `mod tests` 迁出为 `tests_anthropic.rs`(`#![cfg(test)]` + `use super::anthropic::*`)
   - 验证:`cargo check --lib`(provider/mod.rs 路径零改动解析)→ `cargo test --lib` → clippy + fmt

### Phase C:新增 handler 测试(AC7,可与拆分同 commit 或独立)

8. **[commit] handler 单元测试** — tests_anthropic.rs 新增:
   - `handle_content_block_stop` ToolUse → ToolCall(含空 buf 默认 `{}`)
   - `handle_content_block_stop` Thinking 空签名 → None / 非空 → SignatureDelta
   - `handle_content_block_start` 三类块 → BlockState 转换
   - `handle_message_delta` usage 覆盖语义
   - 验证:`cargo test --lib`(1657 + 新增)

### Phase D:文档 sweep

9. **[commit] 引用 sweep** — 范围(评审 P2-1 扩充后):
   - `.trellis/spec/backend/llm-contract.md:533,566`(`apply_deepseek_reasoning_fix` / `anthropic.rs:262` 错误日志)
   - `docs/SESSION-FIRST-MESSAGE-INTERFACE.md:39,292`(`anthropic.rs:789` send 入口)
   - `docs/research/llm-network-resilience-survey.md:179,187,199,230,231`(5 处)
   - 其他 grep 命中项(见下)
   - **不改**:`docs/IMPLEMENTATION/decisions-*.md`(历史决策快照,`anthropic.rs:209-211` timeout fix 锚点保留原样)、`docs/_reviews/`、`docs/_deprecated/`、`.trellis/tasks/archive/`
   - 源码注释:`app/src-tauri/src/` 下 `anthropic.rs:LINE` 行号引用改符号引用
   - 残留核验(两条均应无输出):
     ```bash
     grep -rn "anthropic\.rs:[0-9]" .trellis/spec/ docs/ app/src-tauri/src/ \
       | grep -vE "/_reviews/|/decisions-20|/archive/|/_deprecated/"
     ```

### Phase E:收尾

10. 终验:`cargo test --lib` 全绿 + `cargo clippy --lib --tests` + `cargo fmt --check` 零警告
11. squash merge 回 main → `task.py archive` → 复验 `cargo test --lib`

## 风险提示(design §5)

- handler 必须无 yield(纯函数签名强制);出现 yield 相关编译错误立即回退该 commit
- 事件分发骨架中每类事件的 handler 调用逐一对应原分支;`message_delta` 的 usage 语义(delta 覆盖 vs start 仅 None 时写)严格保持
- 测试迁出时 `use super::*` → `use super::anthropic::*`(tests_anthropic.rs 位于 provider/ 下)

## Review Gates

- [ ] 用户评审 prd/design/implement 通过
- [ ] `task.py start` 后实施
- [ ] 每个提取 commit 独立可回滚(AC3)
- [ ] 终验三绿(AC4)+ handler 测试(AC7)
