<!-- Moved from token-usage-tracking.md 2026-09-13 (doc-split) -->

## Scenario: images_token — request-total image slice (B1, 2026-08-17)

`turn_trace.images_token`(PR4,B1 `08-16-b1-image-multimodal`)与 tools_token / memory_token 同语义的第三切片:**请求内全部图片块的 token 估算**,含历史重建(历史图每轮随请求重发、每请求计费——只算当轮新图会系统性低估,评审 P0-1)。

- **口径**:`estimate_images_token(&turn_messages)` 在 `drive.rs` 的 per-turn 请求 clone 上、resolve 之后计算——Σ 每图 `tokens_est`(attach 时 `(w×h)/750`,前端 FileReader 读粘贴图、后端 `imagesize` crate 读 @图文件头),缺失回退 1600/图垫板。写入点与 tools/memory 同一 Done upsert(`!skip_persist` gate,worker 轮 None)。
- **When this bites**:估算是"字段优先"——`attachments` 字段的精确值**替换**垫板贡献而非叠加;若某消息的 Image 块数与 attachments 数不一致(理论上 attach pass 保证 1:1),会以字段为准。live 实测(08-17):800×600 png → 640 tok 精确落值,无图轮 = 0。
- TurnCard `img` cell 门是 `> 0`(tools/mem 是 `!= null`)——无图轮不渲染噪声 cell。
