//! Build script.
//!
//! de-Tauri（2026-09-30, task `09-30-de-tauri`）：Tauri codegen
//! （`tauri_build::build()`）与 GUI sidecar staging（拷贝 daemon 到
//! `binaries/everlasting-daemon-<triple>`）已随 GUI bin 一起移除。
//! 本脚本只剩一项职责：`EVERLASTING_APP_IDENTIFIER` 编译期注入。
//!
//! 历史上该值读自 `tauri.conf.json` 的 `identifier`；tauri.conf.json
//! 删除后此常量成为唯一事实源。
//!
//! **PATH-COMPAT ANCHOR（不可漂移）**：值必须保持
//! `dev.everlasting.app`——它决定 `resolve_data_dir()` =
//! `dirs::data_dir().join(identifier)`，即 SQLite DB / 附件 / 群聊转录
//! 的落盘根。漂移 = 静默搬走全体用户的数据目录。守护：lib target 的
//! 字面量断言测试（见 `state.rs` tests + CI `--bins` 门）。

fn main() {
    // 值冻结于 de-Tauri 迁移时点，等于历史 tauri.conf.json 的
    // `identifier`。改动前必须先有数据迁移方案。
    println!("cargo:rustc-env=EVERLASTING_APP_IDENTIFIER=dev.everlasting.app");
}
