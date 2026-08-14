#!/usr/bin/env bash
# scripts/ui-review.sh — UI 视觉评审流水线(headless Chromium 截图 + mmx vision 评审)。
#
# 源起 2026-08-14 前端体验评审的手工流程沉淀:daemon serve dist → headless
# Chromium 截 7 个界面(桌面聊天 / 设置 / 记忆 / trace / 侧栏 / 移动端聊天 /
# 移动端配对)→ 每张图过 mmx vision(MiniMax VLM)做结构化视觉评审 →
# 汇总成 report.md。以后任何 UI 样式改动后跑一遍,当"视觉回归评审"用:
# 对比新旧 report 的缺陷清单是否收敛。
#
# 全程只读(浏览现有 session / 打开弹窗再 Esc 关闭),不发消息、不写 DB。
#
# 前置:
#   - daemon 在跑且 serve 最新 dist(./scripts/daemon.sh bg --no-build)
#   - mmx CLI 已安装且已登录(mmx auth status)
#   - ~/.cache/ms-playwright/ 下有 Chromium(装过任一 playwright 版本即有)
#   - node + npm(playwright-core 首次运行自动装到 scratch 目录,不污染 app 依赖)
#
# 用法:
#   ./scripts/ui-review.sh                      # 全流程:截图 + VLM 评审
#   ./scripts/ui-review.sh --port 7456          # 指定 daemon 端口
#   ./scripts/ui-review.sh --screenshots-only   # 只截图不调 VLM(不花 quota)
#   ./scripts/ui-review.sh --out-dir out/ui-review/manual   # 指定输出目录
#
# 产物(默认 out/ui-review/<时间戳>/,gitignored):
#   01-chat-desktop.png … 07-pairing-mobile.png   截图
#   <name>.vlm.json                                每张图的 VLM 原始返回
#   report.md                                      汇总评审报告
#
# 维护提示:截图脚本里的选择器(sidebar__settings 等)若因组件重构失效,
# 更新下方 shoot.mjs 顶部的 SELECTORS 段即可。
set -euo pipefail

PORT=7456
SCREENSHOTS_ONLY=0
OUT_DIR=""
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# playwright-core + shoot.mjs 的 scratch 目录(独立于 app/,避免污染 pnpm 依赖)
SCRATCH="${HOME}/.cache/everlasting-ui-review"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --port) PORT="$2"; shift 2 ;;
    --screenshots-only) SCREENSHOTS_ONLY=1; shift ;;
    --out-dir) OUT_DIR="$2"; shift 2 ;;
    *) echo "未知参数: $1(见脚本头注释)" >&2; exit 1 ;;
  esac
done

BASE_URL="http://localhost:${PORT}"
if [[ -z "$OUT_DIR" ]]; then
  OUT_DIR="${REPO_ROOT}/out/ui-review/$(date +%Y%m%d-%H%M%S)"
fi
mkdir -p "$OUT_DIR"

# ── 前置检查 ────────────────────────────────────────────────────────
if ! curl -sf -m 3 "${BASE_URL}/api/v1/health" >/dev/null; then
  echo "✗ daemon 未在 :${PORT} 响应。先拉起并 serve 最新 dist:" >&2
  echo "    ./scripts/daemon.sh bg --no-build" >&2
  exit 1
fi

if ! command -v mmx >/dev/null 2>&1; then
  echo "✗ 未找到 mmx CLI(评审阶段需要;截图-only 可用 --screenshots-only)" >&2
  exit 1
fi

CHROME_EXE="$(find "${HOME}/.cache/ms-playwright" -maxdepth 4 -type f -name chrome -path '*chromium*' 2>/dev/null | sort -V | tail -1 || true)"
if [[ -z "$CHROME_EXE" ]]; then
  echo "✗ ~/.cache/ms-playwright 下没有 Chromium。装一次 playwright 浏览器即可:" >&2
  echo "    npx playwright install chromium" >&2
  exit 1
fi

# playwright-core 装到 scratch(纯 JS 包,不下载浏览器;已装则跳过)
if [[ ! -d "${SCRATCH}/node_modules/playwright-core" ]]; then
  echo "▸ 首次运行:安装 playwright-core 到 ${SCRATCH}"
  mkdir -p "$SCRATCH"
  (cd "$SCRATCH" && npm init -y >/dev/null 2>&1 && npm i playwright-core --no-audit --no-fund >/dev/null 2>&1)
fi

# ── 截图(playwright-core 驱动 headless Chromium)──────────────────
cat > "${SCRATCH}/shoot.mjs" <<'SHOOT_EOF'
// 由 scripts/ui-review.sh 生成;独立跑需 OUT_DIR / BASE_URL / CHROME_EXE 三个 env。
import { chromium } from "playwright-core";

// 组件重构后若截图失败,先更新这里的选择器(与组件 scoped class 对齐)。
const SELECTORS = {
  settingsBtn: "button.sidebar__settings",
  memoryBtn: "button.chat-panel__memory-btn",
  traceBtn: "button.chat-panel__trace-btn",
  sidebar: "[class*=sidebar]",
};

const OUT = process.env.OUT_DIR;
const BASE = process.env.BASE_URL;
const browser = await chromium.launch({
  executablePath: process.env.CHROME_EXE,
  args: ["--no-sandbox", "--disable-gpu", "--force-color-profile=srgb"],
});

const shot = (page, name) => page.screenshot({ path: `${OUT}/${name}.png` });
const waitSettled = (page) => page.waitForTimeout(2500);

// 桌面视口:主聊天界面 + 三个弹窗/面板(打开 → 截图 → Esc 关闭)
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
await page.goto(BASE, { waitUntil: "domcontentloaded" });
await waitSettled(page);
await shot(page, "01-chat-desktop");

for (const [name, sel] of [
  ["02-settings", SELECTORS.settingsBtn],
  ["03-memory", SELECTORS.memoryBtn],
  ["04-trace", SELECTORS.traceBtn],
]) {
  try {
    await page.click(sel, { timeout: 3000 });
    await page.waitForTimeout(800);
    await shot(page, name);
    await page.keyboard.press("Escape");
    await page.waitForTimeout(300);
  } catch (e) {
    console.log(`${name} skip: ${sel} ${e.message.split("\n")[0]}`);
  }
}

// 侧栏单独截(截元素,不含主内容区;侧栏折叠时图会较窄,属正常)
try {
  const sidebar = await page.$(SELECTORS.sidebar);
  if (sidebar) await sidebar.screenshot({ path: `${OUT}/05-sidebar.png` });
  else console.log("05-sidebar skip: no sidebar element");
} catch (e) {
  console.log(`05-sidebar skip: ${e.message.split("\n")[0]}`);
}

// 移动视口(390×844,PWA 手机场景):聊天 + 配对页
const mob = await browser.newPage({ viewport: { width: 390, height: 844 } });
await mob.goto(BASE, { waitUntil: "domcontentloaded" });
await waitSettled(mob);
await shot(mob, "06-chat-mobile");
await mob.goto(`${BASE}/pairing`, { waitUntil: "domcontentloaded" });
await mob.waitForTimeout(1500);
await shot(mob, "07-pairing-mobile");

await browser.close();
console.log("screenshots done");
SHOOT_EOF

echo "▸ 截图中 → ${OUT_DIR}"
OUT_DIR="$OUT_DIR" BASE_URL="$BASE_URL" CHROME_EXE="$CHROME_EXE" \
  node --experimental-vm-modules "${SCRATCH}/shoot.mjs"

if [[ "$SCREENSHOTS_ONLY" -eq 1 ]]; then
  echo "✓ 截图完成(未调用 VLM):${OUT_DIR}"
  exit 0
fi

# ── VLM 评审(每张图一段针对性 prompt;mmx 输出 JSON,python3 提取)────
vlm_review() {  # $1=截图文件名(不含扩展名) $2=prompt
  local name="$1" prompt="$2"
  local json="${OUT_DIR}/${name}.vlm.json"
  if mmx vision describe --image "${OUT_DIR}/${name}.png" \
      --prompt "$prompt" --output json --quiet > "$json" 2>"${OUT_DIR}/${name}.vlm.err"; then
    python3 - "$json" <<'PY_EOF'
import json, sys
with open(sys.argv[1]) as f:
    print(json.load(f)["content"])
PY_EOF
  else
    echo "(VLM 调用失败,详见 ${name}.vlm.err)"
  fi
}

declare -A PROMPTS
PROMPTS[01-chat-desktop]="你是资深 UI/UX 设计师。评审这张深色主题 AI 聊天应用桌面端截图,覆盖:布局与信息层级、配色对比度、排版(字号/行高/密度)、间距对齐一致性、组件细节。列出至少 3 条具体优点和至少 4 条具体缺陷(缺陷注明位置)。中文回答,简洁分点。"
PROMPTS[02-settings]="你是资深 UI/UX 设计师。评审这张深色主题应用的设置弹窗截图:表单/列表的易用性、视觉层级、控件密度、对齐、可读性。列出至少 4 条具体问题(带位置)和 2 条优点。中文回答,简洁分点。"
PROMPTS[03-memory]="你是资深 UI/UX 设计师。评审这张 AI agent 记忆管理面板截图:信息分组、层级、可读性、操作可达性。列出至少 3 条具体问题和 2 条优点。中文回答,简洁分点。"
PROMPTS[04-trace]="你是资深 UI/UX 设计师。评审这张 AI agent 调试 trace 面板截图:数据可视化方式、信息密度、结构设计、可读性、与对话的可关联性。列出至少 4 条具体问题(带位置)和 2 条优点。中文回答,简洁分点。"
PROMPTS[05-sidebar]="这是应用左侧边栏截图(可能处于折叠状态)。评审:会话列表的视觉层级、选中态辨识度、分组与密度。列出至少 2 条具体问题和 1 条优点。中文回答,简洁分点。"
PROMPTS[06-chat-mobile]="你是资深移动端 UI/UX 设计师。评审这张 390px 宽的移动端深色聊天界面截图:触控目标大小(44px 规范)、单手可达性、信息密度、是否只是桌面压缩版。列出至少 4 条具体问题(带位置)和 2 条优点。中文回答,简洁分点。"
PROMPTS[07-pairing-mobile]="你是资深移动端 UI/UX 设计师。评审这张移动端设备配对页截图:第一印象、引导清晰度、输入体验、品牌感。列出至少 3 条具体问题和 2 条优点。中文回答,简洁分点。"

REPORT="${OUT_DIR}/report.md"
{
  echo "# UI 视觉评审报告"
  echo ""
  echo "- 时间:$(date '+%Y-%m-%d %H:%M:%S')"
  echo "- 目标:${BASE_URL}(评审基线 = 当时 dist)"
  echo "- 截图与逐图 VLM 原始返回见本目录"
  echo ""
} > "$REPORT"

for name in 01-chat-desktop 02-settings 03-memory 04-trace 05-sidebar 06-chat-mobile 07-pairing-mobile; do
  [[ -f "${OUT_DIR}/${name}.png" ]] || continue
  echo "▸ VLM 评审 ${name}"
  {
    echo "---"
    echo ""
    echo "## ${name}"
    echo ""
  } >> "$REPORT"
  vlm_review "$name" "${PROMPTS[$name]}" | tee -a "$REPORT"
  echo "" >> "$REPORT"
done

echo ""
echo "✓ 评审完成:${REPORT}"
