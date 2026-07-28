# `.everlasting/spec/` — 项目代码规范沉淀目录

> **职责**:本目录沉淀 **本项目怎么写代码** 的规范 — pattern、convention、踩坑 + 修复、技术决策。

## 为什么这个目录存在

dev workflow(`07-08-workflow-integration`)的闭环价值:task done 时把决策/教训提炼进本目录;下次 implement → `wf-before-dev` skill → 读 spec → 按规范写 → 又沉淀。

让 AI 每次都按标准写代码,而不是随机发挥。

## 结构约定

目录结构(`<package>/<layer>/index.md` + guideline 文件):

```
.everlasting/spec/
├── README.md                    # 本文件(目录约定说明)
└── <package>/                   # 按代码包划分,如 backend / frontend / docs
    └── <layer>/                 # 按代码层划分,如 agent / llm / memory / commands
        ├── index.md             # 层级索引(规范列表 + 状态)
        └── <topic>.md           # 单条规范文件(标题 + 场景 + 规范 + 反例)
```

**`index.md` 模板**:

```markdown
# <package> 开发规范

> 本目录沉淀 <package> 的代码规范。

## 规范索引

| 指南 | 描述 | 状态 |
|------|------|------|
| [topic-1](./topic-1.md) | 简短描述 | 何时沉淀 |
| [topic-2](./topic-2.md) | 简短描述 | 何时沉淀 |

## 何时读本目录

- implement 入口:加载 `wf-before-dev` skill,`list_dir .everlasting/spec/`
- check:对照本目录的规范做合规检查
- done:用 `wf-update-spec` skill 把本次决策/坑/新 pattern 写进来
```

**单条 guideline 模板**(自由 markdown,不强制结构):

```markdown
# <标题>

## 场景
<什么场景下需要遵守本规范>

## 规范
<具体规则>

## 反例(若有)
<不要怎么做 + 为什么>
```

## 写入流程

1. task 推进到 `done` state(Rust 固定 hook 触发)
2. engine 写入 `[wf:spec-distilled <ts>]` marker 到 `task.json.summary`
3. agent 在 done turn 加载 `wf-update-spec` skill,提炼本次决策/坑/新 pattern
4. agent 用 `write_file` 写 `.everlasting/spec/<package>/<layer>/<topic>.md`
5. agent 更新 `<package>/<layer>/index.md` 规范列表(加一行)

## 读取流程

1. implement state 入口 → `wf-before-dev` skill → `list_dir .everlasting/spec/`
2. read 相关 `<package>/<layer>/<topic>.md`
3. 按规范写代码

## 质量

- 不强求每次都沉淀(没东西可沉淀就不写)
- 无效沉淀(空 / 重复)接受为代价——长跑后有用的会被引用,无用的沉底
- 沉淀不强制结构(自由 markdown),便于 agent 表达