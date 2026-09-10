# 记忆层 CLAUDE.md 硬切换 EVERLASTING.md + 4 槽位植入开关

## Goal

B5 记忆层文件槽位去 Claude 化,两件事一次需求闭环:

1. **硬切换**:`MemorySource::Claude` → `Everlasting`,文件槽位只认 `EVERLASTING.md`(无 fallback、无双读),`~/.claude/` 互操作点退场(推翻 2026-06-26 user-claude-md-home-dir 决策,决策记录追加而非改写)。
2. **4 槽位植入开关**:User/Project × EVERLASTING/AGENTS 四个独立 `app_config` KV key,fail-open 语义,控制各槽位是否注入会话。

需求评审(群聊 session `55838776`,2026-09-10,转录 `out/group-chat-需求评审-记忆层-claude-md-硬切换为-everlasting-md-20260910042452.md`)三方一致通过进入实施,六条 P0 必改项已并入下方 Requirements;存量静默失效被一致否决,以 Preview legacy 检测条补偿。

## Background(2026-09-10 逐项验码)

| 现状事实 | 代码证据 |
|---|---|
| 4 固定槽位 = `MemorySource`(Claude/Agents)× `MemoryKind`(User/Project);User CLAUDE.md 在 `~/.claude/`(Claude Code 互操作),User AGENTS.md 在 `~/.config/everlasting/` | `memory/types.rs:33-100`、`memory/file.rs:81-114` |
| `MemorySource::Claude` 承载三重语义:`<reference>` 包裹(vs AGENTS 的 `<primary instructions>`)、digest 资格(仅 Claude 源且 tokens>阈值)、digest section key 寻址(`Project CLAUDE.md#节名`) | `memory/loader.rs:400-409`、`digest.rs:185-190` |
| 注入面 5 处:主循环(经 D2 session freeze 冻结首轮快照)、worker(`subagent/prompt.rs`,legacy blocks)、群聊参与者、digest 元工具、Preview UI(含 daemon route) | `agent/chat_loop/init.rs:488`、`subagent/prompt.rs:62`、`digest.rs:397`、`commands/memory.rs:53`、`daemon/routes/memory.rs:28` |
| `load_for_session` 恒返回 4 元 Vec,agent loop 靠索引格式化 banner | `memory/loader.rs:197-199` |
| **digest 元工具不经 `load_for_session`**:从 mtime-fence cache 现取层——出口单点过滤会被工具通道穿透(评审硬发现) | `digest.rs:368-373` |
| 开关先例:`app_config` KV fail-open(`"false"` 才关),12 个 key 全无 UI;`SETTABLE_APP_FLAGS` 白名单管 Settings 写入;`commands/config.rs` 测试逐 key match,新 key 不进则 panic | `chat_loop/init.rs:502`、`commands/config.rs:618` |
| 版本 skew 是机制推向的稳定态:health.ts 版本漂移 warn-only + 残留 daemon 占 7456 时新 daemon 拒启动 → 新前端会跟旧 daemon 说话;TS 侧 `stores/memory.ts:57` 手写类型 + `MemoryLayerItem.vue:74` 三元 else 把未知 source 值静默显示成 AGENTS.md(评审硬发现:serde alias 只保护反序列化方向,是假安全感) | `app/src/transport/health.ts:81-104`、`bin/everlasting-daemon.rs:373-388` |
| 后端 ~90 处 `CLAUDE.md` 字符串引用(重灾区 `memory/tests.rs` 29 处、`digest.rs` 20 处,几乎全是 label/path/banner 文本断言,机械改造);`cache_head_stability.rs` 断言 head 字节稳定而非文件名 | grep 实测 |

## Decisions(用户 2026-09-10 拍板)

- **硬切换**,不做 fallback 链/双读/自动迁移——存量 CLAUDE.md 静默失效以 Preview legacy 检测条 + release note 补偿,不做首启 toast/modal。
- **4 槽位全局开关**为开关首版范围;per-project / per-session 维度不做(per-session 与 D2 freeze 语义冲突,明令不做)。
- **不改本仓库根 `CLAUDE.md`**:Claude Code 视角保留 5109 tokens 架构地图;代价是硬切换后 everlasting 自身 agent 不再读它(只剩 AGENTS.md 的 Trellis 块)——接受该状态,legacy 检测条会如实提示;若日后要找回,零维护方案是 `EVERLASTING.md → CLAUDE.md` 符号链接,不在本任务。
- Preview legacy 检测条(评审 P0 #6)按推荐采纳。
- 语义零变化:digest 资格、`<reference>` 包裹、banner 优先级原样继承;`<reference>`("为别的工具写的")语义重审归 PR3 候选。

## Requirements

### R1 — PR1:硬切换改名(评审 P0 #1/#5/#6 并入)

- `MemorySource::Claude` → `Everlasting`:filename/banner label/digest section key(`Project EVERLASTING.md#节名`)/全部 `~/.claude` 引用改净;`user_claude_dir()` 与 `set_user_claude_dir_for_test` 退场,User 层统一 `~/.config/everlasting/EVERLASTING.md`(与 AGENTS.md 同目录);`resolve_path` 收敛单分支。
- **serde + TS 三件套同 PR**(P0 #1):Rust `#[serde(rename = "everlasting", alias = "claude")]` + wire-pin 单测(serialize 出 `"everlasting"`、`from_str("\"claude\"")` 落 Everlasting 变体);`app/src/stores/memory.ts` 读侧显式映射接受旧值 `"claude"`;`MemoryLayerItem.vue` 三元去 else,未知值原样回显;`MemoryPreview.test.ts` 留一条 `source: "claude"` legacy 用例。验收项写明"新前端读旧 daemon 的 `"claude"` 显示正确"。
- **文档白名单 5 条必改**(P0 #5):`.trellis/spec/backend/memory/scenario-two-layer-memory-injection.md` + 同目录 `decisions.md`、`.trellis/spec/frontend/memory-ui.md`、`.trellis/spec/backend/agent-loop-architecture/system-prompt-assembly.md` + `agent-loop-architecture.md`、`docs/ARCHITECTURE.md` §2.5.12;追加 `docs/IMPLEMENTATION/decisions-2026-09.md`(硬切换 + 无 fallback + 4 个 KV key 名单源);`.trellis/spec/backend/memory/decisions.md` 写 key 名 + fail-open 语义。**明令不改**:`docs/_history/**`、`decisions-2026-06.md`、`.trellis/tasks/archive/**`、`.trellis/workspace/**`(保决策审计链)。
- **Preview legacy 检测条**(P0 #6):`read_memory_layers` 响应增 `legacy_files` 数组(对 `~/.claude/CLAUDE.md` 与 `<project>/CLAUDE.md` 各一次 `fs::metadata`,路径解析函数现成),前端 Preview 面板顶部静态提示"检测到 n 个旧 CLAUDE.md 已不再加载" + i18n。
- **不碰** `LayerStatus` / freeze / digest 读路径(P1 前提,保 PR2 正交)。

### R2 — PR2:4 槽位植入开关(评审 P0 #2/#3/#4 并入)

- 4 个 KV key(`memory_user_everlasting_enabled` / `memory_user_agents_enabled` / `memory_project_everlasting_enabled` / `memory_project_agents_enabled`),key 名常量单源(照 `ask_no_timeout` 形态),fail-open 读法照抄现有模式。
- **过滤位置下移**(P0 #2):flags 在块组装之前、cache 读写点(`read_or_load_user` / `read_or_load_project`)生效且在 freeze 快照形成之前——"下一会话生效"由 freeze 机制强制;堵 digest 元工具不经 `load_for_session` 的穿透通道。实施形态:`MemorySlotFlags`(4 bool,Copy)作参数,过滤逻辑单点(`apply_slot_flags`),读取留在各 db 持有方(freeze-miss / worker dispatch / preview 请求 / digest executor)。
- **Vec 不缩短**(P0 #3):新增 `LayerStatus::Disabled` 表达禁用,banner/注入按 `status==Loaded` 过滤天然跳过,freeze 快照形状保持;PR2 完成定义写明 4 槽位 disabled 下的前端渲染(前端 `isLoaded/isMissing/isError` 显式比较不自动覆盖新变体)。
- **机械必改**(P0 #4):4 key 进 `SETTABLE_APP_FLAGS` 白名单 + `commands/config.rs` 测试逐 key match;`get_app_config` 响应 payload 同步。
- UI:Settings Memory 页 4 开关 + Preview 面板"已禁用"徽标(经 `MemoryLayerInfo.status` 上 wire,数据通道现成)。
- `memory_token` 口径无需单独处理:从 `instructions_blocks` 现算(`init.rs:595`),过滤位置对了自然正确。

## Acceptance Criteria

- [ ] AC1(PR1 后端):`cargo test -p everlasting --lib` 全绿(现基线 2343+);wire-pin 单测钉住 serde 值;`grep -rn '"CLAUDE\.md"' app/src-tauri/src --include='*.rs'` 只剩 `types.rs` 单源;PR1 不触碰 LayerStatus/freeze/digest 读路径(diff 可核)。
- [ ] AC2(PR1 文档):白名单 5 条路径逐条 `git diff --stat` 有改动;`grep -rn 'CLAUDE\.md' --include='*.md' . | grep -vE '(_history|tasks/archive|workspace|decisions-2026-0[68])'` 输出 0 行;`decisions-2026-09.md` 已追加;明令不改卷零改动。
- [ ] AC3(PR1 前端):新前端读旧 daemon `"claude"` 显示正确(legacy fixture 用例);`cd app && pnpm test` 全绿(基线 1652+);Preview legacy 检测条对存在 CLAUDE.md 的环境显示提示(vitest)。
- [ ] AC4(PR2):4 开关各自关闭时对应槽位 `LayerStatus::Disabled` 且不进 banner/instructions 块/`memory_token`;digest 元工具对 disabled 层返回不匹配错误(穿透通道已堵);Settings 可写 4 key(白名单 + 逐 key match 测试);全链路单测 + `scripts/turn-smoke.sh` live 验证注入量变化。
- [ ] AC5(回归闸):clippy / vue-tsc / fmt 净;worker 注入路径(`subagent/prompt.rs`)与群聊参与者路径开关生效口径与主循环一致。

## Out of Scope

- per-project / per-session 开关维度(后者与 D2 freeze"首轮冻结"契约冲突,明令不做)。
- 自动迁移、首启 toast/modal(评审否决)。
- 旧 digest section key 迁移或 legacy 寻址 alias(registry 进程级自清,加 alias 会重新引入双命名空间)。
- `<reference>` 包裹语义重审、`MemoryKind` Session/Runtime 变体与开关关系确认(评审建议 PR2 设计时顺带,不阻塞)。
- 本仓库根 `CLAUDE.md` 处置(用户裁定不动;可选 symlink 补救不在本任务)。
