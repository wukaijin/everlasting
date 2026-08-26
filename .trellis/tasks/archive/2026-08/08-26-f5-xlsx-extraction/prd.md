# PRD — F5 follow-up:xlsx 原生提取(CSV 形态)

> 2026-08-26。父任务:`08-26-f5-doc-reading`(已归档,PRD D1 明确「xlsx 表格→文本需单独形态设计」后增量)。本任务收口该 follow-up。**pptx 用户裁定不做**。

## 背景

F5 已落地 PDF + docx 原生提取(`agent/doc_extract.rs` + at_file Degraded 前分流)。xlsx 是表格场景主力(WPS/Excel 导出),当前走 Office Degraded 占位(pandoc 指令兜底),对 LLM 价值低。

## 决策记录

- **D1 范围**:`.xlsx` + `.xlsm` 内置提取;`.xls`(OLE2 老格式)与 `.ods` 保持 Degraded 占位(与 `.doc` 同姿态);**pptx 不做(用户裁定)**。
- **D2 库**:calamine 0.36(纯 Rust,1160 万下载)。依赖树已核验:其 zip 请求 `default-features=false, features=["deflate"]`,与项目「zip 仅 deflate、无 zstd-sys C 构建」契约完全一致;无 C 系依赖;chrono 已是直接依赖,启用 calamine 的 chrono feature 零新增 crate。沿用 F5 姿态:catch_unwind 包裹(turn 不死硬约束),错误只进 tracing。
- **D3 表格形态**(用户三选一拍板):**每 sheet 一段 CSV 块**,RFC4180 转义(含逗号/引号/换行的单元格加引号);sheet 标题行 `## <名> (<R>行×<C>列)`;空 sheet 输出 `(空)`;每行去尾随空单元格。token 最省、转义无歧义。
- **D4 caps**:复用三级 cap 零改动(20MiB 源字节解析前 fail-fast / 150k 字符保头截断);截断文本继续走既有 D10 同请求 span → 关卡⑤硬卡。`units` = sheet 数,marker 属性 `sheets="N"`(wire 仍只带 format/chars/truncated)。
- **D5 单元格渲染**:字符串原样;数字最短表示(Rust f64 Display);布尔 `true/false`;错误值保留 `#REF!` 等;公式取缓存值(calamine 默认);日期经 chrono 转 ISO(`2026-08-26`,非零点补 ` HH:MM:SS`)。

## AC

1. `@xxx.xlsx` 注入 `<doc path kind="xlsx" sheets="N" chars=...>` marker,内容为 CSV 块;manifest 落 `{"kind":"extracted","format":"xlsx",...}`。
2. `.xlsm` 同路径路由;`.xls`/`.ods`/`.ppt*` 保持 Degraded 占位不变。
3. 损坏 zip / 非 xlsx 内容 fail-soft:Err → Degraded 占位,turn 不死,不 panic(catch_unwind 兜底)。
4. 超 20MiB 解析前拒绝;超 150k 字符保头截断 + marker `truncated="true" orig_chars=N`。
5. 前端 hint 行正确显示 `注入 N 字符(XLSX)`,截断徽标照旧。
6. 后端 cargo --lib 全绿(新增 doc_extract 单测 + at_file 集成测试);前端 vitest 全绿;clippy/fmt 零新增。
7. live 冒烟:真实样本 xlsx(中文表头/数字/日期/多 sheet)经 daemon 实跑,at_files_token 合理落值,daemon 存活。

## Out of Scope

- pptx(用户裁定)、pdfium 扫描件渲染(B1 通道 follow-up)、正式 document skill(B4 体系)。
