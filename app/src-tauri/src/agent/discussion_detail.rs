//! `end_discussion` 结构化结论(C2 证据链,任务 09-09-gc-c2-evidence-summary)。
//!
//! moderator 收官时在 `end_discussion` 的结构化参数里给出逐条结论
//! (claim + file:line 锚点 + 实证/推测/争议标注)与开放问题;编排器
//! 落库前对锚点做后校验,校验结果并入锚点的 `check` 字段。
//!
//! 校验语义(Q2 用户裁定 2026-09-09:**只标注不修改**)——机制层只提供
//! 事实,不改写 moderator 的 claim 与 stance;断证信号留给消费方
//! (GUI 徽章 / MCP `detail` / 转录记号)。root 外锚点一律零 fs 访问
//! (编排器直读绕过工具沙盒模型,仅 project_root 前缀内受信读)。

use std::path::Path;

/// 一场讨论的结构化收官产物。wire 形态 = `sessions.discussion_detail`
/// 列的 JSON 文本;键 snake_case(sessions 域 TS 字段惯例,区别于
/// providers 域 camelCase)。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DiscussionDetail {
    #[serde(default)]
    pub conclusions: Vec<Conclusion>,
    #[serde(default)]
    pub open_questions: Vec<String>,
}

/// 单条结论。`stance` 是 moderator 的自我声明,校验**不**改写它。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Conclusion {
    pub claim: String,
    #[serde(default)]
    pub anchors: Vec<Anchor>,
    #[serde(default)]
    pub stance: Stance,
}

/// 证据锚点。`check` 由 [`validate_anchors`] 回填;`None` = 校验尚未
/// 跑过(live 期 GUI 直接渲染 tool_use input 时)。root 缺失 /
/// canonicalize root 失败不是 `None`——那场全部锚点显式标
/// `Some(Unvalidated)`(「查过但查不了」与「没查过」是两个信号)。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Anchor {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<AnchorCheck>,
}

/// 结论的可信度声明(moderator 自报,校验不降级)。缺省 inferred:
/// 未声明 stance 的主张不享受实证待遇(可信度标注的保守缺省)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stance {
    /// 亲自读过代码/证据(通常带锚点)。
    Verified,
    /// 推理得出,未亲证(锚点应缺省)。缺省档。
    #[default]
    Inferred,
    /// 讨论未达共识,存在实质分歧。
    Disputed,
}

/// 锚点后校验结果(只标注,不修改 claim/stance)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorCheck {
    /// 文件存在;带 line 时行号在界内。
    Ok,
    /// root 内路径不存在(或指向目录/不存在于 root 外)。
    NotFound,
    /// 文件存在但行号越界。
    LineOutOfRange,
    /// 解析后落在 project_root 之外(含绝对路径/目录穿越)——零 fs 访问。
    OutsideRoot,
    /// 未能校验(root 缺失 / IO 错误 / 超过读取帽)。
    Unvalidated,
}

/// 从 `end_discussion` 工具入参宽容解析结构化部分:逐条结论独立解析,
/// 坏条目丢弃不连坐;返回 `None` 的全部情形(字段缺失/类型错位/无有效
/// 条目)都等价于「本次收官无结构化产物」,收官路径不因格式问题失败。
pub fn parse_from_input(input: &serde_json::Value) -> Option<DiscussionDetail> {
    #[derive(serde::Deserialize)]
    struct Raw {
        #[serde(default)]
        conclusions: Option<Vec<serde_json::Value>>,
        #[serde(default)]
        open_questions: Option<Vec<serde_json::Value>>,
    }
    // input 里还有 summary 等无关键,未知字段默认忽略。
    let raw: Raw = serde_json::from_value(input.clone()).ok()?;
    let conclusions: Vec<Conclusion> = raw
        .conclusions
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Conclusion>(v).ok())
        .filter(|c| !c.claim.trim().is_empty())
        .collect();
    let open_questions: Vec<String> = raw
        .open_questions
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .filter(|q| !q.trim().is_empty())
        .collect();
    if conclusions.is_empty() && open_questions.is_empty() {
        return None;
    }
    Some(DiscussionDetail {
        conclusions,
        open_questions,
    })
}

/// 行数读取帽(字节):锚点校验只数行,超过此大小的文件按 Unvalidated
/// 处理,防病理大文件拖住收束路径。正常源文件不触。
const ANCHOR_READ_CAP_BYTES: u64 = 16 * 1024 * 1024;

/// 对 detail 内全部锚点回填 `check`。永不失败:任何单锚点异常都落到
/// [`AnchorCheck::Unvalidated`],finalize 照常。
pub fn validate_anchors(detail: &mut DiscussionDetail, project_root: Option<&Path>) {
    let root = match project_root {
        Some(r) => r,
        None => {
            mark_all(detail, AnchorCheck::Unvalidated);
            return;
        }
    };
    let root_canon = match std::fs::canonicalize(root) {
        Ok(p) => p,
        Err(_) => {
            mark_all(detail, AnchorCheck::Unvalidated);
            return;
        }
    };
    for c in &mut detail.conclusions {
        for a in &mut c.anchors {
            a.check = Some(check_one(&a.path, a.line, &root_canon));
        }
    }
}

fn mark_all(detail: &mut DiscussionDetail, check: AnchorCheck) {
    for c in &mut detail.conclusions {
        for a in &mut c.anchors {
            a.check = Some(check);
        }
    }
}

/// 单锚点校验。路径先与 root 拼接再 canonicalize:
/// - 绝对路径 join 时整体替换 base;穿越(`..`)由 canonicalize 展开 ——
///   两者最终落在 root 前缀外即 `OutsideRoot`(存在的绝对路径必走此臂);
/// - root 内不存在 / 目录 → `NotFound`;
/// - 带 line 且文件在读取帽内 → 行数比对,越界 `LineOutOfRange`;
/// - IO / 帽外 → `Unvalidated`。
fn check_one(path: &str, line: Option<u64>, root_canon: &Path) -> AnchorCheck {
    let joined = root_canon.join(path);
    let canon = match std::fs::canonicalize(&joined) {
        Ok(p) => p,
        Err(_) => return AnchorCheck::NotFound,
    };
    if !canon.starts_with(root_canon) {
        return AnchorCheck::OutsideRoot;
    }
    let meta = match std::fs::metadata(&canon) {
        Ok(m) => m,
        Err(_) => return AnchorCheck::Unvalidated,
    };
    if !meta.is_file() {
        return AnchorCheck::NotFound;
    }
    let Some(line) = line else {
        return AnchorCheck::Ok;
    };
    match count_lines_capped(&canon) {
        Some(lines) if line >= 1 && line <= lines => AnchorCheck::Ok,
        Some(_) => AnchorCheck::LineOutOfRange,
        None => AnchorCheck::Unvalidated,
    }
}

/// 流式数行,字节帽 [`ANCHOR_READ_CAP_BYTES`];超帽或 IO 错误 → `None`。
fn count_lines_capped(path: &Path) -> Option<u64> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 64 * 1024];
    let mut total: u64 = 0;
    let mut lines: u64 = 0;
    let mut ends_with_newline = false;
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > ANCHOR_READ_CAP_BYTES {
            return None;
        }
        lines += buf[..n].iter().filter(|&&b| b == b'\n').count() as u64;
        ends_with_newline = buf[n - 1] == b'\n';
    }
    // 无尾换行的末行也是一行(以 \n 结尾的文件不加幽灵行);空文件 0 行。
    if total > 0 && !ends_with_newline {
        lines += 1;
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn detail_from(v: serde_json::Value) -> DiscussionDetail {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn parse_full_structured_input() {
        let input = json!({
            "summary": "叙事",
            "conclusions": [
                {"claim": "A 已修", "anchors": [{"path": "a.rs", "line": 3}], "stance": "verified"},
                {"claim": "B 可能", "stance": "inferred"}
            ],
            "open_questions": ["macOS 未验"]
        });
        let d = parse_from_input(&input).unwrap();
        assert_eq!(d.conclusions.len(), 2);
        assert_eq!(d.conclusions[0].stance, Stance::Verified);
        assert_eq!(d.conclusions[1].stance, Stance::Inferred);
        assert_eq!(d.open_questions, vec!["macOS 未验"]);
    }

    #[test]
    fn parse_defaults_and_tolerates_unknown() {
        let input = json!({
            "conclusions": [{"claim": "x", "extra_unknown": 1}],
            "open_questions": [],
            "no_such_field": true
        });
        let d = parse_from_input(&input).unwrap();
        assert_eq!(
            d.conclusions[0].stance,
            Stance::Inferred,
            "缺省 stance = inferred"
        );
        assert!(d.conclusions[0].anchors.is_empty());
    }

    #[test]
    fn parse_tolerates_null_fields_and_bad_entries() {
        let input = json!({
            "conclusions": [null, {"claim": "  "}, {"claim": "ok", "stance": "bogus_stance"}, 42],
            "open_questions": null
        });
        // "bogus_stance" 反序列化失败 → 该条丢弃;null/42/空 claim 同样丢弃。
        // 全部条目被丢弃 = 无结构化产物 → None(与空输入同语义)。
        assert!(parse_from_input(&input).is_none());
        // 好坏混合:坏条目丢弃不连坐,好条目保留。
        let mixed = json!({
            "conclusions": [null, {"claim": "ok", "stance": "verified"}, 42]
        });
        let d = parse_from_input(&mixed).unwrap();
        assert_eq!(d.conclusions.len(), 1);
        assert_eq!(d.conclusions[0].claim, "ok");
    }

    #[test]
    fn parse_no_valid_output_returns_none() {
        assert!(parse_from_input(&json!({"summary": "只文本"})).is_none());
        assert!(parse_from_input(&json!({})).is_none());
        assert!(parse_from_input(&json!({"conclusions": "not-array"})).is_none());
        // 全部条目被丢弃 = 无结构化产物。
        assert!(parse_from_input(&json!({"conclusions": [{"claim": " "}]})).is_none());
    }

    #[test]
    fn parse_only_open_questions_is_valid() {
        let input = json!({"open_questions": ["q"]});
        assert!(parse_from_input(&input).is_some());
    }

    // ---- 锚点校验(tempdir fixture,覆盖五臂) ----

    fn fixture_root() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gc-c2-anchor-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "l1\nl2\nl3\n").unwrap();
        std::fs::write(dir.join("no_eol.txt"), "only").unwrap();
        dir
    }

    #[test]
    fn validate_arms_ok_not_found_out_of_range() {
        let root = fixture_root();
        let mut d = detail_from(json!({
            "conclusions": [{
                "claim": "c",
                "anchors": [
                    {"path": "src/a.rs", "line": 3},
                    {"path": "src/missing.rs"},
                    {"path": "src/a.rs", "line": 99},
                    {"path": "src/a.rs", "line": 0},
                    {"path": "no_eol.txt", "line": 1},
                    {"path": "src"}
                ]
            }]
        }));
        validate_anchors(&mut d, Some(&root));
        let checks: Vec<_> = d.conclusions[0]
            .anchors
            .iter()
            .map(|a| a.check.unwrap())
            .collect();
        assert_eq!(
            checks,
            vec![
                AnchorCheck::Ok,
                AnchorCheck::NotFound,
                AnchorCheck::LineOutOfRange,
                AnchorCheck::LineOutOfRange, // line 0 越界(1-based)
                AnchorCheck::Ok,
                AnchorCheck::NotFound, // 目录按 not_found
            ]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn validate_absolute_path_inside_root_is_ok() {
        let root = fixture_root();
        let abs = root.join("src/a.rs").canonicalize().unwrap();
        let mut d = detail_from(json!({
            "conclusions": [{"claim": "c", "anchors": [
                {"path": abs.to_string_lossy().to_string(), "line": 2}
            ]}]
        }));
        validate_anchors(&mut d, Some(&root));
        assert_eq!(d.conclusions[0].anchors[0].check, Some(AnchorCheck::Ok));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn validate_outside_root_paths_zero_fs() {
        let root = fixture_root();
        // 存在的绝对路径(join 整体替换 base)→ OutsideRoot,不读内容。
        let etc_hosts = Path::new("/etc/hosts").canonicalize().unwrap();
        let mut d = detail_from(json!({
            "conclusions": [{"claim": "c", "anchors": [
                {"path": etc_hosts.to_string_lossy().to_string(), "line": 1},
                {"path": "../../../etc/passwd"}
            ]}]
        }));
        validate_anchors(&mut d, Some(&root));
        for a in &d.conclusions[0].anchors {
            assert_eq!(
                a.check.unwrap(),
                AnchorCheck::OutsideRoot,
                "path={}",
                a.path
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn validate_no_root_marks_unvalidated() {
        let mut d = detail_from(json!({
            "conclusions": [{"claim": "c", "anchors": [{"path": "a.rs", "line": 1}]}]
        }));
        validate_anchors(&mut d, None);
        assert_eq!(
            d.conclusions[0].anchors[0].check,
            Some(AnchorCheck::Unvalidated)
        );
    }

    #[test]
    fn count_lines_capped_semantics() {
        let root = fixture_root();
        std::fs::write(root.join("empty.rs"), "").unwrap();
        assert_eq!(count_lines_capped(&root.join("empty.rs")), Some(0));
        assert_eq!(count_lines_capped(&root.join("src/a.rs")), Some(3));
        assert_eq!(count_lines_capped(&root.join("no_eol.txt")), Some(1));
        assert_eq!(count_lines_capped(&root.join("missing")), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn wire_shape_is_snake_case() {
        let d = detail_from(json!({
            "conclusions": [{"claim": "c", "anchors": [{"path": "a", "line": 2, "check": "unvalidated"}], "stance": "disputed"}],
            "open_questions": ["q"]
        }));
        let s = serde_json::to_string(&d).unwrap();
        assert!(s.contains("\"open_questions\""));
        assert!(s.contains("\"disputed\""));
        assert!(s.contains("\"unvalidated\""));
        assert!(s.contains("\"line_out_of_range\"") == false);
    }
}
