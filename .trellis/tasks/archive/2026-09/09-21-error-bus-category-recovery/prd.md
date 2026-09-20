# 错误链路收口:全局兜底分类修正 + TransportError category 恢复

## Goal

修复错误处理链路的两个 P1 断点,让「错误从哪来」决定「错误怎么显示」:

1. **全局兜底不再错标**:浏览器本地噪音(ResizeObserver loop 等)不再以「服务端错误」toast 弹给用户;真正的运行时 JS 错误不再被静默丢弃。
2. **category 端到端保真**:daemon 明明放进 HTTP status + body 的 category/retryable,不再在 TransportError 边界蒸发,主链路(http transport)恢复与 Tauri IPC 路径同等的分类/重试语义。

用户价值:错误提示可信(不误报「服务端错误」)、可行动(Auth 引导 / 重试语义保留)、可诊断(kind/request_id 有出口)。

## Background / 证据

触发案例:用户展开 EDIT_FILE diff 卡片或随后滚动,弹「服务端错误 ResizeObserver loop completed with undelivered notifications.」完整解剖与全链路审计见 `research/error-pipeline-audit.md`,要点:

- `app/src/main.ts:32-35` 把所有 window error / unhandledrejection 送进 `useErrorBus.handle`;`app/src/utils/useErrorBus.ts:103-122` 的 `parseAppCommandError` 只认 AppCommandError 形状对象与裸 string:**裸 string 强标 `category:"Server"`**(误报根源),**Error 实例返回 null 被静默丢弃**(与 main.ts:23-27「不丢失」注释相反;`useErrorBus.test.ts` 无 Error 实例用例)。
- `app/src/transport/http.ts:276-296` `TransportErrorBody` 无 category 字段、构造只留 status+message;而 daemon 侧 `app/src-tauri/src/daemon/error.rs` 明明 1:1 映射 category→HTTP status(401/429/400/500/502)且 body 全字段序列化,注释还声称前端 parser "handles this shape unchanged"。http transport 是默认主通道。
- 流式错误链(SSE→message.error→footer 重试)已端到端 category 保真,是本任务的形态范本,不动。

## Requirements

### R1 全局兜底输入分类修正(useErrorBus + main.ts)

- R1.1 已知良性浏览器噪音(前缀匹配 `ResizeObserver loop`,含 `completed with undelivered notifications` 与 `limit exceeded` 两变体)在 main.ts 入口直接过滤,不进 errorBus、不 toast;`console.debug` 留痕。
- R1.2 `Error` 实例(非 AppCommandError 形状)不再静默:至少 `console.error` 兜底;是否 toast 由 design 决定(倾向:不弹——避免重蹈「运行时错误骚扰用户」覆辙,但绝不无声)。
- R1.3 裸 string 的 fallback 不再标 `Server`;改为不进 toast 的本地类目(console.warn),与 InvalidRequest 同待遇,且不 push errors FIFO(评审裁决:关死 AC2 的口子)。
- R1.4 `parseAppCommandError` 对 `TransportError` 的识别随 R2 联动(见 R2.3)。

### R2 TransportError category 恢复(transport/http.ts + error.ts)

- R2.1 `TransportError` 保留 `category`(优先读 body.category 且须过五值域校验;缺失时按 status canonical 逆映射:401→Auth / 429→RateLimit / 400→InvalidRequest / 500→Server / 502→Network;非 canonical status 分档兜底:status=0(unknown-cmd 路径,有生产前科)与非 canonical 4xx→InvalidRequest,其余 ≥500→Server,「Server」仅作构造上不可达的防御默认。群聊评审 09-21 裁决:原「其余兜 Server」会把 status=0 前科路径弹成假「服务端错误」)与 `retryable`(body 有则透传,无则按 category 派生,派生表与 `error.ts:54-71` / Rust `AppError::retryable()` 同源)。
- R2.2 `TransportErrorBody` 类型补 `category?/retryable?` 字段,`[key: string]: unknown` 兼容保留。
- R2.3 `extractErrorMessage` 及/或 errorBus 能从 `TransportError` 恢复 category,使 catch 点在不大改的前提下可拿到分类(具体接口形态 design 定;30+ 现有调用点零破坏是硬约束)。

### R3 回归与测试

- R3.1 `useErrorBus.test.ts` 补 Error 实例走 `handle` 的用例(此前盲区)。
- R3.2 transport 层测试补 category 恢复用例(body 带 category / 只带 status / 均无),并覆盖**构造收窄不变量**:脏 body(body 为 string/undefined/空对象、category/kind/retryable 脏值)构造出的 TransportError 实例恒通过 AppCommandError 形状门。
- R3.5 PR1×PR2 接缝集成用例:真 TransportError 实例经 `useErrorBus.handle` 按其 category 路由(手写形状对象测不出接缝,评审裁决)。
- R3.3 ResizeObserver 消息端到端不产生 toast(main.ts 过滤用例)。
- R3.4 现有 `pnpm test`(app)全绿;transport-parity 测试(`app/src/transport/transport-parity.test.ts`)不回归。

## Acceptance Criteria

- [ ] AC1 手工/e2e 复现原案例:展开 diff 卡 + 滚动,不再出现「服务端错误 ResizeObserver loop…」toast;devtools console 有 debug 级留痕。
- [ ] AC2 单测:裸 string(非 JSON)经 handle 不产生 Server toast,不进 errors 列表(或进但不路由 toast——以 design 为准);`new Error("boom")` 经 handle 不再无声(console.error 可断言)。
- [ ] AC3 单测:daemon 429 响应(body 全字段)→ TransportError.category==="RateLimit"、retryable===true;body 残缺分档:status 500→"Server"、502→"Network"、404→"InvalidRequest"、0(unknown-cmd)→"InvalidRequest";真 TransportError 实例经 handle 按真实 category 路由(接缝集成)。
- [ ] AC4 30+ 处 `extractErrorMessage` 现有调用点零改动即编译通过、行为不回归(文案输出不变)。
- [ ] AC5 `cd app && pnpm test` 全绿;`app/src/transport/transport-parity.test.ts` 通过。

## Out of Scope(P2,后续任务)

- 双 toast 体系合并(useToast vs projectsStore.toast)。
- 静默失败面横扫(ModelsTab/ProvidersTab/权限应答//clear 等 10+ 处仅 console 的用户操作失败)。**验收须写成用户可见标准**:「每个用户主动操作的失败有可见反馈」,而非 console 卫生(评审裁决)。
- errorBus `errors` FIFO 列表接 UI(诊断面);InvalidRequest 的内联渲染推广;useErrorBus 对 PascalCase category 类型的 re-export(chore)。
- category 双 case(PascalCase/snake_case)双轨统一。

## Constraints

- 30+ `extractErrorMessage` 调用点与 16 个 store 的现有 catch 行为是兼容面:本任务只增不改其输出契约(message 字符串语义不变)。
- 后端(daemon/error.rs、error.rs)与流式链路(streamEvents/MessageItemFooter/retryChat)不动。
- toast 分类体系(4 类 + InvalidRequest 不打扰)的原设计语义保留。
