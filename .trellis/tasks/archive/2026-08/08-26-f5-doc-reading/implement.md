# Implement — F5 PDF/Office 文档阅读(@文件原生注入)

> 前置:design.md §1-§7。执行前先走 `trellis-before-dev`(backend + frontend spec)。

## 有序清单

### PR0 spike 闸门(D4 判据落地)

- [x] 1. 样本制备:headless Chromium(playwright 缓存 chromium-1217)打印中文 HTML 成 PDF(`out/f5-spike/sample-zh.pdf`,out/ 已 gitignore)+ `shared-mime-info-spec.pdf` 英文长文档 + 位图页扫描件(Chromium 截图 PNG 再嵌页打印,`sample-scan2.pdf`);pdftotext 输出对照。
- [x] 2. spike 实测结论(2026-08-26,**D4 判定:PASS → 采用 pdf-extract,不升级 pdfium**):① 中文 CJK 零乱码零丢字,段落/表格/代码块全保留;② 英文长文档 33,957 chars vs pdftotext 34,104,语义等价;③ 扫描件(位图页)返回 0 字符 → "<32 字符判扫描件"判据成立;④ 已知瑕疵:英文字距拆词("Ag en t",Chromium kerning)→ 提取后处理合并单字母间距序列;⑤ `extract_text` 无换页符,页数经 lopdf(pdf-extract 传递依赖,零新增)读 `/Pages /Count`,若 API 不便 marker 退化为只报字符数。spike 工程留在 `out/f5-spike/`(不进 git)。

### PR1 后端提取 + at_file 分流

- [x] 3. 依赖落定(按 PR0 结论):`pdf-extract`(或 `pdfium-render`)+ `zip` + `quick-xml`(传递树转直接);`Cargo.lock` 提交。
- [x] 4. 新模块 `agent/doc_extract.rs`:`ExtractedText { text, pages|paras, orig_chars, truncated }` + `try_extract_pdf(bytes)` / `try_extract_docx(bytes)` 纯函数;`MAX_EXTRACT_SOURCE_BYTES = 20 MiB`(字节前置 cap)/ `MAX_EXTRACT_CHARS = 150_000`(截断);docx 魔数 PK 前置。单测:构造 fixture(见 #7)+ corrupt 输入 Err 路径。
- [x] 5. `at_file.rs` 分流:`FileKind::Pdf` → try_extract_pdf(空/近空 <32 字符 = 扫描件 → Degraded);`ext == .docx` → try_extract_docx;成功 → 文本 span 注入 + marker(`[pdf: a.pdf, N 页 M 字符(已截断,原文 K)]` 形态)。`InjectionAction::Extracted { kind, chars, pages, truncated }` additive 变体(wire serde snake_case)。既有两条占位测试改写为成功 + 降级双路径。
- [x] 6. 占位文案升级(design §5 表):`expand_for_kind` 三类文案指令式 + 前端 hint 同步文案语义(实现放 PR2 但后端文案此 PR 定稿)。
- [x] 7. fixture 进 git(小体量):文本型 PDF(英文,手写最小 PDF 或 Chromium 生成后截 1 页)、扫描件 PDF(空文本层——Chromium 打印纯图片页)、docx(python/zip 手工构造一次提交)、corrupt 样本。CJK fixture 体积大则放 PR0 结论决定(本地生成脚本进 scripts/)。

### PR2 前端注入态 + hint

- [x] 8. `FileInjectionsHint.vue`:消费 `Extracted` 变体——pdf/docx 行显示"已注入 N 页/M 字符"+ 截断徽标;`Degraded` hint 行改指令式文案;旧 manifest(无新变体)容错回退。vitest:新变体渲染 / 截断徽标 / 旧数据回退。

### PR3 收尾验证

- [x] 9. 全量:`cargo test -p everlasting --lib` + clippy + fmt + `pnpm test` + `pnpm build`;既有 image/text 通道测试零改动通过(AC6)。
- [x] 10. live 冒烟:重编 daemon → `turn-smoke.sh` 单轮含 `@样本.pdf` 引用(at_files 落值,AC1/AC8);手工:扫描件降级文案(AC2)、@docx(AC3)、超限截断(AC4)。
- [x] 11. spec 沉淀:@文件提取注入契约(agent-loop 或 tool-contract 定位)+ ROADMAP F5 行标注落地范围 + IMPLEMENTATION 决策日志。

## 验证命令

```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cargo clippy -p everlasting --all-targets && cargo fmt --check
cd app && pnpm test && pnpm build
./scripts/turn-smoke.sh            # live 冒烟(含 @pdf 样本)
```

## 风险文件与回滚点

| 文件 | 风险 | 回滚 |
|---|---|---|
| `agent/at_file.rs`(分流 + 文案) | 注入主路径,1367 行热点 | 分流点独立于 Image/Text 分支,revert 不伤既有通道 |
| `agent/doc_extract.rs`(新) | 独立模块,零耦合 | 整文件删除 |
| `Cargo.toml`/`Cargo.lock` | pdf-extract/pdfium + zip 依赖面 | 依赖 revert(注意 pdfium 路线含分发脚本) |
| `FileInjectionsHint.vue` | manifest 消费 additive | 新变体分支独立,回退渲染即回滚 |

## task.py start 前检查

- [x] prd/design/implement 三件套齐且相互引用一致
- [x] 内联工作流(trellis-before-dev 加载上下文),JSONL 门不适用
- [x] 用户已明确批准最终规划摘要(brainstorm 2026-08-26 拍板 D1-D6,范围与分层用户确认"开任务")
