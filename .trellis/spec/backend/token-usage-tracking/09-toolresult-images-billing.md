<!-- Moved from token-usage-tracking.md 2026-09-13 (doc-split) -->

## Scenario: 工具图计费 — ToolResult.images 内联 tokens_est(08-21-b1-image-followups,2026-08-21)

`read_file` 读图返回的 `AttachmentRef` 挂在 `ToolResult.images`(块级字段),不在消息级
`attachments` 清单里 —— `estimate_images_token` 的块扫描对它按内联 `tokens_est` 精确累加
(None 才垫 1600),与 user 文本图的「pad 先加、清单替换」路径互不 double-count(工具图不是
独立 Image 块)。resolve pass 把 `images` 重建为成功加载子集,故 post-resolve 计费 = 实发。
budget 裁剪臂 2 同步覆盖旧轮工具图(resolved+images 双清,`images_freed` 经 estimate 差值
自然计入)。live 实测:1440×900 截图 tokens_est=1728(=(w×h)/750 精确),随 tool_result 进
第二次请求时 `turn_trace.images_token=1728` 入账。
