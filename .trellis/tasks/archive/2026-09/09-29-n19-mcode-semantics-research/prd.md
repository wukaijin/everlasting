# N19 调研:mcode 语义吸收小件包(edit CJK 保真/diff 限界/SIGTERM 宽限/父进程死亡守卫/并行批取消配对)

## Goal

按 BACKLOG 附录 C.2 立项前调研要求,核验 mcode 调研衍生的五条语义(edit_file fuzzy CJK 保真 / edit·diff 大小限界 / 后台 shell SIGTERM 宽限 / 父进程死亡守卫 / 并行只读批取消配对)与本仓库现有实现的差距,产出「现状缺口 + 改动面 + 取舍」三段式结论,回填附录 C,为 N19 是否立项及范围裁定提供依据。

## Requirements

- 逐条代码实读核验(非只读 spec),每条判定附文件:行号证据:
  - edit_file 匹配路径有无 fuzzy/归一化(NFKC 破坏 CJK 的链路是否存在);
  - edit/diff 全链路大小限界(工具本体 / git diff 后端 / 前端 word-diff / LLM 输出面);
  - 前台 shell + 后台 shell 全部终止路径的信号语义(取消/超时/kill/kill_all);
  - 后台 shell 的父进程死亡守卫(spawn 参数 / daemon shutdown 链 / registry 持久性)与崩溃场景推演;
  - 两条取消路径(send 阶段 / 执行阶段)的 tool_use↔tool_result 配对与 wire 层自愈行为;
- 结论回填 `docs/BACKLOG.md` 附录 C N19 行。

## Acceptance Criteria

- [x] `research/n19-semantics-gap-analysis.md` 产出三段式结论(现状缺口 / 改动面 / 取舍),每条判定附代码级证据(文件:行号)
- [x] 五件各自给出实施/不实施建议与改动面量级,含取舍论证(件③ RULE-E-002 设计权衡、件④缺口 A/B 分层、件⑤层次对齐)
- [x] BACKLOG 附录 C N19 行回填调研结论

## 结论(供立项裁定)

- **件① CJK 保真:缺陷形态不存在**——edit_file 纯精确字节匹配(match_indices/replacen),无 fuzzy 路径、零 Unicode 归一化;pi-mono 的 NFKC 全文件归一化链第一环就不存在。降级为 1 条 CJK 往返保真测试锚。
- **件② diff 限界:三层结构天然规避**——edit_file 无 diff 计算;git diff 走 libgit2 C 实现 + untracked 64KiB cap;前端 MAX_PAIR_LEN 4000/MAX_PAIR_RUNS 200(N7 已做);输出面 C6 截断。残余(tracked patch to_buf 无 per-file 上限)低风险,记注不实施。
- **件③ SIGTERM 宽限:真实缺口**——全部终止路径直 SIGKILL 进程组(RULE-E-002),trap 清理逻辑无机会执行。收口 = kill_and_collect 两段式(SIGTERM → 3s 宽限 → SIGKILL),前台后台同改,~30-50 行。
- **件④ 父进程死亡守卫:真实缺口且比预想实**——daemon graceful shutdown 链(server.rs:482-536)**漏 kill_all**(kill_all 只挂在 GUI Full 的 RunEvent::Exit),Thin/sidecar/daemon.sh 的 SIGTERM 优雅退出也孤儿化后台 shell;崩溃面零守卫且超时计时器随进程死。缺口 A(shutdown 链补 kill_all,~10-20 行)为本任务主体;缺口 B(崩溃面 lease/PDEATHSIG)记注等 N11。
- **件⑤ 取消配对:形态存在但有下游自愈**——serial 执行中途取消只落库部分真 result、剩余悬空(tools.rs:912 break + finalize_turn:1926),靠 wire 层每 turn 注入 synthetic 兜住不 400(08-06 群聊事故修复);send 阶段取消已全量补齐(drive.rs:1993)。收口 = finalize_turn 取消臂补齐差集(~20-40 行),层次对齐 + wire 自愈回归真异常本职。

**立项建议范围**:件④缺口A 主体 + 件③ + 件⑤ + 件①测试锚,单 PR 小-中任务;件②与件④缺口B 记注不实施。改动面 = daemon/server.rs + tools/shell.rs + background_shell/in_memory.rs + chat_loop/tools.rs + edit_file 测试锚;spec 落点 tool-contract(RULE-E-002 语义升级注记)+ LIFECYCLE 取消语义段。前置依赖:无。

## Notes

- 调研产物:[research/n19-semantics-gap-analysis.md](./research/n19-semantics-gap-analysis.md)
- 本任务是 research 任务,不含代码改动;实施任务立项后另行创建。
