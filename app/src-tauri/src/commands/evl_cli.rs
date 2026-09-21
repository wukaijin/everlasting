//! evl CLI 宿主机检测 / 安装 IPC(Settings「CLI (evl)」分类)。
//!
//! 照 `commands::disk` 先例:`_inner` 业务函数 + `#[tauri::command]`
//! 薄包装,daemon 侧镜像路由(routes/evl_cli.rs)。**检测与安装动作都
//! 在 daemon 进程执行** —— 「给 daemon 的宿主机安装」的语义由执行位置
//! 保证(GUI Thin 模式下 GUI 与 sidecar 同机;remote 场景装到远端宿主机)。
//!
//! 文件来源:编译期内嵌仓库 `cli/` 运行时三件套(`bin.mjs` + `lib/**` +
//! `package.json`,同 builtin-workflow 的 `include_str!` 先例)—— 打包
//! 应用无仓库 / 无 pnpm / 无 npm registry 也能装,CLI 版本随 daemon
//! 二进制走。安装布局:
//!
//! - 写出 `{app_data_dir}/cli/`(11 个文件,`bin.mjs` 0755);
//! - symlink `~/.local/bin/evl` → `{app_data_dir}/cli/bin.mjs`(Node
//!   ESM 默认 realpath 解析,bin.mjs 的相对 import 指向写出目录,
//!   与 `pnpm link` 同机制)。
//!
//! 安全边界:`~/.local/bin/evl` 已存在且**不是**本功能创建的 symlink
//! 时拒绝覆盖(InvalidRequest,消息带现有路径);「更新」= 重复安装
//! (重写托管目录 + 重建 symlink,幂等)。不做卸载命令(UI 给移除指引)。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::State;

use crate::error::{AppCommandError, ErrorCategory};
use crate::state::AppState;

/// 内嵌的 cli/ 运行时文件清单(相对路径, 内容)。**cli/ 新增运行时文件
/// 时必须同步本表**(防漂移测试 `embedded_files_cover_runtime_sources`
/// 会拦截;`*.test.mjs` / `README.md` 不入表)。路径前缀从
/// `src/commands/` 到仓库根 `cli/`。
pub const EVL_CLI_FILES: &[(&str, &str)] = &[
    ("bin.mjs", include_str!("../../../../cli/bin.mjs")),
    ("package.json", include_str!("../../../../cli/package.json")),
    ("lib/api.mjs", include_str!("../../../../cli/lib/api.mjs")),
    ("lib/args.mjs", include_str!("../../../../cli/lib/args.mjs")),
    ("lib/chat.mjs", include_str!("../../../../cli/lib/chat.mjs")),
    (
        "lib/commands/list.mjs",
        include_str!("../../../../cli/lib/commands/list.mjs"),
    ),
    (
        "lib/commands/status.mjs",
        include_str!("../../../../cli/lib/commands/status.mjs"),
    ),
    (
        "lib/discuss.mjs",
        include_str!("../../../../cli/lib/discuss.mjs"),
    ),
    (
        "lib/format.mjs",
        include_str!("../../../../cli/lib/format.mjs"),
    ),
    ("lib/mcp.mjs", include_str!("../../../../cli/lib/mcp.mjs")),
    ("lib/sse.mjs", include_str!("../../../../cli/lib/sse.mjs")),
];

/// daemon 内嵌的 CLI 版本(编译期快照 package.json 的 `version`)。
pub fn bundled_cli_version() -> &'static str {
    static LOCK: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| {
        EVL_CLI_FILES
            .iter()
            .find(|(p, _)| *p == "package.json")
            .and_then(|(_, c)| version_from_pkg_json(c))
            .unwrap_or_else(|| "unknown".to_string())
    })
}

// ---------------------------------------------------------------------------
// wire payload(camelCase)
// ---------------------------------------------------------------------------

/// 宿主机 Node 探测结果(`node --version`)。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvlNodeStatus {
    /// `node` 二进制是否可达。
    pub found: bool,
    /// 原始版本串(如 `v20.11.1`)。
    pub version: Option<String>,
    /// 是否满足 CLI 的 engines 要求(Node ≥ 20)。
    pub ok: bool,
    /// found=false 或 ok=false 时的一句话原因(直接展示)。
    pub reason: Option<String>,
}

/// evl 在宿主机上的安装形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EvlInstallState {
    /// PATH 与 `~/.local/bin/evl` 均未发现。
    NotInstalled,
    /// `~/.local/bin/evl` 是本功能创建的 symlink(指向 `{data_dir}/cli/bin.mjs`)。
    Managed,
    /// 存在 evl 但非本功能安装(如 `pnpm link` / 手动放置)—— 不覆盖。
    External,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvlStatus {
    pub state: EvlInstallState,
    /// 已发现的 evl 路径(Managed = symlink 路径;External = PATH 解析结果)。
    pub path: Option<String>,
    /// 已装版本(Managed 从写出目录的 package.json 读,不依赖 PATH)。
    pub version: Option<String>,
    /// daemon 进程 PATH 能否直接解析到 `evl`(执行 `evl --version` 成功)。
    pub on_path: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvlCliStatusPayload {
    /// daemon 内嵌的 CLI 版本(「可更新」判定基准)。
    pub bundled_version: String,
    pub node: EvlNodeStatus,
    pub evl: EvlStatus,
    /// 安装目标目录(`~/.local/bin`)。
    pub local_bin_dir: String,
    /// daemon 进程 PATH 是否包含 `local_bin_dir`(提示性;与用户
    /// 交互 shell 的 PATH 可能不同)。
    pub local_bin_on_path: bool,
}

// ---------------------------------------------------------------------------
// 纯函数(解析 / 判定)
// ---------------------------------------------------------------------------

/// `v20.11.1` / `20.11.1` → 20;解析不出返回 None。
fn parse_node_major(s: &str) -> Option<u32> {
    s.trim()
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// `everlasting-cli 0.1.0` → `0.1.0`(bin.mjs `--version` 的输出格式)。
fn parse_evl_version(s: &str) -> Option<String> {
    s.split_whitespace().nth(1).map(str::to_string)
}

/// package.json 文本 → `version` 字段。
fn version_from_pkg_json(content: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(content).ok()?;
    v.get("version")?.as_str().map(str::to_string)
}

/// `~/.local/bin/evl` 的存在形态判定。symlink 且目标恰为托管 bin →
/// Managed;存在但指向别处 / 普通文件 → External;不存在 → NotInstalled。
fn classify_link(link: &Path, managed_target: &Path) -> EvlInstallState {
    match std::fs::symlink_metadata(link) {
        Err(_) => EvlInstallState::NotInstalled,
        Ok(meta) if meta.file_type().is_symlink() => match std::fs::read_link(link) {
            Ok(target) if target == managed_target => EvlInstallState::Managed,
            _ => EvlInstallState::External,
        },
        Ok(_) => EvlInstallState::External,
    }
}

/// PATH 环境变量字符串是否含指定目录(尾斜杠归一;仅字符串比较,
/// 不做 canonicalize —— daemon env 与 shell env 写法通常一致)。
fn path_env_contains_dir(path_env: &str, dir: &Path) -> bool {
    let dir_str = dir.to_string_lossy();
    let want = dir_str.trim_end_matches('/');
    path_env.split(':').any(|p| p.trim_end_matches('/') == want)
}

/// 在 PATH 字符串里找第一个可执行文件命中(不依赖 `which` 二进制)。
fn find_in_path(bin: &str, path_env: &str) -> Option<PathBuf> {
    for dir in path_env.split(':') {
        if dir.is_empty() {
            continue;
        }
        let cand = Path::new(dir).join(bin);
        if is_executable_file(&cand) {
            return Some(cand);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match p.metadata() {
        Ok(m) => m.is_file() && m.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable_file(_p: &Path) -> bool {
    false
}

/// 已写出托管目录的 CLI 版本(文件缺损坏 → None)。
fn installed_pkg_version(cli_dir: &Path) -> Option<String> {
    std::fs::read_to_string(cli_dir.join("package.json"))
        .ok()
        .and_then(|c| version_from_pkg_json(&c))
}

/// `~/.local/bin`(安装目标目录)。
fn default_local_bin_dir() -> Result<PathBuf, AppCommandError> {
    let home = dirs::home_dir()
        .ok_or_else(|| AppCommandError::new(ErrorCategory::Server, "无法解析用户 home 目录"))?;
    Ok(home.join(".local").join("bin"))
}

// ---------------------------------------------------------------------------
// 进程探测
// ---------------------------------------------------------------------------

/// 一次 `bin arg` 探测的三态。
enum ProbeOutcome {
    Success(String),
    NotFound,
    Failed(String),
}

/// spawn + 3s 超时。NotFound 单列(「未安装」是正常态而非异常)。
async fn run_probe(bin: &str, arg: &str) -> ProbeOutcome {
    let fut = tokio::process::Command::new(bin)
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    match tokio::time::timeout(Duration::from_secs(3), fut).await {
        Err(_) => ProbeOutcome::Failed(format!("`{bin} {arg}` 3 秒内未返回")),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => ProbeOutcome::NotFound,
        Ok(Err(e)) => ProbeOutcome::Failed(format!("启动 `{bin}` 失败: {e}")),
        Ok(Ok(out)) if out.status.success() => {
            ProbeOutcome::Success(String::from_utf8_lossy(&out.stdout).trim().to_string())
        }
        Ok(Ok(out)) => ProbeOutcome::Failed(format!("`{bin} {arg}` 退出码非零: {}", out.status)),
    }
}

async fn probe_node_status(node_bin: &str) -> EvlNodeStatus {
    match run_probe(node_bin, "--version").await {
        ProbeOutcome::Success(out) => {
            let major = parse_node_major(&out);
            let version = if out.is_empty() {
                None
            } else {
                Some(out.clone())
            };
            match major {
                Some(m) if m >= 20 => EvlNodeStatus {
                    found: true,
                    version,
                    ok: true,
                    reason: None,
                },
                Some(m) => EvlNodeStatus {
                    found: true,
                    version,
                    ok: false,
                    reason: Some(format!("Node 版本过低(evl 需要 ≥ 20,当前 v{m})")),
                },
                None => EvlNodeStatus {
                    found: true,
                    version,
                    ok: false,
                    reason: Some("无法解析 `node --version` 输出".to_string()),
                },
            }
        }
        ProbeOutcome::NotFound => EvlNodeStatus {
            found: false,
            version: None,
            ok: false,
            reason: Some("宿主机未安装 Node(evl 需要 Node ≥ 20)".to_string()),
        },
        ProbeOutcome::Failed(msg) => EvlNodeStatus {
            found: false,
            version: None,
            ok: false,
            reason: Some(msg),
        },
    }
}

// ---------------------------------------------------------------------------
// detect / install
// ---------------------------------------------------------------------------

/// 组装检测 payload。`node_bin` / `evl_bin` 参数化(单测注入假命令),
/// `data_dir` / `local_bin_dir` 参数化(单测传 tempdir,不碰真实 home)。
async fn detect_with(
    data_dir: &Path,
    local_bin_dir: &Path,
    node_bin: &str,
    evl_bin: &str,
) -> EvlCliStatusPayload {
    let node = probe_node_status(node_bin).await;
    let evl_out = run_probe(evl_bin, "--version").await;
    let on_path = matches!(evl_out, ProbeOutcome::Success(_));

    let cli_dir = data_dir.join("cli");
    let managed_target = cli_dir.join("bin.mjs");
    let link = local_bin_dir.join("evl");
    let state = classify_link(&link, &managed_target);

    let (path, version) = match state {
        EvlInstallState::NotInstalled => (None, None),
        EvlInstallState::Managed => {
            // 版本从托管目录的 package.json 读 —— PATH 不通时「可更新」
            // 判定仍可用。
            (Some(link), installed_pkg_version(&cli_dir))
        }
        EvlInstallState::External => {
            let path = std::env::var("PATH")
                .ok()
                .and_then(|p| find_in_path(evl_bin, &p))
                .or_else(|| {
                    // 不在 PATH(如 ~/.local/bin 未导出)但文件在场。
                    link.symlink_metadata().is_ok().then_some(link)
                });
            let version = match &evl_out {
                ProbeOutcome::Success(out) => parse_evl_version(out),
                _ => None,
            };
            (path, version)
        }
    };

    let local_bin_on_path = std::env::var("PATH")
        .map(|p| path_env_contains_dir(&p, local_bin_dir))
        .unwrap_or(false);

    EvlCliStatusPayload {
        bundled_version: bundled_cli_version().to_string(),
        node,
        evl: EvlStatus {
            state,
            path: path.map(|p| p.display().to_string()),
            version,
            on_path,
        },
        local_bin_dir: local_bin_dir.display().to_string(),
        local_bin_on_path,
    }
}

/// `detect_evl` 业务本体(Tauri / daemon 双入口,无请求体)。
pub async fn detect_evl_inner(
    state: &Arc<AppState>,
) -> Result<EvlCliStatusPayload, AppCommandError> {
    let local_bin = default_local_bin_dir()?;
    Ok(detect_with(&state.app_data_dir, &local_bin, "node", "evl").await)
}

/// `install_evl` 业务本体:写出内嵌文件 + 建链,返回安装后的检测
/// payload(前端一次拿全状态)。
pub async fn install_evl_inner(
    state: &Arc<AppState>,
) -> Result<EvlCliStatusPayload, AppCommandError> {
    if !cfg!(unix) {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            "当前平台不支持 evl CLI 安装(仅 Linux/macOS)",
        ));
    }
    let local_bin = default_local_bin_dir()?;
    install_to(&state.app_data_dir, &local_bin, "node").await
}

/// 安装落盘本体。步骤:node 前置 → 冲突检查 → 写出文件 → 重建
/// symlink → 复跑 detect。
async fn install_to(
    data_dir: &Path,
    local_bin_dir: &Path,
    node_bin: &str,
) -> Result<EvlCliStatusPayload, AppCommandError> {
    // 1. Node 前置:装出来跑不起来就没有意义,fail loud。
    let node = probe_node_status(node_bin).await;
    if !node.ok {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!(
                "宿主机 Node 不满足要求:{}",
                node.reason.unwrap_or_else(|| "未知原因".to_string())
            ),
        ));
    }

    let cli_dir = data_dir.join("cli");
    let managed_target = cli_dir.join("bin.mjs");
    let link = local_bin_dir.join("evl");

    // 2. 冲突检查:已存在的非托管 evl 一律拒绝覆盖。
    if classify_link(&link, &managed_target) == EvlInstallState::External {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!(
                "{} 已被非本应用安装的 evl 占用,拒绝覆盖;如需改用应用内置版请先移除该文件",
                link.display()
            ),
        ));
    }

    // 3. 写出内嵌文件(bin.mjs 0755,其余默认)。
    for (rel, content) in EVL_CLI_FILES {
        let dest = cli_dir.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AppCommandError::new(
                    ErrorCategory::Server,
                    format!("创建目录 {} 失败: {e}", parent.display()),
                )
            })?;
        }
        std::fs::write(&dest, content).map_err(|e| {
            AppCommandError::new(
                ErrorCategory::Server,
                format!("写出 {} 失败: {e}", dest.display()),
            )
        })?;
        if *rel == "bin.mjs" {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).map_err(
                    |e| {
                        AppCommandError::new(
                            ErrorCategory::Server,
                            format!("设置 {} 执行权限失败: {e}", dest.display()),
                        )
                    },
                )?;
            }
        }
    }

    // 4. symlink:目标不存在 / 旧托管链 / 断链都走「先删后建」;
    //    External 已在步骤 2 拦截,这里不会碰用户文件。
    std::fs::create_dir_all(local_bin_dir).map_err(|e| {
        AppCommandError::new(
            ErrorCategory::Server,
            format!("创建目录 {} 失败: {e}", local_bin_dir.display()),
        )
    })?;
    match std::fs::remove_file(&link) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(AppCommandError::new(
                ErrorCategory::Server,
                format!("移除旧链接 {} 失败: {e}", link.display()),
            ))
        }
    }
    create_symlink(&managed_target, &link).map_err(|e| {
        AppCommandError::new(
            ErrorCategory::Server,
            format!("创建 symlink {} 失败: {e}", link.display()),
        )
    })?;

    Ok(detect_with(data_dir, local_bin_dir, node_bin, "evl").await)
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn create_symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("symlink 仅支持 unix 平台"))
}

// ---------------------------------------------------------------------------
// Tauri command 薄包装
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn detect_evl(
    state: State<'_, Arc<AppState>>,
) -> Result<EvlCliStatusPayload, AppCommandError> {
    detect_evl_inner(&state).await
}

#[tauri::command]
pub async fn install_evl(
    state: State<'_, Arc<AppState>>,
) -> Result<EvlCliStatusPayload, AppCommandError> {
    install_evl_inner(&state).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- 解析纯函数 -----

    #[test]
    fn node_major_parses_v_prefixed_and_bare() {
        assert_eq!(parse_node_major("v20.11.1"), Some(20));
        assert_eq!(parse_node_major("20.9"), Some(20));
        assert_eq!(parse_node_major("v19.0.0"), Some(19));
        assert_eq!(parse_node_major(""), None);
        assert_eq!(parse_node_major("abc"), None);
    }

    #[test]
    fn evl_version_takes_second_token() {
        assert_eq!(
            parse_evl_version("everlasting-cli 0.1.0"),
            Some("0.1.0".to_string())
        );
        assert_eq!(parse_evl_version("garbage"), None);
    }

    #[test]
    fn bundled_version_is_parsed_from_embedded_package_json() {
        assert_eq!(bundled_cli_version(), "0.1.0");
    }

    #[test]
    fn path_env_membership_ignores_trailing_slash() {
        let dir = Path::new("/home/u/.local/bin");
        assert!(path_env_contains_dir("/usr/bin:/home/u/.local/bin", dir));
        assert!(path_env_contains_dir(
            "/usr/bin:/home/u/.local/bin/:/bin",
            dir
        ));
        assert!(!path_env_contains_dir("/usr/bin:/bin", dir));
        assert!(!path_env_contains_dir("/home/u/.local/bin-else", dir));
    }

    // ----- 防漂移:内嵌表覆盖 cli/ 全部运行时源文件 -----

    /// 递归收集 `root/sub` 下全部普通文件相对 `root` 的路径(posix
    /// 风格 `/` 分隔)。递归保持 root 不变 —— 相对前缀(lib/、
    /// lib/commands/)不被剥掉。
    fn walk_files(root: &Path, sub: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(root.join(sub)).unwrap() {
            let entry = entry.unwrap();
            let rel = sub.join(entry.file_name());
            if root.join(&rel).is_dir() {
                walk_files(root, &rel, out);
            } else {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }

    /// cli/ 新增运行时文件(如 lib/ 新模块)而 EVL_CLI_FILES 忘记内嵌时,
    /// 打包出的 daemon 会装出残缺 CLI —— 此测试在仓库内跑 cargo test
    /// 时拦截。测试文件与 README 不属于运行时,排除。
    #[test]
    fn embedded_files_cover_runtime_sources() {
        let cli_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli");
        let mut disk_files = Vec::new();
        walk_files(&cli_root, Path::new(""), &mut disk_files);
        let runtime: std::collections::BTreeSet<String> = disk_files
            .into_iter()
            .filter(|f| !f.ends_with(".test.mjs") && f != "README.md")
            .collect();
        let embedded: std::collections::BTreeSet<String> =
            EVL_CLI_FILES.iter().map(|(p, _)| p.to_string()).collect();
        assert_eq!(
            runtime, embedded,
            "cli/ 运行时文件与 EVL_CLI_FILES 内嵌表不一致 —— 同步 evl_cli.rs 的内嵌清单"
        );

        // 内容也必须一致(include_str! 编译期快照 vs 当前磁盘 —— 改了
        // cli/ 必须重编 daemon,本断言在「改完源码直接跑测试」时提醒)。
        for (rel, content) in EVL_CLI_FILES {
            let disk = std::fs::read_to_string(cli_root.join(rel)).unwrap();
            assert_eq!(&disk, content, "内嵌内容与磁盘不一致: {rel}");
        }
    }

    // ----- classify_link 三态 -----

    #[cfg(unix)]
    #[test]
    fn classify_link_covers_three_states() {
        let tmp = tempfile::tempdir().unwrap();
        let managed_target = tmp.path().join("cli/bin.mjs");
        let link = tmp.path().join("evl");

        assert_eq!(
            classify_link(&link, &managed_target),
            EvlInstallState::NotInstalled
        );

        std::fs::write(tmp.path().join("elsewhere"), "#!/bin/sh\n").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("elsewhere"), &link).unwrap();
        assert_eq!(
            classify_link(&link, &managed_target),
            EvlInstallState::External
        );

        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&managed_target, &link).unwrap();
        assert_eq!(
            classify_link(&link, &managed_target),
            EvlInstallState::Managed
        );

        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, "regular file").unwrap();
        assert_eq!(
            classify_link(&link, &managed_target),
            EvlInstallState::External
        );
    }

    // ----- install / detect 落盘行为 -----

    /// 造一个假 `node`(输出 v20.0.0),让 install 的 Node 前置可注入测试。
    #[cfg(unix)]
    fn write_fake_node(dir: &Path) -> String {
        let p = dir.join("fake-node");
        std::fs::write(&p, "#!/bin/sh\necho v20.0.0\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.display().to_string()
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn install_creates_managed_layout_and_is_idempotent() {
        let data_dir = tempfile::tempdir().unwrap();
        let local_bin = tempfile::tempdir().unwrap();
        let node_bin = write_fake_node(data_dir.path());

        let payload = install_to(data_dir.path(), local_bin.path(), &node_bin)
            .await
            .unwrap();
        let cli_dir = data_dir.path().join("cli");
        let link = local_bin.path().join("evl");

        // 布局:symlink 指向托管 bin.mjs,三件套在场,bin.mjs 可执行。
        assert_eq!(std::fs::read_link(&link).unwrap(), cli_dir.join("bin.mjs"));
        assert!(cli_dir.join("package.json").is_file());
        assert!(cli_dir.join("lib/args.mjs").is_file());
        assert!(cli_dir.join("lib/commands/status.mjs").is_file());
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(cli_dir.join("bin.mjs"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "bin.mjs 必须可执行");

        // payload:Managed + 版本来自写出的 package.json(测试机 PATH 无
        // evl 也应给出版本)。
        assert_eq!(payload.evl.state, EvlInstallState::Managed);
        assert_eq!(payload.evl.version.as_deref(), Some("0.1.0"));
        assert_eq!(payload.bundled_version, "0.1.0");
        assert!(!payload.evl.on_path, "测试环境 PATH 上不应有 evl");

        // 幂等:重复安装(更新语义)仍 Ok,状态不变。
        let again = install_to(data_dir.path(), local_bin.path(), &node_bin)
            .await
            .unwrap();
        assert_eq!(again.evl.state, EvlInstallState::Managed);
        assert_eq!(std::fs::read_link(&link).unwrap(), cli_dir.join("bin.mjs"));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn install_refuses_conflicting_link_without_touching_data_dir() {
        let data_dir = tempfile::tempdir().unwrap();
        let local_bin = tempfile::tempdir().unwrap();
        let node_bin = write_fake_node(data_dir.path());

        // 预放用户自己的 evl(普通文件)→ 拒绝,且尚未写出任何托管文件。
        let link = local_bin.path().join("evl");
        std::fs::write(&link, "#!/bin/sh\n").unwrap();
        let err = install_to(data_dir.path(), local_bin.path(), &node_bin)
            .await
            .unwrap_err();
        assert_eq!(err.category, ErrorCategory::InvalidRequest);
        assert!(
            err.message.contains(link.display().to_string().as_str()),
            "冲突错误必须带现有路径: {}",
            err.message
        );
        assert!(!data_dir.path().join("cli").exists());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn install_requires_eligible_node() {
        let data_dir = tempfile::tempdir().unwrap();
        let local_bin = tempfile::tempdir().unwrap();
        let err = install_to(data_dir.path(), local_bin.path(), "no-such-node-xyz")
            .await
            .unwrap_err();
        assert_eq!(err.category, ErrorCategory::InvalidRequest);
        assert!(err.message.contains("Node"), "{}", err.message);
    }

    /// _inner 级接线:真实 home 上的 detect 恒 Ok(不依赖宿主机安装状态),
    /// 仅断言与宿主环境无关的字段。
    #[tokio::test(flavor = "multi_thread")]
    async fn detect_inner_returns_payload_on_fresh_state() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let payload = detect_evl_inner(&state).await.unwrap();
        assert_eq!(payload.bundled_version, "0.1.0");
        assert!(
            payload.local_bin_dir.ends_with(".local/bin"),
            "{}",
            payload.local_bin_dir
        );
    }
}
