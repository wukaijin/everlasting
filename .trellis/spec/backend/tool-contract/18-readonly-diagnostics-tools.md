## Scenario: 只读诊断双工具 llm_diagnostics / test_llm_connection(09-15-n1-onboarding-skills)

### 1. Scope / Trigger

- Trigger: 改动 `app/src-tauri/src/tools/llm_diagnostics.rs` / `test_llm_connection.rs`、`test_model_inner` 的调用面、或内置诊断 skill(doctor/llm-setup)对工具输出的消费契约时。
- 动机:N1 首次引导的 LLM 侧——诊断 skill 需要真实数据(配置快照 + 实测)才能工作,纯知识型 skill 是玩具。

### 2. Signatures

```rust
// tools/llm_diagnostics.rs — 无参数,只读
// execute(ctx) 读 list_providers/list_models + app_config.default_model_id

// tools/test_llm_connection.rs — model_id 可选(缺省 = 默认模型)
// execute(ctx) 调 commands::providers::test_model_inner(&ctx.db, model_id)

// commands/providers.rs — 2026-09-15 签名收敛(state 本就只用 state.db):
async fn test_model_inner(db: &SqlitePool, model_id: String) -> Result<serde_json::Value, String>
// 三调用方:Tauri command(&state.db)/ daemon 路由(&state.db)/ agent 工具(ctx.db)
```

### 3. Contracts

- **llm_diagnostics 输出(脱敏铁律)**:providers(id 全形/display_name/protocol/base_url/disabled/`has_key`)+ models(id/provider_id/model_name/display_name/disabled/context_window/supports_*)+ default_model_id + 一行 summary。
  - **字段名必须是 `has_key` 而非 `has_api_key`**:输出全文不含 `api_key` 子串是最强脱敏断言口径,字段命名服务于断言(与 `ProviderRow::has_key` 同名)。
  - **id 必须全形不截断**:doctor 流程 = diagnostics 引用精确 `models.id` → `get_model` 精确匹配,截断 id 会断链。
- **test_llm_connection 输出**:成功一行(`ok latency=…ms model=…`);失败 = test_model 的 `{success, latencyMs, error}` 翻译 + 五类错误(auth/rate_limit/network/server/invalid_request)修复提示文案(分类与 `llm/error.rs` 同源)。
- 权益面:两工具均 ToolKind::Other → Tier 5 静默放行、串行(不进 `NAME_ELIGIBLE`)、不进 `STUB_CANDIDATES`(小 schema,同 15 号 search_history 决策)、群聊 `group_chat_tool_defs` 白名单不加(自动排除)。

### 4. Validation & Error Matrix

| 条件 | 行为 |
|---|---|
| `llm_diagnostics` 任何路径 | 输出全文禁 `api_key` 子串(明文/密文都不出现);**禁 serde 序列化 ProviderRow 原行**(`api_key` 明文字段) |
| `test_llm_connection` 无 model_id 且无默认模型 | `is_error=true` 文案(不发请求) |
| model_id 缺行 | 透传 test_model 错误矩阵("model … not found",latencyMs=0) |
| 未知协议 | "unsupported protocol"(test-model-contract 矩阵原样) |

### 5. Good/Base/Bad Cases

- **Good**:doctor skill 问诊 → llm_diagnostics 快照 → test_llm_connection(缺省)→ 默认模型 944ms ok → ask_user_question 问哪个模型有 trouble(live 实跑 2026-09-15,session 转录见任务 implement 记录)。
- **Base**:db 空表 → 快照输出空 providers/models + summary "0 providers";非 error。
- **Bad**:`serde_json::to_string(&provider_row)` 直接序列化 → api_key 明文进 tool_result → 进 LLM 上下文 → 发给(别家)LLM 提供商。这是本 scenario 存在的理由。

### 6. Tests Required

- `llm_diagnostics`:输出三段齐全 + **三重脱敏断言**(`api_key` 子串 + 明文 canary + DB 实取密文)——`execute_output_never_contains_key_material`。
- `test_llm_connection`:缺默认/缺行/未知 id/坏协议四路径(免 HTTP 臂;wire 级无自动化测试是 test-model-contract §6 的既定契约,手动冒烟替代)。
- 预算线:加两工具后 `static_token_budget_classic_chat_first_turn` 实测平移(09-15 实测 4410 → 线 4500,校准注 7)。

### 7. Wrong vs Correct

#### Wrong — 序列化整行

```rust
// ❌ ProviderRow 含解密后的明文 api_key(#[serde(skip)] 只挡 IPC,不挡这里手写 json!)
let out = json!({ "providers": providers });  // providers: Vec<ProviderRow>
```

#### Correct — 逐字段构造脱敏视图

```rust
// ✅ 手工挑字段;has_key 命名保住 !out.contains("api_key") 断言口径
let view = json!({ "id": p.id, "protocol": p.protocol, "has_key": !p.api_key_enc.is_empty(), /* … */ });
```

### Related

- [test-model-contract.md](../test-model-contract.md) — test_model 全契约(本工具的探测内核,语义 1:1)。
- [12-builtin-plugin-source-layer.md](./12-builtin-plugin-source-layer.md) — GlobalBuiltin skill 层(消费这两个工具的 doctor/llm-setup 所在层)。

---
