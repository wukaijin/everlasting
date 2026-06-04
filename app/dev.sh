#!/usr/bin/env bash
# Everlasting Tauri dev 启动脚本（带 IME 支持）
set -e
cd /usr/local/code/github/everlasting/app

export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig:${PKG_CONFIG_PATH}"
export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u)/bus"
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export GTK_IM_MODULE=fcitx
export QT_IM_MODULE=fcitx
export XMODIFIERS=@im=fcitx
export INPUT_METHOD=fcitx5
export SDL_IM_MODULE=fcitx

exec pnpm tauri dev
