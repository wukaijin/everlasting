# Implement:错误链路收口

前置:`design.md` D1–D5。改动面 3 个源文件 + 3 个测试文件,无后端、无 UI 组件。

## 顺序清单

### PR1 TransportError category 恢复(design D2)

- [ ] 1. `app/src/transport/http.ts`:`TransportErrorBody` 补 `category?: string; retryable?: boolean;`(索引签名保留)。
- [ ] 2. 同文件:私有无导出 `categoryFromStatus(status)` 逆映射表(401/429/400/500/502;default undefined)。
- [ ] 3. 同文件:`TransportError` 构造函数解析 body,新增只读 `category`(body 校验值域 → canonical 逆映射 → **分档兜底:status=0 与非 canonical 4xx→InvalidRequest、其余 ≥500→Server,「Server」为不可达防御默认**,见 design D2.1)、`kind`、`retryable`(body 透传 → `categoryRetryable` 派生)、`requestId`。注意 body 可能是 string(非 JSON 路径,`http.ts:420`)——全部字段走「body 为对象才读」守卫。category 类型**仅从 `utils/error.ts` 导入**(单一事实源,评审裁决):先在 `error.ts` 新增导出 PascalCase 五值 union(如 `AppErrorCategory`),transport 与 useErrorBus 均从此取,不得本地定义;retryable 派生 import `categoryRetryable`。
- [ ] 4. `app/src/transport/http.test.ts`(若该路径用例在 `transport.test.ts` 则写在那里),四组用例——
    - category 恢复:body 全字段(429→RateLimit/retryable=true 透传);
    - canonical 逆映射:body 残缺仅 status,500→Server、502→Network;
    - **分档兜底(评审裁决,替代原「404→Server」)**:status=0→InvalidRequest、404→InvalidRequest、503→Server;body.category 脏值("Foo")→按缺失走 status;
    - **构造收窄不变量**:脏 body(body 为 string / undefined / `{}` / kind 缺失 / retryable 非 boolean)构造出的 TransportError 实例恒通过 `isAppCommandError` 形状门——PR2 接缝不被脏数据打破。
- [ ] 验证:`cd app && pnpm test -- transport`

### PR2 全局兜底分类修正(design D1)

- [ ] 5. `app/src/utils/useErrorBus.ts`:导出 `isBenignBrowserNoise(msg)`(前缀 `ResizeObserver loop`,白名单式)。
- [ ] 6. 同文件:`parseAppCommandError` string 分支收敛——JSON.parse 成功且形状合法才返回;非 JSON 返回 null(删掉降级 Server 的构造)。
- [ ] 7. 同文件:`handle(e)` 重排——①形状识别(AppCommandError 对象,含 TransportError)→ push+路由;②`isBenignBrowserNoise` 短路(对 string)→ console.debug;③其他 string → console.warn(不 push FIFO);④其他 Error 实例 → console.error,其中 `name === "TransportError"` 的单独打标 `[errorBus:transport-shape-miss]`(design D2.3 观测兜底)。**顺序即 D2.3 的约束:形状识别必须先于 instanceof Error。**
- [ ] 8. 同文件:导出 `extractErrorCategory(e): ErrorCategory | null`(形状识别取 category,否则 null)。
- [ ] 9. `app/src/main.ts`:error 监听入口先过 `isBenignBrowserNoise(event.message)`(仅 event.error 为空时);补 `app.config.errorHandler = (err, _inst, info) => console.error("[vue:errorHandler]", info, err)`(bootstrap 前挂);**同步改写 main.ts:23-27 注释**(PR2 后原「统一入错误总线/不丢失」描述变假,按新分级语义改写,评审裁决)。
- [ ] 10. `app/src/utils/useErrorBus.test.ts`:
    - 改写「原始 string 降级 Server/Unknown」族 → 新行为(console.warn、不 push、不 toast);
    - 新增:`new Error("boom")` 走 handle → 不 push、不 toast(断言 console.error 被调);
    - 新增:TransportError 形状对象(带 category:"RateLimit")走 handle → push 且 toast category 正确;
    - 新增(评审裁决):**真 TransportError 实例**(`new TransportError(...)` from `transport/http.ts`)过 handle → 按真实 category 路由——PR1×PR2 接缝集成,手写形状对象测不出;顺带把「形状识别先于 instanceof Error」顺序变成可失败断言(顺序回归该用例必红);
    - 新增:Error 分支对 `name==="TransportError"` 打 `[errorBus:transport-shape-miss]` 标(断言 console.error 参数);
    - 新增:`isBenignBrowserNoise` 两变体 + 不误杀(如 "ResizeObserverx" / 真错误消息);
    - 新增:`extractErrorCategory` 三态(AppCommandError / TransportError 形状 / 其他)。
- [ ] 验证:`cd app && pnpm test -- useErrorBus`

### PR3 收尾验证

- [ ] 11. 全量:`cd app && pnpm test`(含 `transport-parity.test.ts`)。
- [ ] 12. 类型:`cd app && pnpm exec vue-tsc --noEmit`(或项目等价 typecheck 脚本,先看 package.json scripts)。
- [ ] 13. 手工冒烟(有 daemon 时):`scripts/turn-smoke.sh` 或直接 GUI 展开消息 diff 卡 + 滚动,确认无「服务端错误 ResizeObserver…」toast、console 有 debug 留痕;**401 叠加验收(评审裁决,不划 P2)**:模拟未认证 → 未捕 401 rejection 弹 Auth toast + onAuthFailed 跳转仅落 pairing 页一次,无双跳、toast 是该页唯一因果解释。无 daemon 时跳过并在收尾说明。(裸 string 降级掩盖面已二次复核 09-21:app/src 全仓 `reject("…")`/`reject('…')` 零命中,见 research/group-chat-review-0921.md。)
- [ ] 14. lint:按 `app/package.json` scripts 跑项目既有 lint 命令。

## 风险文件 / 回滚点

| 文件 | 风险 | 回滚 |
|---|---|---|
| `app/src/transport/http.ts` | body 为 string 的构造路径(http.ts:420)字段守卫遗漏 → 运行时 undefined 读值 | PR1 独立成立,单文件 revert |
| `app/src/utils/useErrorBus.ts` | handle 分支顺序错(形状识别未先于 instanceof)→ TransportError 重新被吞 | PR2 有专测用例守门(第 10 条第 3 例) |
| `app/src/main.ts` | errorHandler 在 bootstrap 失败路径前挂错位置 | 5 行改动,肉眼可审 |

## start 前检查

- [ ] prd/design/implement 三件套齐(本文件);
- [ ] implement.jsonl / check.jsonl 已填真实条目;
- [ ] 用户已评审(Phase 1.4 gate)。
