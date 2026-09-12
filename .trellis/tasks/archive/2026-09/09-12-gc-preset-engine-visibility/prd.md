# GCE-P2: M1/MCP 引擎侧用户预设可见性——daemon 运行时拉取

## Goal

让 M1 CLI(`scripts/group-chat-run.mjs`)与 MCP server(`scripts/group-chat-mcp.mjs`)
在运行时经 daemon HTTP 拉取 `group_chat_presets` 表(用户行 + P1b 内置档覆盖行),
与烤进模块图的内置 JSON 四档合并消费——收掉 spec「边界(P2 未做)」的引擎可见性缺口:
用户在 Settings 改的阵容/覆盖,经 MCP(外部消费主路径)与 M1 发起的审议从此可见。

## Requirements

1. **合并语义与前端 mergedPresets 镜像**(GCE-P1/P1b 定案的引擎侧对偶):
   - 用户行:追加为新档,key = 行 id(UUID);moderator/participants 直接用 UUID。
   - 覆盖行(builtinKey 非空):原位顶替内置槽——key/name 保持内置 key,
     阵容换行内容;不追加新键。
   - 未知 builtinKey 的脏行(未来 JSON 删 key 的存量):跳过,不当用户档追加。
   - persona kind 经 JSON personas + persona_common 展开(与 composePresets 同构)。
2. **预设引用三趟解析**(与 normalizeModelRef 同构):key 直配(内置 key / 用户行
   id)→ 用户行 name 精确 → name 大小写不敏感;DB 校验保证内置 key 与用户 name
   无撞名空间。失配报「可用清单」。
3. **降级**:daemon 不可达 → 拉取失败降级为内置四档(fail-open),stderr 警告;
   此时引用用户预设的报错必须提示「用户预设拉取失败」而非含混的「预设不存在」。
   零 daemon 场景(非 live 冒烟、离线 dry-run)不回归。
4. **MCP 新增 `list_presets` 只读内省工具**(对齐 09-11 list_models 先例):宿主
   发现用户预设 key/UUID 的唯一 MCP 通道;返回合并视图(内置四 key 恒在 +
   degraded 标记)。
5. **MCP start_discussion.preset schema 从 enum 四档放宽为 string**(动态 schema
   否决:buildToolShapes 是无 daemon 环境消费的同步纯函数),校验移到运行时
   (coreStart 既有未知预设报错路径)。
6. **standalone bin 免重部署**:内置四档仍以烤进 bin 的 JSON 为唯一来源;
   用户行/覆盖行运行时拉取;deploy 脚本零改动。
7. **`presets` 子命令展示合并视图**:内置(含「已覆盖」标记)/ 用户档(key +
   管理名 + 阵容),daemon 不在降级内置四档。

## Constraints

- scripts/ 纯 JS 侧改造;**Rust / daemon / 前端 / deploy 脚本零改动**
  (HTTP 路由 P1 已就绪,GUI mergedPresets 已消费)。
- 定时任务 fire 语义不变(快照语义:preset_key 纯出处、fire 零读取)。
- wire 预算锁纪律:八工具实测后定锁值;若升锁须同步四处(常量注释 / mcp.test
  断言 / smoke BUDGET / spec),实测值记入注释——本 PRD 即「升锁须过评审」的
  评审材料,task start 视为放行。
- `--dry-run` 语义收窄:「零网络」→「不建 session、不发 LLM」;本地 daemon
  元数据拉取允许,失败降级内置四档(help 文案同步)。
- mock deps 默认 `listPresets → []`:既有 coreStart 单测零扰动。

## Acceptance Criteria

- [ ] AC1 mergePresets 纯函数:用户行追加(key=id)/ 覆盖行原位顶替(key/name
      保持内置 key)/ 未知 builtinKey 跳过 / persona_md 展开 / 脏行结构错误
      fail-loud throw / 空行集 = 内置不变(单测锁)。
- [ ] AC2 M1:`run --preset <uuid|name|builtinKey>` 正确展开(覆盖档吃到覆盖后
      阵容);`--dry-run` 支持用户预设;`presets` 子命令合并视图;三趟解析 +
      失配报可用清单。
- [ ] AC3 MCP:coreStart 消费用户预设/覆盖行(created_via=mcp 链路不变);
      preset schema 放宽为 string;list_presets 工具返回合并视图,内置四 key
      恒在、degraded 带标记。
- [ ] AC4 降级:daemon 停机时 M1/MCP 内置四档照常(非 live 冒烟不依赖 daemon
      的性质保持);引用用户预设的报错含降级提示(单测锁)。
- [ ] AC5 wire 预算:八工具实测 ≤ 锁值;四处同步完成;实测值记录。
- [ ] AC6 门禁:`node --test scripts/group-chat-run.test.mjs` /
      `group-chat-mcp.test.mjs` 全绿(基线 16+19 递增);MCP 非 live 冒烟全绿
      (八工具名);live 零 LLM 验证链:真 daemon 下 `presets` 见用户行/覆盖行、
      `run --dry-run --preset <用户档>` 全链、MCP `list_presets` 返回用户行。
- [ ] AC7 文档同步:AGENTS(GCE-P1 边界句 + M2 七工具句)/ DAEMON-API §6.4 /
      spec group-chat-presets.md(边界改定 + 引擎合流契约)/ group-chat skill /
      mcp-deploy spec(内置 JSON 烤 bin 语义不变 + 运行时增量)。

## Non-goals

- 自定义 persona 文本(P3)、overwrite 回溯(已评估否决)。
- daemon 侧把内置档入库;GUI 任何改动;定时 fire 路径。
- MCP 动态 tools/list schema(否决理由见 design)。
