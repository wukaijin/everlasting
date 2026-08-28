# remote daemon 部署指南(云服务器)

`everlasting-remote` 是 epic 08-11-remote-control-epic 的边缘 daemon,跑在
国内 2C2G 云服务器,收各 PC daemon 的 WSS outbound 连接,为手机 PWA
提供配对 + 反向代理。**ubuntu 原生二进制,零系统库依赖,无 docker 无
runtime。**

架构决策见 `.trellis/tasks/archive/2026-08/08-11-remote-control-epic/prd.md`(single
source of truth);本节只讲运维。

## 1. 编译

```bash
# 本机(WSL / CI)或服务器上均可:
cargo build --release -p everlasting-remote
# 产物:target/release/everlasting-remote(ubuntu ELF,直接可跑)
```

零系统库依赖 —— **不需要** PKG_CONFIG_PATH / webkit2gtk / gdk-pixbuf
(对比 daemon)。

## 2. 运行

```bash
# 方式一:直接跑(前台)
./everlasting-remote --port 7457 --shared-secret <SECRET>

# 方式二:仓库脚本管理(推荐,仿 scripts/daemon.sh)
./scripts/remote.sh bg --secret <SECRET>          # 后台 + 日志
./scripts/remote.sh status                        # 进程 + health
./scripts/remote.sh logs                          # tail -f
./scripts/remote.sh stop
```

参数:

| 参数 | 默认 | 说明 |
|---|---|---|
| `--port` | `7457` | 监听端口(daemon 的 7456 错开;nginx 反代到本端口) |
| `--db-path` | `~/.local/share/dev.everlasting.remote/remote.db` | remote 自己的 SQLite(只存 nodes/devices/配对码三张表,**不存 agent 数据**) |
| `--shared-secret` | **无,必传** | PC daemon 连 WSS 的握手密钥。可换 env `EVERLASTING_REMOTE_SECRET`;缺了启动直接 panic(Q-S1,防裸跑) |

健康检查:`GET /health` 或 `GET /api/v1/health`(无认证,nginx 健康检查用)。

## 3. nginx 反代(HTTPS,用户自理证书)

```nginx
server {
    listen 443 ssl;
    server_name remote.yourdomain.com;

    # 手机 PWA + 普通 API(remote 自己伺服前端,ServeDir)
    location / {
        proxy_pass http://127.0.0.1:7457;
    }

    # /ws **必须单独 location**:WebSocket 升级头要透传,普通 HTTP 反代
    # 会断 WSS。
    # proxy_read_timeout 300s 必加(心跳 30s × 倍数,否则空闲 60s 被 nginx
    # 默认值掐断);access_log off 建议(?secret=<S> 在 query 里,会被记进
    # $request,服务器日志别躺着共享密钥)。
    location /ws {
        proxy_pass http://127.0.0.1:7457;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_read_timeout 300s;
        access_log off;
    }
}
```

> 生产建议:systemd unit 托管(开机自启 + 崩溃拉起),`--shared-secret`
> 放 env 文件(600 权限),不要进命令行历史。

## 4. 用 deploy-remote.sh 部署(日常更新)

首次按 §1/§2 手工铺好(二进制 + dist + 下文 start.sh)后,日常更新在**本机**
跑 `scripts/deploy-remote.sh`,一条命令完成 编译 → 上传 → 重启 → 验证:

1. **编译**:`cargo build --release -p everlasting-remote`(二进制)+
   `pnpm -C app build`(前端 dist)
2. **上传(.new 暂存)**:scp 二进制 → `$DIR/everlasting-remote.new`、
   `dist/` → `$DIR/dist.new` —— 先传 .new 再切换,上传中断不留半成品
3. **切换 + 重启**:`pkill -f 'everlasting-remote --port'` 停旧进程 → mv
   `.new` 就位;只更前端时跳过重启(ServeDir 实时读,零中断);更二进制时
   `nohup '$DIR/start.sh'` 拉起(中断 ~2s,PC tunnel 自动重连,手机请求重试)
4. **验证**:服务器本地 `curl localhost:7457/health` + 外网
   `curl $REMOTE_URL/health`(经 nginx,需先配好 §3)

配置:`cp scripts/.deploy-remote.env.example scripts/.deploy-remote.env`
(gitignored,填真实值),三个变量:

| 变量 | 含义 |
|---|---|
| `REMOTE_HOST` | ssh host(~/.ssh/config 别名,或 user@host) |
| `REMOTE_URL` | remote 对外 https URL(脚本用它做外网 health 验证) |
| `REMOTE_DIR` | 服务器部署目录 = 下文 `$DIR`(二进制 + dist + start.sh 所在) |

选项:`--no-binary`(只更前端)/ `--no-frontend`(只更二进制)/
`--no-build`(都不编译,用现有产物),详见脚本 `--help`。

> ⚠️ **硬依赖 `$DIR/start.sh`**:脚本重启时执行 `nohup '$DIR/start.sh'`,
> 该文件**不在仓库里,需在服务器上手工创建一次**(放 `$DIR`,与二进制、
> `dist/` 同级)。内容:注入 `EVERLASTING_REMOTE_SECRET` + 指向前端产物的
> `EVERLASTING_REMOTE_DIST_DIR` + nohup 启动 `everlasting-remote` 二进制。
> 模板如下(变量名与 `.deploy-remote.env.example` 的 `REMOTE_DIR` 语义一致):

```bash
#!/usr/bin/env bash
# $REMOTE_DIR/start.sh — deploy-remote.sh 的启动钩子(服务器上手工创建一次)
# 注意:exec 行必须带 --port —— deploy-remote.sh 用 pkill -f 'everlasting-remote --port' 匹配。
set -euo pipefail
cd "$(dirname "$0")"

export EVERLASTING_REMOTE_SECRET="<你定的共享密钥>"   # PC daemon 连 WSS 的握手密钥
DIST_DIR="$PWD/dist"                                  # 前端 PWA 产物
export EVERLASTING_REMOTE_DIST_DIR="$DIST_DIR"

exec ./everlasting-remote --port 7457
```

创建后 `chmod +x start.sh` 并收紧权限(600,密钥在里面)。升级不碰
remote.db(nodes/devices/配对码),无需重新配对。

## 5. 运营

**日志**(tracing,关键事件):

- `INFO ... PC daemon tunnel connected node_id=...` — PC 连上
- `INFO ... tunnel disconnected, node offline` — PC 断开/超时
- `INFO ... pairing code generated` / `pairing code redeemed` — 配对
- `WARN ... shared_secret_rejected ip=...` — 伪 daemon 尝试(应被 nginx 挡在
  HTTPS 外,这里兜底)

**监控**(S1 最小):`GET /health` + `GET /api/v1/nodes`(带 device_token
看在线状态)。V2 视需要加 Prometheus 端点。

**数据**:remote.db 只有 nodes/devices/配对码三张表,删了不影响任何 PC 侧
数据;删除后手机需重新配对。

## 6. 端到端冒烟

无 S2(PC 侧 tunnel client)时,用脚本模拟 PC daemon 验证全链路:

```bash
./target/release/everlasting-remote --port 7457 --shared-secret test123 &
node scripts/remote-e2e-smoke.mjs
# 期望:E2E OK — 全链路通过
```

(Node >= 22,内置 WebSocket。脚本覆盖:WSS 握手 → 配对码生成 →
redeem → 节点状态 → 非流式反向代理转发。)

## 7. 安全模型(多层,epic NFR P0)

| 层 | 手段 |
|---|---|
| 传输 | HTTPS(nginx 终结);WSS 内网加密 |
| 认证 | `shared_secret`(PC 连 WSS 握手,常时比较)+ `device_token`(手机 HTTP,header 或 query) |
| 防暴扫 | redeem per-IP 限速(10/min)+ 配对码 60s 一次性 |
| 暴露面 | remote 只监听端口,nginx 只放 443;错误 secret 有 WARN 日志 |
| 数据 | remote.db 不存 agent 数据;token 不流向 PC(转发时剥离) |
