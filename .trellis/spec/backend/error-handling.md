# Error Handling

> How errors are handled in this project. 全栈错误契约的单一事实源 spec。
> 09-21 起由任务 09-21-error-bus-category-recovery 补齐(此前主体是模板;四层模型曾只活在代码注释里)。

---

## Overview:四层模型

```
① 错误类型层(Rust)        10 个领域错误 impl AppError(category/user_message/retryable)
                                ↓ From<E> for AppCommandError
② wire 层                  AppCommandError{category,kind,message,retryable,requestId?} — 全字段 camelCase
                                ↓ 双通道同 shape
③ 传输层                   Tauri IPC reject(序列化对象)/ daemon HTTP(status=category 1:1 + body)
                                ↓ HTTP 通道经 TransportError(09-21 起携带 ② 的四字段)
④ 前端消费层               useErrorBus.handle 全局兜底分级路由 / extractErrorMessage(30+ 调用点)
                            流式独立通道:ChatEvent::Error → message.error{message,category} → footer 重试
```

分层要点:

- ② 与 ③ 是**两个独立通道**(IPC command vs ChatEvent stream),category 类型不复用(设计决策,见 error.rs Overview 注释)。
- ④ 的流式链是 category 端到端保真的范本:SSE error → `streamEvents.ts` → `MessageItemFooter` 按 `categoryRetryable` 出重试按钮 → `retryChat` 原位重开。
- ④ 的 command 链在 09-21 前有断点(TransportError 丢 category、全局兜底误标 Server),09-21 修复,契约见下。

---

## Error Types

**`ErrorCategory`(5 类,Rust `error.rs` + 前端 `utils/error.ts` 双端定义,值域锁死)**:

| category | HTTP | retryable(默认派生) | 前端路由 |
|---|---|---|---|
| Auth | 401 | false | toast 引导(检查 key) |
| RateLimit | 429 | true | toast |
| InvalidRequest | 400 | false | **不 toast**,console.warn(本地错误不打扰) |
| Server | 500 | true | toast + 重试 |
| Network | 502 | true | toast |

retryable 派生三处同源(Rust `AppError::retryable()` 默认 impl / 前端 `categoryRetryable` / wire 字段透传优先),改任何一处必须同步另两处或改成 wire 字段——禁止前端私加第四处派生。

**category 字符串双 case 现状(有意容忍,勿"统一")**:command 链 PascalCase(`"RateLimit"`),流式链 snake_case(`"rate_limit"`,ChatEvent serde)。`utils/error.ts` 的双 case union 是 helper 入参容忍类型;**字段类型单一事实源 = `AppErrorCategory`(PascalCase 五值 union,`utils/error.ts` 导出)**,transport/useErrorBus 的字段声明只准 import,不准本地定义。

**`AppCommandError` wire 字段(全 camelCase)**:`category` / `kind`(类型短名,诊断用)/ `message`(中文,直接展示)/ `retryable` / `requestId?`(高频 command 透传前端 requestId)。

---

## Error Handling Patterns

**daemon 侧**:每个 handler 返 `Result<Json<T>, AppCommandError>`,`IntoResponse` 按 category 出 status(上表),body 为同一序列化 struct(`daemon/error.rs`,有 `status_mapping_is_stable` 测试锁定)。

**前端全局兜底 `useErrorBus.handle(e)` 分级(09-21 起,顺序不可换)**:

1. **形状识别**(category/kind/message/retryable 四字段 + category 五值域;TransportError 即此形状)→ push errors + 按 category 路由 toast/console;
2. `isBenignBrowserNoise(string)`(白名单前缀 `ResizeObserver loop`)→ console.debug,丢弃;
3. 其他裸 string → console.warn,**不 push FIFO、不 toast**(不标 Server——未知本地错误不该弹「服务端错误」);
4. 其他 Error 实例 → console.error(`name==="TransportError"` 单独打标 `[errorBus:transport-shape-miss]`,形状门被打破时可从 console 发现)。

**禁止的兜底模式**:「未知 → 标 Server」。任何把无法识别的输入默认成 Server 的兜底,都会把本地/协议噪音弹成假「服务端错误」(09-21 事故:ResizeObserver loop 警告;群聊评审同日裁决:status=0 unknown-cmd 路径若兜 Server 会复刻同病)。

**吞错可见性**:见下方 RULE-ERR-SURFACE-001;前端同判据——handle 各分支必须有可见通道(debug/warn/error),不得无声。

---

## API Error Responses

### Scenario: daemon 错误体 → TransportError category 恢复(09-21)

#### 1. Scope / Trigger

http transport(默认主通道)消费 daemon 错误响应;TransportError 是前端主链路错误的唯一载体,category/retryable 必须跨边界存活。

#### 2. Signatures

```ts
// app/src/transport/http.ts
export class TransportError extends Error {
  public readonly status: number;                  // 401 处理等只读 status,不变
  public readonly body: TransportErrorBody | string;
  public readonly category: AppErrorCategory;      // 恢复链见 §3
  public readonly kind: string;                    // body.kind ?? "Transport"
  public readonly retryable: boolean;              // body.retryable ?? categoryRetryable(category)
  public readonly requestId?: string;              // wire 即 camelCase,同名直读
}
```

#### 3. Contracts

- body 字段名 **camelCase**(`category`/`kind`/`message`/`retryable`/`requestId`);`TransportErrorBody` 声明这些字段 + `[key: string]: unknown` 兼容残留。
- body 可能是 string(非 JSON 路径,http.ts unknown-cmd 兜底)——全部字段读取走「body 为对象才读」守卫。
- **category 恢复链(优先级序)**:① `body.category` 过五值域校验(脏值按缺失);② canonical 逆映射 401→Auth / 429→RateLimit / 400→InvalidRequest / 500→Server / 502→Network;③ 分档兜底(status=0 与非 canonical 4xx→InvalidRequest,其余 ≥500→Server——「Server」仅构造上不可达的防御默认)。
- 形状门不变量:**任何** status × 任何脏 body 构造出的 TransportError 实例恒通过 `isAppCommandError`(接缝由构造收窄保证,有 42 组合不变量测试)。

#### 4. Validation & Error Matrix

| 输入 | category 结果 | 用户可见 |
|---|---|---|
| 429 + body 全字段 | body.category(如 RateLimit) | toast(可重试) |
| 500 + body 残缺 | Server(逆映射) | toast |
| 502 + body 残缺 | Network(逆映射) | toast |
| 0(unknown-cmd)/ 404 等非 canonical 4xx | **InvalidRequest(分档)** | **不 toast**,console.warn |
| 503 等其他 ≥500 | Server(分档) | toast |
| body.category 脏值 | 按缺失走 ②③ | 同上 |

#### 5. Good/Base/Bad Cases

- Good:daemon 正常 4xx/5xx → body 权威,category 逐字段恢复,未捕 rejection 按真实 category 路由。
- Base:body 非 JSON / 残缺 → status 分档,unknown-cmd 类不骚扰用户。
- Bad(禁止):「其余 status 兜 Server」——status=0 前科路径(handoff_session / list_queued_messages)会弹假「服务端错误」。

#### 6. Tests Required

- `app/src/transport/http.test.ts`:恢复/逆映射/分档/构造收窄不变量(脏 body × status 全组合过形状门)。
- `app/src/utils/useErrorBus.test.ts`:**真 TransportError 实例**过 handle 的接缝集成用例(手写形状对象测不出;含「形状识别先于 instanceof Error」的可失败顺序断言)。

#### 7. Wrong vs Correct

```ts
// Wrong:兜 Server + snake_case 读值(两个历史前科)
this.category = validBodyCategory ?? categoryFromStatus(status) ?? "Server";
const rid = b?.request_id;   // daemon body 是 camelCase,恒 undefined 死字段

// Correct:分档兜底 + camelCase 直读
this.category = validBodyCategory
  ?? categoryFromStatus(status)
  ?? fallbackCategory(status);   // 0/非canonical 4xx→InvalidRequest,≥500→Server
const rid = b?.requestId;
```

---

## Common Mistakes
<!-- 错误处理上真实踩过的坑;新条目按日期追加 -->

### RULE-ERR-WIRE-CASE(2026-09-21,任务 09-21-error-bus-category-recovery):wire 字段 casing 三轨,读值前先对层

**事故**:design 阶段误记「wire 是 `request_id`(snake_case)」,照此实现则 `TransportError.requestId` 恒 undefined(死字段);单测手写 snake body 反而把错误锁死,check 阶段对照 Rust 单测(`"requestId":null`)才发现。

**规则**:三轨并存且都是有意设计——① command 错误体 **camelCase**(`AppCommandError`,`rename_all="camelCase"`);② ChatEvent/message.error 的 category **snake_case**(`"rate_limit"`);③ 前端内部路由键 **PascalCase**(`"RateLimit"`)。跨边界读值前先确认在哪一层;`utils/error.ts` 的 helper(categoryToastKey/categoryRetryable)双 case 容忍,新代码不得再产出新 case 形态。

### RULE-ERR-SURFACE-001(2026-09-01,session 2e438939):吞错必须对 LLM 可见

**事故**:`resolve_current_task` 按容错策略跳过解析失败的 `task.json`(只 warn 日志),workflow 会话因此每轮都解析为「无 active task」;同时 `create_task` 的 AlreadyExists 报错指向一个不存在的「open the existing one」工具。LLM 被两个互斥的守卫卡死,烧了 ~25 条消息后靠 `rm -rf` 任务目录脱困。同 session 另发:子代理流中网络错误后整 run 报废、进度全丢,parent 从零重派。

**规则**:

1. **面向 LLM 的恢复路径上,任何被吞掉的错误必须透传到模型可见的通道**(breadcrumb / tool_result / system 注入),不能只进 daemon 日志——日志模型看不见,等于让引擎和模型活在两个世界里。
2. **错误提示只允许指向真实存在的工具或动作**;提示里出现的每个「下一步」都必须是模型可执行的调用。
3. **自愈型容错(serde default / lenient parse)与显式报错互补**:读侧对常见手写缺口给 default,仍解析失败的文件把 `(slug, 原始错误)` 透出到 breadcrumb 让模型修文件。
4. **worker 错误退出时若历史仍 pair-safe(合成 tool_result 补对 + marker),保留 messages 供 `resume_from` 续跑**,并在 tool_result 里明确告知 parent 怎么续(见 `drive.rs` 09-01-subagent-network-resume 块)。
