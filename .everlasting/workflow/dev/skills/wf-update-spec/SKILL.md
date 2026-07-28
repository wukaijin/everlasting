---
name: wf-update-spec
description: task done 时把决策/坑/新 pattern 提炼进 .everlasting/spec/
allowed-tools: []
---

# 沉淀 spec(done)

## 何时沉淀
- task done 时(Rust 固定 hook 会触发你加载本 skill)
- 只沉淀**可复用**的:新 pattern / convention / 踩坑 + 修复 / 技术决策
- 不沉淀 task 一次性细节(那个进 progress.md)

## 沉淀到哪
- `.everlasting/spec/<package>/<layer>/index.md` + 具体 guideline 文件

## 格式
- 自由 markdown,不强制结构
- 标题 + 场景 + 规范 + 反例(若有)

## 质量
- 不强求每次都沉淀(没东西可沉淀就不写)
- 无效沉淀(空/重复)接受为代价——长跑后有用的会被引用,无用的沉底