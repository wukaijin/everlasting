//! B2 @文件补全 — Tauri command surface.
//!
//! Thin IPC over [`crate::files::walk_files`]. The frontend's
//! `<TriggerMenu>` calls `list_files` when the user types `@` to
//! populate the file-completion panel (root-relative forward-slash
//! paths). The walk is synchronous + git2-based, so it runs on
//! `spawn_blocking` to avoid blocking the async runtime on std::fs +
//! libgit2.
//!
//! `max_depth = None` returns the full bounded walk (legacy default,
//! still used for the `@/`-prefixed full-project search). `Some(n)`
//! caps depth to `n` layers under the project root — the default `@`
//! trigger sends `Some(3)` so the panel opens instantly even on huge
//! repos.
//!
//! `read_image_at_inner` / `read_raw_at_inner` / `stat_local_file_inner`
//! (下方)是本模块仅有的三块
//! 非 IPC 逻辑:daemon 的 `GET /api/v1/files/image`、
//! `GET /api/v1/files/raw` 与 `GET /api/v1/files/stat` 路由专用(前端
//! `<img>`/`fetch` 直连,不走 `transport.invoke`),故无 `#[tauri::command]`
//! 壳、不进 lib.rs / `CMD_TO_DOMAIN` 注册表(GET binary 路由与 attachments
//! 的 `get_attachment` 同一先例)。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::State;

use crate::error::{AppCommandError, ErrorCategory};
use crate::state::AppState;

/// Phase 2.2 `_inner` (Q0): shared business logic. Callable from
/// the Tauri command wrapper below + the axum route handler in
/// `daemon::routes::files`.
pub async fn list_files_inner(
    state: &Arc<AppState>,
    project_id: Option<String>,
    max_depth: Option<u32>,
) -> Result<Vec<String>, AppCommandError> {
    let project_path = match project_id {
        Some(pid) => crate::db::get_project(&state.db, &pid)
            .await
            .map_err(|e| anyhow::anyhow!("list_files: get_project failed: {}", e))?
            .map(|p| p.path),
        None => None,
    };
    let Some(path) = project_path else {
        return Ok(Vec::new());
    };
    let root = PathBuf::from(path);
    let depth = max_depth.map(|d| d as usize);
    // std::fs + git2 are blocking → offload onto the blocking pool.
    let paths = tokio::task::spawn_blocking(move || match depth {
        Some(d) => crate::files::walk_files_with_depth(&root, d),
        None => crate::files::walk_files(&root),
    })
    .await
    .map_err(|e| anyhow::anyhow!("list_files: walk join failed: {}", e))?;
    Ok(paths)
}

/// List files under the current project root as root-relative
/// forward-slash paths, for the `@`-mention completion panel.
///
/// * `project_id` selects the project; the project's `path` is the walk
///   root.
/// * `max_depth = None` walks with the default cap
///   ([`crate::files::MAX_DEPTH`]); `Some(n)` caps at `n` layers under
///   the root (use a small value for the default `@` trigger).
///
/// Returns an empty vec when there is no project / the project has no
/// path / the walk fails — the frontend renders an empty panel
/// ("无匹配文件") rather than surfacing an error.
#[tauri::command]
pub async fn list_files(
    state: State<'_, Arc<AppState>>,
    project_id: Option<String>,
    max_depth: Option<u32>,
) -> Result<Vec<String>, AppCommandError> {
    list_files_inner(&state, project_id, max_depth).await
}

/// Phase 2.2 `_inner` (Q0): shared business logic. Callable from
/// the Tauri command wrapper below + the axum route handler in
/// `daemon::routes::files`. Doesn't take `Arc<AppState>` because
/// the walk is fully defined by its inputs (root + max_depth).
pub async fn list_files_at_inner(
    root: String,
    max_depth: Option<u32>,
) -> Result<Vec<String>, AppCommandError> {
    let root_path = PathBuf::from(&root);
    if !root_path.is_absolute() {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!("list_files_at: root must be absolute, got {:?}", root_path),
        ));
    }
    // Only the literal filesystem root is allowed. We don't accept
    // arbitrary subdirs of `/` here — that would let a chat turn
    // walk `/home/...` or other users' homes. If the need arises,
    // add a separate `list_files_under` with explicit boundary checks.
    if root_path != *"/" {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!(
                "list_files_at: root must be `/`, got {:?} (use list_files for project paths)",
                root_path
            ),
        ));
    }
    let depth = max_depth.map(|d| d as usize);
    let paths = tokio::task::spawn_blocking(move || match depth {
        Some(d) => crate::files::walk_system(&root_path, d),
        None => crate::files::walk_system(&root_path, crate::files::MAX_DEPTH),
    })
    .await
    .map_err(|e| anyhow::anyhow!("list_files_at: walk join failed: {}", e))?;
    Ok(paths)
}

/// List files under an arbitrary absolute `root` for the `@/`-prefixed
/// system-root mention panel. Returns root-relative forward-slash
/// paths (so a file at `/etc/hosts` comes back as `etc/hosts`).
///
/// `root` MUST be absolute and MUST live under `/`. Non-`/` roots
/// are rejected — `@/foo` is reserved for the filesystem root view;
/// project-relative paths go through the project-aware `list_files`.
/// The walk uses the wider [`crate::files::SYSTEM_EXCLUDE`] set so
/// `/proc`, `/sys`, `/dev`, etc. never get visited (would either
/// hang on virtual fs or pollute the picker with device nodes).
///
/// `max_depth = None` uses [`crate::files::MAX_DEPTH`]; `Some(n)` caps
/// the walk at `n` layers. We strongly recommend `Some(<= 4)` —
/// `/usr/share/*` alone has tens of thousands of files.
#[tauri::command]
pub async fn list_files_at(
    root: String,
    max_depth: Option<u32>,
) -> Result<Vec<String>, AppCommandError> {
    list_files_at_inner(root, max_depth).await
}

// --- 图片路径预览(2026-09-13)----------------------------------------------
// daemon `GET /api/v1/files/image?path=<abs|~前缀>` 的支撑逻辑:聊天
// markdown 里的本地图片路径被渲染成可点击链接,点击后弹层 `<img>` 直连
// 本路由取字节。安全面:扩展名白名单 + 大小上限(见下方常量),svg 有意
// 排除 —— `<img>` 内加载 svg 不执行脚本,但同一 URL 被"新标签打开"
// 兜底按钮消费时是独立文档,脚本会跑;白名单里不给它位置就两态皆安。
// 错误分类由 route 层映射 HTTP 状态码(400/404/413),不走
// `AppCommandError`(其 ErrorCategory 无 NotFound/PayloadTooLarge 档)。

/// 单文件大小上限 32 MiB:聊天预览场景足够(截图/图表),防止误点大文件
/// 把 daemon 内存打爆。TOCTOU 兜底:metadata 检查后再读,读后复核。
pub const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

/// 白名单扩展名(小写)→ Content-Type。不在表内 → `None`(调用方拒 400)。
fn image_content_type(ext: &str) -> Option<&'static str> {
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "avif" => Some("image/avif"),
        "ico" => Some("image/x-icon"),
        _ => None,
    }
}

/// `read_image_at_inner` 的错误分类。route 层(`daemon/routes/files.rs`)
/// match 此枚举映射 HTTP 状态码;此处不携带 HTTP 语义。
#[derive(Debug)]
pub enum ReadImageError {
    /// 路径非绝对形态、无扩展名或扩展不在白名单 → 400。
    InvalidRequest(String),
    /// 文件不存在或不是普通文件 → 404。
    NotFound,
    /// 超过 [`MAX_IMAGE_BYTES`] → 413。
    TooLarge,
    /// 文件系统 IO 失败(权限等)→ 500。
    Io(String),
}

/// `~` 前缀展开为真实 home(与 `get_home_dir_inner` 同源,`dirs` crate)。
fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

/// 读取一个本地图片文件供 `<img>` 直连消费。
///
/// 契约:`path` 必须是绝对路径或 `~/` 前缀(相对路径 400 —— 相对路径由
/// 前端按会话 cwd 解析成绝对路径后才进本路由,daemon 侧的 cwd 语义
/// 不明,不猜)。返回 `(Content-Type, 字节)`;错误分类见
/// [`ReadImageError`]。
pub async fn read_image_at_inner(path: String) -> Result<(&'static str, Vec<u8>), ReadImageError> {
    let expanded = expand_home(&path);
    if !expanded.is_absolute() {
        return Err(ReadImageError::InvalidRequest(format!(
            "path must be absolute or `~/`-prefixed, got {:?}",
            path
        )));
    }
    let ext = path_extension_lower(&expanded);
    let content_type = match ext.as_deref().and_then(image_content_type) {
        Some(ct) => ct,
        // 无扩展名/非白名单统一 400,不给"存在性"旁信道(路径探测面)。
        None => {
            return Err(ReadImageError::InvalidRequest(
                "path must end in a whitelisted image extension".to_string(),
            ))
        }
    };
    let meta = tokio::fs::metadata(&expanded)
        .await
        .map_err(|_| ReadImageError::NotFound)?;
    if !meta.is_file() {
        return Err(ReadImageError::NotFound);
    }
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(ReadImageError::TooLarge);
    }
    let bytes = tokio::fs::read(&expanded)
        .await
        .map_err(|e| ReadImageError::Io(e.to_string()))?;
    // TOCTOU 兜底:metadata 与 read 之间文件可能被写大。
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(ReadImageError::TooLarge);
    }
    Ok((content_type, bytes))
}

// --- 文件路径预览(2026-09-13,图片通道的平行扩展)---------------------------
// daemon `GET /api/v1/files/raw?path=<abs|~前缀>` 的支撑逻辑:聊天/工具
// 输出里的本地文本与 pdf 路径被渲染成可点击链接,点击后 FileViewerModal
// fetch 本路由取内容(pdf 不进弹层,新标签走浏览器原生 viewer)。
// 安全面三条硬约束:
//   - 白名单统一 400,不给"存在性"旁信道(沿 image 先例);
//   - 文本类**一律** `text/plain; charset=utf-8` 下发 —— .html/.htm 在
//     白名单里也只作文本查看,MIME 即闸门,任何消费方(弹层 hljs /
//     新标签)见到的都是源码;不存在 text/html 或 image/svg+xml 的下发
//     路径(svg 连文本查看都不给,维持 image 通道的排除决策);
//   - 文本类额外做严格 UTF-8 校验:弹层按字符串消费(markdown 渲染 /
//     hljs 高亮),lossy 替换符会掩盖二进制误命名,直接 400。

/// 文本类单文件大小上限 2 MiB:文本会被前端弹层整串读进 DOM(markdown
/// 渲染 / hljs 高亮),32 MiB 文本足以卡死 UI;2 MiB ≈ 2M 字符远超正常
/// 查看需求。pdf 档沿用 [`MAX_IMAGE_BYTES`] 量级(浏览器原生 viewer
/// 流式消费,不经前端字符串化),不单设常量。
pub const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;

/// `/files/raw` 的内容分档:文本类(强制 text/plain)与 pdf。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RawClass {
    Text,
    Pdf,
}

/// 文本类白名单扩展名(小写)。与前端 `utils/markdown.ts` 的识别集
/// 有意各持一份(后端是唯一安全闸门,前端集偏大只会点开见 400);
/// 对齐约定记录在 `.trellis/spec/frontend/chat/message-list-and-markdown.md`
/// §5,改动任一侧须对照另一侧。
const RAW_TEXT_EXTS: &[&str] = &[
    "md", "markdown", "txt", "log", "json", "jsonl", "csv", "tsv", "yaml", "yml", "toml", "ini",
    "conf", "cfg", "xml", "html", "htm", "css", "js", "mjs", "cjs", "jsx", "ts", "tsx", "vue",
    "svelte", "py", "rs", "go", "java", "kt", "kts", "c", "h", "cpp", "hpp", "cc", "cs", "rb",
    "php", "sh", "bash", "zsh", "fish", "sql", "proto", "graphql", "gql", "diff", "patch",
];

/// 白名单扩展名(小写)→ (Content-Type, 分档)。不在表内 → `None`
/// (调用方拒 400)。
fn raw_content_type(ext: &str) -> Option<(&'static str, RawClass)> {
    if ext == "pdf" {
        Some(("application/pdf", RawClass::Pdf))
    } else if RAW_TEXT_EXTS.contains(&ext) {
        Some(("text/plain; charset=utf-8", RawClass::Text))
    } else {
        None
    }
}

/// `read_raw_at_inner` 的错误分类。形状镜像 [`ReadImageError`],route 层
/// (`daemon/routes/files.rs`)match 映射 HTTP 状态码。
#[derive(Debug)]
pub enum ReadRawError {
    /// 路径非绝对形态、扩展不在白名单、文本内容非法 UTF-8 → 400。
    InvalidRequest(String),
    /// 文件不存在或不是普通文件 → 404。
    NotFound,
    /// 超过分档上限(文本 [`MAX_TEXT_BYTES`] / pdf [`MAX_IMAGE_BYTES`])→ 413。
    TooLarge,
    /// 文件系统 IO 失败(权限等)→ 500。
    Io(String),
}

/// 读取一个本地文本/pdf 文件供 FileViewerModal fetch 消费。
///
/// 契约与 [`read_image_at_inner`] 同构:`path` 必须是绝对路径或 `~/`
/// 前缀(相对路径 400)。返回 `(Content-Type, 字节)`;文本类保证是合法
/// UTF-8(校验失败 400),错误分类见 [`ReadRawError`]。
pub async fn read_raw_at_inner(path: String) -> Result<(&'static str, Vec<u8>), ReadRawError> {
    let expanded = expand_home(&path);
    if !expanded.is_absolute() {
        return Err(ReadRawError::InvalidRequest(format!(
            "path must be absolute or `~/`-prefixed, got {:?}",
            path
        )));
    }
    let ext = path_extension_lower(&expanded);
    let (content_type, class) = match ext.as_deref().and_then(raw_content_type) {
        Some(v) => v,
        // 无扩展名/非白名单统一 400,不给"存在性"旁信道(路径探测面)。
        None => {
            return Err(ReadRawError::InvalidRequest(
                "path must end in a whitelisted file extension".to_string(),
            ))
        }
    };
    let max = match class {
        RawClass::Text => MAX_TEXT_BYTES,
        RawClass::Pdf => MAX_IMAGE_BYTES,
    };
    let meta = tokio::fs::metadata(&expanded)
        .await
        .map_err(|_| ReadRawError::NotFound)?;
    if !meta.is_file() {
        return Err(ReadRawError::NotFound);
    }
    if meta.len() > max {
        return Err(ReadRawError::TooLarge);
    }
    let bytes = tokio::fs::read(&expanded)
        .await
        .map_err(|e| ReadRawError::Io(e.to_string()))?;
    // TOCTOU 兜底:metadata 与 read 之间文件可能被写大。
    if bytes.len() as u64 > max {
        return Err(ReadRawError::TooLarge);
    }
    // 文本类严格 UTF-8(见上方安全面注释)。from_utf8 成功后 into_bytes
    // 原样取回(零拷贝往返),失败即 400。
    let bytes = if class == RawClass::Text {
        match String::from_utf8(bytes) {
            Ok(s) => s.into_bytes(),
            Err(_) => {
                return Err(ReadRawError::InvalidRequest(
                    "text file is not valid UTF-8".to_string(),
                ))
            }
        }
    } else {
        bytes
    };
    Ok((content_type, bytes))
}

// --- 路径存在性探针(2026-09-14,图片/文件通道的第三位成员)------------------
// daemon `GET /api/v1/files/stat?path=<abs|~前缀>`:前端 linkify 对本地路径
// **乐观渲染**链接后,用本路由异步确认存在性,确认缺失才把锚点降级回纯
// 文本(抖动权衡见前端 `utils/pathExistence.ts` 模块注释)。与 image/raw
// 的关系:白名单取两路由之**并**(前端识别集 `FILE_EXT` 同并集,可链接 ⇔
// 可探);不读内容、无大小上限 —— `metadata` 一把 O(1),超限文件照样
// "存在可点",点击后由 image/raw 的 413 错误态兜底。oracle 面与 image/raw
// 严格持平:白名单外统一 400,存在性信息只对白名单扩展"泄露"(image/raw
// 的 200/404 本就泄露同口径信息)。

/// `stat_local_file_inner` 的错误分类。无 TooLarge(stat 不读内容)、无 Io
/// (metadata 失败一律折叠进 NotFound,与 image/raw 的 `map_err(|_| NotFound)`
/// 同口径:调用方不区分 ENOENT/EACCES)。
#[derive(Debug)]
pub enum StatError {
    /// 路径非绝对形态、无扩展名或扩展不在并集白名单 → 400。
    InvalidRequest(String),
    /// 不存在或不是普通文件 → 404。
    NotFound,
}

/// 扩展是否在 image ∪ raw 白名单内(供 [`stat_local_file_inner`])。
fn is_previewable_ext(ext: &str) -> bool {
    image_content_type(ext).is_some() || raw_content_type(ext).is_some()
}

/// 探测一个本地文件是否存在且是普通文件。契约与 [`read_image_at_inner`]
/// 同构:`path` 必须是绝对路径或 `~/` 前缀(相对路径 400)。`Ok(())` =
/// 存在(200);错误分类见 [`StatError`]。不读文件内容。
pub async fn stat_local_file_inner(path: String) -> Result<(), StatError> {
    let expanded = expand_home(&path);
    if !expanded.is_absolute() {
        return Err(StatError::InvalidRequest(format!(
            "path must be absolute or `~/`-prefixed, got {:?}",
            path
        )));
    }
    let ext = path_extension_lower(&expanded);
    if !ext.as_deref().map(is_previewable_ext).unwrap_or(false) {
        return Err(StatError::InvalidRequest(
            "path must end in a whitelisted image/text/pdf extension".to_string(),
        ));
    }
    match tokio::fs::metadata(&expanded).await {
        Ok(m) if m.is_file() => Ok(()),
        // 不存在 / 目录误命名 / metadata 失败 → 统一 404,不给调用方
        // 更细的区分(存在性探针只需要一个比特)。
        _ => Err(StatError::NotFound),
    }
}

fn path_extension_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}
