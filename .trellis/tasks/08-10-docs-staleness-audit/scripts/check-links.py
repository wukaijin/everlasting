#!/usr/bin/env python3
"""docs 活文档相对链接完整性检查:目标文件存在 + 锚点可解析。

扫描范围:根目录 5 个 md + docs/ 顶层 md + docs/IMPLEMENTATION/*.md +
docs/WORKFLOW-INTEGRATION/*.md(与审查范围一致的"活文档")。
规则:
- 相对链接(含 `./` / `../` / `../../` 前缀)按所在文件目录解析,目标必须存在;
- 带 `#anchor` 的链接,anchor 按 GitHub 风格 slugify 后必须命中目标文件某个标题的 slug;
- 外部链接(http/https/mailto)与纯 `#` 文件内锚点(按同一文件标题校验)同样处理;
- 退出码 0 = 全部通过;非 0 = 有失败(打印 file:line 明细)。

用法:python3 .trellis/tasks/08-10-docs-staleness-audit/scripts/check-links.py [--quiet]
"""
import os
import re
import sys
import unicodedata

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))))
QUIET = "--quiet" in sys.argv

SCAN = [
    "AGENTS.md", "CLAUDE.md", "README.md", "STRUCTURE.md", "THIRD_PARTY_LICENSES.md",
]
for sub in ("docs",):
    d = os.path.join(ROOT, sub)
    SCAN += [os.path.join(sub, f) for f in sorted(os.listdir(d)) if f.endswith(".md")]
for sub in ("docs/IMPLEMENTATION", "docs/WORKFLOW-INTEGRATION"):
    d = os.path.join(ROOT, sub)
    SCAN += [os.path.join(sub, f) for f in sorted(os.listdir(d)) if f.endswith(".md")]

LINK_RE = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
HEADING_RE = re.compile(r"^#{1,6}[ \t]*(.*?)[ \t]*$")

def slugify(anchor: str) -> str:
    """GitHub 风格标题 slug:小写;保留字母/数字/下划线/连字符/空格,其余剥离;空格->'-'。"""
    out = []
    for ch in anchor.lower():
        if ch in " -_":
            out.append(ch)
        elif ch.isalnum():
            out.append(ch)
    return "".join(out).replace(" ", "-")

_HEADINGS_CACHE = {}

def _strip_fences(lines):
    """只去掉围栏代码块(标题检测用;行内代码 span 保留,避免破坏含代码的标题)。"""
    out = []
    in_fence = False
    for line in lines:
        if line.lstrip().startswith("```"):
            in_fence = not in_fence
            out.append("\n")
            continue
        if in_fence:
            out.append("\n")
            continue
        out.append(line)
    return out

def _strip_code(lines):
    """链接扫描用:围栏代码块 + 行内代码 span 都替换为占位(代码示例不是正文链接)。"""
    return [re.sub(r"`[^`]*`", "", line) for line in _strip_fences(lines)]

def _headings_of(path: str) -> set:
    if path in _HEADINGS_CACHE:
        return _HEADINGS_CACHE[path]
    res = set()
    try:
        with open(path, encoding="utf-8") as f:
            lines = f.readlines()
        for line in _strip_fences(lines):
            m = HEADING_RE.match(line.rstrip("\r\n"))
            if m and m.group(1):
                res.add(slugify(m.group(1)))
    except OSError:
        pass
    _HEADINGS_CACHE[path] = res
    return res

def main() -> int:
    failures = []
    for rel in SCAN:
        path = os.path.join(ROOT, rel)
        if not os.path.isfile(path):
            failures.append(f"{rel}: FILE MISSING FROM SCAN LIST")
            continue
        base_dir = os.path.dirname(path)
        with open(path, encoding="utf-8") as f:
            raw_lines = f.readlines()
        lines = _strip_code(raw_lines)
        for lineno, line in enumerate(lines, 1):
            for m in LINK_RE.finditer(line):
                target = m.group(1).strip()
                if target.startswith(("http://", "https://", "mailto:", "javascript:")):
                    continue
                if target.startswith("#"):
                    # 纯文件内锚点:拆出 anchor 对同一文件校验
                    anchor = target[1:]
                    heads = _headings_of(path)
                    if anchor and slugify(anchor) not in heads:
                        failures.append(f"{rel}:{lineno}: 死锚 #{anchor}")
                    continue
                # 去掉 <...> 包裹与 title 部分
                if target.startswith("<") and ">" in target:
                    target = target[1:target.index(">")]
                target = target.split(' "')[0].split(" )")[0]
                if " " in target:
                    target = target.split(" ")[0]
                file_part, _, anchor_part = target.partition("#")
                if not file_part:
                    resolved = path
                else:
                    resolved = os.path.normpath(os.path.join(base_dir, file_part))
                if not os.path.isfile(resolved) and not os.path.isdir(resolved):
                    failures.append(f"{rel}:{lineno}: 目标不存在 {target} -> {os.path.relpath(resolved, ROOT)}")
                    continue
                if anchor_part:
                    heads = _headings_of(resolved)
                    if slugify(anchor_part) not in heads:
                        failures.append(
                            f"{rel}:{lineno}: 死锚 {target} (heading slug 未命中: {slugify(anchor_part)!r})"
                        )
    if failures:
        print(f"FAIL: {len(failures)} broken link(s)")
        for f in failures:
            print("  " + f)
        return 1
    if not QUIET:
        print(f"OK: {len(SCAN)} files scanned, all relative links resolve")
    return 0

if __name__ == "__main__":
    sys.exit(main())
