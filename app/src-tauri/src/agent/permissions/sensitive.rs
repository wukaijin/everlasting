//! Sensitive-path deny-list + trusted-external allow-list (read-side
//! boundary decouple, 2026-07-01).
//!
//! 对标 `dangerous.rs`(Tier 2 shell kill-list)的**路径维度**版本。read
//! 族(read_file / grep / glob / list_dir)去掉 tool 层
//! `assert_within_root` 硬卡后,这两份 static list 在权限层 `check.rs`
//! 补位:
//!
//! - **deny-list**(Tier 2.5,早于 yolo bypass):项目外路径命中即硬
//!   `Deny`、含 yolo、不可绕过(无"高危确认"解锁窗)。堵"私钥/凭证进
//!   LLM context"的不可逆泄露面。仅对**项目外**路径生效(Q1.2:项目内
//!   `.env` / `*.pem` 信任不挡)。
//! - **allow-list**(Tier 4 Path 分支,项目外、ask 前):
//!   `~/.config/everlasting/**` 免 ask 直接放行 —— app 自己的运行时数据
//!   (commands / memory / config),agent 读它本就不该每次弹窗。
//!
//! **优先级**:deny-list > allow-list > ask(由 `check.rs` 的调用顺序
//! 保证:deny 在 Tier 2.5,allow 在 Tier 4)。两者 pattern 实际不重叠。
//!
//! **匹配**:`globset`(`Cargo.toml:44` 已在依赖),`literal_separator(true)`
//! 使 `*` 不跨 `/`、`**` 跨(与 sqlite GLOB / shell glob 语义一致)。lexical
//! 匹配(不 `canonicalize` —— read 不存在路径要走 IO error 不该走 deny)。
//! `~` 在编译 pattern 前用 `dirs::home_dir()`(`Cargo.toml:61`)展开。GlobSet
//! 编译结果缓存在 `OnceLock`(对标 `memory/tokens.rs:35` ENCODOR /
//! `subagent/mod.rs:378` REGISTRY 的项目惯例)。
//!
//! **作用域**:仅 read 族。write/edit 族的 tool 层 `assert_within_root`
//! 硬边界保留(本 task 不动)。

use std::path::Path;
use std::sync::OnceLock;

use globset::{GlobBuilder, GlobSet};

/// 敏感路径 pattern(中等档)。命中即硬 deny。`~` 占位(编译时展开)。
///
/// `.env` 系列用**枚举**而非 `**/.env.*` 通配,使 `.env.example` /
/// `.env.sample` / `.env.template` 天然不命中(它们不是真凭证)。
pub(crate) const SENSITIVE_PATH_PATTERNS: &[&str] = &[
    // === 私钥 / 证书 ===
    "~/.ssh/**",
    "**/*.pem",
    "**/*.key",
    "**/*.p12",
    "**/*.pfx",
    "**/*.keystore",
    // === 系统密钥(明文哈希) ===
    "/etc/shadow",
    "/etc/gshadow",
    // === 凭证文件(明文 token) ===
    "**/.env",
    "**/.env.local",
    "**/.env.production",
    "**/.env.staging",
    "**/*credentials*",
    "**/*secret*",
    "~/.aws/credentials",
    "~/.netrc",
    "~/.npmrc",
    "~/.docker/config.json",
];

/// 受信项目外 allow-list 的 static 段。基于 `~` 展开(`build_set` 内
/// `dirs::home_dir()`),与平台无关。
///
/// 动态段(worktree root `<app_data_dir>/worktrees/**`)由
/// [`build_trusted_external_patterns`] 在启动期拼接,见下面 doc。
pub(crate) const STATIC_TRUSTED_EXTERNAL_PATTERNS: &[&str] = &["~/.config/everlasting/**"];

/// 拼接完整受信 allow-list 列表(static 段 + Tauri 解析的
/// `<app_data_dir>/worktrees/**`)。
///
/// worktree 路径格式(`git::worktree`):
/// - session worktree:`<app_data_dir>/worktrees/<project_uuid>/<session_uuid>`
/// - worker worktree: `<app_data_dir>/worktrees/<project_uuid>/worker/<run_id>`
///
/// 二者都被 `<dir>/worktrees/**` 覆盖。`agent` 读自己创建的 worktree
/// 是合理且常见操作(任务上下文往往需要 cross-file 看),不该每次弹窗
/// (本 task 的原始动机:`read_file ~/.config/...` 不该卡 worktree 同款
/// friction)。
///
/// 为什么需要动态 `<app_data_dir>` 而非硬编码 `/root/.local/share/...`:
/// 1. 平台差异: Linux `/root/.local/share/<bundle-id>/` / macOS
///    `~/Library/Application Support/<bundle-id>/` / Windows
///    `%APPDATA%\<bundle-id>\`,硬编码任一种在另两个 OS 都会误判
///    (allow 漏命中 / 误命中别处)。
/// 2. bundle id 从 `tauri.conf.json::identifier` 读(staging / dev /
///    prod build 可能不同),Rust 端不应写死,只读 Tauri 解析结果。
pub(crate) fn build_trusted_external_patterns(app_data_dir: &Path) -> Vec<String> {
    let mut patterns: Vec<String> = STATIC_TRUSTED_EXTERNAL_PATTERNS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    if !app_data_dir.as_os_str().is_empty() {
        patterns.push(format!("{}/worktrees/**", app_data_dir.display()));
    }
    patterns
}

/// 把 pattern 列表编译成 GlobSet。`~` 展开为 home dir;
/// `literal_separator(true)` 让 `*` 不跨 `/`、`**` 跨。
///
/// panic 只在 pattern 语法非法时(开发期错误,非运行期输入)。
fn build_set<S: AsRef<str>>(patterns: &[S]) -> GlobSet {
    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut b = globset::GlobSetBuilder::new();
    for p in patterns {
        let pat = p.as_ref();
        let expanded = if let Some(rest) = pat.strip_prefix("~/") {
            if home.is_empty() {
                // home 解析失败(Linux 之外的极端环境)—— 退化为 strip 后的
                // 相对 pattern,匹配大概率失败但不 panic。
                rest.to_string()
            } else {
                format!("{}/{}", home, rest)
            }
        } else {
            pat.to_string()
        };
        let glob = GlobBuilder::new(&expanded)
            .literal_separator(true)
            .build()
            .unwrap_or_else(|e| panic!("invalid sensitive-path pattern {pat:?}: {e}"));
        b.add(glob);
    }
    b.build()
        .unwrap_or_else(|e| panic!("invalid sensitive-path pattern set: {e}"))
}

static SENSITIVE_SET: OnceLock<GlobSet> = OnceLock::new();
static TRUSTED_SET: OnceLock<GlobSet> = OnceLock::new();

/// One-time 初始化受信 allow-list 集合。**必须**在 `AppState::load`
/// 解析完 `app_data_dir` 之后调用,否则动态 worktree pattern 缺失。
///
/// Idempotent:重复调用是 no-op(first writer wins;后续调用 `warn!`
/// 一行)。用不同 dir 重复调用**忽略后者** —— `app_data_dir` 在进程
/// 生命周期内固定,后续重 init 用不同值是 caller bug。
pub fn init_trusted_external(app_data_dir: &Path) {
    let patterns = build_trusted_external_patterns(app_data_dir);
    let set = build_set(&patterns);
    if TRUSTED_SET.set(set).is_err() {
        tracing::warn!(
            app_data_dir = %app_data_dir.display(),
            "init_trusted_external called more than once; first call wins (subsequent dir ignored)"
        );
    }
}

fn sensitive_set() -> &'static GlobSet {
    SENSITIVE_SET.get_or_init(|| build_set(SENSITIVE_PATH_PATTERNS))
}

fn trusted_set() -> &'static GlobSet {
    // 如果 `init_trusted_external` 从未调用(tests / CLI 工具 / 早期
    // boot 路径),回退到 static-only set —— 与 pre-init 行为一致
    // (仅 `~/.config/everlasting/**`)。生产路径走 `AppState::load`,
    // 任意 read 触发前 init 已完成。
    TRUSTED_SET.get_or_init(|| build_set(STATIC_TRUSTED_EXTERNAL_PATTERNS))
}

/// `abs_path` 是否命中敏感路径 deny-list。lexical 匹配(不 canonicalize)。
///
/// caller(`check.rs` Tier 2.5)负责先用 `is_within_root(ctx.worktree_path)`
/// 判定项目外,仅对项目外路径调用本函数(Q1.2:项目内不挡)。
pub fn is_sensitive_path(abs_path: &Path) -> bool {
    sensitive_set().is_match(abs_path)
}

/// `abs_path` 是否命中受信项目外 allow-list。lexical 匹配。
///
/// caller(`check.rs` Tier 4 Path 分支)在项目外、deny 未命中、`ask_path`
/// 之前调用。完整 set 包含 `~/.config/everlasting/**`(static) +
/// `<app_data_dir>/worktrees/**`(动态,见 `init_trusted_external`)。
pub fn is_trusted_external(abs_path: &Path) -> bool {
    trusted_set().is_match(abs_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> std::path::PathBuf {
        dirs::home_dir().expect("HOME resolved in test env")
    }

    // === deny-list:私钥命中 ===
    #[test]
    fn ssh_dir_is_sensitive() {
        assert!(is_sensitive_path(&home().join(".ssh/id_rsa")));
        assert!(is_sensitive_path(&home().join(".ssh/config")));
        assert!(is_sensitive_path(&home().join(".ssh/deep/nested/key")));
    }

    #[test]
    fn private_key_extensions_are_sensitive() {
        assert!(is_sensitive_path(std::path::Path::new("/home/x/secret/server.pem")));
        assert!(is_sensitive_path(std::path::Path::new("/home/x/k.pfx")));
        assert!(is_sensitive_path(std::path::Path::new("/home/x/k.key")));
        assert!(is_sensitive_path(std::path::Path::new("/home/x/k.p12")));
        assert!(is_sensitive_path(std::path::Path::new("/home/x/.keystore")));
    }

    // === deny-list:.env 命中,但 .env.example 不命中(枚举式 pattern) ===
    #[test]
    fn dotenv_is_sensitive_but_example_is_not() {
        assert!(is_sensitive_path(std::path::Path::new("/repo/.env")));
        assert!(is_sensitive_path(std::path::Path::new("/repo/.env.local")));
        assert!(is_sensitive_path(std::path::Path::new("/repo/.env.production")));
        assert!(is_sensitive_path(std::path::Path::new("/repo/sub/.env")));
        // example / sample / template 不是真凭证,不命中。
        assert!(!is_sensitive_path(std::path::Path::new("/repo/.env.example")));
        assert!(!is_sensitive_path(std::path::Path::new("/repo/.env.sample")));
        assert!(!is_sensitive_path(std::path::Path::new("/repo/.env.template")));
    }

    #[test]
    fn credentials_filenames_are_sensitive() {
        assert!(is_sensitive_path(std::path::Path::new("/repo/aws-credentials.yaml")));
        assert!(is_sensitive_path(std::path::Path::new("/repo/secrets.txt")));
        assert!(is_sensitive_path(&home().join(".aws/credentials")));
        assert!(is_sensitive_path(&home().join(".netrc")));
        assert!(is_sensitive_path(&home().join(".npmrc")));
        assert!(is_sensitive_path(&home().join(".docker/config.json")));
    }

    #[test]
    fn etc_shadow_is_sensitive() {
        assert!(is_sensitive_path(std::path::Path::new("/etc/shadow")));
        assert!(is_sensitive_path(std::path::Path::new("/etc/gshadow")));
    }

    // === 非敏感路径不命中 ===
    #[test]
    fn normal_source_files_not_sensitive() {
        assert!(!is_sensitive_path(std::path::Path::new(
            "/usr/local/code/repo/src/main.rs"
        )));
        assert!(!is_sensitive_path(std::path::Path::new("/repo/README.md")));
        assert!(!is_sensitive_path(std::path::Path::new("/repo/package.json")));
        assert!(!is_sensitive_path(std::path::Path::new("/repo/.gitignore")));
    }

    // === allow-list ===
    #[test]
    fn everlasting_app_data_is_trusted_external() {
        assert!(is_trusted_external(
            &home().join(".config/everlasting/commands/test-b3.md")
        ));
        assert!(is_trusted_external(&home().join(".config/everlasting/memory/x.md")));
        // 子目录也命中(**)
        assert!(is_trusted_external(
            &home().join(".config/everlasting/deep/nested/file")
        ));
    }

    #[test]
    fn other_paths_not_trusted_external() {
        assert!(!is_trusted_external(&home().join(".config/other-app/config")));
        assert!(!is_trusted_external(&home().join(".ssh/id_rsa")));
        assert!(!is_trusted_external(std::path::Path::new("/usr/local/code/repo/x")));
    }

    // === 优先级文档化:allow 与 deny pattern 集合无交集 ===
    // check.rs 用调用顺序(deny Tier 2.5 早于 allow Tier 4)保证 deny 优先;
    // 这里锁定"everlasting 目录下的常规文件不命中 deny"这一前提。
    #[test]
    fn everlasting_dir_does_not_overlap_deny() {
        let candidate = home().join(".config/everlasting/commands/test-b3.md");
        assert!(is_trusted_external(&candidate));
        assert!(!is_sensitive_path(&candidate));
    }

    // === 动态段:`<app_data_dir>/worktrees/**` ===
    //
    // 测 `build_trusted_external_patterns` + `build_set` 的纯函数行为,
    // 不污染全局 `TRUSTED_SET`(那是 `init_trusted_external` 的职责,
    // 生产代码 `AppState::load` 调一次,测试不能动)。校验:
    //   1. pattern 列表里包含 `<dir>/worktrees/**`
    //   2. 编译出来的 set 命中 session worktree + worker worktree + 嵌套
    //   3. static 段(`~/.config/everlasting/**`)仍然命中 —— init 不能
    //      丢掉原 allow-list
    //   4. 不相关子目录(如 `<dir>/not-worktrees/...`)不命中
    #[test]
    fn dynamic_worktree_pattern_builds_and_matches() {
        let test_dir = std::path::PathBuf::from("/tmp/everlasting-test-app-data");
        let patterns = build_trusted_external_patterns(&test_dir);
        // 1. pattern 列表包含 worktree 段
        let wt_pattern = format!("{}/worktrees/**", test_dir.display());
        assert!(
            patterns.iter().any(|p| p == &wt_pattern),
            "patterns 缺 worktree 段: {patterns:?}"
        );
        // 1b. static 段保留
        assert!(
            patterns.iter().any(|p| p == "~/.config/everlasting/**"),
            "patterns 缺 static 段: {patterns:?}"
        );

        // 用该 pattern 列表单独编译 set(纯函数,不动全局)
        let pattern_refs: Vec<&str> = patterns.iter().map(String::as_str).collect();
        let set = build_set(&pattern_refs);

        // 2. session worktree 命中
        let session_wt = test_dir.join("worktrees/proj-uuid/sess-uuid/file.rs");
        assert!(set.is_match(&session_wt));
        // 2b. 嵌套子目录也命中(`**` 跨 `/`)
        let nested = test_dir.join("worktrees/proj-uuid/sess-uuid/deep/nested/f");
        assert!(set.is_match(&nested));
        // 2c. worker worktree 命中
        let worker_wt = test_dir.join("worktrees/proj-uuid/worker/run-123/x");
        assert!(set.is_match(&worker_wt));

        // 3. static 段仍然命中
        assert!(set.is_match(&home().join(".config/everlasting/commands/x.md")));

        // 4. 不相关子目录不命中
        assert!(!set.is_match(&test_dir.join("not-worktrees/foo")));
        // 4b. worktrees 兄弟目录不命中
        assert!(!set.is_match(&test_dir.join("other/foo")));
    }

    // 空 app_data_dir 不抛、只生成 static 段(防御性,见
    // `build_trusted_external_patterns` 的 `is_empty` 分支)。
    #[test]
    fn empty_app_data_dir_yields_static_only() {
        let patterns = build_trusted_external_patterns(std::path::Path::new(""));
        assert_eq!(patterns, vec!["~/.config/everlasting/**".to_string()]);
    }
}
