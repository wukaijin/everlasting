# group-chat-mcp 部署面契约(standalone bin)

> 任务 09-06-gce-mcp-standalone(2026-09-06)。消费方文档:DAEMON-API §6.1 部署面小节;根因与探针实证:任务 `research/bun-compile-feasibility.md`。

## 1. Scope / Trigger

MCP server(`scripts/group-chat-mcp.mjs`)有两种等价运行形态:**node 直连**(开发态默认,改 .mjs 即生效)+ **bun compile standalone bin**(部署态,免 node / 免 node_modules / 免源码检出)。本契约管部署面;工具语义契约归 M2 design(archive)。

触发 code-spec 深度:新增 CLI 命令(deploy)+ 宿主配置文件写入(integration)。

## 2. Signatures

- CLI:`node scripts/group-chat-mcp-deploy.mjs [--revert | --uninstall] [--config <path>]`
  - 三模式:默认 deploy(build→install→config)/ `--revert`(配置回 node 挂载,bin 保留)/ `--uninstall`(删配置项 + 删 bin);`--revert` 与 `--uninstall` 互斥
  - flag 值拒绝 `--` 开头(防 `--config --revert` 吞 flag:把 flag 当路径,白构建 98MB 后在 cwd 落同名垃圾文件)
- bin 落点:`${XDG_DATA_HOME|~/.local/share}/dev.everlasting.app/bin/everlasting-group-chat-mcp`;sidecar `<bin>.build-info`(行 1 = git rev-parse --short,行 2 = ISO 时间戳;诊断 stale bin)
- 编译 entry:`scripts/group-chat-mcp-standalone-entry.mjs`(**唯一**合法 entry;禁止直接拿 group-chat-mcp.mjs 当 entry,理由见 §7)
- 纯函数区(零副作用,供 node --test):`mergeServersConfig` / `removeServerEntry` / `readServerEntry` / `buildRevertEntry` / `parseArgs` / `expandHome`
- bun 定位:PATH `bun` → miss 再试 `~/.bun/bin/bun`

## 3. Contracts

- 配置形状:`~/.zcode/cli/config.json` 的 **`mcp.servers["everlasting-group-chat"]`**(真实形状是 mcp 嵌套——任务文档曾误记顶层 `servers`;部署器两种容器都兼容,优先 mcp.servers)。bin 挂载体 = `{ command: <bin 绝对路径> }`(**无 args**);node 挂载体 = `{ command: 'node', args: [<scripts>/group-chat-mcp.mjs] }`。
- 写盘:读-改-写;变更前备份 `<config>.mcp-deploy.bak`(**单份覆盖**);新文与现文**逐字节一致时跳过写与备份**——首跑备份才是真回滚点,幂等重跑绝不 clobber;JSON 损坏抛错不写盘(报错带文件路径)。
- 保留语义:其他顶层键、其他 server 原样;uninstall 删项后空容器不剪枝(保键)。
- 记账 XDG state(`~/.local/state/dev.everlasting.app/mcp-discussions.json`)双形态共享,互通即兼容,不做隔离。

## 4. Validation & Error Matrix

| 条件 | 行为 |
|---|---|
| bun 不存在(PATH + `~/.bun/bin/bun` 均 miss) | 中文报错 + 安装指引(https://bun.sh),exit 1 |
| 配置文件 JSON 损坏 | 报错带路径,**不写盘**(绝不覆盖用户手工配置) |
| flag 值以 `--` 开头 | 报错拒收(吞 flag 防御) |
| `--revert` 与 `--uninstall` 同现 | 报互斥 |
| 构建中断/失败 | 产物走 `<bin>.tmp-<pid>` + rename,正式路径永不出现半成品 |

## 5. Good / Base / Bad Cases

- **Good**:deploy 连跑两次 → 第二次「已一致,未触盘」,`.bak` md5 前后不变(幂等不 clobber 备份)。
- **Base**:配置缺失/空白 → 建 `{ mcp: { servers: {} } }` 骨架(真实宿主形状)。
- **Bad**:以 mcp.mjs(或静态 import 引擎的入口)直接 compile → 产物 usage 横幅打上 stdout 协议通道 + exit(0),server 永远起不来(§7)。

## 6. Tests Required

- `node --test scripts/group-chat-mcp-deploy.test.mjs`(纯函数:merge 保键/幂等、骨架与双容器兼容、损坏拒绝、revert/uninstall 语义、parseArgs 守卫)
- `node scripts/group-chat-mcp-smoke.mjs [--bin <path>]` —— 双形态**同构**断言链(spawn + tools/list 六工具 + wire 预算 3200 + handler 错误链);node 直连必须持续为默认回归路径
- 免 node 实证:`env -i HOME=... PATH=/usr/bin:/bin` 下 bin 完成 MCP handshake(自包含)
- AC 级守门:`git diff --stat -- scripts/group-chat-run.mjs scripts/group-chat-mcp.mjs app/` 必须恒为空(引擎与 Rust/前端零改动)

## 7. Wrong vs Correct:bun compile 下 isMain 守卫恒真(核心 gotcha)

**Wrong**(直接以 mcp.mjs 为 entry,或 entry 静态 import 引擎):

```js
import { createServer } from './group-chat-mcp.mjs'; // 静态 import 提升到任何代码之前求值
```

bun compile 下**所有打包模块的 `import.meta.url` 一律指向可执行文件自身**(内嵌 bunfs 抹平模块身份),而 `process.argv[1]` 也是可执行路径 → `group-chat-run.mjs` CLI 壳守卫(`argv[1] === fileURLToPath(import.meta.url)`)恒真 → 以空参数跑 CLI main → usage 横幅污染 stdout(协议通道)→ `process.exit(0)` 杀死 server。

**Correct**(`scripts/group-chat-mcp-standalone-entry.mjs`,引擎一行不改):

```js
process.argv[1] = '/nonexistent-everlasting-mcp-standalone'; // 哨兵:必在动态 import 之前
const { createServer } = await import('./group-chat-mcp.mjs'); // 顺序不可倒
```

约束:哨兵赋值与动态 import 的**顺序是本文件唯一硬约束**(静态 import 会提升到哨兵前);未来把其他 `scripts/*.mjs` 打 standalone 时同样适用「哨兵入口 + 动态 import」模式,不走 `--define` bun CLI 不支持下标键。
