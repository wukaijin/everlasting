#!/usr/bin/env bash
# scripts/remote.sh — everlasting-remote 边缘 daemon 管理脚本
#
# remote daemon 跑云服务器(国内 2C2G),收 PC daemon 的 WSS outbound
# 连接 + 手机配对/反向代理(epic 08-11-remote-control-epic,S1)。
#
# 与 daemon.sh 的区别:
#   - **零系统库依赖**(无 webkit/gdk-pixbuf),不需要 PKG_CONFIG_PATH
#   - `--shared-secret` **必传**(Q-S1:secret 无默认,缺了直接 panic)
#   - 默认端口 7457(daemon 的 7456 错开)
#
# 用 PID 文件管理进程,避免 pkill -f 自匹配(脚本命令行含二进制名会被误杀)。
#
# 用法:
#   ./scripts/remote.sh start   [--port N] [--secret S] [--db-path P] [--no-build]
#   ./scripts/remote.sh bg      [--port N] [--secret S] [--db-path P] [--no-build]
#   ./scripts/remote.sh stop
#   ./scripts/remote.sh restart [--port N] [--secret S] [--db-path P]
#   ./scripts/remote.sh rebuild
#   ./scripts/remote.sh status  [--port N]
#   ./scripts/remote.sh logs
#
# 选项:
#   --port N    监听端口(默认 7457)
#   --secret S  shared_secret,必传;也可用 env EVERLASTING_REMOTE_SECRET
#   --db-path P SQLite 路径(默认 ~/.local/share/dev.everlasting.remote/remote.db)
#   --no-build  start/bg/restart 时跳过 release 编译
#   -h, help    显示帮助
set -euo pipefail

# ── 路径常量(脚本可移植,不硬编码绝对路径) ──────────────────────────
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REMOTE_BIN="$REPO_ROOT/target/release/everlasting-remote"
PID_FILE="$REPO_ROOT/.everlasting-remote.pid"
LOG_FILE="/tmp/everlasting-remote.log"
DEFAULT_PORT=7457

# ── 输出辅助 ──────────────────────────────────────────────────────
log()  { printf '\033[32m▸ %s\033[0m\n' "$*"; }   # 绿色 ▸
warn() { printf '\033[33m⚠ %s\033[0m\n' "$*" >&2; } # 黄色 ⚠
err()  { printf '\033[31m✗ %s\033[0m\n' "$*" >&2; } # 红色 ✗
die()  { err "$*"; exit 1; }

# ── 进程管理(基于 PID 文件) ───────────────────────────────────────
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
    log "编译 remote release 二进制(零系统库依赖,~1-2 min)"
    (
        cd "$REPO_ROOT"
        cargo build --release -p everlasting-remote
    )
    log "编译完成 → $REMOTE_BIN"
}

do_start() {
    local port="$DEFAULT_PORT" secret="" db_path="" background=false do_build_flag=true
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --port)     port="$2"; shift 2;;
            --secret)   secret="$2"; shift 2;;
            --db-path)  db_path="$2"; shift 2;;
            --bg)       background=true; shift;;
            --no-build) do_build_flag=false; shift;;
            *) die "start: 未知参数 '$1'(可用:--port N --secret S --db-path P --bg --no-build)";;
        esac
    done

    # secret 双通道:CLI flag > env(Q-S1 必传,缺了 remote 启动即 panic)。
    if [[ -z "$secret" ]]; then
        secret="${EVERLASTING_REMOTE_SECRET:-}"
    fi
    if [[ -z "$secret" ]]; then
        die "缺少 shared_secret:传 --secret <S> 或设置 env EVERLASTING_REMOTE_SECRET(防裸跑,Q-S1)"
    fi

    if pid="$(running_pid)"; then
        die "remote 已在运行(PID $pid)。先 ./scripts/remote.sh stop 再 start,或用 restart。"
    fi

    $do_build_flag && do_build
    [[ -x "$REMOTE_BIN" ]] || die "二进制不存在:$REMOTE_BIN(先跑 rebuild?)"

    local args=(--port "$port" --shared-secret "$secret")
    [[ -n "$db_path" ]] && args+=(--db-path "$db_path")

    log "启动 remote (port=$port, db-path=${db_path:-默认})"
    if $background; then
        nohup "$REMOTE_BIN" "${args[@]}" > "$LOG_FILE" 2>&1 &
        local pid=$!
        echo "$pid" > "$PID_FILE"
        sleep 1
        if kill -0 "$pid" 2>/dev/null; then
            log "后台运行中(PID $pid)→ 日志:tail -f $LOG_FILE"
        else
            err "进程已退出,看日志排查:"
            tail -20 "$LOG_FILE" >&2 || true
            rm -f "$PID_FILE"
            exit 1
        fi
    else
        echo $$ > "$PID_FILE"
        trap 'rm -f "$PID_FILE"' EXIT INT TERM
        exec "$REMOTE_BIN" "${args[@]}"
    fi
}

do_stop() {
    if pid="$(running_pid)"; then
        log "停止 remote (PID $pid)"
        kill "$pid" 2>/dev/null || true
        # 优雅等 10s:SIGTERM → 先关全部 WSS 隧道(close_all)再 drain。
        for _ in 1 2 3 4 5 6 7 8 9 10; do
            kill -0 "$pid" 2>/dev/null || break
            sleep 1
        done
        if kill -0 "$pid" 2>/dev/null; then
            warn "10s 后仍未退出,SIGKILL"
            kill -9 "$pid" 2>/dev/null || true
        fi
        rm -f "$PID_FILE"
        log "已停止"
    else
        log "remote 未运行(或 PID 文件陈旧,已清理)"
        rm -f "$PID_FILE"
    fi
}

do_restart() {
    log "重启 remote"
    do_stop
    do_start "$@" --bg
}

do_status() {
    local port="$DEFAULT_PORT"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --port) port="$2"; shift 2;;
            *) shift;;
        esac
    done
    if pid="$(running_pid)"; then
        log "remote 运行中(PID $pid)"
        if command -v curl >/dev/null 2>&1; then
            local resp
            if resp="$(curl -sS -m 2 "http://localhost:$port/api/v1/health" 2>/dev/null)"; then
                log "health: $resp"
            else
                warn "进程在跑但 health 无响应(port $port)——可能还在启动中"
            fi
        fi
    else
        warn "remote 未运行"
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
        *) die "未知命令 '$cmd'。用 ./scripts/remote.sh help 查看用法。";;
    esac
}

main "$@"
