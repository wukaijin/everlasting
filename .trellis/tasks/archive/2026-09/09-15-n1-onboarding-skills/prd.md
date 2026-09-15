# N1 首次引导:轻页面引导 + 内置诊断/引导/配置 LLM skills

## Goal

消除新用户冷启动与 provider 配置排障的死胡同(BACKLOG 附录 B N1,群聊共识 P0「留存漏斗」)。方向已按用户裁定调整(2026-09-15):**不做重 3 步向导**;页面只做轻量状态检测引导,重心是内置三类 skills——诊断式(doctor)/ 引导式(onboarding)/ 配置 LLM 友好(llm-setup)——依托既有 B4 skill 系统,让 LLM 自己会配、会修、会带新人,并配两个只读 agent 工具让诊断有数据可查。

## Background(第一性原理约束)

**零 provider / 全部 provider 失效时 LLM 无法运行,skill 帮不上忙。** 因此冷启动与「唯一 provider 挂了」两条路径必须由页面(非 LLM)承载;skill 的价值区间 = 「至少一个 provider 活着」之后:配新 provider 的知识、模型选型、经活 provider 诊断死 provider、新用户带逛。分层据此划定。

证据汇总见 [research/codebase-evidence.md](./research/codebase-evidence.md)(skill 四层/工具注册/权限/test_model 契约/前端改动点/错误分类链,含 file:line)。

## Requirements

### R1 页面轻引导(状态检测卡 + 错误行行动点)

- R1.1 聊天空状态(`ChatPanel.vue:1096-1119`)按配置现状分态,以 `config.loaded` 为 gate 防加载闪变:
  - 无 provider → 引导卡:① 添加 provider ② 粘贴 API key ③ 添加模型并测试;按钮一键打开 Settings 并落在 **Providers** 分类;
  - 有 provider 无模型 → 引导卡按钮落 **Models** 分类;
  - 有模型未设默认 → 提示选默认模型,按钮落 Models 分类;
  - 配置齐 → 保持现状「开始对话」空状态,零行为变化。
- R1.2 聊天错误行(`MessageItemFooter.vue:206-224`)对 auth / network / server 类错误新增「测试连接」行动点:直调既有 `test_model`(非 LLM、三传输模式可用),结果行内呈现(成功延迟 / 失败错误文案);model_id 取 session 级(群聊场景解析不到则隐藏按钮)。

### R2 全局内置 skill 层 + 三件套

- R2.1 新增 skill 全局内置层(GlobalBuiltin):编译期 `include_str!`(源 `app/src-tauri/resources/builtin-skills/`),**所有会话可见**(L0 清单 + use_skill + `/` 面板);优先级垫底(用户/项目同名可覆盖),照 agent 链 `… > User > Builtin` 先例。
- R2.2 三份内置 SKILL.md(名字暂定,英文 description,面向 LLM 内容):
  - `llm-setup`(配置 LLM 友好):常见 provider 知识库(Anthropic / OpenAI / DeepSeek / GLM(智谱)/ Kimi(Moonshot)/ OpenRouter / 本地 Ollama 的 base_url、协议选择 anthropic|openai、已知坑)+ 配置步骤 + 配好经 `test_llm_connection` 自测;内嵌安全规则:**永不要求用户把 API key 粘进对话**(对话内容会发给 LLM 提供商,key 只进 Settings 表单)。
  - `doctor`(诊断式):排障入口——问症状 → `llm_diagnostics` 读配置态 → `test_llm_connection` 实测可疑模型 → 按错误分类(auth=481/key 失效、rate_limit=429/529 过载、network=base_url/代理/DNS、server=服务商状态、invalid_request=模型名/参数)给修复步骤。
  - `onboarding`(引导式):新用户带逛——能力地图(三档模式 / 权限系统 / 工具概览 / skill 发现 / 定时任务 / 群聊 / 多项目 worktree / 记忆系统)+ 怎么开第一个项目 + 指向另两个 skill。

### R3 只读 agent 工具面(诊断的数据底座)

- R3.1 `llm_diagnostics`(只读):返回 providers(id/display_name/protocol/base_url/disabled/**has_api_key: bool**)、 models(id/provider_id/model_name/display_name/disabled/context_window/supports_*)、default_model_id。**api_key 明文与密文绝不出现在任何输出**(ProviderRow 不得直接序列化,单测断言)。
- R3.2 `test_llm_connection`:可选参数 `model_id`(缺省用默认模型);复用 `test_model_inner` 语义(per-model 1-token round-trip,契约见 spec `backend/test-model-contract.md`,语义零改动),仅收敛签名为 `(db: &SqlitePool, model_id)`(两调用方 Tauri command / daemon 路由同步)。
- 权益面:两工具均 ToolKind::Other → Tier 5 静默放行(search_history 先例)、串行(不进 L2 并行名单)、不进 STUB_CANDIDATES(schema 小);api_key 材料边界 = R2.2 安全规则 + R3.1 脱敏。

## Constraints

- **部署边界(用户 2026-09-15 追加)**:全部内置内容(skill 三份 + 双工具)编译期嵌入二进制(`include_str!`),**daemon-only 部署必须可用**——无源码检出、无 node、无 `scripts/`、无仓库 `.agents/skills/`(那是开发仓 Trellis 工具,与本机制无关)。承 GCE stdio MCP 部署面教训(绑定源码检出 → 09-14 收敛 daemon 的动机);禁止任何运行时读源码目录或以仓库脚本为交付物的形态。
- tools[] 静态 token 预算线(`static_token_budget_classic_chat_first_turn`,≤4200)随两新工具 schema 实测平移,注释记实测值(schedule 家族平移先例)。
- daemon HTTP API 契约零变化(test_model wire 不动);DB 零 schema 变更;GUI 既有路径零行为变化(配置齐时空状态与现状逐字节一致)。
- `http.routes-sync.test.ts` 守卫不得红(不新增命令映射,天然满足)。

## Acceptance Criteria

- [ ] AC1 Rust 单测:全局内置层三 skill 在普通会话 L0 清单可见、use_skill 可解析;同名 project/user skill 覆盖全局内置;`/` 面板列出三 skill 且同名用户 skill 优先。
- [ ] AC2 Rust 单测:`llm_diagnostics` 输出含 providers/models/default_model_id 三段,且序列化全文不含 api_key 明文与 api_key_enc 密文(脱敏断言)。
- [ ] AC3 `test_model_inner` 签名收敛后:IPC / daemon 路由行为不变(既有测试 + routes-sync 全绿);`test_llm_connection` 缺省 model_id 走默认模型、缺默认/缺行错误路径有单测。
- [ ] AC4 静态 tools token 预算测试线平移并通过,实测值入注释。
- [ ] AC5 前端 vitest:空状态四分态(未加载 gate / 无 provider / 有 provider 无模型 / 齐配原样)+ 引导卡按钮打开 Settings 并落对应分类;config.loaded 防闪变。
- [ ] AC6 前端 vitest:错误行「测试连接」按钮分态(auth/network/server 显示、invalid_request 不显示、无 modelId 隐藏)、emit 接线、行内结果渲染(running/ok/fail)。
- [ ] AC7 手动冒烟(live):真 provider 配置走空状态 → 一键开 Settings → 测试连接闭环;`/doctor` 实跑调 llm_diagnostics + test_llm_connection 出结构化诊断;`/llm-setup` 指导配置并自测;`/onboarding` 带逛;配置齐全的既有项目界面零变化。
- [ ] AC8 门禁全绿:`cd app && pnpm test`、vue-tsc 零错、`cargo test -p everlasting --lib`(WSL 需 PKG_CONFIG_PATH)、clippy 对 main 基线零新增。
- [ ] AC9 daemon-only 冒烟(零 LLM、零源码依赖):仅 daemon 服务(thin/独立二进制)经 HTTP 路由——`/` 面板 list 含三 skill、`get_skill_body` 可取三份全文;证明内容随二进制分发而非读源码检出。

## Out of Scope

- 配置**写**工具(add_provider/set_default_model 等)与相应权限域——真实需求出现再立项。
- LLM 错误历史持久化(doctor 依赖现场症状 + 实测,不建错误日志存储)。
- 重向导 UI / 空状态内嵌表单(用户已否);i18n;群聊场景的 skill/工具特化(群聊工具白名单外自动排除,默认行为即可);远程 PWA 特化。
- onboarding 的「首次会话自动注入」机制(L0 description 匹配已足够,不加规则引擎)。

## 已定决策记录

| 决策 | 结论 | 时间 |
|---|---|---|
| 页面引导形态 | 状态检测卡(非最小文案/非内嵌表单) | 2026-09-15 |
| skill 集 | 三件套 llm-setup / doctor / onboarding,走新全局内置层 | 2026-09-15 |
| agent 工具面 | 只读 + 实测两工具;写配置缓做;api_key 永不进对话上下文 | 2026-09-15 |
