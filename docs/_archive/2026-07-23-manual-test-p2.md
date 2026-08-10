# 远程访问 Phase 2 — 手动测试指南

> **目的**:Phase 2 代码 + 自动化测试已全部落地并提交(P2.1–P2.5,commit `e6b7a2f`)。
> 这份指南覆盖 `implement.md` 里**本环境做不了、必须手动验证**的 E4 smoke + Phase 2 整体验收项。
> 跑完所有 ✅ 后,task `07-20-remote-access-daemon-split` 才能标记 Phase 2 完成。
>
> **前置**:一台 **GUI-capable 机器**(能跑 Tauri 窗口或浏览器)+ 可选真实 LLM 凭据(chat 实跑需要;health/SSE/进程清理不需要)。
> **环境**:WSL 2 + Ubuntu 22.04(见 `docs/HACKING-wsl.md`);Windows 宿主浏览器经 localhost forwarding 访 WSL daemon。
>
> **速查命令**:`PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"` 是 WSL 编译 Rust 的硬性前置(CLAUDE.md 坑 1),下文 `$PCP` 代指它。

---

## 0. 预检:构建产物 + 健康检查(全部场景通用)

```bash
# A. 编译前端 dist(ServeDir 要用)
cd app && pnpm build                      # 产物: app/dist/;vue-tsc + vite build,0 err 才算过

# B. 编译 daemon release 二进制(含 sidecar staging)
cd src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo build --release --bin everlasting-daemon
#    首次编译 ~3-5 min(vendored libgit2);产物 target/release/everlasting-daemon
#    build.rs 会把 daemon 二进制 staging 到 src-tauri/binaries/everlasting-daemon-<triple>(GUI sidecar 要)

# C. 确认 daemon 能起 + 健康检查通过
cd src-tauri && ./target/release/everlasting-daemon --port 7456 &
sleep 1
curl -s http://localhost:7456/api/v1/health | python3 -m json.tool
#    期望 200 + JSON:
#      {"daemonId":"<uuid>","daemonVersion":"0.1.0","apiVersions":["v1"],"uptimeSeconds":N,"sessionCount":...}
#    ⚠️ apiVersions 必须含 "v1"(GUI health 握手 Q5 协议门;不含则 fail-loud)
kill %1
```

- [ ] A `pnpm build` 成功(dist/ 生成,vue-tsc 0 err)
- [ ] B daemon release 编译成功
- [ ] C health 返回 200 + `apiVersions` 含 `"v1"`

---

## 场景一:生产模式(单二进制部署)— Windows 宿主浏览器访问 WSL daemon

> 这是用户主用场景。daemon 自己 serve 前端 + API(同源),浏览器 `http://localhost:7456` 直达完整功能。

```bash
# WSL 内
cd app/src-tauri && ./target/release/everlasting-daemon --port 7456
#    日志:everlasting-daemon listening 0.0.0.0:7456 + serving static frontend from dist dir
```

Windows 宿主(PowerShell 或浏览器):

```powershell
# 健康检查(经 WSL 2 localhost forwarding)
curl http://localhost:7456/api/v1/health    # 期望 200
start http://localhost:7456                  # 浏览器打开
```

- [ ] Windows 宿主 `curl localhost:7456/api/v1/health` 返回 200(localhost forwarding 通)
- [ ] 浏览器打开 `http://localhost:7456` 加载出前端 UI(不是空白/404)
- [ ] 浏览器 console **无** CORS error(同源,不该有 preflight)

**若 localhost forwarding 不通**(降级,详见 `docs/HACKING-wsl.md` §远程访问 daemon 部署):
```bash
# WSL 内取虚拟 IP
ip -4 addr show eth0 | grep -oP '(?<=inet\s)\d+(\.\d+){3}'   # 例 172.x.x.x
# Windows 浏览器改访问 http://172.x.x.x:7456(daemon 已绑 0.0.0.0)
```

---

## 场景二:核心功能实跑(需 LLM 凭据)

> 在浏览器里跑一轮完整对话,验证 httpTransport 全链路。需先配 provider + model(经 Settings UI 或下述 API)。

### 2.1 配置 provider + model

浏览器打开 `http://localhost:7456` → Settings → Providers,或用 API:

```bash
BASE=http://localhost:7456/api/v1
# 1. 加 provider(protocol=anthropic,base_url 指你的中转或官方)
curl -s -X POST $BASE/providers/add_provider -H 'Content-Type: application/json' \
  -d '{"protocol":"anthropic","display_name":"my-anthropic","base_url":"https://api.anthropic.com","api_key":"sk-ant-..."}'
#    期望 {"id":"<provider-id>",...}

# 2. 加 model(provider_id 填上一步返回的 id)
curl -s -X POST $BASE/providers/add_model -H 'Content-Type: application/json' \
  -d '{"provider_id":"<provider-id>","model_name":"claude-sonnet-4-5","display_name":"Sonnet","max_tokens":16384,"thinking_effort":null,"supports_thinking":false,"context_window":200000}'

# 3. 设为默认
curl -s -X POST $BASE/providers/set_default_model -H 'Content-Type: application/json' -d '{"model_id":"<model-id>"}'
```

### 2.2 创建 project + session → 发消息

浏览器 UI:左上 "+ Project"(选一个真实目录)→ 新建 session → 输入框发消息。

或 API:
```bash
# 4. 创建 project(path 必须是真实存在的目录)
curl -s -X POST $BASE/projects/create_project -H 'Content-Type: application/json' \
  -d '{"path":"/home/carlos/some-real-dir"}'

# 5. 创建 session(project_id + initial_cwd 填真实路径)
curl -s -X POST $BASE/sessions/create_session -H 'Content-Type: application/json' \
  -d '{"project_id":"<project-id>","initial_cwd":"/home/carlos/some-real-dir","model":null}'
```

### 2.3 实跑验证项

- [ ] 发普通消息 → **流式逐字输出**(chat-event SSE live,不是卡死后一次性出现)
- [ ] 发触发工具的消息(如 "读一下 README.md")→ **permission 弹窗**出现 → 批准 → 工具执行 → 结果回显
- [ ] 触发 **ask_user_question**(让模型问你问题)→ 问题卡片出现 → 回答后继续
- [ ] 触发 **subagent**(@某 worker 或 dispatch)→ subagent drawer 显示 worker 运行 + `subagent:event` 实时
- [ ] 切换 session → 消息历史正确加载(load_session)

---

## 场景三:SSE 断网重连(无需 LLM)

> 验证 EventSource 自动重连 + Last-Event-ID 回放 / resync sentinel → snapshot 恢复。

```bash
# 1. WSL 内起 daemon + 浏览器连上(场景一或 tauri dev)
# 2. 浏览器 DevTools → Network → 找到 /api/v1/stream 的 EventSource 连接
# 3. 发几条消息,确认 stream 有 chat-event 帧到达(Network 里能看到 event stream)
```

**测试断网重连:**
- [ ] DevTools → 勾 "Offline" → 发消息(应无新帧)→ 取消 "Offline" → **EventSource 自动重连** + 之前漏掉的事件补齐(Last-Event-ID 回放)
- [ ] 或:WSL 内 `kill` daemon → 浏览器 console 出 health 错误 → 重启 daemon → 浏览器是否恢复(取决于 httpTransport 重连逻辑)

**测试 resync sentinel(buffer overrun,高级):**
> 手动难触发(需 buffer 淘汰 >512 帧 + Last-Event-ID 落在淘汰区)。自动化已覆盖(`tests/e2e.rs` E1b `resync_sentinel_on_buffer_overrun`)。手动可跳过,信自动化测试。

---

## 场景四:大输出不丢(5MB shell,需 LLM + 可执行工具)

> 验证 `LARGE_PAYLOAD_THRESHOLD`(256KB)旁路:大 tool_result 走 live channel 不入 buffer,前端不丢。

```bash
# 让模型跑一个输出 5MB 的命令,如:
# "运行 `yes | head -c 5000000` 并把结果给我"
# 或用 dd/head 生成大文件后让模型读
```

- [ ] 工具执行后,**5MB 输出完整回显**(不截断、不空白)
- [ ] DevTools Network → stream 帧里能看到大 tool_result 帧
- [ ] 之后**断网重连**,该大输出**不回放**(因 >256KB 不入 buffer,走 snapshot 重拉)—— 这是预期行为,不是 bug

---

## 场景五:Tauri GUI sidecar 模式(P2.4)

> `pnpm tauri dev`:GUI 自动 spawn daemon sidecar,前端默认走 httpTransport(同源 sidecar),关窗 SIGTERM sidecar。

```bash
cd app && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" pnpm tauri dev
#    首次编译 ~5-10 min(Tauri runtime + webkit)
#    GUI 日志应有:sidecar spawn + daemon listening 0.0.0.0:7456
```

- [ ] Tauri 窗口弹出 + 自动连上 sidecar daemon(health 握手通过,无全屏错误覆盖层)
- [ ] **发消息/流式/permission/subagent 全通**(同场景二,但走 sidecar 同源)
- [ ] **GUI 进程无 SQLite 句柄**(瘦客户端 AC):
  ```bash
  # 另开终端,找 GUI 进程 PID
  pgrep -f "everlasting( |$)" | head    # 或 ps aux | grep everlasting
  lsof -p <gui-pid> 2>/dev/null | grep -i "everlasting.db\|sqlite\|\.db"
  #    期望:空(Thin 模式不开 SqlitePool)
  ```
- [ ] **关 Tauri 窗口 → daemon 进程退出**(SIGTERM):
  ```bash
  # 关窗前
  pgrep -f everlasting-daemon    # 有进程
  # 关窗后
  pgrep -f everlasting-daemon    # 期望:空(RunEvent::Exit → child.kill())
  ```
- [ ] **逃生通道** `?transport=tauri`:GUI 启动时 URL 带 `?transport=tauri` → 走 Full 模式(原 in-process AppState)+ tauriTransport,**不连 daemon**。验证:关掉 sidecar 手动(若 full 模式不 spawn),GUI 仍能用。

---

## 场景六:双进程无 SQLITE_BUSY(需 GUI + 浏览器同时)

> 验证 WAL + busy_timeout=5s 消除双进程写竞争。Thin GUI 不开 db,但 daemon 独占;同时开浏览器也连 daemon。

```bash
# 1. 起 Tauri GUI(场景五,sidecar daemon 起)
# 2. 同时浏览器开 http://localhost:7456(连同一个 daemon)
# 3. 两个客户端往【同一个 session】发消息(交替或同时)
```

- [ ] daemon 日志**无** `SQLITE_BUSY` / `database is locked`
- [ ] 两个客户端的消息都正常落库 + 流式显示(不丢、不卡)
- [ ] 切换 session 再切回,消息历史完整

---

## 场景七:dev 模式(前后端分离,热更新)

> 日常开发用。vite 1420 热更新前端,daemon 7456 跑 API,跨域。

```bash
cd app && pnpm dev:all
#    = concurrently 起 vite(1420)+ daemon(7456)
#    或分两个终端:
#      终端1: pnpm dev:daemon  (cargo run --bin everlasting-daemon -- --port 7456)
#      终端2: pnpm dev         (vite 1420)
```

浏览器:`http://localhost:1420?daemonUrl=http://localhost:7456`
- [ ] vite 热更新生效(改 .vue 文件,浏览器自动刷新)
- [ ] `?daemonUrl=` 把 httpTransport 指向 7456(跨域,daemon CORS 放行)
- [ ] console 无 CORS block(CorsLayer::very_permissive 放行 1420)

---

## Phase 2 整体验收清单(全部 ✅ 后标记 Phase 2 完成)

- [ ] P2.1 ~ P2.5 全部 commit 在主分支 ✅(已满足:`5a212f0`/`f2a675b`/`84d4689`/`e6b7a2f`)
- [ ] **本机浏览器可访问 daemon**(场景一)
- [ ] **WSL→Windows 宿主浏览器访问跑通**(场景一,用户主用场景)
- [ ] **Tauri 版仍可用**(场景五,走 httpTransport 连 sidecar)
- [ ] 核心功能 httpTransport 全通(场景二:消息/流式/permission/question/subagent)
- [ ] 10 类 SSE 事件 + 4 类 round-trip 端到端通过(场景二+三)
  - 10 类:`chat-event` / `tool:call` / `tool:result` / `permission:ask` / `tool:question` / `mode:change:request` / `task:state:transition:request` / `subagent:event` / `subagent:finished` / `stream-resync`
  - 4 类 round-trip:permission / question / mode_change / task_state_transition
- [ ] 断网重连后 UI 完整恢复(场景三)
- [ ] 5MB 输出不丢(场景四)
- [ ] 关 Tauri 窗口后 daemon 进程清理(场景五)
- [ ] `lsof` GUI 无 SQLite 句柄(场景五,瘦客户端 AC)
- [ ] 无 SQLITE_BUSY(场景六)
- [ ] daemon 单二进制部署可用(场景一,前端 + API 同源)
- [ ] `?transport=tauri` 逃生通道可用(场景五)

---

## 验收后的操作

全部 ✅ 后:
1. 更新 `docs/REMOTE-ACCESS-ROADMAP.md` Phase 2 整体验收段,把 `[ ]` 改 `[x]`
2. 更新 `.trellis/tasks/07-20-remote-access-daemon-split/implement.md` E4/E5 勾选
3. 启动 **E5 dogfooding 计时**(主分支日常用 ≥ 2 周无 P0/P1 → Phase 2 整体完成 → 可启动 Phase 3)
4. task 保持 `in_progress` 直到 dogfooding 满 2 周

## 常见问题排查

| 现象 | 排查 |
|---|---|
| 浏览器白屏 | daemon 日志有无 "serving static frontend from dist dir"?无 = dist 未构建(预检 A)或路径错(`EVERLASTING_DIST_DIR` 覆盖) |
| health 返回非 200 | 端口被占?`ss -tlnp \| grep 7456`;daemon Q1 端口冲突探测会 fail-loud |
| CORS error | 生产模式不该有(同源);dev 模式检查 `?daemonUrl=` 是否对 + daemon CORS 层在 |
| Windows 宿主访问不通 | localhost forwarding 失效 → 走虚拟 IP / netsh portproxy(`docs/HACKING-wsl.md` §远程访问) |
| GUI 全屏错误覆盖层 | health 握手失败;检查 sidecar 是否 spawn(`pgrep everlasting-daemon`)+ 日志 |
| `?transport=tauri` 也失败 | Full 模式走原 AppState,排查原 Tauri 路径(Phase 1 前的功能) |
| SQLITE_BUSY | 检查是否 GUI 误开了 db(Full 模式?)+ daemon WAL 配置(`db/migrations.rs` init_pool) |
