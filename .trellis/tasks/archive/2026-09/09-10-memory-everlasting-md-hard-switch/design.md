# Design:CLAUDE.md 硬切换 EVERLASTING.md + 4 槽位植入开关

PRD:`prd.md`(同目录)。评审共识:session `55838776` 转录(out/ 20260910042452)。
两段交付:PR1 改名(不碰 LayerStatus/freeze/digest 读路径)、PR2 开关(仅新增变体与穿参,不动 freeze 契约)。

## PR1 — 硬切换

### D1.1 枚举与 wire

- `Memory/types.rs`:`MemorySource::Claude` → `MemorySource::Everlasting`,variant 上
  `#[serde(rename = "everlasting", alias = "claude")]`(enum 级 `rename_all = "snake_case"`
  本就产出 "everlasting",显式 rename 钉意图;alias 保 Rust 侧反序列化旧值)。
- `filename()` / `label()` → `"EVERLASTING.md"`;banner label 变 `[User EVERLASTING.md]` 等,
  digest 寻址键随之变 `Project EVERLASTING.md#节名`(进程级 registry 自清,无迁移)。
- **wire-pin 单测**(memory/tests.rs):serialize Everlasting ⇒ `"everlasting"`;
  `from_str("\"claude\"")` ⇒ Everlasting;`"agents"` ⇒ Agents。
- `is_digest_layer` 的 source 判定改 `MemorySource::Everlasting`,digest 资格/阈值/`<reference>`
  包裹语义零变化(锁定决策)。

### D1.2 路径收敛

- `file.rs`:`user_claude_dir()` / `USER_CLAUDE_DIR_OVERRIDE` / `set_user_claude_dir_for_test`
  删除;`resolve_path` User 臂两源统一 `user_dir()`(`~/.config/everlasting/`)。
  测试里原本 override claude dir 的用例改用 `set_user_dir_for_test`。
- `loader.rs::all_paths`:User 两项都走 `user_dir()`;canonical 序不变
  (User EVERLASTING → User AGENTS → Project EVERLASTING → Project AGENTS),`slot_index` 不变(0/1)。

### D1.3 legacy 检测条(P0 #6)——独立命令,不改 `read_memory_layers` 响应壳

评审原文"响应带 legacy_files 数组"会破坏 wire 壳(Vec→对象,新旧 GUI 双向解析全断),
与同场 P0 #1 的 skew 结论自相冲突。**修正为 additive 独立命令**:

- 后端新增 `read_legacy_memory_files(project_id) -> Vec<String>`:对
  `~/.claude/CLAUDE.md`(home_dir 直拼,不走已删的 user_claude_dir)与
  `<project>/CLAUDE.md` 各一次 `fs::metadata`,存在即收录。Tauri command + daemon route
  两面都挂(commands/memory.rs + daemon/routes/memory.rs)。
- 旧 GUI 不认识该命令 ⇒ 不调用,零影响;新 GUI 打旧 daemon ⇒ 命令缺失报错,
  store 捕获后 fail-open 空数组(与 legacy 检测条"尽力而为"语义一致)。
- 前端 `stores/memory.ts` 加 `legacyFiles` 状态 + `loadLegacyFiles()`;MemoryPreview
  顶部静态条:`检测到 n 个旧 CLAUDE.md 已不再加载`(i18n zh/en)。

### D1.4 前端读侧(serde+TS 三件套,P0 #1)

- `stores/memory.ts`:`type MemorySource = "everlasting" | "agents"`;store 边界加
  normalize:读侧收到 `"claude"` 映射为 `"everlasting"`(新前端读旧 daemon 的唯一症状点)。
- `MemoryLayerItem.vue`:source→文件名改显式映射表(everlasting/agents 两键),
  未知值**原样回显** source 字符串,禁 else 兜底成 AGENTS.md。
- `MemoryPreview.test.ts` 留 `source: "claude"` legacy fixture 用例断言显示 EVERLASTING.md。
- settings/registry.ts keywords 保留 "claude.md" 检索别名(.ts 不在 grep 验收范围,
  存量用户搜旧名仍能找到面板)。

### D1.5 文档白名单(P0 #5)

改:`.trellis/spec/backend/memory/scenario-two-layer-memory-injection.md`(23 处)、
同目录 `decisions.md`(8 处)、`.trellis/spec/frontend/memory-ui.md`(12 处)、
`.trellis/spec/backend/agent-loop-architecture/system-prompt-assembly.md` + `agent-loop-architecture.md`、
`docs/ARCHITECTURE.md` §2.5.12。追加:`docs/IMPLEMENTATION/decisions-2026-09.md`(硬切换决策
+ 无 fallback + 4 KV key 名)、`.trellis/spec/backend/memory/decisions.md`(key 名 + fail-open)。
**明令不改**:`docs/_history/**`、`decisions-2026-06.md`、`tasks/archive/**`、`workspace/**`。

验收 grep(写进 PR 描述):
`grep -rn 'CLAUDE\.md' --include='*.md' . | grep -vE '(_history|tasks/archive|workspace|decisions-2026-0[68])'` ⇒ 0 行;
`grep -rn '"CLAUDE\.md"' app/src-tauri/src --include='*.rs'` ⇒ 仅 types.rs 单源。

## PR2 — 4 槽位开关

### D2.1 `LayerStatus::Disabled`(P0 #3)

- 新变体 `Disabled`(无 payload)。`render_prompt_section` 的 `matches!(Loaded)` 过滤
  天然跳过;banner / instructions blocks / `memory_token`(从 blocks 现算)零改动。
- freeze 快照形状保持 4 元 Vec,禁用槽位以 Disabled 进快照 ⇒ "下一会话生效"由机制强制。
- 前端 `MemoryLayerItem.vue` 三比较(isLoaded/isMissing/isError)外增 disabled 分支:
  行保留 + "已禁用"徽标 + token 区显示 —;内容查看(read_memory_content)不受开关影响
  (开关管注入,不管用户看自己的文件)。

### D2.2 `MemorySlotFlags` 穿参(P0 #2,过滤下沉)

```rust
// memory/flags.rs(新)
pub struct MemorySlotFlags { pub user_everlasting: bool, pub user_agents: bool,
                             pub project_everlasting: bool, pub project_agents: bool }
impl MemorySlotFlags {
    pub const fn all_on() -> Self;                       // 缺省
    pub async fn read(db: &SqlitePool) -> Self;          // 4 key fail-open("false" 才关)
}
pub fn apply_slot_flags(layers: Vec<MemoryLayer>, f: &MemorySlotFlags) -> Vec<MemoryLayer>;
// 单点过滤:命中关闭槽位 ⇒ 该层 status=Disabled、content/tokens 清零,Vec 长度/索引不变
```

- key 名常量单源 `pub const KEY_*: &str`(照 ask_no_timeout 形态);SETTABLE_APP_FLAGS
  与 Settings UI 都引用这组常量,杜绝双边字面量。
- **过滤不在 cache 内做**:cache 存原始 Loaded 层(否则翻开关不动 mtime,Disabled 会
  在 cache 里滞留)。flags 在 cache 读出之后、块组装之前施加:
  - 主循环:`load_for_session_frozen(cache, sid, pid, path, flags)` — miss 分支 =
    `apply_slot_flags(load_for_session(...), &flags)` 再 freeze;hit 分支返回冻结快照。
  - worker:`subagent/prompt.rs` dispatch 时 `flags.read(db)` + apply(下一 dispatch 生效,
    与父循环的短暂偏差评审已判可接受)。
  - digest executor:`execute_load_memory_sections` 调用点同样 apply ⇒ disabled 层
    content 已清空,section 匹配不中走既有 "no loaded layer/section matched" 自纠路径,
    **穿透通道关闭**。
  - preview:`read_memory_layers` 出口 apply(徽标数据通道 = status 字段)。
- `load_for_session` 签名不加 flags(纯缓存语义不变,memory/tests.rs 纯缓存测试零 sqlite 依赖);
  flags 施加点 = 上述四个 db 持有方调用点。

### D2.3 config 面(P0 #4)

- `commands/config.rs`:`SETTABLE_APP_FLAGS` 增 4 key;`get_app_config` payload 手写结构体
  增 4 bool 字段;同文件测试逐 key match 补齐(不进则 panic,当编译期用)。
- daemon `routes/config.rs` 白名单同步(ask_no_timeout 先例:两面各有一份)。

### D2.4 Settings UI

- Settings Memory 页(MemoryPreview 所在页)增 4 开关(分组:用户层/项目层 × 两源),
  经既有 `get_app_config` / `set_app_config_flag` 通道读写;vitest 覆盖开关→KV 写入。

## 验证

- `cargo test -p everlasting --lib`(基线 2343+,PKG_CONFIG_PATH 照 AGENTS.md)
- `cd app && pnpm test`(基线 1652+)+ vue-tsc + clippy + fmt
- PR2 live:`scripts/turn-smoke.sh` 看 per-turn memory 注入量随开关变化
- PR1 grep 验收(D1.5)+ diff 核对未触碰 LayerStatus/freeze/digest 读路径

## 风险与回滚

- PR1 唯一行为变化 = 文件名与路径(语义继承);回滚 = revert 单 PR。
- PR2 全部 additive(新变体/新 key/新 UI),`apply_slot_flags` 缺省 all-on,
  不写任何 key 时与 main 行为逐字节一致(单测锁)。
