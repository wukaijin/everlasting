# 清理 RULE-SMOKE-001 / RULE-PERM-002

## Goal

闭合 `.trellis/reviews/DEBT.md` 两条 P3 债:turn-smoke.sh 轮询提前命中导致多轮工具 turn 被
delete_session 腰斩(RULE-SMOKE-001);grant API 对 Shell 类工具接受永不生效的 tool 级
授权(RULE-PERM-002)。顺带修复研究中发现的同族坑:run_background_shell 的 prefix 授权行
因读侧硬编码 `tool_name='shell'` 永不命中。

## Requirements

### R1 — turn-smoke.sh 等请求终态而非 trace 行出现(RULE-SMOKE-001)

- `send_and_wait` 的成功条件改为:全局 SSE 流(`/api/v1/stream`)上出现本 request_id 的
  终态事件(`kind=="done"`);`kind=="error"` 视为失败并报错退出。
- SSE 订阅必须先于发送建立(新连接无历史回放);订阅为脚本级常驻,EXIT trap 清理。
- `--assert-turn-usage` 复用同一份 SSE 日志,不再自建第二个订阅。
- 报告段(列存在性检查 / token 报表)行为不变;`--compact` / `--handoff` / `--keep`
  / `--turns` 语义不变。
- 超时语义更新为"等请求终态",超时提示文案随之路径仍指 daemon logs。

### R2 — grant 入口 kind↔工具类别校验(RULE-PERM-002)

- `grant_tool_permission_inner`(IPC + daemon route 共用)按 `classify_tool` 校验
  match_kind:Path→`path`、Shell→`prefix`、WebFetch/GitMutation/Other→`tool`;
  不匹配返回 `ErrorCategory::InvalidRequest`,文案说明该工具唯一合法 kind。
- 校验抽为纯函数并单测(全矩阵 + 默认 kind="tool" 对 Shell 拒绝的债项原始场景)。
- agent loop AllowAlways 写入路径(permission_response → match_value_for_allow_always)
  产出的组合必须全部通过校验(它们本就按矩阵挑 kind,零回归)。

### R3 — prefix 授权读侧放宽 run_background_shell(同族新发现)

- `check_prefix_grant` 查询从 `tool_name='shell'` 放宽为
  `tool_name IN ('shell','run_background_shell')`,使两条写路径(IPC grant /
  AllowAlways)与存量行都被消费。
- 补集成用例:`run_background_shell` 名下 prefix 行命中短路 Allow(复用
  tests_check.rs 既有 seed helper 模式)。

## Acceptance Criteria

- [ ] AC1:`bash -n scripts/turn-smoke.sh` 语法通过;`--help` 输出正常。
- [ ] AC2:daemon 在跑时 live 冒烟:默认单轮 + 一条会触发工具的多轮消息(如
      `--message "运行 ls 看看当前目录"`),退出码 0,daemon 日志无
      "cancelled in-flight chat",报告段 turn_trace 行完整(多轮 = 多行)。
      daemon 不在跑则记录为"静态验证 + cargo 全量测试通过",AC2 留 live 复验标记。
- [ ] AC3:`cargo test -p everlasting --lib` permissions 相关用例全绿,含新增:
      grant 校验矩阵纯函数测试 + run_background_shell prefix 命中集成用例;
      既有 permissions 用例零回归。
- [ ] AC4:clippy gate(`cargo clippy -p everlasting -- -D warnings` 或项目等价命令)通过。
- [ ] AC5:DEBT.md 删除 RULE-SMOKE-001 / RULE-PERM-002 两条;P3 计数 12→10。
- [ ] AC6:spec 更新落位(permission-layer.md 补 grant 校验矩阵与 prefix 读侧契约;
      turn-smoke 行为变化在 AGENTS.md 烟测速查段落无需改——用法未变)。

## Notes

- 技术设计与全部 file:line 证据链见 `research/deep-dive.md`。
- 本任务为轻量任务(PRD + research 足够,无独立 design.md/implement.md)。
