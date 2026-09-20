# Design:错误链路收口(全局兜底分类 + TransportError category 恢复)

对应 `prd.md` R1–R3。核心思路:**信任来源数据,不信猜测**——category 只从 wire 数据来(daemon body / status 逆映射),来不了就没有;本地噪音按本地对待,不再借「Server」之名弹用户。

## D1 全局兜底输入分类(main.ts + useErrorBus.ts)

### D1.1 良性噪音过滤(纯函数,可测)

`useErrorBus.ts` 导出纯函数:

```ts
/** 已知良性浏览器噪音(不进 errorBus、不 toast;console.debug 留痕)。 */
export function isBenignBrowserNoise(msg: string): boolean {
  return msg.startsWith("ResizeObserver loop");
}
```

前缀匹配覆盖两个已知变体(`completed with undelivered notifications` / `limit exceeded`)。**白名单式**——只封确切已知的噪音,不搞宽松的 includes 匹配(防误杀真错误)。main.ts 在 `handle()` 之前过滤:`event.error` 为空且 `event.message` 命中 → `console.debug` + return。

### D1.2 Error 实例:console.error 兜底,不 toast

`handle(e)` 对 `e instanceof Error`(TransportError 除外,见 D2.3)→ `console.error("[errorBus:uncaught]", e)` + return。不 push errors、不 toast:

- 未捕运行时错误几乎都不可由用户行动修复,弹 toast 只会复刻「噪音训练用户忽略弹窗」的原病。
- 与 InvalidRequest 同判据:可见(devtools)但不打扰。

同时补 Vue 层:`main.ts` 设 `app.config.errorHandler = (err, _inst, info) => console.error("[vue:errorHandler]", info, err)`。现状 Vue 组件生命周期/异步错误经默认 handler 只 warn 且部分形态到不了 window.onerror——这是 R1.2「不再无声」的补口。

### D1.3 裸 string 去除 Server 标签

`parseAppCommandError` 的 string 分支收敛为:**只认能 JSON.parse 出 AppCommandError 形状的字符串**;非 JSON 字符串返回 null。`handle()` 对落空产物(非 AppCommandError 的 string)→ `console.warn("[errorBus:uncaught-string]", e)` + return,不 push errors FIFO、不 toast(评审裁决:裸 string 不进 errors FIFO,与 InvalidRequest 同待遇)。

行为变化(有意):
- 老链路 Tauri 原始 String rejection(http 模式下已不存在,仅 `?transport=tauri` 逃生模式可能残留)从「Server toast」变为 console.warn——它本质是未知本地错误,原标法就是误标。
- `useErrorBus.test.ts` 现有「原始 string 降级 Server/Unknown」族用例按新行为改写(AC2 的测试面)。

`extractErrorMessage` 输出不变:parse 落空后走 `e instanceof Error` / `typeof e === "string"` 分支,字符串原样返回(AC4 的兼容面)。

## D2 TransportError category 恢复(transport/http.ts)

### D2.1 逆映射表 + 分档兜底(transport 内私有不导出)

```ts
function categoryFromStatus(status: number): ErrorCategory | undefined {
  switch (status) {  // daemon/error.rs status_for_category 的逆,canonical 1:1
    case 401: return "Auth";
    case 429: return "RateLimit";
    case 400: return "InvalidRequest";
    case 500: return "Server";
    case 502: return "Network";
    default:  return undefined;  // 非canonical → 走分档兜底
  }
}
// 分档兜底(群聊评审 09-21 裁决,替代原「其余兜 Server」):
// status=0(unknown-cmd 路径,http.ts:420,有生产前科)与非 canonical 4xx → InvalidRequest
// 其余 ≥500(如代理 503/504)→ Server,「Server」仅作构造上不可达的防御默认。
// 动机:原兜 Server 会把 status=0 前科路径从静默丢弃翻转为弹假「服务端错误」——
// 复刻本任务要修的原病。
```

逆映射与分档放 transport 层而非 `utils/error.ts`:它是 daemon HTTP 契约的一部分,与 `daemon/error.rs` 成对演化;`error.ts` 保持纯 category 助手定位。retryable 派生复用 `error.ts` 的 `categoryRetryable`(同源承诺落在函数上,见其文件头注释)。

### D2.2 TransportError 携带四字段(成为 AppCommandError 形状)

`TransportError` 构造时解析 body,新增只读字段:

```ts
export class TransportError extends Error {
  public readonly category: ErrorCategory;   // body(过值域校验)→ canonical 逆映射 → 分档兜底(见 D2.1)
  public readonly kind: string;              // body.kind ?? "Transport"
  public readonly retryable: boolean;        // body.retryable ?? categoryRetryable(this.category)
  public readonly requestId?: string;        // body.request_id
  // status / body / message 原样保留(auth.ts 401 处理与既有测试只读 status,不动)
}
```

`TransportErrorBody` 补 `category?: string; retryable?: boolean;`(`[key: string]: unknown` 保留)。category 值校验:body.category 不在五值域时按缺失处理(防脏数据直通 toast 路由)。兜底链:**body(权威)→ canonical 逆映射 → 分档兜底**;「Server」仅保留为构造上不可达的防御默认(default 分支已被分档穷尽)。

**类型单一事实源**(评审裁决):`error.ts` 新增导出 PascalCase 五值 union(如 `type AppErrorCategory = "Auth" | "RateLimit" | "InvalidRequest" | "Server" | "Network"`),transport 与 useErrorBus 的**字段类型**一律从此导入,不得本地平行定义;`error.ts` 现有双 case union 降级为 helper 入参容忍类型(理由:snake_case 脏值须类型安全地存在、但必须过不了 `VALID_CATEGORIES` 形状门)。useErrorBus 侧 re-export 推 P2/chore。

### D2.3 自动接入现有形状识别(零改动联动)

`useErrorBus.isAppCommandError` 是**形状检查**(category/kind/message/retryable 四字段 + category 合法值)。TransportError 变成 AppCommandError 形状后:

- 未捕 transport rejection → `handle()` → 形状识别通过 → 按 body 真实 category 路由(Auth→toast 引导 / RateLimit→toast / InvalidRequest→console.warn)。**A5「防静默」承诺在主通道上第一次真正成立**——此前这类错误被 parse 返回 null 静默丢弃。
- 不需要 import TransportError 到 useErrorBus(无环);不需要 instanceof。
- D1.2 的 `e instanceof Error` 分支须放在形状识别**之后**执行(TransportError 也是 Error 实例,顺序错会再次吞掉它)。该顺序约束用**真 TransportError 实例过 handle 的接缝集成用例**守门(手写形状对象测不出 PR1×PR2 接缝,评审裁决)。
- 运行时观测兜底:D1.2 的 Error 分支对 `name === "TransportError"` 的实例单独打标 `console.error("[errorBus:transport-shape-miss]", e)`——若未来构造收窄被打破(TransportError 落到形状门外),console 里可从标签直接发现,而不是无声。

### D2.4 分类读取接口(给未来,不接 UI)

`useErrorBus.ts` 新增导出:

```ts
export function extractErrorCategory(e: unknown): ErrorCategory | null
```

容错序:AppCommandError 形状(含 TransportError)→ category 字段;否则 null。本任务只交付函数 + 测试,不改任何展示面(重试按钮接线是 P2+ 的事,防 scope 蔓延)。

## D3 数据流(修后)

```
daemon 错误响应(status+body 全字段)
  → TransportError{status, category, kind, message, retryable, requestId}
  → catch 点:extractErrorMessage → message 不变(30+ 调用点零改动)
  → 未捕:handle() 形状识别 → 按真实 category 路由 toast/console
window error/unhandledrejection:
  → isBenignBrowserNoise → console.debug 丢弃(ResizeObserver 案例)
  → TransportError/AppCommandError 形状 → 路由(真服务端错误终于能看到)
  → 其他 Error 实例 → console.error(不 toast)
  → 裸 string → console.warn(不再标 Server)
```

## D4 兼容与风险

| 面 | 变化 | 风险与对策 |
|---|---|---|
| 30+ extractErrorMessage 调用点 | 无(message 语义不变) | AC4 编译+全量测试守门 |
| auth.ts 401 流(`http.ts:448`) | 无(只读 status) | transport.test 既有用例不回归 |
| 老链路 string rejection(仅 tauri 逃生模式) | Server toast → console.warn | 有意收窄;方向正确,design 记录。收益面仅限非 AppCommandError 字符串——tauri 模式 rejection 是 Rust 序列化对象,照常按 category 路由 |
| 未捕 transport rejection | 静默丢弃 → category toast | 新增可见性;dedupe(5s)+ max3 防风暴。**双弹不成立**(评审结构性否定):handle 唯一消费方是 main.ts 全局监听,被 catch 的 rejection 不触发 unhandledrejection |
| 401 Auth toast × onAuthFailed 跳转叠加 | 本任务新引入 | 第 13 步冒烟验收:仅存配对→pairing 页,toast+落页无双跳、toast 是该页唯一因果解释;不划 P2(评审裁决) |
| fetch 裸 TypeError(daemon 掉线) | 不经 TransportError → 全局兜底走 console.error 分支 | 设计记录:Network 路由在全局兜底层不可达,系现状而非本任务回归;SSE 断线重连自愈路径不变 |
| errors ref 消费方 | 无(本就不存在) | — |
| main.ts:23-27 注释 | PR2 后原描述变假 | 随 PR2 同步改写(评审裁决) |

回滚:三个文件(main.ts / useErrorBus.ts / http.ts)各自独立成立,可逐文件 revert;无 schema/迁移。

## D5 权衡记录

- **string 不新增「Local」category**:5 类是前后端 wire 契约,前端私加类目会让 `categoryToastKey`/`categoryRetryable` 双轨变三轨;console 三级(debug/warn/error)是与 category 正交的严重度通道,已够表达「本地、不弹」(评审三方一致)。
- **status 逆映射放 transport 不放 error.ts**:见 D2.1。
- **Error 实例不 toast**:见 D1.2,与 A5「InvalidRequest 不打扰」同一设计判据。
- **类型单一事实源定 error.ts**:见 D2.2,防三处平行定义漂移。
- **requestId 字段名**:wire 是 `requestId`(camelCase)——`daemon/error.rs` 的 `AppCommandError` 带 `#[serde(rename_all = "camelCase")]`,body 全字段 camelCase(error.rs 单测断言 `"requestId":null`)。实例字段与 wire 同名,读取零转换。【check 阶段纠正:本条原误记 wire 为 snake_case,若照原文实现则 requestId 恒 undefined 死字段,已按 camelCase 实现】
