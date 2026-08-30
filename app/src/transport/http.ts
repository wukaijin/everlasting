// httpTransport — Phase 2.3 C6 实现(2026-07-21, task
// `07-20-remote-access-daemon-split`).
//
// daemon 版的 Transport:`invoke` 走 HTTP POST,`listen` 走单全局
// `EventSource` + event-name 分发表。与 `tauriTransport` 实现同一
// `Transport` 接口(types.ts),P1 的 20+ 调用点零改动。
//
// # args 字段命名(关键设计决策)
//
// Tauri 2 的 `#[tauri::command]` 自动把 Rust snake_case 参数名暴露
// 为 JS camelCase(前端 `invoke("chat", { requestId, sessionId })`
// ↔ Rust `request_id, session_id`)。daemon handler 的 serde 默认
// snake_case,**不做这个转换**。所以本 transport 在 `invoke` 时把
// args 的**顶层 key** camelCase → snake_case,嵌套值(messages /
// struct)原样透传 —— 因为返回值方向 Tauri 与 daemon 序列化同一
// Rust struct(同 serde),前端读到的嵌套对象在两 transport 下一致,
// 只有顶层 command 参数名需要扳正。详见
// `.trellis/tasks/07-20-remote-access-daemon-split/research/p2.3-c6-http-transport.md`。
//
// # 单全局 EventSource
//
// design §1.3:所有事件(chatevent / tool:call / ... / subagent:event)
// 经 `GET /api/v1/stream` 单流下发,前端按 event name 分发到各
// `listen` handler。本 transport 维护一个 lazy 创建的 `EventSource`
// + `Map<event, Set<handler>>` 分发表;断网时浏览器自动重连并回带
// `Last-Event-ID`(SSE `id:` 字段),daemon 侧 `SseRegistry` 据此回放
// 或发 `stream-resync` sentinel —— sentinel 作为普通 event 透传给
// 注册了 `listen("stream-resync", ...)` 的 store,由 store 自己 GET
// `/api/v1/sessions/{current}/snapshot`(transport 不持有 session 状态)。
//
// # 未完成(C6 后续,见 research note)
//
// - **CORS**:dev 模式前端(vite 1420)与 daemon(7456)跨域,
//   daemon 需加 `tower-http::services::CorsLayer`(permit localhost:1420)。
//   P2.4 sidecar 同源后无需 CORS。当前 httpTransport 代码就绪,但
//   dev 跨域运行需先补 daemon CORS。
// - **base URL**:默认 `http://localhost:7456`,dev 可用 `?daemonUrl=`
//   query 覆盖。P2.4 sidecar 同源时改用 `location.origin`。
// - **api-types.ts(C8)**:84 handler 入参/返参 + SSE payload 的手写
//   TS 类型(渐进,先 SSE + chat 精确,其余 `unknown`)。

import type { Transport, UnlistenFn } from "./types";
import { currentDeviceToken, dropCurrentNodeToken } from "./auth";

// ---------------------------------------------------------------------------
// cmd → domain 映射(81 endpoint,从 `app/src-tauri/src/daemon/routes/*.rs`
// 的 `.route("/<cmd>", ...)` 提取)。invoke("cmd", args) →
// POST `{daemonBase}/api/v1/{domain}/cmd`。新增 command 时同步加这里
// (Rust 侧 routes 注册 + 此映射是两处需手工对齐的点)。
// ---------------------------------------------------------------------------
// 导出供 `http.routes-sync.test.ts` 守卫测试消费(该测试解析 daemon
// Rust 路由源码,断言每条 POST 路由都在本表 —— 防"新增 daemon 命令漏
// 加映射"第三次发生:save_attachment → get_web_search_config → F1 三条)。
export const CMD_TO_DOMAIN: Record<string, string> = {
  // agent
  chat: "agent",
  // B1 (2026-08-17): paste-image upload — 缺这行时前端报
  // `unknown cmd "save_attachment"`(PR5 遗漏,贴图发送即断)。
  // GET 附件走 <img> 直连不进本表。
  save_attachment: "attachments",
  // cancel
  cancel_chat: "cancel",
  // command_palette
  get_command_body: "command_palette",
  list_commands: "command_palette",
  // config
  get_llm_config: "config",
  get_home_dir: "config",
  // S2 remote tunnel(2026-08-11, task `08-11-tunnel-client`,design §3.1
  // 清单第 6 条):缺这 3 行时 sidecar/浏览器模式报
  // `unknown cmd "get_remote_config"`(Tauri Full 模式侥幸走 IPC)。
  get_remote_config: "config",
  set_remote_config: "config",
  get_tunnel_status: "config",
  // 自定义 node_id / 显示名(08-26-custom-node-id 及同日增补):缺这两行
  // 时浏览器/sidecar 模式报 `unknown cmd "set_tunnel_node_id"`(Tauri Full
  // 模式侥幸走 IPC)。
  set_tunnel_node_id: "config",
  set_tunnel_display_name: "config",
  // F4 web_search 配置(2026-08-25):缺这两行时浏览器/sidecar 模式报
  // `unknown cmd "get_web_search_config"`(Tauri Full 逃生侥幸走 IPC)。
  get_web_search_config: "config",
  set_web_search_config: "config",
  // F6 异步 agent 任务(2026-08-27):app_config 开关面读出口。
  get_app_config: "config",
  // Settings「通用」开关写入口(2026-08-29 settings-shell):缺这行时
  // 浏览器/sidecar 模式报 `unknown cmd "set_app_config_flag"`。
  set_app_config_flag: "config",
  // P3b(2026-08-31,评审 W1):列表型 app_config 字段写通道
  // (sandbox_extra_writable;缺这行时 transport-parity 测试红)。
  set_app_config_list: "config",
  // S2 配对码生成(新 domain pairing)
  generate_pairing_code: "pairing",
  // files
  list_files: "files",
  list_files_at: "files",
  // memory
  delete_autonomous_memory: "memory",
  list_autonomous_memories: "memory",
  open_memory_in_editor: "memory",
  read_memory_content: "memory",
  read_memory_layers: "memory",
  update_autonomous_memory: "memory",
  update_autonomous_memory_status: "memory",
  // F1 消息队列(2026-08-25):缺这三行时浏览器/sidecar/remote 模式
  // 打开 session 即报 `unknown cmd "list_queued_messages"`(排队视图
  // 刷新失败;Tauri IPC 模式不经过本表,侥幸测不出)。
  list_queued_messages: "message_queue",
  remove_queued_message: "message_queue",
  recall_queued_message: "message_queue",
  // panel
  get_skill_body: "panel",
  list_panel_items: "panel",
  list_subagents: "panel",
  // permissions
  clear_session_trace: "permissions",
  grant_tool_permission: "permissions",
  list_session_audit_events: "permissions",
  // RULE-PERM-001 (2026-08-30): keyset 分页审计读(AuditLogModal「加载
  // 更多」)。缺这行时浏览器/sidecar 模式报 `unknown cmd`(Tauri IPC
  // 模式侥幸不经过本表;http.routes-sync.test.ts 守卫)。
  list_session_audit_events_page: "permissions",
  list_session_tool_permissions: "permissions",
  list_turn_traces: "permissions",
  // 08-20-worker-turn-trace-persist: per-run worker turn rows
  // (SubagentDrawer "Token 明细", PR3 前端消费;后端读路径先落地)。
  list_worker_turn_traces: "permissions",
  permission_response: "permissions",
  revoke_tool_permission: "permissions",
  set_session_mode: "permissions",
  // projects
  create_project: "projects",
  hide_project: "projects",
  list_hidden_projects: "projects",
  list_projects: "projects",
  unhide_project: "projects",
  update_project_name: "projects",
  update_project_path: "projects",
  // providers
  add_model: "providers",
  add_provider: "providers",
  delete_model: "providers",
  delete_provider: "providers",
  get_default_model: "providers",
  list_models: "providers",
  list_providers: "providers",
  set_default_model: "providers",
  test_model: "providers",
  update_model: "providers",
  update_provider: "providers",
  update_session_model_id: "providers",
  // question
  get_pending_interaction: "question",
  resolve_mode_change: "question",
  resolve_task_state_transition: "question",
  resolve_tool_question: "question",
  // sessions
  clear_session_messages: "sessions",
  compact_session: "sessions",
  create_session: "sessions",
  delete_session: "sessions",
  diff_worktree: "sessions",
  edit_user_message: "sessions",
  group_chat_cache_rates: "sessions",
  // /handoff (08-18-handoff-mechanism): 缺这行时浏览器/remote 模式报
  // `unknown cmd "handoff_session"`(Tauri IPC 模式侥幸不经过本表)。
  handoff_session: "sessions",
  list_sessions: "sessions",
  // 08-20-turn-usage-event-quota-view WP2: 5h 窗口配额 IPC(POST,
  // /api/v1/usage 组)。
  usage_window: "usage",
  set_quota_settings: "usage",
  list_workflow_plugins: "sessions",
  load_session: "sessions",
  record_tool_duration: "sessions",
  rename_session: "sessions",
  // D2 (cross-session search, 2026-08-17): global SearchModal.
  search_messages: "sessions",
  // review (C2)
  get_review_state: "review",
  get_current_task_slug: "review",
  // F2 定时任务(2026-08-28, task `08-28-f2-scheduled-tasks`):Settings
  // 「定时任务」tab 的 CRUD 四条。缺映射时浏览器/sidecar/remote 模式
  // 打开 Settings tab 即报 `unknown cmd`(Tauri IPC 模式侥幸走 IPC,
  // 同 save_attachment / F1 三条的老坑;http.routes-sync.test.ts 守卫)。
  create_scheduled_task: "scheduled_tasks",
  delete_scheduled_task: "scheduled_tasks",
  list_scheduled_tasks: "scheduled_tasks",
  update_scheduled_task: "scheduled_tasks",
  set_session_color: "sessions",
  set_session_plugin_name: "sessions",
  set_session_workflow_enabled: "sessions",
  update_message_latency: "sessions",
  // Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E3): the
  // edit-mode IPC `GroupChatConfigModal` uses to overwrite the
  // group-chat session's `metadata` JSON blob (participants
  // roster). Without this entry, the default httpTransport (the
  // browser/daemon mode) throws `unknown cmd "..."` — the
  // Tauri-IPC fallback path (the Sidecar spawn) still works
  // because it doesn't consult this table.
  update_session_metadata: "sessions",
  // subagent_runs
  discard_worker_run: "subagent_runs",
  get_subagent_run: "subagent_runs",
  list_subagent_runs_by_session: "subagent_runs",
  merge_worker_run: "subagent_runs",
  // subagents
  list_subagents_with_model: "subagents",
  set_subagent_model: "subagents",
  // task
  archive_task: "task",
  create_task: "task",
  // ui
  apply_ui_diff: "ui",
  // worktree
  attach_worktree: "worktree",
  delete_worktree: "worktree",
  detach_worktree: "worktree",
  publish_session_to_main: "worktree",
};

/// daemon error body(`AppCommandError` wire shape,见
/// `daemon::error::AppCommandError` 的 Serialize)。transport 不强类型
/// 化错误体(C8 渐进),只透传 status + body。
export interface TransportErrorBody {
  kind?: string;
  message?: string;
  request_id?: string;
  [key: string]: unknown;
}

export class TransportError extends Error {
  constructor(
    public readonly status: number,
    public readonly body: TransportErrorBody | string,
  ) {
    const msg =
      typeof body === "string"
        ? body
        : body?.message ?? `HTTP ${status}`;
    // BUGLIST CH3-3/CH7-3 (2026-08-29): the old "[httpTransport]
    // <status>: " prefix leaked straight into user-facing toasts —
    // every display path (useErrorBus, store catches) reads
    // `e.message` verbatim. Identification stays available for
    // debugging via `name` + `status`.
    super(msg);
    this.name = "TransportError";
  }
}

/// daemon base URL(P2.4 D3.2 + D7.2)。
///
/// 解析优先级:
/// 1. `?daemonUrl=` query(无尾斜杠)—— dev 跨域 / 自定义 daemon 地址。
/// 2. **DEV**(`import.meta.env.DEV`,vite 1420 serve 前端):`http://localhost:7456`
///    —— dev 模式前端与 daemon 跨域(1420 ↔ 7456),需显式指向 daemon。
/// 3. **PROD**:`window.location.origin` —— sidecar 同源,daemon ServeDir
///    服务前端,fetch/EventSource 同源免 CORS。
///
/// P2.3 时硬编码 `http://localhost:7456`;P2.4 sidecar 同源后 PROD 走
/// `location.origin`(同源单二进制部署),DEV 仍走 7456(vite 跨域)。
export function daemonBase(): string {
  if (typeof window !== "undefined") {
    const q = new URLSearchParams(window.location.search);
    const fromQuery = q.get("daemonUrl");
    if (fromQuery) return fromQuery.replace(/\/+$/, "");
    // DEV 探测:vite 注入 `import.meta.env.DEV`(build 时静态替换为
    // false,tree-shake 掉 prod 分支)。
    if (import.meta.env.DEV) {
      return "http://localhost:7456";
    }
    return window.location.origin;
  }
  return "http://localhost:7456";
}

/// 顶层 key camelCase → snake_case(`requestId` → `request_id`)。
/// 嵌套值原样保留 —— Tauri/daemon 对 struct 用同一 serde,前端读
/// 到的嵌套对象在两 transport 下一致,只有顶层 command 参数名需扳正。
function camelToSnakeKey(key: string): string {
  // 仅处理 ASCII A-Z → _a-z;已含下划线或全小写的 key 无变化。
  return key.replace(/[A-Z]/g, (c) => "_" + c.toLowerCase());
}

function transformArgsTopLevel(
  args?: Record<string, unknown>,
): Record<string, unknown> {
  if (!args) return {};
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(args)) {
    out[camelToSnakeKey(k)] = v;
  }
  return out;
}

// ---------------------------------------------------------------------------
// 单全局 EventSource + event-name 分发表。lazy 创建:首个 `listen`
// 触发 `new EventSource`。EventSource 浏览器原生自动重连 + 回带
// `Last-Event-ID`(SSE `id:` 字段),daemon `SseRegistry` 据此回放。
// ---------------------------------------------------------------------------
const handlersByEvent = new Map<string, Set<(payload: unknown) => void>>();
let eventSource: EventSource | null = null;

function ensureEventSource(): void {
  if (eventSource) return;
  const base = daemonBase();
  // 08-26 多节点:token 按"当前选中节点"解析(currentDeviceToken)。
  // S4 pwa-remote(token 存在):SSE 经 proxy + access_token query。
  // EventSource 不能设 header,走 remote `auth.rs` 的 query 通道。
  // browser-local(无 token):直连 daemon(现状不变)。
  const token = currentDeviceToken();
  const url = token
    ? `${base}/api/v1/proxy/api/v1/stream?access_token=${encodeURIComponent(token)}`
    : `${base}/api/v1/stream`;
  eventSource = new EventSource(url);
  // 具名 event 的 listener 在首个 listen(event) 时按需 addEventListener
  // (见 listen 实现)。这里只持有 EventSource 实例 + 错误日志。
  eventSource.onerror = (e) => {
    // EventSource 会自动重连;这里只记录。浏览器重连成功后 daemon
    // 按 Last-Event-ID 回放,store 自然恢复。
    console.warn("[httpTransport] EventSource error (will auto-reconnect)", e);
  };
}

/// 关闭并清空当前 EventSource 引用。供配对成功(无 token→有 token)或
/// 登出 / 401(有 token→无 token)后调用:下一次 `listen` 会用新的 token
/// 状态重建 EventSource(pwa-remote 走 proxy + query,local 直连)。
///
/// 不清 `handlersByEvent` —— 已注册的 handler 仍保留(其 unlisten 闭包
/// 持引用)。重建后的 EventSource 会按需重新 addEventListener(首个
/// `listen(newEvent)` 触发);MVP 配对/登出后通常伴随视图卸载重建,
/// store 重新 `start()` 注册新 handler(详见 design §6)。
export function resetEventSource(): void {
  if (eventSource) {
    eventSource.close();
    eventSource = null;
  }
}

// ---------------------------------------------------------------------------
// 401 全局处理(P2-1,design §6.2):transport.invoke 是所有 app 命令的
// 唯一 choke point,调用方系统性 try/catch+swallow 会让 401 静默(errorBus
// 只收未捕获异常,收不到)。故在 invoke 的 `!resp.ok` 分支拦 401:清 token
// + 关 EventSource + 触发模块级回调(Step 5 由 App 注册
// `router.push("/pairing")`)。
//
// 模块级回调通过 setOnAuthFailed 注册;App.vue 挂载时设置,生命周期内常驻。
export let onAuthFailed: (() => void) | null = null;

export function setOnAuthFailed(cb: (() => void) | null): void {
  onAuthFailed = cb;
}

function parsePayload(data: string): unknown {
  try {
    return JSON.parse(data);
  } catch {
    return data;
  }
}

export const httpTransport: Transport = {
  invoke: async <T = unknown>(
    cmd: string,
    args?: Record<string, unknown>,
  ): Promise<T> => {
    const domain = CMD_TO_DOMAIN[cmd];
    if (!domain) {
      throw new TransportError(
        0,
        `unknown cmd "${cmd}" — no domain mapping in httpTransport (sync CMD_TO_DOMAIN with daemon routes)`,
      );
    }
    const base = daemonBase();
    // S4 pwa-remote 模式(D3):有 token → app 命令经 remote proxy 透传到
    // 绑定的 PC 节点(加 `/api/v1/proxy` 前缀 + Bearer auth);无 token →
    // browser-local 直连 daemon(现状不变,proxyPrefix 为空 + 无 auth 头)。
    // 08-26 多节点:token = 当前选中节点的那条(currentDeviceToken)。
    const token = currentDeviceToken();
    const proxyPrefix = token ? "/api/v1/proxy" : "";
    const url = `${base}${proxyPrefix}/api/v1/${domain}/${cmd}`;
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
    };
    if (token) headers["Authorization"] = `Bearer ${token}`;
    const resp = await fetch(url, {
      method: "POST",
      headers,
      body: JSON.stringify(transformArgsTopLevel(args)),
    });
    if (!resp.ok) {
      // P2-1(design §6.2):token 失效(吊销/过期)remote 返 401。在抛
      // TransportError 前先做副作用 —— 修剪当前节点的 token(drop 语义:
      // 多配对下只掉这一个,其余节点照常;见 auth.ts)+ 关 EventSource
      // (下次重建无 auth)+ 触发 App 注册的跳转回调(→ /nodes 或
      // /pairing,按剩余配对)。调用方 catch 与否都必过此处,避免 401 静默。
      if (resp.status === 401 && currentDeviceToken()) {
        dropCurrentNodeToken();
        resetEventSource();
        onAuthFailed?.();
      }
      let body: TransportErrorBody | string;
      try {
        body = (await resp.json()) as TransportErrorBody;
      } catch {
        body = await resp.text().catch(() => "");
      }
      throw new TransportError(resp.status, body);
    }
    // daemon handler 返回 `Json<T>`。F1 (2026-08-25): chat 现返回
    // `{status:"started"|"queued", position?}`、cancel_chat 返回
    // `{cancelled, clearedQueued}` —— JSON 透传即 P2-1 的 transport
    // 透传要求;空 body 解析为 null(兼容 legacy 单元返回)。
    const text = await resp.text();
    if (text.length === 0) return null as T;
    return JSON.parse(text) as T;
  },

  listen: <T = unknown>(
    event: string,
    handler: (payload: T) => void,
  ): Promise<UnlistenFn> => {
    let set = handlersByEvent.get(event);
    if (!set) {
      set = new Set<(payload: unknown) => void>();
      handlersByEvent.set(event, set);
      ensureEventSource();
      // 按需为该 event 名注册具名 SSE listener。SSE frame 的 `event:`
      // 字段 → EventSource 触发具名 event(addEventListener(event))。
      const onEvent = (e: MessageEvent) => {
        const payload = parsePayload(e.data);
        // 复制一份遍历,允许 handler 内 unlisten 自己不死循环。
        for (const h of [...(handlersByEvent.get(event) ?? [])]) {
          h(payload);
        }
      };
      eventSource!.addEventListener(event, onEvent as EventListener);
    }
    set.add(handler as (payload: unknown) => void);
    // UnlistenFn:从分发表移除 handler。不 close EventSource(其他
    // event 的 handler 可能在用;P2.3 单页生命周期内 EventSource 常驻)。
    const unlisten: UnlistenFn = () => {
      handlersByEvent.get(event)?.delete(handler as (payload: unknown) => void);
    };
    return Promise.resolve(unlisten);
  },
};
