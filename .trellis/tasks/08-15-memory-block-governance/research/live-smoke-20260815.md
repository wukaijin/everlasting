# Live 烟测验证记录(2026-08-15,daemon release 重编后实跑)

环境:本仓库 project;二进制含 PR1(be873d4 度量)+ PR2(8dafc71 digest);daemon 经 `scripts/daemon.sh start` 前台拉起(Bash 工具进程组会杀后台子进程,需常驻任务承载)。三轮 `turn-smoke.sh` + 两轮定向探针,全部真实 LLM 调用。

## AC1 基线(digest off:app_config.memory_digest_enabled=false)

| seq | tools_token | memory_token | ctx_input | tools_pct | mem_pct |
|-----|------------|--------------|-----------|-----------|---------|
| 1 | 3664 | **10124** | 13992 | 26% | **72%** |

- 实测基线 10124 高于 08-14 估算(~7-8k):cl100k 对 CJK 密集内容的实际计数 + banner/wrappers 开销。
- 双轮:第二轮 cache_read=13952/13991 = **99.7%**(全量注入下前缀缓存健康)。

## AC2 收益(digest on,默认;首轮无拉取)

| seq | tools_token | memory_token | ctx_input | tools_pct | mem_pct |
|-----|------------|--------------|-----------|-----------|---------|
| 1 | 3805 | **2080** | 7421 | 51% | **28%** |

- **memory 10124 → 2080(-79.5%)**,≤2500 目标达成;首轮 context 14079→7421(**-47%**)。
- tools 3664→3805(Δ141 = `load_memory_sections` def append,预期内;净收益仍 -6.6k)。
- 组成核对:AGENTS.md 全量 ~1.3k + CLAUDE.md 目录 + user 层 + banner/wrappers ≈ 2.1k,与预估一致。

## AC4 cache 不劣化(digest on,--turns 2)

| seq | cache_read | ctx_input | cache 率 |
|-----|-----------|-----------|---------|
| 1 | 128 | 7511 | 1.7%(首轮无缓存,正常) |
| 3 | **7494** | 7510 | **99.8%** |

- digest on 第二轮 99.8% vs digest off 99.7% — **不劣化**(略优:前缀更短,断点前内容更稳)。
- 此为"无拉取"场景;拉取节后的一次性 prefix miss 已由 design §3.4 论证(粘性注入后重新稳定)。

## AC3 行为代理验证(定向探针 ×2)

1. 「整体架构(进程模型)」→ 模型**未拉节**,直接作答 —— 合法:细节来自全量注入的 AGENTS.md(primary)+ 目录标题,无编造。
2. 「Tech Stack (Locked) 节锁定了哪些技术」→ 模型 thinking 明确识别 digest("Looking at the digest provided, item 9…")→ **主动调用 `load_memory_sections`** → 返回节全文 → 逐条作答。**渐进披露闭环成立**(目录可发现 → 按需拉取 → 遵循)。
- 边界:本代理为问答型;"改代码/跑命令型任务中规范遵循不退化"需日常使用观察(AC3 完整闭环留给真机使用,机制侧已验证)。

## 其他

- smoke session 全部经 delete_session API 清理(含 2 个 --keep 的);该路径同时验证了 digest registry 的清理接线(无 panic,删除正常)。
- 环境还原:app_config 无 memory_digest_enabled 键(回缺省 on);daemon 停止(初始即未运行)。
