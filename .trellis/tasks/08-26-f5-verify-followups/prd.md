# F5 验证三问题修复:@面板缓存失效 / pdf-extract panic×abort / @引用视觉标识

## Goal

F5(PDF/docx 原生注入,08-26-f5-doc-reading)真机验证暴露的三个问题收口。三问题相互独立,按 P0→P1→P2 分层;诊断与证据链已完整(见 F5 task notes + journal Session 112),本任务直接进入修复,无需重新 brainstorm。

## Requirements

### 问题 1(P0):pdf-extract panic × `panic="abort"` 击穿 daemon

- **现象**:用户真实中文 PDF(WPS/Word 导出,含非嵌入 `STSong-Light` + `UniGB-UCS2-H` 编码)触发 pdf-extract 0.12 panic(`unsupported encoding`,pdf-extract lib.rs:986);release profile `panic = "abort"`(根 Cargo.toml)使 `doc_extract.rs` 的 `catch_unwind` 兜底失效(abort 模式不 unwind)→ **daemon 整进程 abort**。无注入提示(seq metadata 空)、无 LLM 请求、SSE 无 Done/Error → 前端流永久挂起。2026-08-26 03:25 实际发生,daemon 已手动重启恢复。
- **修复要求**(两条腿,都要做):
  1. **提取健壮性**:pdf-extract 0.12 对 `UniGB-UCS2-H`(及同类非嵌入 CJK CMap 编码)不得 panic。路径二选一(实现前拍板):① fork/patch pdf-extract(`[patch.crates-io]`),不支持的编码返回 Err;② 按 D4 预案升级 `pdfium-render`(工业级文本质量,连带解锁扫描件渲染 follow-up;注意四平台动态库分发,daemon.sh + Tauri bundler 两处)。
  2. **panic 隔离**:`panic = "abort"` 与「turn 不死」硬约束结构性冲突——不止 pdf-extract,任何第三方库 panic 都能杀 daemon。最小修:release profile 改回 unwind(评估二进制体积/lto 影响);或文档提取挪独立子进程(进程边界隔离,abort 也杀不死 daemon)。
- **解码层调研结论(2026-08-26,源码级)**:差距是**两层**,只修 panic 会落入更隐蔽的坑——
  - 层 1(panic):`pdf-extract/src/lib.rs:986` 的 `PdfCIDFont::new` 对 Name 形式的 /Encoding 只硬编码认 `Identity-H/V`,其余直接 panic(但它支持内嵌 CMap 流,走 `adobe_cmap_parser`);
  - 层 2(静默丢字):`decode_char` 只查 ToUnicode 表,miss 时返回空串。实测 `STSong-Light` **无 ToUnicode**(pdffonts uni=no)——只修层 1 的话该字体全部字符输出 "",比 panic 更难察觉;
  - **UCS2 家族有零资源精确解**:Adobe 规范中 `Uni{GB,CNS,JIS,KS}-UCS2-{H,V}` 的码值本身就是 UCS-2(Unicode)码位(2 字节大端直读即字符),无需任何外部 CMap 资源。patch 形态(~30 行 fork):① Name arm 接受 UCS2 家族 → codespace 2 字节 + cid identity + 置 ucs2 标志;② `decode_char` 在 ToUnicode miss 且 ucs2 时 `char::from_u32(code)` 直出;
  - **覆盖边界**:老编码家族(GB-EUC/B5pc/GBK-EUC 等)code≠Unicode,需真 CMap 资源才能解(pdfium 内嵌,pdf-extract 不带)——patch 路线下这些返回 Err 降级。pdftotext 实证本文件可完整恢复 13,153 字符。
  - 建议分层:P0 用 patch(快、零新依赖、精确覆盖 Word/WPS 中文导出主流形态)+ panic 隔离;pdfium 升级留给扫描件渲染 follow-up 档(该档本来就需要它)。
- **回归样本**:验证 fixture 必须含非嵌入 CJK 字体形态的 PDF(本次的 `/usr/local/code/typhoon/xxx.pdf` 可直接作为 fixture;spike 教训:Chromium 打印样本覆盖不到该形态)。

### 问题 2(P1):`@` 面板文件列表缓存不失效,同项目新放入文件不可见

- **现象**:typhoon 项目先放的 docx 能 `@` 到,后 copy 进项目根的 pdf 永远 `@` 不到。
- **根因**:两层缓存只进不出——`chatInputCodeMirror.ts` 的 `shallowLoaded` 标志(面板重开跳过重拉,`closeFilePalette` 注释明确保留缓存)+ `ChatInput.vue` 模块级 `fileCache`(按 projectId);唯一失效途径 = 切项目往返或整窗重载。后端 `files.rs` 头注释声称 "frontend re-fetches on each `@` open anyway" 与实际不符。
- **修复要求**:面板每次打开即重拉浅层 walk(3 层小 walk 毫秒级,保留 in-flight 去重防抖),或 fileCache 加短 TTL;同步修正 `files.rs` 错误注释。

### 问题 3(P2):`@` 文档引用视觉上像纯文本(输入框 + 发送后消息)

- **现象**:`@docx` 后输入框内无任何标识(`onFileSelect` 纯文本拼接插入,CodeMirror 无 Decoration 层);发送后气泡内 `@token` 无高亮,唯一标识是气泡下方 FileInjectionsHint 小字行(xt/次要色/mono,B2 PR3 有意低调)。数据链路完整非 bug,是视觉设计缺口。
- **修复要求**(分层,最低成本起步):
  1. 输入框:`@token` chip 化(CodeMirror ViewPlugin + `Decoration.mark`,背景/圆角/文件类型图标),palette 选中即可视;
  2. 气泡内:`@token` 高亮或附件卡片化(参照 `MessageImages` 形态:PDF/docx 图标 + 字符数);
  3. hint 行视觉升级(更大字号/图标徽章)——可与 2 二选一或叠加。
- **约束**:改样式后跑 `scripts/ui-review.sh` 视觉回归;注意 VLM 评审方法局限(静态截图看不见透明 hit area/hover,行高结论需代码复核)。

## Acceptance Criteria

> 回填说明(2026-08-26,子代理串行修复 + 主线终验):AC1/AC2 经单测 + live 双层验证;AC3/AC4 单测层验证(vitest DOM 断言),GUI 肉眼确认留给用户。实现要点:pdf-extract 0.12 vendor 进 `vendor/pdf-extract/`(workspace exclude + path 依赖,patch 处 `EVERLASTING PATCH` 标记);release profile 删 `panic="abort"`(unwind,二进制 +1.52MiB/+9.2%,catch_unwind 恢复生效);输入框 chip 复用既有 `cm-token-file` 装饰层升级 CSS(顺修 `/` 路径正则缺口),气泡内 @token 走渲染前行内 code 包裹 + `file-ref` 着色。

- [x] AC1(P0):`@` 含非嵌入 CJK 字体(`UniGB-UCS2-H`)的中文 PDF → turn 正常完成,daemon 存活;无论注入成功还是降级占位,均不得杀进程。(单测 `pdf_unigb_ucs2_cjk_extraction` 断言中文完整恢复;live:真实 xxx.pdf 经 daemon 实跑一轮,`at_files_token=13862`、output 652、daemon 存活——即 03:25 杀死 daemon 的同一输入)
- [x] AC2(P0):panic 隔离落地——修复后人为注入一个会 panic 的提取路径,daemon 不死、turn 走降级/错误兜底(验证 catch_unwind/子进程隔离真实生效,不再是死代码)。(单测 `pdf_unknown_cid_encoding_skips_font_without_panicking` **直接调用 pdf-extract 不经 catch_unwind** 证明未知编码不再 panic 而是跳过该字体;兄弟 panic/todo!/assert!/unwrap 收口为降级;profile 回 unwind 后 catch_unwind 真实生效)
- [x] AC3(P1):同项目内新 copy 的文件,`@` 面板重开即可见(无需切项目/重载窗口);面板打开期间快速连打 `@` 不产生重复 in-flight 请求。(vitest 5 新用例:重开重拉/in-flight 共享/失败不缓存空表/reset 丢弃迟到写回/system_root 仍会话缓存)
- [x] AC4(P2):`@` 文件选中后输入框内有肉眼可辨的 chip 标识;发送后消息有可辨的引用标识(高亮/卡片/升级版 hint 至少其一)。(vitest 13 新用例断言 chip class 与气泡 file-ref span;ui-review 7 截图无回归;chip 效果截图 fixture 不含 @token 态,最终观感以用户 GUI 确认为准。**2026-08-26 用户 GUI 实测回捞:CJK 文件名无色(pdf 有色、`@台风智能体文档.docx` 无色)—— 根因是 token 正则共三份且全是 ASCII `\w`(JS 的 `\w` 不匹配 CJK):输入框 FILE_RE、气泡包裹(同 FILE_RE)、气泡打 class 的 CODE_AT_TOKEN_RE(独立第三份)。补修:抽 `FILE_TOKEN_BODY` 单一常量(`\p{L}\p{N}` + `u` flag,字符集不窄于后端 `@([^\s@]+)`)三处共用,CJK 用例 2 个补进测试;1221/1221 过,dist 已重建**)
- [x] AC5:全量回归:cargo + vitest + vue-tsc 全绿;既有 image/text 注入通道零改动通过。(cargo --lib 1983 过/1 失败为 tunnel 时序 flaky 隔离复跑过、与改动无交集;vitest 1219/1219;vue-tsc 零错;pnpm build 过;clippy 自有代码零新增;**vendored 上游 29 条预存警告已用 crate 级 `[lints]` 声明式压制**(vendor/pdf-extract/Cargo.toml,path 依赖会被完整 lint 而 registry 版被 --cap-lints 静默,不压则淹没每次 clippy 输出);fmt 全绿含 vendored)
- [x] AC6:`files.rs` 头注释与前端实际刷新行为对齐(或前端行为改为注释描述的形态,二者取一,文档与代码一致)。(注释已改为:浅层列表每次面板打开重拉,仅 system_root 会话级缓存)

## Notes

- 诊断证据链:daemon.log(03:25 panic + 03:35 重启)、DB messages.metadata(seq=0 docx manifest 完整 / seq=2 空)、pdffonts 输出(STSong-Light UniGB-UCS2-H)。详见 08-26-f5-doc-reading task notes + journal-4.md Session 112。
- 问题 1 的库选型(patch vs pdfium)是本任务唯一需要预研拍板的点,其余两个问题方案已定。
- 三个问题独立成 PR,问题 1 优先(用户验证被阻塞)。
