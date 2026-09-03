#!/usr/bin/env bash
# scripts/daemon.sh — everlasting-daemon 浏览器模式管理脚本
#
# 覆盖"WSL 跑 daemon + Windows 宿主浏览器访问"的全流程。
# 这是 Phase 2 的核心使用场景(daemon 自己 serve 前端 + API,同源无 CORS)。
#
# 用 PID 文件管理进程,避免 pkill -f 自匹配(脚本命令行含 daemon 名会被误杀)。
#
# 用法:
#   ./scripts/daemon.sh start   [--port N]   编译 daemon 档二进制 + 启动(前台日志)
#   ./scripts/daemon.sh bg      [--port N]   同 start,但后台运行(日志由 daemon 进程内
#                                           appender 落 $XDG_STATE_HOME 下 daemon.log,
#                                           追加写 + >10MiB 滚 3 代)
#   ./scripts/daemon.sh stop                 停止运行中的 daemon
#   ./scripts/daemon.sh restart [--port N]   stop + start(改前端后重新 serve dist)
#   ./scripts/daemon.sh rebuild [--release]  只重新编译二进制(不重启)
#   ./scripts/daemon.sh status               显示进程状态 + health 检查
#   ./scripts/daemon.sh logs                 跟踪日志(bg 模式的日志文件)
#
# 选项:
#   --port N    监听端口(默认 7456)
#   --no-build  start/bg/restart 时跳过编译(用现有二进制)
#   --release   用发布档(fat LTO + CGU=1,target/release/)代替默认的
#               daemon 迭代档(无 LTO + 增量,target/daemon/,根 Cargo.toml
#               [profile.daemon],2026-08-29):发布档单次改动重编 ~6-7min
#               (fat LTO 与增量互斥,leaf 全量重编 + 全程序 LTO/链接),
#               只在需要发布体积 / 极致性能时用;daemon 档改后端代码
#               ~1-2min 出二进制,运行性能与发布档基本一致(I/O 密集)。
#   -h, help    显示帮助
#
# 设计参考:docs/HACKING-wsl.md §远程访问 daemon 部署。
set -euo pipefail

# ── 路径常量(脚本可移植,不硬编码绝对路径) ──────────────────────────
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC_TAURI="$REPO_ROOT/app/src-tauri"
# 2026-08-11 workspace 翻转(task 08-11-remote-daemon-core)后,cargo 把
# 成员 crate 的产物统一落在 workspace 根 `target/`(不再是
# `app/src-tauri/target/`)。daemon.sh 必须指向根 target,否则干净
# checkout 上 rebuild 出的新二进制在根 target、脚本却拉旧路径的陈旧
# 产物(S2 给 daemon 加 tunnel client 后这会让脚本一直跑旧 daemon)。
# 2026-08-29:默认档从 release 换成 daemon 迭代档(增量编译,见头部
# --release 说明);--release 时经 use_release_profile 切回发布档。
BUILD_PROFILE=daemon
DAEMON_BIN="$REPO_ROOT/target/$BUILD_PROFILE/everlasting-daemon"
use_release_profile() {
    BUILD_PROFILE=release
    DAEMON_BIN="$REPO_ROOT/target/release/everlasting-daemon"
}
PID_FILE="$REPO_ROOT/.everlasting-daemon.pid"
# 2026-08-24 RULE-DAEMON-001 收口:日志从 /tmp(重启即覆盖 + 无限增长)
# 迁到 XDG state 目录。2026-09-03 F3 PR2 起追加写 + 大小轮转(>10MiB 滚
# 3 代)由 daemon 进程内 appender 负责(disk/log_rotation.rs),脚本不再
# 重定向/轮转,只保留路径常量(logs 子命令 tail 同一路径)。
# HOME 缺失(cron/systemd 裸环境)时 `${HOME:-…}` 退回 /tmp:`set -u` 下裸
# `$HOME` 会 unbound 连坐杀死整个脚本(连 help 都打不出),比退化路径更糟。
STATE_DIR="${XDG_STATE_HOME:-${HOME:-/tmp}/.local/state}/dev.everlasting.app"
LOG_FILE="$STATE_DIR/daemon.log"
DEFAULT_PORT=7456

# WSL 编译 Rust 的硬性前置(CLAUDE.md 坑 1:不加则撞 gdk-pixbuf not found)。
PKG_CONFIG_PATH="${PKG_CONFIG_PATH:-/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig}"
export PKG_CONFIG_PATH

# ── 输出辅助 ──────────────────────────────────────────────────────
log()  { printf '\033[32m▸ %s\033[0m\n' "$*"; }   # 绿色 ▸
warn() { printf '\033[33m⚠ %s\033[0m\n' "$*" >&2; } # 黄色 ⚠
err()  { printf '\033[31m✗ %s\033[0m\n' "$*" >&2; } # 红色 ✗
die()  { err "$*"; exit 1; }

# ── 日志轮转(RULE-DAEMON-001,2026-08-24;2026-09-03 F3 PR2 退役)──
# 轮转改由 daemon 进程内 appender 负责(disk/log_rotation.rs:>10MiB 滚
# 3 代,运行期实时检查而非仅启动时),原 rotate_log 函数删除;存量
# .1/.2/.3 由 Rust 滚动循环自然覆盖消化,无需迁移。

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
    # rebuild 直调时可带 --release;start/bg/restart 在各自参数解析里
    # 已切换过,再传会重复但幂等(无参调用时循环体不执行)。
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --release) use_release_profile; shift;;
            *) die "rebuild: 未知参数 '$1'(可用:--release)";;
        esac
    done
    log "编译 daemon 二进制($BUILD_PROFILE 档;该档首次全量编依赖,之后增量)"
    (
        cd "$SRC_TAURI"
        # --profile 对内置 profile 名同样合法(--profile release == --release)。
        cargo build --profile "$BUILD_PROFILE" --bin everlasting-daemon
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
            --release)  use_release_profile; shift;;
            *) die "start: 未知参数 '$1'(可用:--port N --bg --no-build --release)";;
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
        # 2026-08-24 RULE-DAEMON-001;2026-09-03 F3 PR2:日志落盘改由 daemon
        # 进程内 appender 负责(追加写 + >10MiB 滚 3 代,路径不变,见
        # disk/log_rotation.rs),脚本不再 `>> $LOG_FILE` 重定向 —— 消除
        # 双写者(fd 交错 + 与滚动打架),运行期也能轮转。目录仍兜底创建
        # (appender 自建,双保险)。
        mkdir -p "$STATE_DIR"
        nohup "$DAEMON_BIN" --port "$port" >/dev/null 2>&1 &
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
        rebuild)  do_build "$@";;
        status)   do_status "$@";;
        logs)     do_logs "$@";;
        help|-h|--help) show_help;;
        *) die "未知命令 '$cmd'。用 ./scripts/daemon.sh help 查看用法。";;
    esac
}

main "$@"
