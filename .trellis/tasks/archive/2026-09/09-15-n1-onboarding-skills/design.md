# Design — N1 轻页面引导 + 内置诊断/引导/配置 LLM skills

> 证据与 file:line 见 [research/codebase-evidence.md](./research/codebase-evidence.md),需求见 [prd.md](./prd.md)。本文只讲边界、契约与取舍。

## 1. 架构总览

```
┌─ 页面层(非 LLM,冷启动兜底)─────────────────────┐
│ ChatPanel 空状态四分态卡 ──open──▶ SettingsModal │
│ MessageItemFooter 错误行 ──test──▶ test_model    │
└──────────────────────────────────────────────────┘
┌─ LLM 层(≥1 个活 provider 后生效)───────────────┐
│ L0: <available-skills> 三行(llm-setup/doctor/   │
│     onboarding) ← GlobalBuiltin skill 层        │
│ L1: use_skill 取 SKILL.md 全文                   │
│ 工具: llm_diagnostics(只读) +                  │
│       test_llm_connection(1-token 实测)         │
└──────────────────────────────────────────────────┘
```

两层互不依赖、可独立提交(见 implement.md 分步)。

## 2. skill 全局内置层(GlobalBuiltin)

- `SkillSource` 加 `GlobalBuiltin`(`skill/loader.rs:63-78`);**优先级垫底**:merge 插入顺序 `global-builtin → user → project → builtin-plugin → plugin`,查找顺序反向(先命中返回)——照 agent 链 `… > Project > User > Builtin` 先例(spec 12 号),用户/项目同名可覆盖 app 默认。
- 源:`app/src-tauri/resources/builtin-skills/{llm-setup,doctor,onboarding}/SKILL.md`,编译期 `include_str!` 常量(照 `agent/workflow/builtin.rs` 形制;`parse_skill_content` 复用同一 frontmatter parser,解析行为 100% 一致)。
- **部署边界(硬约束)**:include_str! 是唯一交付形态——内容随 daemon/GUI 二进制分发,运行时**零文件读取、零源码依赖、零 node/scripts**;daemon-only(thin 模式 / PWA remote / 无源码机器)天然可用。同款决策先例 = 09-14 mcp.rs 内置预设(GCE stdio MCP「绑定源码检出」教训的收敛物)。显式否决:运行时扫安装目录、以仓库 `.agents/skills/`(Trellis 开发工具,另一机制)或 scripts/ 为载体。
- 可见性:
  - L0 清单(`build_skill_listing_block`,注入点 `init.rs:641-711`):非 workflow 会话现只有 user+project,改为 ∪ global-builtin(被同名高层覆盖时不重复);
  - `use_skill` 解析(`find_skill_with_workflow` → `find_skill_in_layers`)加同层;
  - `/` 面板:`list_skill_infos`(`loader.rs:456-458`)加 global-builtin 层,panel 去重规则不变(builtin command > skill > custom command;skill 层内同名高层覆盖底层)。
- workflow 会话:global-builtin 同样并入(plugin/builtin-plugin 仍在其上,同名 wf-* 不受影响;本任务三名字与 wf-* 无冲突)。
- 不动:`SkillCache`(磁盘 mtime fence 只管磁盘层,内置常量无缓存问题)、`locate_agent_file` 类路径 API(内置层只读,与 BuiltinPlugin 同款 InvalidInput 语义——skill 侧无 locate API,天然无需)。

### 三份 SKILL.md 内容边界

- frontmatter:`name` + `description`(英文,含 when-to-use,是 L0 唯一触发面,按 search_history description 先例打磨);`allowed-tools` 标注 `[llm_diagnostics, test_llm_connection]`(信息性)。
- `llm-setup`:provider 知识表(七家:Anthropic/OpenAI/DeepSeek/GLM/Kimi/OpenRouter/Ollama;每家 base_url、协议、注意事项)→ 配置步骤(Settings 路径)→ 验证(test_llm_connection)→ **安全规则:永不让用户在对话里粘 API key**(理由:对话上下文会发给 LLM 提供商)。篇幅目标 ≤4KB。
- `doctor`:问诊(intake 症状与最近错误文案)→ `llm_diagnostics` 读配置 → `test_llm_connection` 实测可疑模型 → 五类错误 → 修复动作映射表(auth→Settings 换 key;rate_limit→等待/换默认模型;network→查 base_url/代理/DNS;server→服务商状态页/稍后重试;invalid_request→改模型名/参数)→ 何时建议 `/llm-setup`。
- `onboarding`:能力地图(三档模式/权限审批/工具族概览/skill 发现方式 `/`/定时任务/群聊审议/worktree 多项目/记忆系统)+ 开第一个项目步骤 + 「配置模型找 llm-setup、出问题找 doctor」交叉指引。内容点到为止,详细操作以 UI 为准,避免随功能演化过期。

## 3. 两个只读 agent 工具

### 3.1 llm_diagnostics(`tools/llm_diagnostics.rs`)

- schema:`{}`(无参数);description 英文说明「snapshot of LLM provider/model configuration for diagnosis; never contains API keys」。
- 实现:`ctx.db` 读 `list_providers` / `list_models` + `app_config.default_model_id`;**手工构造脱敏视图**:

```rust
// ❌ ProviderRow 直接 serde 序列化 —— api_key 明文泄露
// ✅ 逐字段构造,api_key/api_key_enc 均不引用;has_api_key = !api_key_enc.is_empty()
```

- 输出(LLM 紧凑文本/JSON):providers(id 短形、display_name、protocol、base_url、disabled、has_api_key)、models(同形)、default_model_id、一行 summary(几配几禁几缺 key)。
- 权限:未列名 → ToolKind::Other → Tier 5 静默放行;串行;不进 STUB_CANDIDATES(无参小 schema);worker 默认可见(只读无害)。

### 3.2 test_llm_connection(`tools/test_llm_connection.rs`)

- schema:`{ model_id?: string }`(缺省 = 默认模型;再缺省报「no default model」is_error)。
- 实现:前置 `test_model_inner` 签名收敛 `(state: &Arc<AppState>, …) → (db: &SqlitePool, …)`(该函数本就只用 state.db);工具调 `test_model_inner(&ctx.db, model_id)`,把 `{success, latencyMs, error}` 翻译成 LLM 紧凑文本,失败时附分类提示文案(供 doctor 直接引用)。
- **语义零改动**:per-model 1-token round-trip、15s 超时、错误矩阵、`body.model`=model_name 等全部遵守 spec `backend/test-model-contract.md`;两调用方(Tauri command `commands/providers.rs:552`、daemon 路由 `daemon/routes/providers.rs:257`)改传 `&state.db`,wire 契约不动。
- egress 论证(照 web_search 先例写进 description 注释):端点 = 用户自己配置的 provider base_url,与正常聊天同一 egress 面,固定最小 payload,无用户可控 URL 拼接之外的攻击面;真实产生 1-token 计费,description 中明示慎用。

### 3.3 注册与预算

- `tools/mod.rs` 三处:模块声明 / `builtin_tools()` 末尾追加两 definition / `execute_tool_inner` 两 match arm。
- 静态预算:`static_token_budget_classic_chat_first_turn`(stub.rs:348-389,现 ≤4200)按新增后实测平移,实测值入注释(schedule 家族先例)。L0 清单 +3 行属消息块不计入该测试。
- 群聊:`group_chat_tool_defs` 白名单不加 → 群聊 speaker 调不到两工具(自动排除),skill L0 可见无害。

## 4. 前端

### 4.1 settingsModal 打开通道(新 `stores/settingsModal.ts`)

- 新 pinia store:`{ open: boolean; initialCategory: string | null }` + `openSettings(category?)`。
- `Sidebar.vue` 的本地 `settingsOpen` ref 改绑 store(:74-78, :238);`SettingsModal.vue` 加 optional prop `initialCategory`,打开时若非空则本次跳过 localStorage 恢复直落该分类,消费后由 store 清空(一次性,不污染用户上次停留记忆)。
- 备选(否决):provide/inject 跨 ChatWindow→Sidebar 层级脆;事件总线无先例。

### 4.2 空状态四分态(ChatPanel.vue:1096-1119)

```
!config.loaded            → 原空状态(gate,防闪变)
providers.length === 0    → 卡 A「还没有可用的模型」3 步 + [打开设置 → Providers]
                            尾注:配好后可 /llm-setup 找 AI 帮忙、/doctor 排障(纯文案)
models.length === 0       → 卡 B「provider 已就绪,去添加模型」+ [打开设置 → Models]
!defaultModelId           → 卡 C「选择默认模型」+ [打开设置 → Models]
else                      → 原「开始对话」空状态(逐字节不变)
```

- 数据源 `useProvidersStore` / `useModelsStore`(启动即载,config.ts:161-169);样式照 `EmptyProjectState.vue` BEM + design token;ChatPanel 新增对应 vitest(pinia + mock transport,照 MessageItem.test.ts 模式)。

### 4.3 错误行「测试连接」(MessageItemFooter.vue)

- 新 props:`modelId?: string`;按钮渲染条件 = `error && modelId && category ∈ {auth, network, server}`(invalid_request 是请求构造问题,测连接无意义;rate_limit 已有 retry)。挂 retry 按钮旁(:214-223),复用 `btn btn--sm`。
- 组件保持零 store:按钮 `emit("test-connection")`;父 `MessageItem.vue` 持有 testState(照 ModelsTab runTest TestState 三态),经 prop 回传行内渲染(成功「连接正常 · 412ms」/ 失败错误文案);modelId 由父解析(session.model_id,群聊 participant 解析先例 :233-242,解析不到不传 → 按钮隐藏)。
- 调用 `transport.invoke("test_model", { modelId })`——三传输模式全通(daemon 路由 + CMD_TO_DOMAIN 映射已存在,http.ts:175),零新增命令映射。

## 5. 兼容与回滚

- 无 DB 迁移、无 daemon wire 变化、无 settings 持久化格式变化(localStorage 行为保留)。既有用户(配置齐)可见差异 = L0 多 3 行 + `/` 面板多 3 项 + 工具面多 2 个,聊天行为零变化。
- **部署面**:skill/工具/前端全随二进制与 dist 分发,daemon-only 场景(独立 daemon + PWA/remote)不依赖源码检出;本任务不改变既有 dist 分发模型。
- 回滚:四个实施步各自独立成 commit,单步 revert 即回滚该面;skill 层与工具面不动前端,前端两步不动后端。

## 6. 取舍记录

- **全局内置层垫底(而非高于 user)**:app 默认应可被用户定制覆盖;agent 链已有同款先例,一致性强于「内置优先」。
- **不建错误历史存储**:doctor 的输入 = 用户口述症状 + 现场实测,错误持久化是新存储面,收益存疑(MVP 砍,PRD 已列 OOS)。
- **不进 STUB_CANDIDATES**:两工具 schema 极小(0-1 参),stub 化反而多一轮 load 往返(search_history 校准注 3 同款决策)。
- **测试连接不解决 invalid_request**:语义上那是请求/模型名构造错误,连接层测试给不了新信息。
