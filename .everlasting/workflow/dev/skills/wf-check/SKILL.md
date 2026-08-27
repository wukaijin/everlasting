---
name: wf-check
description: 验收方法:lint/typecheck/测试/跨层一致性/spec 合规
allowed-tools: []
---

# 验收方法(in_progress 每项后)

## 验收维度
- lint(cargo clippy / eslint 等,按项目)
- typecheck(cargo check / tsc)
- 测试(相关单测 + 集成测试)
- 跨层一致性(前后端数据流、Rust↔TS interface、wire shape)
- spec 合规(对照 `.everlasting/spec/`)

## 委托 checker
- delegation 填本次验收对象(哪一项 / 全量)
- checker 只读 + shell,产出验收报告

## 不通过怎么办
- 报告具体问题(文件:行 + 描述 + 建议修法)
- 在 in_progress 内继续修(主 LLM 派 implementer 修,修完再派 checker 验收,直到 PASS)