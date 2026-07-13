---
name: wf-before-dev
description: in_progress 入口加载项目 spec 规范,确保按标准写代码
allowed-tools: []
---

# 写代码前加载 spec(in_progress)

## 必读
- `list_dir .everlasting/spec/` 看有哪些规范
- read_file 跟本次 task 相关的 spec(按包/层)
- 对照 spec 检查:命名 / 数据流 / 错误处理 / 测试约定

## 委托 implementer 前
- 把相关 spec 引用塞进 delegation message(让 implementer 也读)
- delegation 模板里有 `{summary}`,你填时带上"遵守 .everlasting/spec/xxx"

## 若 spec 缺失
- 按现有代码风格推断(读相邻文件)
- 本次 task done 时通过 wf-update-spec 把新发现的规范沉淀