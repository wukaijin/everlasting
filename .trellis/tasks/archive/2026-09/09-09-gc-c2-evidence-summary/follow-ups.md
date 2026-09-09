# Follow-ups(live 评审场 026b420c 挖出,2026-09-09)

live 验证场(议题=评审 C2 实现本身)收官 9 条结论全锚点实证。其中 1 项(注释失真)当日修复(`89a187bf`);以下 5 项为真实打磨候选,P2 级,不阻塞本任务验收:

1. **✓ 记号对 file-only 锚点语义过强**:line=None 只验了文件存在,但渲染成与 line 级验证相同的 ✓——「最强记号用在最弱验证上」。修法:渲染层区分 `path ✓`(存在)与 `path:line ✓`(行级),或 wire 加 `ok_file_only` 变体;Rust+JS 双侧同步。
2. **unvalidated 与未校验视觉同形**:渲染器 `_`/bare 臂把「查过但查不了」与「没查过」折叠——两者是不同信号(detail 数据层已区分,纯渲染层问题)。
3. **Rust↔JS 渲染器防御不对称**:空 claim Rust render 不跳(parse 已滤,但 render 无再校验)vs JS 跳;未知 stance Rust 整条丢 vs JS 归一 inferred。双实现演化漂移的天然风险,可考虑共享 JSON fixture 对拍测试兜底。
4. **canonicalize 错误粒度**:路径不存在/权限拒绝/软链断裂一律 NotFound;权限拒绝语义上应 Unvalidated(match io::ErrorKind)。
5. **claim 含换行破坏 markdown 列表**:双侧渲染器都不转义;LLM 输出换行时 list item 断裂,渲染前 replace \n → 空格。

观察项(非缺陷):本场 open_questions[0] 文本混入工具调用式标记残片(`</item>`/`<<commit>` 字样)——moderator 输出 glitch,落库为惰性文本无执行面;若再现可查 moderator 收束 prompt。

# 任务工件勾账

- [x] Phase 1 后端(1.1-1.7 全)
- [x] Phase 2 前端(2.1-2.3)
- [x] Phase 3 JS(3.1-3.2)
- [x] Phase 4 文档(4.1-4.3)
- [x] Phase 5 门禁 + live(5.1-5.2)
- [x] AC1-AC6 全过(AC6 live:session 026b420c,9 结论/22 锚点/抽查 3 吻合/MCP 源 wire 验证)
