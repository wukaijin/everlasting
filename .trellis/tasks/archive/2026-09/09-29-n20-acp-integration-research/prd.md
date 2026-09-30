# N20 调研:ACP (Agent Client Protocol) 接入——协议面映射与工作量评估

## Goal

评估把 everlasting 作为 ACP agent 接入编辑器(Zed 等)的可行性、改动面与取舍,回答用户问题「加 ACP 工作量大吗」并按 BACKLOG 附录 C.2 纪律产出「现状缺口 + 改动面 + 取舍」三段式结论,为 N20 是否立项及范围裁定提供依据。

## Requirements

- ACP 协议面盘点(v1 stable,2026-09 现状):方法全集(agent 侧/client 侧)、session/update 事件变体、关键枚举(StopReason / ToolCallStatus / PermissionOption)、能力协商降级面、传输与生命周期;以官方 schema v1 实测提取为准,非二手转述;
- Rust crate 生态核实(crates.io API):`agent-client-protocol` 官方 crate 存在性/版本线/维护活跃度,1.x(v1 stable)与 2.x(unstable)取舍;
- Zed 注册路径(外部 agent 配置格式 + 调试通道);
- everlasting 现状核对(代码实读,行号证据):SSE 事件面全集与粒度、chat 驱动端点、权限 ask 桥(permission:ask / permission_response)、cancel、session/project/mode API、evl CLI 外部客户端先例;
- 全量映射表:ACP 面 ↔ daemon API/SSE 逐项对照,标注映射难度与缺口;
- 结论回填 `docs/BACKLOG.md` 附录 C(N20 候选行 + C.2 调研条目闭环)。

## Acceptance Criteria

- [x] `research/acp-integration-analysis.md` 产出三段式结论(现状缺口 5 项 / 改动面 4 PR 拆解 / 取舍 8 项决策),ACP 侧以 schema v1 实测提取为据,everlasting 侧附 文件:行号 证据
- [x] 工作量定级有对照系(与 N2 checkpoint / daemon 化 epic 对照),架构路线(shim bin 连回 daemon,agent core 零改动)有 evl CLI 先例支撑
- [x] BACKLOG 附录 C 回填 N20 候选行 + C.2 调研条目闭环(链接本任务报告)

## 结论(供立项裁定)

- **架构路线**:新增 `everlasting-acp` shim bin(Zed spawn 的子进程,stdio JSON-RPC)连回 daemon HTTP/SSE——第五个客户端形态,agent core MVP 零改动;`evl chat` 已完整验证外部瘦客户端驱动一轮 turn 的全部通路(SSE 消费 + 权限应答 + 取消 + 终态判定)。
- **协议匹配度高**:权限选项 `allow_once/allow_always/deny` 与 ACP PermissionOption 同名同义;delta/thinking 均 token 级流直映射;fs/terminal/elicitation 走能力不声明的合法降级(零工作量);worktree 默认 none 态即直连 Zed 工作区目录(ARCHITECTURE.md:240-245),零语义冲突。
- **最大缺口**:在途 permission ask 无恢复面(pending_interaction 无 Permission 变体)——MVP 用「shim 启动即挂 SSE」规避,残余缝记 follow-up 小增量(~20-40 行 daemon 侧)。
- **Rust crate 选型问题消失**:官方 `agent-client-protocol` crate(475 万下载,2026-09-18 发 2.2.0),锚 1.x(v1 stable)。
- **工作量:中——4 PR,与 N2 checkpoint 同级**;体验降级仅 tool 无流式中间输出(两态跳变协议合法)。
- **立项建议**:P2,PR1 生命周期 / PR2 翻译层 / PR3 交互桥 / PR4 测试与 Zed 实测;前置依赖无。

## Notes

- 调研产物:[research/acp-integration-analysis.md](./research/acp-integration-analysis.md)
- 本任务是 research 任务,不含代码改动;实施任务立项后另行创建(建议 slug `n20-acp-shim-mvp`)。
