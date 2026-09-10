# Implement:CLAUDE.md 硬切换 + 4 槽位开关

步骤序(每步后跑对应门禁,PR1/PR2 分段提交):

## PR1(改名,不碰 LayerStatus/freeze/digest 读路径)

1. `memory/types.rs`:枚举改名 + serde rename/alias + filename/label
2. `memory/file.rs`:删 user_claude_dir 族,resolve_path 收敛 user_dir
3. `memory/loader.rs`:all_paths/banner 注释与 user 臂
4. `memory/digest.rs`:is_digest_layer 判定、layer_key、工具描述文案、测试字符串
5. `memory/{freeze,mod,tokens}.rs` + `commands/memory.rs` 注释与 legacy 命令
6. `daemon/routes/memory.rs`:挂 read_legacy_memory_files
7. 零散引用清扫:llm/types/message.rs、agent/{budget,memory_recall}.rs、
   tests_agent_loop/{cache_head_stability,compaction_summary}.rs、llm/provider/tests_wire.rs
   (注释/文案为主,逐处核对是否行为字符串)
8. `memory/tests.rs` 29 处 + wire-pin 单测
9. 前端:stores/memory.ts(normalize)、MemoryLayerItem.vue(显式映射)、
   MemoryPreview(legacy 条 + loadLegacyFiles)、i18n、测试 fixture
10. 文档白名单 5 路径 + decisions-2026-09.md + spec decisions.md key 名段(PR2 key 先占位?)
    ——否,key 名段随 PR2 落,PR1 只写硬切换决策
11. 门禁:cargo test --lib / pnpm test / clippy / vue-tsc / fmt + D1.5 grep 验收

## PR2(开关,全 additive)

12. `memory/flags.rs`:MemorySlotFlags + KEY_* 常量 + read(fail-open) + apply_slot_flags + 单测
13. `memory/types.rs`:LayerStatus::Disabled
14. 四个施加点:freeze(load_for_session_frozen 增参)/ subagent/prompt.rs /
    digest executor / commands::memory read_memory_layers
15. `commands/config.rs` + `daemon/routes/config.rs`:SETTABLE_APP_FLAGS + payload + 逐 key match 测试
16. 前端:MemoryLayerItem disabled 徽标、MemoryPreview/Settings 4 开关、i18n、vitest
17. 门禁 + turn-smoke live(开关前后 memory 注入量)
18. spec decisions.md 补 4 key 名 + fail-open 语义段
