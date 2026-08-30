// e2e fixtures — 浏览器交互回归的 canonical harness(design §2.3 契约)。
//
// # 被测面诚实边界
// 本层守护「UI 交互 + store 联动 + 渲染行为」,不是 wire 契约(后者
// Rust `--test e2e` 已有基线)。所以 fetch 层全拦截 + 注册表应答,
// SSE 层用受控 fake EventSource(测试内 `stream.emit` 程序化推事件),
// 不依赖 Rust daemon / LLM / 网络 —— 确定性是 CI blocking 门禁的前提。
//
// # wire 事实(约束,勿凭直觉改)
// - **无 envelope**:daemon 成功响应是 `Json<T>` 原样透传(空 body →
//   null,http.ts:403-409);错误才包 `TransportError(status, {kind?,
//   message?, request_id?})`。mockCmd 的 payload 就是 `Json<T>` 本体。
// - **请求体顶层键 camelCase→snake_case**(`transformArgsTopLevel`,
//   http.ts:270-289):`requestId` → `request_id`。注册匹配与请求体
//   断言一律按 snake_case(嵌套值原样透传,不受转换影响)。
// - **health 是 mount 硬门**(health.ts):`GET /api/v1/health` 返回
//   `{daemonId, daemonVersion, apiVersions:["v1"]}`(camelCase,镜像
//   Rust `HealthResponse` 的 `rename_all = "camelCase"`)。缺失/不含
//   "v1" → main.ts 渲染全屏错误 overlay,app 永不 mount。**不得**带
//   `remoteId` 字段 —— 带了会被 router 判成 remote 上下文踢去 /pairing。
//
// # fail-loud(必须)
// vite dev 自带 `/api → localhost:7456` proxy(vite.config.ts server
// .proxy)—— 漏 mock 的请求若放行,会经 proxy 静默漏到真 daemon,掩盖
// 缺陷。所以单条 catch-all dispatcher:注册表 miss → 500 fail-loud,
// 且**所有**请求(含 miss 与 OPTIONS preflight)先记入 `reqs` 供断言。
//
// # CORS(为什么 fulfill 要带 allow-origin 头)
// dev 模式 `daemonBase()` = `http://localhost:7456`(http.ts DEV 探测),
// 页面源是 1422 → fetch 是跨域,Chromium 会对 fulfill 的响应做 CORS
// 校验;`Content-Type: application/json` 的 POST 还会先发 OPTIONS
// preflight。dispatcher 对 OPTIONS 回 204 + allow 头,其余响应统一补
// `Access-Control-Allow-Origin: *`。
//
// # boot 顺序约束
// route 与 addInitScript 必须先于 goto(health 握手发生在 main.ts
// mount 前;EventSource 替身必须在首个 listen 前就位)—— world fixture
// 的 setup 即完成安装,test 里的 boot() 只负责 goto + 等 mount。
// localStorage 保持空 → browser-local 直连模式(无 proxy 前缀、无配对门)。
import { test as base, type Page, type Route } from "@playwright/test";

// ---------------------------------------------------------------------------
// 类型
// ---------------------------------------------------------------------------

/** dispatcher 记录的一条请求(所有 /api/v1/** 请求,含 miss/preflight)。 */
export interface RecordedRequest {
  method: string;
  /** pathname,如 `/api/v1/agent/chat`(query 剥离)。 */
  path: string;
  /** URL 的 domain 段(非 /api/v1/{domain}/{cmd} 形状为 null)。 */
  domain: string | null;
  /** URL 的 cmd 段(同上为 null)。 */
  cmd: string | null;
  /** body 解析后的 JSON(GET / 解析失败时为原始 string / null)。 */
  body: unknown;
}

/** mockCmd 的应答体 —— daemon `Json<T>` 原样,无 envelope(见文件头)。 */
export type MockPayload = unknown;

// ---------------------------------------------------------------------------
// 选择器(既有稳定 class hook,ui-review.sh SELECTORS 先例;
// 不引入 data-testid —— 登记表见 e2e/README.md)
// ---------------------------------------------------------------------------

/** CodeMirror 编辑器 contenteditable 根(ChatInput.vue 的 CM host)。 */
export const EDITOR = ".chat-input__field .cm-content";
/** CM 的行元素:真实换行 = `.cm-line` 数量增加(软换行不拆行)。 */
export const EDITOR_LINE = ".chat-input__field .cm-content .cm-line";
/** 发送键(ChatInput.vue;流式空草稿时变形成 `.chat-input__stop`)。 */
export const SEND_BUTTON = ".chat-input__send";

// ---------------------------------------------------------------------------
// fake EventSource(addInitScript 注入,先于任何页面脚本)
//
// 最小实现:addEventListener / removeEventListener / close + readyState。
// **刻意不实现重连** —— 原生 SSE 重连语义是被排除的被测面(Rust e2e +
// stream-resync spec 覆盖);实现了反而掩盖回归。emit 构造真实
// MessageEvent 按具名 event 派发(无 listener 的事件丢弃,与原生一致;
// http.ts 只用 addEventListener,不走 onmessage)。
// ---------------------------------------------------------------------------
const FAKE_EVENT_SOURCE_INIT = (): void => {
  interface FakeSource {
    url: string;
    readyState: number;
    withCredentials: boolean;
    onopen: unknown;
    onmessage: unknown;
    onerror: unknown;
    _listeners: Map<string, Set<(e: MessageEvent) => void>>;
  }
  interface StreamControl {
    __sources: FakeSource[];
    emit: (name: string, payload: unknown) => void;
  }
  const w = window as unknown as {
    EventSource: unknown;
    __stream?: StreamControl;
  };
  class FakeEventSource {
    static CONNECTING = 0;
    static OPEN = 1;
    static CLOSED = 2;
    url: string;
    readyState = 1; // OPEN(测试内恒已连,省去 readyState 机)
    withCredentials = false;
    onopen: unknown = null;
    onmessage: unknown = null;
    onerror: unknown = null;
    _listeners = new Map<string, Set<(e: MessageEvent) => void>>();
    constructor(url: string) {
      this.url = url;
      w.__stream!.__sources.push(this as unknown as FakeSource);
    }
    addEventListener(name: string, fn: (e: MessageEvent) => void): void {
      if (!this._listeners.has(name)) this._listeners.set(name, new Set());
      this._listeners.get(name)!.add(fn);
    }
    removeEventListener(name: string, fn: (e: MessageEvent) => void): void {
      this._listeners.get(name)?.delete(fn);
    }
    close(): void {
      this.readyState = 2;
    }
  }
  w.EventSource = FakeEventSource;
  w.__stream = {
    __sources: [],
    emit(name: string, payload: unknown) {
      const data =
        typeof payload === "string" ? payload : JSON.stringify(payload);
      for (const src of w.__stream!.__sources) {
        if (src.readyState === 2) continue;
        const handlers = src._listeners.get(name);
        // 具名事件无 listener 即丢弃(与原生 EventSource 一致)。
        if (!handlers) continue;
        const evt = new MessageEvent(name, { data, lastEventId: "" });
        for (const fn of [...handlers]) fn.call(src, evt);
      }
    },
  };
};

// ---------------------------------------------------------------------------
// 默认 boot 注册表(ChatWindow.onMounted → config/projects/chat watcher
// 的最小 boot 面 + send 流消费的 cmd;形状读各 store 的 invoke 泛型)。
// world 创建时装入,test 侧 mockCmd 任何时候调用都覆盖(Map.set 语义)。
// ---------------------------------------------------------------------------
function bootDefaults(): Record<string, MockPayload> {
  return {
    // config.load():providers/models 目录 + home + app 开关面
    "providers/list_providers": [],
    "providers/list_models": [],
    "providers/get_default_model": null,
    "config/get_home_dir": "/home/e2e",
    "config/get_app_config": {
      turnCompleteNotifyEnabled: true,
      scheduledTasksEnabled: true,
    },
    // projects.loadProjects()(body `{"filter":{"hidden":false}}`);
    // 返回 1 个项目 → ChatWindow 选中它 → ChatPanel 渲染(输入框挂载)。
    "projects/list_projects": [
      {
        id: "e2e-project",
        name: "e2e-project",
        path: "/home/e2e/e2e-project",
        is_git_repo: false,
        git_branch: null,
        is_legacy: false,
        created_at: "2026-01-01T00:00:00Z",
        updated_at: "2026-01-01T00:00:00Z",
        hidden: false,
        metadata: null,
      },
    ],
    // AppShell.onMounted → scheduledTasks.load()(启动徽章拉全量)
    "scheduled_tasks/list_scheduled_tasks": [],
    // HiddenProjectsMenu.onMounted → loadHiddenProjects(test-environment
    // .md §6 的 mount 重载面;空 = 无隐藏项目)。
    "projects/list_hidden_projects": [],
    // ChatInputTokenUsage mount → quota.refresh()(5h 窗口报告,
    // camelCase wire,见 quota.ts UsageWindowReportWire;空 providers
    // = 零用量)。
    "usage/usage_window": {
      windowHours: 5,
      limitTokens: null,
      providers: [],
      topSessions: [],
    },
    // chat watcher onProjectChange → loadSessions;空列表 → 保持
    // currentSessionId=null(发送走「懒建 session」路径)。
    "sessions/list_sessions": [],
    // createNewSession 的消费面:session.id + session.current_cwd。
    "sessions/create_session": {
      id: "e2e-session-1",
      title: "新对话",
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
      model: "",
      project_id: "e2e-project",
      current_cwd: "/home/e2e/e2e-project",
    },
    // ensureLoaded:fresh session 无历史 → null(代码分支 `loaded ? … : []`)。
    "sessions/load_session": null,
    // ChatPanel watch currentSessionId → messageQueue.hydrate
    "message_queue/list_queued_messages": [],
    // ensureLoaded → 权威 pending 拉平(spec test-environment.md §8:
    // mock 必须与 seed 一致;此处恒 null = 无提问卡)。
    "question/get_pending_interaction": null,
    // startRequest 的受理面(F1 ChatAcceptance):started = 开流,
    // 后续事件经 stream.emit 推。
    "agent/chat": { status: "started" },
    // startRequest → traceStore.resetForNewSession → loadHistory(每轮
    // 发送都会拉 trace 时间线;fire-and-forget catch,但正常发送面不该
    // 靠 500 兜底)。
    "permissions/list_turn_traces": [],
    "permissions/list_session_audit_events": [],
  };
}

/** health.ts DaemonHealth(见文件头「health 硬门」;daemonVersion 与
 *  app/package.json 同版,避免 build-drift console.warn)。 */
function defaultHealth(): Record<string, unknown> {
  return {
    daemonId: "e2e-daemon",
    daemonVersion: "0.1.0",
    apiVersions: ["v1"],
  };
}

// ---------------------------------------------------------------------------
// world + catch-all dispatcher
// ---------------------------------------------------------------------------

interface E2EWorld {
  page: Page;
  /** cmd 注册表:key = `${domain}/${cmd}`。后注册覆盖先注册。 */
  registry: Map<string, MockPayload>;
  /** `GET /api/v1/health` 应答(mockHealth 可覆盖)。 */
  health: Record<string, unknown>;
  reqs: RecordedRequest[];
}

function parseApiPath(
  pathname: string,
): { domain: string | null; cmd: string | null } {
  const m = /^\/api\/v1\/([^/]+)(?:\/([^/]+))?$/.exec(pathname);
  if (!m) return { domain: null, cmd: null };
  return { domain: m[1] ?? null, cmd: m[2] ?? null };
}

function corsHeaders(extra: Record<string, string>): Record<string, string> {
  return { "Access-Control-Allow-Origin": "*", ...extra };
}

async function installDispatcher(world: E2EWorld): Promise<void> {
  await world.page.route("**/api/v1/**", async (route: Route) => {
    const req = route.request();
    const url = new URL(req.url());
    const { domain, cmd } = parseApiPath(url.pathname);
    let body: unknown = null;
    const postData = req.postData();
    if (postData) {
      try {
        body = JSON.parse(postData);
      } catch {
        body = postData;
      }
    }
    // 先记录(含 miss / preflight)—— 请求观察断言不依赖注册表。
    world.reqs.push({
      method: req.method(),
      path: url.pathname,
      domain,
      cmd,
      body,
    });

    // dev 跨域(1422 → daemonBase 7456)的 preflight,先于注册表应答。
    if (req.method() === "OPTIONS") {
      await route.fulfill({
        status: 204,
        headers: corsHeaders({
          "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
          "Access-Control-Allow-Headers": "Content-Type, Authorization",
          "Access-Control-Max-Age": "600",
        }),
      });
      return;
    }

    if (req.method() === "GET" && url.pathname === "/api/v1/health") {
      await route.fulfill({
        status: 200,
        headers: corsHeaders({ "Content-Type": "application/json" }),
        body: JSON.stringify(world.health),
      });
      return;
    }

    const key = domain && cmd ? `${domain}/${cmd}` : null;
    const payload = key ? world.registry.get(key) : undefined;
    if (payload === undefined) {
      // fail-loud(文件头):漏 mock 的请求宁可炸穿,也不许经 vite /api
      // proxy 静默漏到真 daemon。请求已记录,断言侧可见。
      await route.fulfill({
        status: 500,
        headers: corsHeaders({ "Content-Type": "application/json" }),
        body: JSON.stringify({
          kind: "e2e-mock-miss",
          message: `no e2e mock registered for ${req.method()} ${url.pathname} — add mockCmd(domain, cmd, payload) in the spec`,
        }),
      });
      return;
    }
    await route.fulfill({
      status: 200,
      headers: corsHeaders({ "Content-Type": "application/json" }),
      // 无 envelope:payload 即 Json<T> 本体。"null" 是合法 JSON,与
      // daemon 的 JSON null 透传等价(http.ts 空 body → null 同语义)。
      body: JSON.stringify(payload),
    });
  });
}

function createWorld(page: Page): E2EWorld {
  const world: E2EWorld = {
    page,
    registry: new Map(Object.entries(bootDefaults())),
    health: defaultHealth(),
    reqs: [],
  };
  return world;
}

// ---------------------------------------------------------------------------
// 扩展 test —— 统一词表(design §2.3):
// mockCmd / mockHealth / stream.emit / reqs / waitForCmd / boot
// ---------------------------------------------------------------------------

export interface StreamFixture {
  /** 推一个 SSE 事件给所有存活 fake EventSource(事件名与 payload 形状
   *  照抄 http.ts listen 分发表,如 `chat-event` / `tool:question`)。 */
  emit: (name: string, payload: unknown) => Promise<void>;
}

interface TestFixtures {
  /** per-test 世界(page + 注册表 + 请求记录)。world fixture 的 setup
   *  完成 route + initScript 安装(先于任何 goto),一般不直接消费。 */
  world: E2EWorld;
  /** 注册 `POST /api/v1/{domain}/{cmd}` 应答(payload = Json<T> 原样)。
   *  后注册覆盖先注册(含覆盖 boot 默认表)。 */
  mockCmd: (domain: string, cmd: string, payload: MockPayload) => void;
  /** 覆盖 `GET /api/v1/health` 应答(DaemonHealth,camelCase;缺省用
   *  defaultHealth)。供需要自定义 health 形状的用例使用。 */
  mockHealth: (health: Record<string, unknown>) => void;
  stream: StreamFixture;
  /** dispatcher 观察到的全部 /api/v1/** 请求(含 miss;先记录后判定)。 */
  reqs: RecordedRequest[];
  /** 等 `POST /api/v1/{domain}/{cmd}` 出现并返回该记录(轮询 reqs)。 */
  waitForCmd: (
    domain: string,
    cmd: string,
    timeoutMs?: number,
  ) => Promise<RecordedRequest>;
  /** goto → 等 app mount(route/initScript 已在 world setup 装好)。
   *  path 缺省 "/"(router redirect → /chat)。 */
  boot: (path?: string) => Promise<void>;
}

export const test = base.extend<TestFixtures>({
  world: [
    async ({ page }, use) => {
      const world = createWorld(page);
      await installDispatcher(world);
      await page.addInitScript(FAKE_EVENT_SOURCE_INIT);
      await use(world);
    },
    // page fixture 每测试独立 → route / initScript / 记录天然无跨测试
    // 串扰(fullyParallel 安全,playwright.config 注)。
    { auto: true },
  ],
  mockCmd: async ({ world }, use) => {
    await use((domain, cmd, payload) => {
      world.registry.set(`${domain}/${cmd}`, payload);
    });
  },
  mockHealth: async ({ world }, use) => {
    await use((health) => {
      world.health = health;
    });
  },
  stream: async ({ world }, use) => {
    await use({
      emit: (name, payload) =>
        world.page.evaluate(
          ([n, p]) =>
            (
              window as unknown as {
                __stream: StreamControlLike;
              }
            ).__stream.emit(n, p),
          [name, payload],
        ),
    });
  },
  reqs: async ({ world }, use) => {
    await use(world.reqs);
  },
  waitForCmd: async ({ world }, use) => {
    await use((domain, cmd, timeoutMs = 10_000) => {
      const deadline = Date.now() + timeoutMs;
      const poll = async (): Promise<RecordedRequest> => {
        for (;;) {
          const hit = world.reqs.find(
            (r) => r.method === "POST" && r.domain === domain && r.cmd === cmd,
          );
          if (hit) return hit;
          if (Date.now() > deadline) {
            throw new Error(
              `waitForCmd: POST /api/v1/${domain}/${cmd} not observed within ` +
                `${timeoutMs}ms(observed: ` +
                `${world.reqs.map((r) => `${r.method} ${r.path}`).join(", ") || "nothing"})`,
            );
          }
          await world.page.waitForTimeout(50);
        }
      };
      return poll();
    });
  },
  boot: async ({ world }, use) => {
    await use(async (path = "/") => {
      await world.page.goto(path, { waitUntil: "domcontentloaded" });
      // 等 app mount:health 握手过 → router → ChatView → 项目选中 →
      // ChatPanel → CM 编辑器挂载。这就是「app mount 完成」的观察点。
      await world.page.waitForSelector(EDITOR, {
        state: "attached",
        timeout: 30_000,
      });
    });
  },
});

/** evaluate 侧的 window.__stream 最小形状(与 init script 内一致)。 */
interface StreamControlLike {
  emit: (name: string, payload: unknown) => void;
}
