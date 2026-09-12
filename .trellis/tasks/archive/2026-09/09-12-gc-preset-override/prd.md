# PRD:群聊预设内置档覆盖层(GCE-P1b)

## 背景

GCE-P1(09-12-gc-preset-settings)已落地用户群聊预设:`group_chat_presets` 表
(UUID 主键,模型引用 UUID)、Settings「群聊预设」页 CRUD、前端
`mergedPresets` 把用户预设叠加在内置四档(review / fe_review / arch / retro,
单一事实源 `scripts/group-chat-presets.json`,只读)之后。

**遗留缺口**:内置预设按模型**名**引用目录且烤在前端 bundle 里,目录里的模型被删/
改名后内置档就修不掉——想修复只能改源码重建。本任务给内置四档加**覆盖层**。

**方向已定案(用户 2026-09-12,勿再评估)**:覆盖层(override row),不是「内置档
入库可编辑」——那会造成 JSON 与 DB 两源分裂(M1/MCP 读 JSON、GUI 读 DB 一改就
打架),已否决。

## 需求

### R1 覆盖行的存储与唯一性

- `group_chat_presets` 表加可空列 `builtin_key`(值 ∈ 四个内置 key);带该列的行 =
  覆盖行。
- 每个内置 key 至多一条覆盖行:SQLite UNIQUE 索引保证(NULL 互不相撞,普通用户行
  不受影响);列迁移走 columns.rs 幂等加列先例(add_scheduled_tasks_column_if_missing
  模式),CREATE TABLE 段同步补列(新库直建,scheduled_tasks F2b 同款双路径)。

### R2 Settings 覆盖编辑 / 恢复内置

- 内置四档区每行加「覆盖编辑」:预填内置定义(主持人/参与者模型名经 resolveModelRef
  解析成 UUID;解析不出的留空由用户重选——这正是修复场景),存成带 `builtin_key`
  的行;已存在覆盖行时改为编辑该行。
- 已覆盖的内置档标「已覆盖」;覆盖行删除 = 「恢复内置」,回落 JSON 源码定义。
- 覆盖行在用户预设列表区**不重复出现**(在内置区原位管理,避免双管理面)。
- 校验沿用 commands 层 `validate_preset_input` 单源;name 仍不许撞内置 key(display
  名与链接键分离,`builtin_key` 才是顶替关系)。

### R3 消费方吃到覆盖后阵容(零侵入)

- 覆盖行在 `mergedPresets` 里**原位顶替**对应内置槽(key 仍是内置 key,不是追加
  新键):两消费方(ScheduledTasksTab 定时表单 / GroupChatConfigModal 建群弹窗)
  选中该档用的就是覆盖后阵容,解析/预填/禁用警告链路逐字同形(UUID 借道
  resolveModelRef byId 首趟,不写新解析分支)。
- 语义不变:快照语义(预设编辑不回溯已建任务)、`preset_key` 出处(旧任务的
  `preset_key: "arch"` 落在覆盖后定义上,B9 stale 提示对覆盖后定义生效)、P2 边界
  (MCP/M1 仍只认 JSON 内置档,`scripts/` 零改动)。

## 非目标(明确出栈)

- **修复入口快捷键**(用户标注打磨项可砍):两消费方「模型缺失」警告条上加「覆盖
  修复」跳转。评估后砍——SettingsModal 打开状态是 Sidebar 局部 ref,无全局打开
  通道,跨视图跳转需新增全局机制,成本与打磨收益不成比例。修复路径已可达:Settings
  → 群聊预设 → 对应档「覆盖编辑」。
- 内置档入库可编辑(已否决,见背景)。
- MCP server / M1 CLI 消费覆盖行(P2,与用户预设可见性同批)。
- 自定义 persona 文本(P3)。
- overwrite 语义(覆盖行编辑不回溯已建任务;快照语义照旧)。

## 验收标准

- [ ] AC1 内置档可覆盖编辑,两消费方(定时表单/建群弹窗)选中该档用的是覆盖后阵容。
- [ ] AC2 删除覆盖行即恢复内置原样;每个内置 key 至多一条覆盖(DB 层 UNIQUE 索引
  保证,普通 NULL 行互不相撞)。
- [ ] AC3 旧任务 `preset_key` 指向内置 key 时,stale 提示(B9 逻辑)对覆盖后定义
  生效(预设被覆盖且与存档阵容不同 → 提示亮)。
- [ ] AC4 后端 `cargo test -p everlasting --lib` 全绿(WSL 需 PKG_CONFIG_PATH)、
  前端 `pnpm test` 全绿 + routes-sync 守卫过(不动路由/命令名,应天然过);`scripts/`
  零改动。
- [ ] AC5 文档同步:DAEMON-API §6.4、spec group-chat-presets.md 增补 override 契约、
  AGENTS.md GCE-P1 行补 override 一句。
