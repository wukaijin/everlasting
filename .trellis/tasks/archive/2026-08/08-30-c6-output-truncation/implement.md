# Implement — C6 大输出截断统一(执行计划)

> 前置:start 前 `git status` 干净;基线 `cargo test -p everlasting --lib` 绿
> (PKG_CONFIG_PATH 见 AGENTS.md);行号以 main `7b1dc90` 后续演进为准,动手前重对。

## PR1 — 共享模块 + 四工具迁移 + spill 迁移(R1/R2/R3 + R7 常量表)✅ 已完成

> **实现期两项计划外发现(均已修复)**:
> 1. **>64KB 输出死锁(存量 P1)**:shell 与 background_shell 的 wait 臂都是
>    `child.wait()` 后才读管道——输出超过管道容量(~64KB)时子进程写阻塞、
>    永不退出,烧满 120s 超时(被杀后只剩部分输出,spill 对 >64KB 从未生效)。
>    修法:select 前把 stdout/stderr take 出来、spawned task 并发排空
>    (`spawn_pipe_drain`/`collect_drain`,两模块共享一份);cancel/timeout 臂
>    杀完组后从 task 收部分输出。由 CJK spill 测试暴露(100KB 用例 120s 挂),
>    修复后同套件 0.61s。
> 2. **session_id 管道已铺好**:`shell::execute` 本带 `_session_id` parity 参数
>    (未使用),方案 (b) 的 ToolContext 字段取消,零改动用现状参数流。
>
> 另:三处工具描述文案(shell / shell_status / run_background_shell)里的旧
> `<cwd>/.everlasting/outputs/` 路径同步更新为「app data dir 落点 + offset/limit
> 分页」表述。

1. ✅ 新建 `app/src-tauri/src/tools/tool_output.rs`:
   - 常量表:`INLINE_CAP_BYTES`(50KB)/ `SPILL_THRESHOLD_BYTES`(30KB)/
     `SPILL_PREVIEW_BYTES`(1KB)/ `WEB_INLINE_CAP_BYTES`(100KB),注释注明
     token 预算依据(R7)。
   - `head_tail_truncate(&str, head, tail)`:char-boundary 安全(实现可复用
     `floor/ceil_char_boundary` 思路);property 测试:任意含 CJK / emoji /
     U+FFFD 输入 × 任意 cap 不 panic + 头尾长度 + 中段省略。
   - **首个用例复现 shell 裸切片 panic 场景**(RULE-E-009,design §1.1b)。
   - `truncation_marker(omitted, total, unit, recovery)`:golden 测试三模式
     (A spill 路径 / B range 参数 / C 收窄)+ `matches` 变体。
   - `spill(data_dir, session_id, contents: &[u8]) -> io::Result<PathBuf>`
     (字节入口,design §2.1)。
2. ✅ ~~`ToolContext` 增 `session_id` 字段~~ **(实现期取消)**:`shell::execute` 已带
   `_session_id: Option<&str>` parity 参数(shell.rs:314-317,未使用),in_memory
   spill 调用点 session_id 就在作用域(registry 按 `(session_id, shell_id)` 键)。
   两处直接消费既有参数,`None` 退 `_no_session` 目录;零 ToolContext / 构造点改动。
3. ✅ `sensitive.rs::build_trusted_external_patterns` 增 `{app_data_dir}/outputs/**`
   动态段(同 worktrees 拼法,`sensitive.rs:89-106`);补 pattern 命中 / 不越界
   测试;确认 deny(Tier 2.5)先于 allow 的顺序无扰动。
4. ✅ `shell.rs` 迁移:spill 改调 `tool_output::spill(&ctx.data_dir, session_id,
   combined.as_bytes())`;preview / inline 截断 / 标记走统一生成器;删本地
   `spill_to_disk` / `head_tail_preview` / `truncate_output` 与 `SPILL_DIR` 常量;
   `cleanup_outputs_dir` doc 的 stale「PRD §R8」引用更新;补 multibyte 用例
   (AC2 两条:>30KB CJK spill 成功 preview / spill 失败回退 inline)。
5. ✅ `background_shell/in_memory.rs` 迁移(独立副本,非复用点):删私有
   `spill_to_disk`(`:757-780`)与三常量(`:66-81`);stdout/stderr 字节直传
   `tool_output::spill`;`[stderr]` 合并语义与失败 warn+`None` 语义保留
   (design §2.3);核对 `:653` 调用点(kill_and_collect 路径)与
   `shell_status` 的 preview 消费端。
6. ✅ `read_file.rs` 迁移:`truncate_output` / `truncate_full_output` 内部实现换
   `tool_output`;`truncate_full_output` 的 `pub(crate)` 出口保留
   (`agent::at_file:47,765` 复用,格式等价由既有测试守护);截断标记统一。
7. ✅ `web_fetch.rs` 迁移(仅截断):删本地 `truncate_output`(`:646-661`)换共享;
   标记统一(spill 恢复留 PR2;5MiB TooLarge 不动)。
8. ✅ `grep.rs`:`cap_line_lengths` 保留 `chars().take`(天然安全);标记文案统一;
   head_limit 提示并入统一格式(行级 file:line 指引留 PR3)。
9. ✅ sweep 双路径:新增 `sweep_session_outputs(data_dir, session_id)` 整目录删;
   legacy `cleanup_outputs_dir(cwd)` 保留;入口 `commands/sessions.rs:371-373`
   两路都调,空 cwd 容错保持。
10. ✅ PR1 验证(全绿):`cargo test -p everlasting --lib` + `cargo clippy --lib -- -D warnings`
    + `cargo fmt --check`;`scripts/turn-smoke.sh` live(spill 新路径在真实轮次出现,
    `~/.local/share/dev.everlasting.app/outputs/<sess>/` 有文件且 session 删除后消失)。

## PR2 — web_fetch 落盘恢复(R4)✅ 已完成

11. ✅ ✅ web_fetch 转换后 >`WEB_INLINE_CAP`:spill 到 `tool_output`(转换后文本落盘);
    标记带绝对路径;不做 offset 重取(约束 C3)。
12. ✅ ✅ 集成测试:大内容 fetch → 标记路径 → `read_file` offset/limit 切片走通恢复链;
    权限断言:trusted pattern 命中 + `ToolAllowed` 审计、无 ask(AC4/AC5)。

## PR3 — grep 行级指引 + spec 收编(R5/R6/R7)✅ 已完成

13. ✅ ✅ grep 行截断标记带 `read_file {file} offset {line}`(rg `--line-number` 现成);
    head_limit 提示核对措辞。
14. ✅ ✅ spec 新档 `.trellis/spec/backend/agent-loop-architecture/pattern-output-truncation.md`:
    三模式契约 + 标记格式 + 分层陈述(截断 = 工具侧 vs 关卡⑤ = 轮级)+ spill 落点
    三依据 + RULE-E-009 唯一实现声明;spec index 登记。
15. ✅ ✅ ROADMAP C6 行移 §1.2 已实施;本文件与 review.md 归档随 task archive。

## 回滚点

- 每 PR 独立可回退。PR1 步骤 2(ToolContext 字段)是最大机械面;若构造点冲突
  严重可退方案 (a)(execute 独立参数),但 AC3 的 session_id 断言依赖 (b),
  回退需同步改 AC3(设计已定案 (b),非必要不退)。

## 验证命令速查

```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cargo clippy -p everlasting --lib -- -D warnings
cargo fmt --check
scripts/turn-smoke.sh
```
