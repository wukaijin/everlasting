//! daemon 日志的进程内文件 sink + 大小轮转(F3 PR2 / P0-b,2026-09-03,
//! task `09-03-f3-disk-governance` design §3)。
//!
//! **为什么**:此前唯一轮转是 `scripts/daemon.sh` bg 启动前的 >10MiB
//! 检查——连续运行期单文件无限涨(实证 29M 单代);且打包 GUI(sidecar
//! 模式)的 daemon 日志完全不落盘。进程内 appender 让三种启动路径
//! (daemon.sh bg / start / sidecar)统一落同一文件,运行期实时轮转。
//!
//! **选型:零依赖手写,不引 tracing-appender**(仓库「不为此拉新
//! crate」先例 `tools/glob.rs:205-207`;轮转逻辑 30 行内,新依赖不
//! 成比例)。
//!
//! 契约(与退役的 `daemon.sh::rotate_log` 逐条对齐):
//! - 路径 `${XDG_STATE_HOME:-${HOME:-/tmp}/.local/state}/dev.everlasting.
//!   app/daemon.log`(照 daemon.sh:54 的 bash `:-` 语义:空串视为未设);
//!   `logs` 子命令 `tail -f` 同一路径,零改动。
//! - 单文件超过 [`LOG_MAX_BYTES`](10MiB,与脚本现行一致)→ 滚动
//!   `daemon.log→.1→.2→.3`,最旧删除,保留 [`LOG_KEEP`](3)个旧代,
//!   合计 ≈ 4×10MiB 封顶。
//! - **降级铁律**:打开 / 创建 / 重开失败 → 文件 sink 退化为 no-op,
//!   绝不 panic——daemon 必须能起来(终端 layer 仍在,日志不丢)。
//! - 轮转检查**不逐条 stat**:fd 打开时 stat 一次校准基线,之后自计数
//!   写入字节数,超阈值在下一次 write 前触发滚动(「记录上次检查点」
//!   节流的极限形态,steady state 零系统调用开销)。
//! - 并发:tracing 的 writer 可能被多线程并发调用,状态全部收敛在
//!   `std::sync::Mutex` 内(`&self` 即可写)。
//! - writer 内部**禁止 tracing 宏**(会经同一订阅器递归回 write);
//!   降级提示走 `eprintln!` 直写 stderr。

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use tracing_subscriber::fmt::MakeWriter;

/// 单文件大小上限(超过即滚动)。与 daemon.sh 退役的 `rotate_log`
/// 现行值完全一致(RULE-DAEMON-001)。
pub const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// 滚动保留的旧代数(`.1` … `.3`,最旧删除)。
pub const LOG_KEEP: usize = 3;

/// XDG state 下的应用目录(daemon.sh:54 同名常量)。
const STATE_APP_DIR: &str = "dev.everlasting.app";

/// 日志文件名(`logs` 子命令 tail 的同一路径)。
const LOG_FILE_NAME: &str = "daemon.log";

/// 运行期路径解析:读进程 env(`XDG_STATE_HOME` → `HOME`)。纯函数
/// 核心 [`state_log_path_from`] 见下(单测锚点,避免 set_var 竞态)。
pub fn state_log_path() -> PathBuf {
    state_log_path_from(
        std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

/// 路径解析纯函数核心:镜像 `daemon.sh:54` 的 bash 语义
/// `${XDG_STATE_HOME:-${HOME:-/tmp}/.local/state}/dev.everlasting.app/daemon.log`
/// —— `:-` 把**空串**也视为未设;`HOME` 缺失(cron/systemd 裸环境)退
/// `/tmp`,防 `set -u` 连坐(脚本侧先例的原注释语义一并保留)。
pub fn state_log_path_from(xdg: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    let base = xdg
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            home.unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".local/state")
        });
    base.join(STATE_APP_DIR).join(LOG_FILE_NAME)
}

/// 打开(或创建)日志文件,返回 fd + 当前长度(stat 一次,轮转基线)。
fn open_file(path: &Path) -> io::Result<(File, u64)> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let len = file.metadata()?.len();
    Ok((file, len))
}

/// 滚动一代:`.3` 删 → `.2→.3` → `.1→.2` → `log→.1`。逐代存在性守卫
/// + 失败降级,逐条对应脚本 `rotate_log` 的 `[[ -f ]] && mv -f … || true`
/// 先例:轮转是附属保障,任何失败都不能挡住日志写入(主文件滚动失败
/// 则继续追加写原文件)。
fn rotate(path: &Path, keep: usize) {
    let _ = std::fs::remove_file(suffixed(path, keep)); // 最旧代直接删
    for i in (1..keep).rev() {
        let from = suffixed(path, i);
        if from.exists() {
            let _ = std::fs::rename(&from, suffixed(path, i + 1));
        }
    }
    if let Err(e) = std::fs::rename(path, suffixed(path, 1)) {
        eprintln!("log_rotation: 滚动失败({e}),继续追加写 {}", path.display());
    }
}

/// `<path>.<gen>` 后缀路径(`OsString` 拼接,防非 UTF-8 文件名走样)。
fn suffixed(path: &Path, gen: usize) -> PathBuf {
    let mut name = std::ffi::OsString::from(path.file_name().unwrap_or_default());
    name.push(format!(".{gen}"));
    path.with_file_name(name)
}

struct Inner {
    /// `None` = 降级模式(打开/滚动后重开失败):write 变 no-op(假装
    /// 成功),终端 layer 仍在,daemon 不受影响。一次性降级,不重试
    /// (重启即恢复;运行期重试引入的状态机不成比例)。
    file: Option<File>,
    path: PathBuf,
    max_bytes: u64,
    keep: usize,
    /// 自 fd 打开以来累计的字节数(**含打开时 stat 到的既有长度**)。
    /// 轮转判定只看它,steady state 零 stat。
    len: u64,
}

impl Inner {
    /// 关旧 fd → 滚动 → 重开。任何一步失败落降级(`file = None`),
    /// 不 panic。
    fn rotate_and_reopen(&mut self) {
        // 先关 fd 再滚动:否则 fd 继续指向改名后的旧文件(Unix 上
        // rename 不影响已打开的 fd,写入会跟着旧 inode 走)。
        self.file = None;
        rotate(&self.path, self.keep);
        match open_file(&self.path) {
            Ok((file, len)) => {
                self.file = Some(file);
                self.len = len;
            }
            Err(e) => {
                eprintln!(
                    "log_rotation: 滚动后重开 {} 失败({e}),文件 sink 降级",
                    self.path.display()
                );
                self.len = 0;
            }
        }
    }
}

/// 进程内轮转文件 writer:实现 `MakeWriter` 供
/// `tracing_subscriber::fmt::layer().with_writer(...)` 消费。
///
/// 生产构造走 [`Self::open_default`](env 解析 + [`LOG_MAX_BYTES`]/
/// [`LOG_KEEP`] 常量);测试用 [`Self::new`] 注入小阈值。降级模式下
/// 构造不 panic、write 全部 no-op 成功。
pub struct RotatingFileWriter {
    inner: Mutex<Inner>,
}

impl RotatingFileWriter {
    /// 全参构造(测试注入口)。打开失败 → 降级模式(不 panic)。
    pub fn new(path: PathBuf, max_bytes: u64, keep: usize) -> Self {
        let inner = match open_file(&path) {
            Ok((file, len)) => Inner {
                file: Some(file),
                path,
                max_bytes,
                keep,
                len,
            },
            Err(e) => {
                // eprintln 而非 tracing!:此刻订阅器未装好,且 writer
                // 内部发 tracing 事件会递归。
                eprintln!(
                    "log_rotation: 打开 {} 失败({e}),文件 sink 降级为 no-op(终端 layer 仍在)",
                    path.display()
                );
                Inner {
                    file: None,
                    path,
                    max_bytes,
                    keep,
                    len: 0,
                }
            }
        };
        Self {
            inner: Mutex::new(inner),
        }
    }

    /// 生产构造:`$XDG_STATE_HOME` 路径 + 常量参数(与退役脚本轮转
    /// 的 10MiB×3 完全一致)。
    pub fn open_default() -> Self {
        Self::new(state_log_path(), LOG_MAX_BYTES, LOG_KEEP)
    }

    fn write_bytes(&self, buf: &[u8]) -> io::Result<usize> {
        // 毒锁恢复:writer 内若曾 panic,继续服务比永久卡死日志好。
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        // write 前轮转检查:既有长度**已超**阈值(严格 >,与脚本
        // `>10MiB` 同语义)→ 先滚再写。降级模式恒 no-op。
        if inner.len > inner.max_bytes {
            inner.rotate_and_reopen();
        }
        match inner.file.as_mut() {
            // 降级:假装写成功(终端 layer 仍输出,不丢日志)。
            None => Ok(buf.len()),
            Some(file) => {
                let n = std::io::Write::write(file, buf)?;
                inner.len += n as u64;
                Ok(n)
            }
        }
    }
}

// `make_writer` 返回 `&'a self`,故 Write 落在引用上(内部状态全在
// Mutex,`&self` 即可安全写)。
impl io::Write for &RotatingFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_bytes(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        // File 无用户态缓冲(每次 write 直 syscall),无物可 flush。
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for RotatingFileWriter {
    type Writer = &'a Self;

    fn make_writer(&'a self) -> Self::Writer {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 通过 `&writer` 走生产同一条 write 路径(MakeWriter 产物形态)。
    fn write_line(w: &RotatingFileWriter, payload: &[u8]) {
        let mut handle: &RotatingFileWriter = w;
        handle.write_all(payload).expect("write must not fail");
    }

    /// 轮转全代次:60B × 9 条,阈值 100。期望时间线(len 累计,超
    /// 100 触发下一条 write 前滚动):
    /// w1/w2 → len 120;w3 滚出 .1(120B)→ … 三轮后 .1/.2/.3 齐全,
    /// .4 永不存在,滚动后新文件继续写成功。
    #[test]
    fn rotation_rolls_generations_keeps_three_and_keeps_writing() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("daemon.log");
        let w = RotatingFileWriter::new(log.clone(), 100, 3);

        let payload = vec![b'x'; 60];
        for _ in 0..9 {
            write_line(&w, &payload);
        }

        // 三个旧代齐全,内容各自完整(60×2 = 120B,滚动前的满代)。
        for gen in 1..=3 {
            let path = suffixed(&log, gen);
            assert!(path.exists(), ".{gen} must exist after 9 writes");
            assert_eq!(
                std::fs::metadata(&path).unwrap().len(),
                120,
                "generation .{gen} carries a full pre-rotation file"
            );
        }
        // 最多 3 个旧代:.4 不存在。
        assert!(!suffixed(&log, 4).exists(), "at most 3 old generations");
        // 滚动后继续写新文件成功:当前文件非空且仍在同一路径。
        let cur = std::fs::read(&log).unwrap();
        assert!(!cur.is_empty(), "current file keeps receiving writes");
        assert!(cur.len() <= 120);
    }

    /// 启动时轮转语义(对齐退役脚本的"启动前 >10MiB 检查"):打开时
    /// stat 到既有 150B > 100 阈值 → **首条** write 就滚动,旧内容完整
    /// 挪到 .1,新文件从空开始追加。
    #[test]
    fn startup_rotation_when_existing_file_exceeds_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("daemon.log");
        std::fs::write(&log, vec![b'o'; 150]).unwrap();

        let w = RotatingFileWriter::new(log.clone(), 100, 3);
        write_line(&w, b"fresh");

        assert_eq!(std::fs::read(&log).unwrap(), b"fresh");
        assert_eq!(
            std::fs::read(suffixed(&log, 1)).unwrap(),
            vec![b'o'; 150],
            "oversized existing file rolls to .1 intact on first write"
        );
    }

    /// 目录惰性创建:父目录不存在 → 构造自建(`create_dir_all`)。
    #[test]
    fn creates_missing_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("nested/deeper/daemon.log");
        let w = RotatingFileWriter::new(log.clone(), 100, 3);
        write_line(&w, b"hello");
        assert_eq!(std::fs::read(&log).unwrap(), b"hello");
    }

    /// 降级铁律:路径不可创建(父链上横着普通文件 → ENOTDIR)→
    /// 构造不 panic,后续 write 走降级 no-op(Ok),不落任何文件。
    #[test]
    fn unwritable_path_degrades_to_noop_without_panic() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"regular file").unwrap();
        let bad = blocker.join("sub/daemon.log");

        let w = RotatingFileWriter::new(bad.clone(), 100, 3);
        write_line(&w, b"dropped");
        write_line(&w, b"also dropped");
        assert!(!bad.exists(), "degraded writer must not create files");
    }

    /// 路径解析(纯函数核心,镜像 daemon.sh:54 bash 语义):XDG 优先;
    /// XDG 空串 = 未设(`:-` 语义)→ HOME 回退;双缺 → /tmp 回退。
    #[test]
    fn state_log_path_mirrors_daemon_sh_semantics() {
        assert_eq!(
            state_log_path_from(Some("/xdg/state".into()), Some("/home/u".into())),
            PathBuf::from("/xdg/state/dev.everlasting.app/daemon.log"),
            "XDG_STATE_HOME wins when set"
        );
        assert_eq!(
            state_log_path_from(Some("".into()), Some("/home/u".into())),
            PathBuf::from("/home/u/.local/state/dev.everlasting.app/daemon.log"),
            "empty XDG is unset per bash :- semantics"
        );
        assert_eq!(
            state_log_path_from(None, Some("/home/u".into())),
            PathBuf::from("/home/u/.local/state/dev.everlasting.app/daemon.log"),
            "HOME fallback when XDG unset"
        );
        assert_eq!(
            state_log_path_from(None, None),
            PathBuf::from("/tmp/.local/state/dev.everlasting.app/daemon.log"),
            "bare env (cron/systemd) falls back to /tmp like the script"
        );
    }

    /// 并发安全:多线程并发 write(阈值内含触发滚动的组合)→ 无
    /// panic、无死锁(测试完成即断言),当前文件完好可读。
    #[test]
    fn concurrent_writes_are_safe() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("daemon.log");
        let w = std::sync::Arc::new(RotatingFileWriter::new(log.clone(), 1000, 3));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let w = std::sync::Arc::clone(&w);
                std::thread::spawn(move || {
                    let payload = vec![b't'; 40];
                    for _ in 0..200 {
                        write_line(&w, &payload);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("no writer thread may panic");
        }
        assert!(log.exists() && !std::fs::read(&log).unwrap().is_empty());
    }
}
