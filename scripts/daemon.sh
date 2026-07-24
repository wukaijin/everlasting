#!/usr/bin/env bash
# scripts/daemon.sh — everlasting-daemon 浏览器模式管理脚本
#
# 覆盖"WSL 跑 release daemon + Windows 宿主浏览器访问"的全流程。
# 这是 Phase 2 的核心使用场景(daemon 自己 serve 前端 + API,同源无 CORS)。
#
# 用 PID 文件管理进程,避免 pkill -f 自匹配(脚本命令行含 daemon 名会被误杀)。
#
# 用法:
#   ./scripts/daemon.sh start   [--port N]   编译 release 二进制 + 启动(前台日志)
#   ./scripts/daemon.sh bg      [--port N]   同 start,但后台运行 + 日志写文件
#   ./scripts/daemon.sh stop                 停止运行中的 daemon
#   ./scripts/daemon.sh restart [--port N]   stop + start(改前端后重新 serve dist)
#   ./scripts/daemon.sh rebuild              只重新编译 release 二进制(不重启)
#   ./scripts/daemon.sh status               显示进程状态 + health 检查
#   ./scripts/daemon.sh logs                 跟踪日志(bg 模式的日志文件)
#
# 选项:
#   --port N    监听端口(默认 7456)
#   --no-build  start/bg/restart 时跳过 release 编译(用现有二进制)
#   -h, help    显示帮助
#
# 设计参考:docs/HACKING-wsl.md §远程访问 daemon 部署。
set -euo pipefail

# ── 路径常量(脚本可移植,不硬编码绝对路径) ──────────────────────────
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC_TAURI="$REPO_ROOT/app/src-tauri"
DAEMON_BIN="$SRC_TAURI/target/release/everlasting-daemon"
PID_FILE="$REPO_ROOT/.everlasting-daemon.pid"
LOG_FILE="/tmp/everlasting-daemon.log"
DEFAULT_PORT=7456

# WSL 编译 Rust 的硬性前置(CLAUDE.md 坑 1:不加则撞 gdk-pixbuf not found)。
PKG_CONFIG_PATH="${PKG_CONFIG_PATH:-/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig}"
export PKG_CONFIG_PATH

# ── 输出辅助 ──────────────────────────────────────────────────────
log()  { printf '\033[32m▸ %s\033[0m\n' "$*"; }   # 绿色 ▸
warn() { printf '\033[33m⚠ %s\033[0m\n' "$*" >&2; } # 黄色 ⚠
err()  { printf '\033[31m✗ %s\033[0m\n' "$*" >&2; } # 红色 ✗
die()  { err "$*"; exit 1; }

# ── 进程管理(基于 PID 文件) ───────────────────────────────────────
# 读 PID 文件;校验进程确实存活(避免 PID 文件陈旧导致误判)。
running_pid() {
    [[ -f "$PID_FILE" ]] || return 1
    local pid; pid="$(cat "$PID_FILE" 2>/dev/null || true)"
    [[ -n "$pid" ]] || return 1
    if kill -0 "$pid" 2>/dev/null; then
        echo "$pid"
        return 0
    fi
    return 1
}

# ── 子命令实现 ────────────────────────────────────────────────────
do_build() {
    log "编译 daemon release 二进制(首次 ~3-5 min,vendored libgit2)"
    (
        cd "$SRC_TAURI"
        cargo build --release --bin everlasting-daemon
    )
    log "编译完成 → $DAEMON_BIN"
}

do_start() {
    local port="$DEFAULT_PORT" background=false do_build_flag=true
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --port)     port="$2"; shift 2;;
            --bg)       background=true; shift;;
            --no-build) do_build_flag=false; shift;;
            *) die "start: 未知参数 '$1'(可用:--port N --bg --no-build)";;
        esac
    done

    # 已有实例在跑 → 不重复启动(Q1 决议:避免多 daemon 数据分裂)。
    if pid="$(running_pid)"; then
        die "daemon 已在运行(PID $pid)。先 ./scripts/daemon.sh stop 再 start,或用 restart。"
    fi

    $do_build_flag && do_build
    [[ -x "$DAEMON_BIN" ]] || die "二进制不存在:$DAEMON_BIN(先跑 build?)"

    log "启动 daemon (port=$port)"
    if $background; then
        nohup "$DAEMON_BIN" --port "$port" > "$LOG_FILE" 2>&1 &
        local pid=$!
        echo "$pid" > "$PID_FILE"
        sleep 1.5
        if kill -0 "$pid" 2>/dev/null; then
            log "后台运行中(PID $pid)→ 日志:tail -f $LOG_FILE"
        else
            err "进程已退出,看日志排查:"
            tail -20 "$LOG_FILE" >&2 || true
            rm -f "$PID_FILE"
            exit 1
        fi
    else
        # 前台模式:直接 exec,日志打终端;Ctrl+C 退出(PID 文件在 trap 里清)。
        # 先写 PID 文件,trap 保证退出时清理。
        echo $$ > "$PID_FILE"
        trap 'rm -f "$PID_FILE"' EXIT INT TERM
        exec "$DAEMON_BIN" --port "$port"
    fi
}

do_stop() {
    if pid="$(running_pid)"; then
        log "停止 daemon (PID $pid)"
        kill "$pid" 2>/dev/null || true
        # 优雅等 15s。SIGTERM 让 daemon 走 axum graceful_shutdown:
        # 先 sse.shutdown() 关 SSE 长连接(亚秒),再 cancel+drain 活跃
        # agent loop(最多 DAEMON_SHUTDOWN_LOOP_DRAIN_SECS=8s,让 in-flight
        # tool 跑完 persist_turn 落库),再 axum drain 短请求(SHUTDOWN_GRACE_SECS=3s)。
        # 最坏 8s + 3s = 11s,15s 窗口留 4s 余量。超时再 SIGKILL 兜底防卡死。
        # (2026-07-24,task 07-24-daemon-agent-loop-shutdown:从 8s 拉到 15s,
        # 否则 SIGKILL 会抢先于 agent loop drain 斩断落库。)
        for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do
            kill -0 "$pid" 2>/dev/null || break
            sleep 1
        done
        # 仍未退出 → SIGKILL 兜底(防卡死)。
        if kill -0 "$pid" 2>/dev/null; then
            warn "15s 后仍未退出,SIGKILL"
            kill -9 "$pid" 2>/dev/null || true
        fi
        rm -f "$PID_FILE"
        log "已停止"
    else
        log "daemon 未运行(或 PID 文件陈旧,已清理)"
        rm -f "$PID_FILE"
    fi
}

do_restart() {
    log "重启 daemon"
    do_stop
    do_start "$@" --bg  # restart 默认后台跑(最常见:改前端后重启 serve 新 dist)
}

do_status() {
    local port="$DEFAULT_PORT"
    # status 也接受 --port(万一非默认端口启动,health 检查才能命中)。
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --port) port="$2"; shift 2;;
            *) shift;;
        esac
    done
    if pid="$(running_pid)"; then
        log "daemon 运行中(PID $pid)"
        if command -v curl >/dev/null 2>&1; then
            local resp
            if resp="$(curl -sS -m 2 "http://localhost:$port/api/v1/health" 2>/dev/null)"; then
                log "health: $resp"
            else
                warn "进程在跑但 health 无响应(port $port)——可能还在启动中"
            fi
        fi
    else
        warn "daemon 未运行"
        exit 1
    fi
}

do_logs() {
    [[ -f "$LOG_FILE" ]] || die "日志文件不存在:$LOG_FILE(先用 bg/restart 启动)"
    log "跟踪日志(Ctrl+C 退出): $LOG_FILE"
    tail -f "$LOG_FILE"
}

show_help() {
    sed -n '2,/^set -euo/p' "$0" | sed 's/^# \?//' | sed '/^set -euo/d'
}

# ── 入口 ──────────────────────────────────────────────────────────
main() {
    local cmd="${1:-help}"
    shift || true
    case "$cmd" in
        start)    do_start "$@";;
        bg)       do_start "$@" --bg;;
        stop)     do_stop "$@";;
        restart)  do_restart "$@";;
        rebuild)  do_build;;
        status)   do_status "$@";;
        logs)     do_logs "$@";;
        help|-h|--help) show_help;;
        *) die "未知命令 '$cmd'。用 ./scripts/daemon.sh help 查看用法。";;
    esac
}

main "$@"
