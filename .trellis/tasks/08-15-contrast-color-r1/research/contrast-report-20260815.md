# 定向视觉评审 · 色调与文字对比度(2026-08-15)

- 方法:token 对比度样本页(`specimen.html`,token 直读 `app/src/style.css`,真实字体/字号渲染,
  每格标注计算 WCAG 比值)+ `mmx vision` 分章节定性评审 + 数值复核三方交叉。
- 分工实证:**数值管亮度**(mmx 两轮均出现编造比值/读反方向,数字一律以计算为准);
  **VLM 管观感**(字号衰减、色相互斥、状态可辨性这类数值算不出的维度);
  **渲染验证**(首跑全页大图导致 VLM 幻觉,分章节截图后恢复正常——样本页必须分章节出图)。
- 产物:`specimen.html`(可离线打开)、`specimen-{a,b,c,d}.png`、`specimen-{a..d}.vlm.json`(mmx 原始返回)。

## 结论

### A · 灰阶(数值全过 AA,但感知分层明显)

- primary(13.0/12.0/10.9)与 secondary(7.7/7.0/6.4)全部无压力。
- **muted 档 + 11px mono 是全场感知最弱组合**(数值 5.3-6.3 过 AA,双 VLM 独立判读"有停顿感/
  需要凑近")——字号惩罚真实存在。修法:高频注视的元数据(侧栏分组头、时间戳、工具卡 meta)
  升 secondary,muted 只留给一次性角标;不建议再全局提 muted(8-05 刚提过一档)。
- muted × accent-muted = 4.41 FAIL,双证据确认。

### B · 彩色文字(VLM 观感 vs 数值,两处校正)

| 组合 | 数值 | VLM 观感 | 采信 |
|---|---|---|---|
| accent 作文字 × surface/elevated | 3.14/2.87 FAIL | "最暗一档、边缘发糊" | 一致,确认 |
| tool-error × accent-muted/elevated | 3.60/4.32 FAIL | "暗红撞深蓝最差" | 一致,确认 |
| tool-thinking × accent-muted | 4.98 过 AA | "深紫沉入蓝底" | **数值过线但观感成立**:紫与蓝底同色系,亮度算不出色相贴近,彩底上避免紫字 |
| status-warn × accent-muted | 8.13(最高档) | "发暗像泥土色" | **数值优先**:亮度充足,记为"黄蓝互斥"待真机确认,不据此改值 |

### C · 交互态(新发现)

- **hover(6% primary wash)与 default 差异"几乎隐形"**,active(10%)更弱。这推翻了此前把
  "无 hover 提示"全归因于静态截图盲区的判断——hover 叠加本身偏弱,建议 6%→10-12%,
  active 相应 10%→14-16% 一并抬。
- selected(12% accent)区分度足够,认可现状。
- VLM 建议(采纳进后续规范):给 hover/active 定"最小可辨 ΔL"量化规范,不再纯靠肉眼。

### D · 现状 vs 候选(mmx 逐组裁决)

| 组 | 修法 | mmx 裁决 | 最终 |
|---|---|---|---|
| D1 | 链接/强调 accent→#7c9aff | 采纳,亮度克制无荧光感 | **采纳**(P1) |
| D2 | 错误文案 #ef4444→#f87171 | 采纳;黑底组微有荧光感,可再议降半档 | **采纳**(P1),真机复核荧光感 |
| D3 | 彩底元数据 muted→secondary | 采纳,收益小但稳 | **采纳**(P2,立"彩底最低 secondary"规则) |
| D4 | elevated 提亮 #1d2433 | **改值再议**:"只动背景是半步方案" | **降级缓议**:D4 动机本是卡片边界感知(border×elevated=1.05)而非文字可读,VLM 按文字可读评它确属收益有限;与 border 体系问题合并到"层级感知"专题再议,或两端一起调 |

## 最终行动清单(取代上轮优先级表)

1. **P1** 新增 `--color-accent-text:#7c9aff`,替换 20+ 处 accent 文字(含 markdown 链接)——三方证据。
2. **P1** 新增 `--color-tool-error-text:#f87171` 给错误文案(填充/左条仍用 #ef4444)。
3. **P2** 彩底上最低 secondary 规则 + memory 卡片定点修。
4. **P2** hover 叠加 6%→10-12%、active 10%→14-16%(本轮新发现,量级待真机确认)。
5. **P2** 11px mono 元数据高频位 muted→secondary 定向提升。
6. **P3** elevated/border 层级感知专题(含 D4),另启。
7. 规范沉淀:最小可辨 ΔL(hover/active)、彩底禁紫字/禁 accent 蓝字。
