# PRD:群聊预设 Settings 可编辑/可新增(GCE-P1)

## 背景

群聊预设(preset)目前单一事实源是仓库文件 `scripts/group-chat-presets.json`(内置四档:review / fe_review / arch / retro),被 M1 CLI、MCP server、前端(ScheduledTasksTab + GroupChatConfigModal)三处消费。预设按**模型名**引用目录,目录改名会断链(实证:commit 7773b927,deepseek-v4-flash → deepseek-flash,预设全链路哑火,修复要动 JSON + 两个镜像断言测试)。预设置是代码工件而非用户数据:用户既修不了也定制不了。

本任务 = 评估结论的 P1(GUI 闭环)。用户已确认三个方向性决策:

1. **存储:DB 新表**(subagent/models 先例),不用 app-data JSON 文件。
2. **模型引用:用户预设存 UUID**(本机数据,改名下稳定;7773b927 类断链从构造上消灭;内置预设保持名字引用不动)。
3. **内置四预设只读**(run.test 镜像断言 / MCP 帮助文本 / 跨机器可移植性依赖 JSON 原样),用户预设叠加其上。

## 需求

### R1 用户预设的 CRUD(Settings 内)

- Settings 新增"群聊预设"管理页(global scope):列出用户预设(名称/描述/主持人/参与者摘要),可新增、编辑、删除。
- 表单字段:预设名、描述(可选)、主持人模型(下拉,来自模型目录)、参与者 2-3 人(每人:名字 + 模型下拉 + persona 类型下拉,限定内置 5 种:arch / product / backend / frontend / outsider)。
- 内置四预设在管理页只读展示(名称 + 描述),作为参照;不可编辑、不可删除。

### R2 校验(保存时前后端双保险)

- 预设名:trim 后非空、≤40 字符;与其它用户预设名大小写不敏感不重名;与内置 key(review/fe_review/arch/retro)大小写不敏感不撞名。
- 主持人与全部参与者模型必须存在于模型目录(允许已禁用模型保存,使用处已有禁用反诊 UX)。
- 参与者 2-3 人(沿 GroupChatConfigModal MVP 边界);参与者名字 trim 非空、预设内不重名。
- persona 必须是 5 种内置 kind 之一(MVP 不支持自定义 persona 文本)。
- 服务端(commands 层)强制同样规则;前端校验只为即时反馈。

### R3 两个既有消费方吃到用户预设

- ScheduledTasksTab(定时审议表单)与 GroupChatConfigModal(群聊弹窗)的 preset 选择区:内置四档之后追加用户预设(展示名区分,如标注"自定义");用户预设的模型引用按 UUID 解析,选中即预填阵容 + 主持人默认,行为与内置档一致(含解析失败/禁用模型的既有警告 UX)。
- 提交语义不变:仍在表单提交时展开为 UUID 阵容快照,DB 与 daemon 的任务配置不留预设引用(快照语义,见 R4)。

### R4 快照语义保持 + 出处字段(为未来留门)

- 编辑/删除用户预设**不回溯**影响已建定时任务(现有"未重选 preset = 存档配置继续生效"语义保持)。
- 定时任务 config 新增可选 `preset_key` 字段(创建/更新任务时若选了预设则记录;daemon fire 路径完全忽略该字段),为未来"预设已更新,重选可应用"提示留门。
- 删除用户预设无守卫(快照语义下任务自包含,不悬空)。

## 非目标(明确出栈)

- MCP server / M1 CLI 消费用户预设(P2,另立任务;本任务不动 scripts/)。
- 自定义 persona 文本(P3;MVP 限 5 种内置 kind)。
- overwrite 语义(预设编辑回溯更新已建任务)——已评估否决;仅通过既有"重选 preset"显式生效。
- 内置预设的编辑/覆盖。
- stale 提示 UI(比对存档阵容 vs 当前预设展开)——仅在收尾时成本允许才做,可砍。

## 验收标准

- [ ] AC1:Settings 能新增用户预设并出现在列表;重启应用(重查 DB)仍在。
- [ ] AC2:能编辑(改名/换模型/换 persona/改参与者)与删除;列表即时反映。
- [ ] AC3:校验规则全量生效——重名/撞内置 key/模型不存在/参与者数越界/persona 非法均被服务端拒绝(HTTP 400 类错误),前端有可读提示。
- [ ] AC4:ScheduledTasksTab 与 GroupChatConfigModal 的 preset 选择区出现用户预设;选中后预填/主持人默认/警告行为与内置档一致;提交建群/建任务成功且任务 config 为 UUID 快照。
- [ ] AC5:新建定时任务的 config 含 `preset_key`(选了预设时);daemon fire 路径行为不变(既有 scheduler 测试全绿)。
- [ ] AC6:内置四预设只读;`scripts/` 全部测试零改动零回归。
- [ ] AC7:Rust 新增 db 冒烟测试 + 路由 oneshot wiring 测试 + 命令校验测试;前端新增 tab 组件测试 + merged 映射单测;`http.routes-sync.test.ts` 守卫过。
- [ ] AC8:docs/DAEMON-API.md、docs/DEBUG_DB.md schema 索引、AGENTS.md 群聊段落同步。
