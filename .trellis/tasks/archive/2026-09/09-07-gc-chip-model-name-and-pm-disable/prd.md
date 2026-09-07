# 群聊 speaker chip 模型名 + Provider/Model 禁用

> 2026-09-07 用户两项需求一次交付;禁用语义、默认模型拦截、wire 参数名坑
> 均为当日逐题定案,本文即决策记录。无独立 design.md(改动面收敛,关键
> 决策直接落各文件注释)。

## Goal

1. 群聊消息的 `msg-speaker-chip` 在发言者名字旁显示其模型显示名。
2. Settings 的 Provider / Model 增加禁用功能。

## Requirements / 决策记录

### R1 speaker chip 模型名(MessageItem.vue)

- chip 两段式:「● 名字 · 模型显示名」,模型名段 muted 色 + title 兜底。
- 解析链:参与者经 `chatStore.currentSessionParticipants`(name → model id)
  → `modelsStore.byId`;主持人经 `session.model_id`。
- 优雅降级:解析链任何一环缺位(roster 改名失配 / 模型已删 / models 未加载)
  整段省略,chip 退回纯名字形态。

### R2 禁用 = 选用层开关(非停用)

- **语义边界**:被禁模型只从「选模型」入口消失,分发 catalog 照常收录——
  已在用的会话与全局默认不受影响。删除才是破坏性操作,禁用是"暂藏不删"。
- 有效禁用 = `models.disabled` OR `providers.disabled`(后端 list_models
  JOIN 反范式 `provider_disabled`,前端一次拉取即得,`isModelEffectivelyDisabled`)。
- schema:`providers.disabled` / `models.disabled` INTEGER NOT NULL DEFAULT 0
  (greenfield CREATE 带列 + 存量 probe+ALTER,幂等;存量行默认启用)。
- IPC:`set_model_disabled` / `set_provider_disabled`(Tauri + daemon 双注册,
  CMD_TO_DOMAIN 同步;不 rebuild_catalog——分发集合与禁用态无关)。
- `update_model` 不触碰 disabled 列(禁用态由开关单独管理,回读带回真实值)。
- 六个选用入口过滤:ModelSelect / DefaultTab / GroupChatConfigModal /
  SubagentsTab / ProjectSubagentsTab / ScheduledTasksTab——后四者做
  「启用 ∪ 当前已选值」并集,编辑态回显的旧值仍可见可切走。
- 旧 daemon 兼容:wire 字段前端可选,undefined 按启用处理。

### R3 默认模型拦截(用户当日追加)

- Models 页:默认模型的禁用开关置灰,提示「先在 Default 页更换默认模型」。
- Providers 页:持有当前默认模型的 provider 同款拦截。
- **单向**:只锁「禁用」方向;已禁用态(存量/竞态)的「启用」方向放开,
  恢复正常态不误伤。Default 页换默认后开关响应式解锁/锁定。

### R4 wire 参数名坑(当日实测翻车,已修)

- 前端曾发 `{ modelId }`,后端 Tauri 参数 / daemon 路由字段都是 `id`:
  HTTP 路径 transport 转 snake_case 后变 `model_id` → axum 422
  `missing field 'id'`(Tauri 路径同样会缺参)。
- 修正:统一 `{ id, disabled }`(对齐 delete_model/delete_provider 约定)。
- 教训:store 单测全量 mock transport,只验「发了什么」不验「后端收什么」,
  wire 契约类 bug 是 mock 盲区;本次以 live curl(错误形状复现 + 修复形状
  返回 null)闭环验证。

## Acceptance Criteria

- [x] chip 显示模型名,四种解析路径(参与者/主持人/失配/模型已删)测试锁定
- [x] 禁用开关 + 徽标 + 六入口过滤,前后端测试全绿
- [x] 默认模型双入口拦截(Models 行级 + Providers 级),3+3 测试锁定
- [x] 后端 `cargo test -p everlasting --lib` 2333 通过;新增 4 个 disabled 测试
- [x] 前端 vitest 125 文件 / 1646 测试全绿;`vue-tsc --noEmit` 干净
- [x] live 验证:daemon 实测 `set_model_disabled` 两种 body 形状

## Notes

- 已知边界:providers 级拦截按「持有默认模型」判定;若默认指向已删模型
  (`byId` 落空)不锁任何行。
- 后端全量跑批中 `serve_daemon_keeps_serving_without_signal_past_grace_window`
  偶发失败(单跑即过,时序抖动),与本次改动无关。
