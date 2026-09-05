---
name: group-chat
description: "跨模型群聊审议驱动:用 scripts/group-chat-run.mjs 一条命令召集一场多 agent 讨论(评审/架构决策/复盘)拿回有据可查的共识结论。Use whenever 用户要召集群聊/审议/评审团/多模型讨论/复盘会,或说 group-chat / 审议 / 评审团 / 跨模型讨论,或在一个议题值得多视角交叉验证时。注意成本:一场 5-15 分钟、数十万 token,慎用。"
---

# 群聊审议(group-chat)

一个调用方(你)说清**议题 + 目录 + 参与角色**,拿回一份有共识清单的转录。
引擎是 `scripts/group-chat-run.mjs`(确定性);本 skill 只负责判断与组装,**编排逻辑永远不自己手搓 curl**——生命周期语义锁在脚本里。

## 先过成本闸(必读)

一场审议 = 5-15 分钟 + 数十万 token。发起前自问:
- 这个议题值得吗?**单一事实问题不要审议**(直接查代码/文档);**有多视角权衡、结论影响后续走向**的才值得。
- 议题里的未知能不能先自己消掉?把「X 是什么」消掉,只留「X 该怎么选」给审议。

## 建群三要素(全靠脚本内省,别背参数)

```bash
node scripts/group-chat-run.mjs projects   # ① 目录:审议对象的项目(证据基地)
node scripts/group-chat-run.mjs models     # ② 模型:当前可用清单(UUID/名字都收)
node scripts/group-chat-run.mjs presets    # ③ 配方:review/arch/retro + 覆盖语法
```

**目录(--project)是第一要素**:参与者能查什么证据由它决定,默认当前目录。
跨仓库议题选主仓库;--topic 里可以点名相对路径。

## 发起

```bash
node scripts/group-chat-run.mjs run \
  --project /path/to/repo \
  --preset review \                # 评审团(架构+产品+后端);单决策点用 arch;复盘用 retro
  --topic-file /tmp/topic.md \     # 主推:长议题/含引号都走文件(短议题可 --topic 内联)
  --timeout 1800                   # 默认 30min;超时自动 cancel 并导部分转录
```

`run --help` 看全量参数。覆盖语法:`--set name.model=<id>` / `--set name.persona=@file`(单人两级)、`--participants '<json>'`(整名单替换——增删参与者的唯一方式)、`--moderator-model <id>`。
**在别的 cwd 跑时用脚本绝对路径**;转录固定落 everlasting 仓库 `out/`,结束时会打印绝对路径。
长跑建议后台化(如 `&` + nohup),轮询脚本输出拿转录路径与退出码;进度粒度 10s 一拍(±10s),没有发言级实时进度。

### 议题写法(质量杠杆,两场 live 实证)

- **给背景**:相关文件/文档路径、已尝试过什么、约束是什么——参与者第一轮就在证据上,不瞎猜。
- **给角色分工提示**:点名各参与者从什么视角切入(如「请产品从用户场景开刀」)。
- **指定产出形态**:「收官时给出一份简短的共识清单 + 未决项」——moderator 的 end_discussion 会照此产出 summary。
- **别把答案写进问题**:议题里预设结论(「请论证 X 是对的」)会让整场讨论围着你的框架偏见转;问「X 该怎么选」,别问「X 为什么好」。
- 反面教材:「大家觉得怎么样」——会得到一篇散文。

## 读结果

- 退出码:`0` 正常收官 / `2` 轮次帽(30 轮)截断 / `3` 被取消 / `4` 连续错误熔断 / `1` 脚本自身错(同样导出部分转录,现场不丢)。
- 转录 markdown:头部有阵容/时长/token/`discussion_summary`(共识清单一等字段,优先读它;缺失时头会有 ⚠️ 警告行),正文 `seqN **speaker**: > 正文`(blockquote 隔离),工具轮带 `工具调用 name{参数}` 证据链。
- session 默认保留(可在 GUI 复盘整场;`--cleanup` 仅成功路径删)。

## 边界

- 本 skill 不做:打断/注入/实时跟随(M3 之前讨论不可驾驶,发起前把议题写全)。
- 无人值守安全:无 GUI 观察者时权限请求 8s 快拒,参与者会自己绕路——议题里给的路径要真实存在,减少无效审批。
- 嵌套消费(在 daemon 会话里由 agent 驱动本脚本):shell 沙箱禁网会拦脚本的 daemon 连接。脱沙箱路径(2026-09-06 live 实证):①脚本错误文案已带 `Operation not permitted (EPERM)` 签名,沙箱升级分类器能识别;②对会话预授权 shell prefix(注意:prefix 授权按**命令首词的 basename** 匹配——存 `node`,不是脚本全路径,全路径是死数据);③命令**务必裸写**,加重定向/`&&` 等组合符授权即失效。满足后沙箱首跑失败会自动无沙箱重跑,零人工。**双发警告**:升级重跑会生成新的后台句柄——原句柄显示 Failed 不代表任务失败,等新句柄/查 session 列表,**不要手动重发**(live 实证:手动重发导致两场审议并发跑,双倍成本)。
