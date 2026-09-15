# Implement — 执行清单

> 前置阅读:research/codebase-evidence.md(A-E 节)。每步独立可提交、独立可回滚;步序 = 依赖序(1/2 后端 → 3 内容 → 4/5/6 前端 → 7 门禁)。

## Step 1 后端:skill 全局内置层

- [x] `skill/loader.rs`:`SkillSource::GlobalBuiltin`;`merge_skill_layers` 插入顺序最前(global-builtin → user → …)、`find_skill_in_layers` 查找最底;`build_skill_listing_block` / `list_skill_infos` 并入该层(同名高层覆盖不重复)。
- [x] 内置常量:新建 `resources/builtin-skills/` 三目录占位 SKILL.md(内容 Step 3 填),`include_str!` 常量 + `global_builtin_skills()`(照 `builtin_plugin_skills()` loader.rs:302-316 形制,`parse_skill_content` 复用)。
- [x] 单测:空项目普通会话 → 三 skill 可见/可解析(AC1);同名 project/user skill 覆盖;`/` 面板 list 含三 skill(dedup 高层赢);workflow 会话 wf-* 不受影响(既有测试全绿)。
- 验证:`cargo test -p everlasting --lib "skill::"`(WSL 需 `PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"`,下同)

## Step 2 后端:两个只读工具 + test_model_inner 签名收敛

- [x] `commands/providers.rs`:`test_model_inner(state: &Arc<AppState>, …)` → `(db: &SqlitePool, …)`;两调用方(command :552 / daemon routes/providers.rs:257)改传 `&state.db`;**语义零改动**(spec test-model-contract.md 全矩阵保持)。
- [x] `tools/llm_diagnostics.rs`:definition(无参,英文 description)+ execute(ctx.db 读 list_providers/list_models/default_model_id,手工脱敏视图,**禁序列化 ProviderRow 原行**);单测:输出三段齐全 + 序列化全文不含 `api_key`/`api_key_enc` 子串(AC2)。
- [x] `tools/test_llm_connection.rs`:definition(`model_id?`)+ execute(缺省解析默认模型;缺默认/缺 model 行 → is_error 文案);错误路径单测走 db 缺行臂(不发真 HTTP,与既有「无自动化 HTTP 测试」契约一致,AC3)。
- [x] `tools/mod.rs`:模块声明 + `builtin_tools()` 末尾追加 + `execute_tool_inner` 两 arm。
- [x] `tools/stub.rs`:`static_token_budget_classic_chat_first_turn` 预算线按实测平移,注释记实测值(AC4);确认 STUB_CANDIDATES 不加(两数组长度不变量测试保持绿)。
- 验证:`cargo test -p everlasting --lib`;`cargo clippy -p everlasting --lib` 对 main 基线零新增

## Step 3 三份 SKILL.md 内容(design §2 边界)

- [x] `llm-setup`:七家 provider 知识表 + 配置步骤 + 验证 + 「key 不进对话」安全规则。
- [x] `doctor`:问诊 → llm_diagnostics → test_llm_connection → 五类错误修复映射 → 指向 llm-setup。
- [x] `onboarding`:能力地图 + 开第一个项目 + 交叉指引。
- [x] 每份 frontmatter description 按「L0 唯一触发面」标准打磨(when-to-use 明确);≤4KB/份。
- 验证:frontmatter 解析单测(loader 既有);`cargo test -p everlasting --lib "skill::"`

## Step 4 前端:settingsModal 通道

- [x] 新 `stores/settingsModal.ts`(open/initialCategory/openSettings);`Sidebar.vue` 本地 ref 改绑 store;`SettingsModal.vue` 加 `initialCategory` prop(一次性消费,不动 localStorage 记忆行为)。
- [x] vitest:store 状态机 + 打开落指定分类 + localStorage 记忆不被污染。
- 验证:`cd app && pnpm test -- settingsModal` + `pnpm vue-tsc`

## Step 5 前端:空状态四分态卡

- [x] `ChatPanel.vue:1096-1119` 四分态(design §4.2);引导卡照 EmptyProjectState BEM/token;引 providers/models store + config.loaded gate。
- [x] vitest:四分态 + gate 防闪变 + 按钮触发 openSettings('providers'|'models')(AC5)。
- 验证:`cd app && pnpm test -- ChatPanel`

## Step 6 前端:错误行测试连接

- [x] `MessageItemFooter.vue`:modelId prop + 按钮(auth/network/server 门控)+ emit;`MessageItem.vue`:testState 三态持有 + modelId 解析(session.model_id → 群聊 participant → defaultModelId → 隐藏)。
- [x] vitest:按钮分态(AC6)+ emit 接线 + 行内 running/ok/fail 渲染;`http.routes-sync.test.ts` 天然绿(零新命令)。
- 验证:`cd app && pnpm test -- MessageItem`

## Step 7 门禁 + 冒烟 + 收尾

- [x] 全量门禁(AC8):`cd app && pnpm test`;`pnpm vue-tsc`;`cargo test -p everlasting --lib`;clippy;lefthook pre-commit 会拦 cargo fmt——提交前跑 `cargo fmt`。
- [x] 手动冒烟(AC7,live;LLM 侧已验:/doctor 全闭环 + test_model 错误路径,GUI 走查留用户):① 临时清空 providers(或新 data dir)看空状态卡 → 一键开 Settings 落 Providers;② 配真 provider 走完 3 步卡消失回落原空状态;③ 人为填错 key 发消息 → 错误行「测试连接」出分类结果;④ `/doctor`、`/llm-setup`、`/onboarding` 三场对话实跑(doctor 需真调两工具出结构化结论)。
- [x] daemon-only 冒烟(AC9,零 LLM 零源码,验证部署边界):`./scripts/daemon.sh` 起独立 daemon(重建产物)→ ① POST 面板 list 路由断言三 skill 在列;② POST `get_skill_body` 断言三份全文可取;③(发布视角复查)对二进制 `grep -c "<SKILL.md 里的一段独有文案>"` 非零,证明内容编译期嵌入而非读检出。
- [x] spec 更新(Phase 3.3):test-model-contract.md 签名段补收敛记录;tool-contract 新场景或 12 号扩「全局内置层」;AGENTS/文档如涉及。

## 风险与回滚点

| 风险 | 缓解 |
|---|---|
| `skill/loader.rs` 分层改动伤既有优先级语义 | Step 1 单测矩阵(覆盖/不覆盖/workflow)先行,既有 skill 测试全绿为门 |
| `tools/mod.rs` 注册漏 arm(定义了分发不到) | 走既有「definition+match」成对惯例;check 阶段 grep definition 名 |
| 预算线平移引发 stub 不变量测试红 | stub.rs 两数组长度不变量测试保持不动;只移数值线 |
| SettingsModal 改动破坏 localStorage 记忆 | Step 4 专属 vitest 覆盖记忆不被一次性 initialCategory 污染 |
| 内置内容退化成「读源码检出」交付(GCE stdio 教训) | include_str! 编译期嵌入是唯一形态;AC9 daemon-only 冒烟(重建产物 + 无源码依赖探针)守门 |
| 前端闪变(store 未载先渲染卡) | config.loaded gate 有专属用例 |

回滚:Step 1-6 各自独立 commit,`git revert` 单步即回该面;无迁移无 wire 变化,无跨步耦合。

## 追记:daemon 侧真实浏览器 E2E(2026-09-15 晚,首轮提交后)

- 走查路径:Playwright 真实 Chromium 打 daemon 服的 dist(隔离实例
  `--data-dir /tmp --port 7457`),8/8 PASS;截图 `out/n1-daemon-walkthrough/`。
- **发现并修复真 bug**:error 终态 `last.error` 被 `reloadAfterFinalize`
  的 DB 权威替换冲掉(DB 不存错误态)→ 错误行/「测试连接」只闪现一瞬。
  修复 = `RequestState.terminalError` 暂存 + reload 后挂回最后 assistant
  行(streamEvents.ts / streamController.ts;spec 已沉淀到
  frontend/chat/session-busy-visibility.md;测试 terminalErrorPersist.test.ts)。
- **产品发现(待裁定)**:`db/config.rs` 首启幂等播种(2 provider 空key +
  4 模型 + 默认)使**真实新用户首启看不到卡A**(齐配态),冷启动引导实际
  走「首条消息 → EmptyApiKey auth 错误行 → 测试连接」路径。可选跟进:
  空状态加「默认 provider 缺 key」分态卡(卡A'),或维持现状(错误行路径
  已可用)。未单方面扩scope,留用户裁定。
- 环境注:bogus provider 走本机代理(HTTP_PROXY)时 502 归 Server 类而非
  Network —— 错误分类按实际响应,行为正确。
