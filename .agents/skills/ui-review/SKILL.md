---
name: ui-review
description: "UI 视觉评审流水线:全量回归(scripts/ui-review.sh 截 7 界面过 mmx vision)与主题定向审计(对比度样本页 + mmx vision 定性 + WCAG 数值三方交叉)。Use whenever 前端样式改动后要做视觉回归、用户提到 ui-review / 视觉评审 / 截图评审 / VLM 评审 / ui-review.sh,或要做配色、对比度、层级、间距类的主题审计,或调色后要验证缺陷是否收敛。"
---

# UI 视觉评审

两层流水线,先判断走哪条:

| 场景 | 走法 |
|---|---|
| 样式改动后的视觉回归(默认) | Flow A:全量 7 界面截图 + VLM 泛化评审,对比新旧 report 看缺陷收敛 |
| 主题性审计(对比度/配色/层级/间距/交互态) | Flow B:token 样本页系统性覆盖 + 定向 prompt 评审 + 数值复核 |

改动面小时只跑相关的那条;大改(换主题/动 token)两条都跑。

## Flow A · 全量回归

```bash
./scripts/ui-review.sh                # 前置:daemon serve 最新 dist + mmx 已登录
```

- 产物:`out/ui-review/<ts>/`(7 张截图 + report.md)。前置不满足时脚本会自检并给出提示。
- 评审基线是**当时的 dist**——改动后先确认 dist 比源码新,否则评的是旧界面。
- 拿新 report 与上一轮对比缺陷清单,确认收敛;未收敛项带入下一轮。

## Flow B · 主题定向审计(以对比度/配色为例)

### Step 1 · 盘点审计面

从 `app/src/style.css` 的 `@theme` 块列出该主题涉及的 token 组合。对比度主题 =
文字 token × 背景 token 的全矩阵 + 交互态叠加 + 彩色文字。真实截图只能覆盖
"恰好出现在当前界面上的组合",accent-作-文字、彩底-muted 这类 FAIL 组合在
Flow A 里时有时无——这是 Flow B 存在的理由。

### Step 2 · 生成样本页并截图

```bash
node scripts/ui-review-specimen.mjs --out out/ui-review/<ts>-<theme> --shoot
```

- token 直读 style.css(不会与真实 token 脱节);每格自动标注计算的 WCAG 比值;
  内嵌 HarmonyOS Sans SC 保证字体渲染与 app 一致;`--shoot` 输出全页图 +
  `specimen-{a,b,c,d}.png` 分章节图。
- 章节结构:A 灰阶×背景 / B 彩色文字×背景 / C 交互态叠加 / D 现状 vs 候选。
  改章节时同步改本 skill 的 prompt 段和脚本头部注释。
- 有候选修法时写进脚本的 `CANDIDATES` 段(D 章会渲染成并排对比)。**候选评审
  通过进了 style.css 之后,删掉对应 CANDIDATES 条目**——候选常驻会让 D 章
  变成无意义的自我对照。

### Step 3 · mmx vision 分章节定性评审

逐章节调用(prompt 要点:交代页面结构、**明确禁止读数值徽章**、只要定性观感):

```bash
mmx vision describe --image specimen-a.png --output json --quiet \
  --prompt "<该章的判读问题,只要观感>" > specimen-a.vlm.json
python3 -c "import json;print(json.load(open('specimen-a.vlm.json'))['content'])"
```

问对问题比泛化"评审这张图"重要得多。对比度主题的四个角度:A 章问"哪些 11px
小字发灰、哪个文字档位最吃力";B 章问"哪些彩色文字明显更暗/在彩底上吃力、
按可读性排序";C 章问"hover/default 差异可辨吗、哪几对易混";D 章逐组问
"候选是否改善成立、有没有过头、给采纳/改值/维持结论"。

### Step 4 · 数值复核(结论的裁判)

VLM 的数值一律不信(见下面"已知坑")。样本页上的比值徽章是算好的可直接用;
临时色对现算:

```bash
node -e 'function L(h){const c=[1,3,5].map(i=>parseInt(h.slice(i,i+2),16)/255).map(v=>v<=.04045?v/12.92:Math.pow((v+.055)/1.055,2.4));return .2126*c[0]+.7152*c[1]+.0722*c[2]};function C(a,b){const[x,y]=[L(a),L(b)].sort((p,q)=>q-p);return((x+.05)/(y+.05)).toFixed(2)};console.log(C("#8a93a8","#1e2a5e"))'
```

判定标准:正文 ≥4.5(AA),大字/图形 ≥3,AAA ≥7;`color-mix()` 叠加态算不了,
交给 VLM 观感 + 真机确认。

### Step 5 · 三方交叉写报告

写到 `out/ui-review/<ts>-<theme>/report.md`,结构照抄
`out/ui-review/20260815-contrast/report.md`(方法交代 → 分章结论 →
VLM 观感 vs 数值的冲突校正表 → 现状 vs 候选裁决 → 带优先级的行动清单)。

裁决规则:**数值管亮度,VLM 管色相观感,自己看图管渲染问题**。冲突时:
- VLM 说暗但数值 ≥7 → 不改值,记"色相互斥"待真机复核(实证:warn 黄在
  深蓝底 8.13 仍被评"泥土色")
- 数值 FAIL 但 VLM 没抱怨 → 仍算 FAIL 修掉(VLM 对慢疲劳不敏感)
- 数值过线但 VLM 说"沉底" → 采信观感(紫字在蓝底 4.98 过线仍难分离,
  亮度算不出色相贴近)

## VLM 已知坑(实证记录,别再踩)

1. **全页长图必幻觉**:下采样后文字成糊,模型把 AAA 格说成 FAIL、暗底说成
   浅底。样本页必须分章节出图;Flow A 的整屏截图是视口尺寸,没这个问题。
2. **VLM 会编造/读反数值**:两轮实证(编 2.94 实为 6.28、把"最深底对比度
   最高"说反)。所以数值自算,prompt 里禁它读数。
3. **静态截图盲区**:hover / 透明 hit area / 折叠分组内状态看不见(AGENTS.md
   已记录)。但**别把"看不见 hover"一律归为盲区**——C 章样本页可复现叠加
   强度,2026-08-15 就发现了 hover 6% 叠加本身过弱的真问题。
4. **行高/触控结论不可靠**:正文 1.6 行高曾被连续两轮判"行高过低"
   (AGENTS.md 实证)。此类结论需代码级复核后再采信。
5. Flow A 的泛化 prompt 会产出大量布局/密度意见——对主题审计是噪音,只挑
   与主题相关的条目进报告。

## 收尾

- 报告行动清单里的持久规则(如"彩底上最低 secondary"、"hover 最小可辨 ΔL")
  沉淀到 `.trellis/spec/frontend/design-tokens.md`(走 trellis-update-spec),
  不要只留在报告里——报告是 gitignored 的。
- 改完 style.css 后重跑对应 Flow 验证收敛;候选色采纳进 token 后同步更新
  `design-tokens.md` 的决策记录(该文件已有"Decision"段落惯例)。
