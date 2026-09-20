# 错误链路审计(2026-09-21 会话评估,证据存档)

来源:用户报「服务端错误 ResizeObserver loop completed with undelivered notifications.」toast;顺藤摸出全局兜底与传输层两处断点,随后做了全链路评估。本文件是 P1 修复的依据,行号以 2026-09-21 工作区为准。

## 触发案例解剖(ResizeObserver 误报)

- 虚拟滚动动态高度测量是 ResizeObserver 驱动的:`app/src/composables/useVirtualizedMessages.ts:311-315`(注释明示),行挂载走 `app/src/components/chat/MessageList.vue:40` 的 `measureElement`。展开 EDIT_FILE diff 卡 → 行高变化 → 同帧级联重排 → 浏览器报 "ResizeObserver loop completed with undelivered notifications"。
- 该错误以 `window.onerror` 形式抛出,**只有 `event.message` 字符串、没有 `event.error` 对象**。
- `app/src/main.ts:32-35` 全局监听把所有未捕错误送进 `useErrorBus.handle()`。
- `app/src/utils/useErrorBus.ts:103-122` `parseAppCommandError`:裸 string → JSON.parse 失败 → **硬编码 `{category: "Server", kind: "Unknown"}`**。
- `app/src/components/common/ToastProvider.vue:63-64` Server → 前缀「服务端错误」。误报链闭合。

## P1-1:全局兜底对 Error 实例是死的,对字符串是错标的

`parseAppCommandError` 只认两种输入:

| 输入形态 | 行为 | 问题 |
|---|---|---|
| AppCommandError 形状对象(category/kind/message/retryable 四字段齐全且 category 合法) | 透传 | 正常 |
| 裸 string | 强标 `Server` category | 本地噪音被错标「服务端错误」(ResizeObserver 案例) |
| **Error 实例(含 TransportError、未捕 TypeError)** | **返回 null → handle 静默丢弃** | main.ts:23-27 注释声称「任何漏掉的 invoke .catch 或运行时错误都进 errorBus,不丢失」——对 Error 对象是假的 |

测试盲区佐证:`app/src/utils/useErrorBus.test.ts` 全部用例没有一例用 `new Error()` 走 `handle`。

## P1-2:TransportError 在传输边界丢 category/retryable

- daemon 侧契约完整:`app/src-tauri/src/daemon/error.rs` — `AppCommandError` IntoResponse,category→HTTP status 1:1(Auth→401 / RateLimit→429 / InvalidRequest→400 / Server→500 / Network→502),body 是全字段 camelCase JSON,有 `status_mapping_is_stable` 测试锁定。注释称 "frontend's existing TransportError parser handles this shape unchanged"。
- 前端断点:`app/src/transport/http.ts:276-281` `TransportErrorBody` 只声明 `kind?/message?/request_id?`(**无 category**);`TransportError` 构造只留 status + message(body 是 `[key: string]: unknown`,运行时 category 字段其实在,只是没读)。
- 后果:http transport 是默认主通道,主链路上所有 catch 点只能拿到 message;Auth 错误无法引导、retryable 语义丢失、kind/request_id 诊断字段蒸发。`app/src/utils/error.ts:40-49` 注释强调 retryable「No double-source、drift 会静默破坏契约」,而主通道恰恰断了 source。
- status→category 是 1:1 可逆映射,恢复成本极低。

## 链路全景(哪些是好的,不动)

- 后端契约 `app/src-tauri/src/error.rs`:五字段 wire shape + 10 个领域错误 impl AppError + retryable 按 category 派生 + request_id 透传。**扎实,不动。**
- 聊天流式错误链(唯一端到端 category 保真的路径):SSE error → `app/src/stores/streamEvents.ts:689-694` `last.error={message,category}` → `app/src/components/chat/MessageItemFooter.vue:157-162` `categoryRetryable` 出重试 → `app/src/stores/chatMessageActions.ts:317` `retryChat` 原位重开 + terminalError 重载补偿(`streamController.ts:112/1203`、`streamEvents.ts:1496-1518`)。**范本,不动。**
- `extractErrorMessage`(`useErrorBus.ts:133-139`)被 16 store + ~30 文件采用,文案提取统一。**保留兼容。**
- toast 防风暴:`app/src/composables/useToast.ts`(max 3 + 5s dedupe + TTL)。

## 相关但明确不在本任务范围(P2,拆后续任务)

- 双 toast 体系并存:`useToast`(右上 4 类)vs `projectsStore.toast`(`app/src/stores/projects.ts:80`,底部单 slot 后到覆盖)。合并是行为面大改。
- 静默失败面(仅 console):ModelsTab/ProvidersTab 保存删除、ModelSelect 切换、权限应答、/clear /new、TitleBar 窗口控制、`ButtonPrimitive.vue:113` apply 失败等 10+ 处。横扫型工作。
- errorBus 的 `errors` 50 条 FIFO 列表无消费方(write-only)——本任务不动它,但 design 里记一笔。
- `.trellis/spec/backend/error-handling.md` 主体是模板——spec 补齐走本任务 Phase 3.3(trellis-update-spec)自然承接。

## 错误展示面清点(2026-09-21 探查代理产出,摘要)

- 全局:useErrorBus→useToast→ToastProvider(右上);projectsStore.toast(底部单 slot);renderFatalOverlay(bootstrap 失败全屏,`main.ts:64-83`)。
- 内联:PairingView / NodeListView(带重试)/ DirBrowserModal / MemoryLayerItem / SearchTab / RemoteTab / PermissionGrantsModal / AuditLogModal / ChatPanel diffError / 三卡片 submitError / MessageItem 编辑态等,形制各造。
- 流式:见上文链路全景。
- 移动/远程:PairingView、NodeListView 自有 banner;其余共享 AppShell 全局面。
