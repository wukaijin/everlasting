# Research: bun compile 打包 group-chat-mcp 可行性探针(2026-09-06)

> 任务:09-06-gce-mcp-standalone。探针为临时文件(`scripts/.probe-entry.mjs` + /tmp 产物),验证后已删,结论在此留档。

## 环境

- bun **1.3.6**(`/home/carlos/.bun/bin/bun`)已装;**deno 未装** → 工具链定 bun(路线图 §5 措辞 bun/deno 并列,bun 居首)。
- node v24.15.0(开发路径继续用 node,不受影响)。
- 现用户级挂载(`~/.zcode/cli/config.json`):`mcp.servers."everlasting-group-chat" = { command: "node", args: ["<repo>/scripts/group-chat-mcp.mjs"] }` —— **勘误(2026-09-06 checker)**:本行原记「顶层键是 `servers`」有误,真实形状是嵌套 `mcp.servers`(顶层键 = `mcp` + `plugins`;非 Claude Code 式顶层 `mcpServers` 这半句是对的)。部署器据此双容器兼容、优先 `mcp.servers`。

## 探针结果

`bun build --compile <entry> --outfile ...`:244 modules,**98MB**(bun 内嵌运行时的正常量级;--bytecode 可加速启动,体积同量级)。

对编译产物走 MCP stdio 全链(initialize → notifications/initialized → tools/list):

- ✅ initialize 返回 serverInfo `everlasting-group-chat 1.0.0`
- ✅ tools/list 六工具齐(start/status/result/cancel/interrupt/inject)
- ✅ stdout 零污染(协议通道干净),诊断走 stderr
- 备注:探针粗算 wire 用 python `json.dumps` 默认分隔符(带空格)得 3294,与 AC4 单测的计数口径不同;wire 预算地面真值仍是 `node --test scripts/group-chat-mcp.test.mjs` 的单测。

## 关键坑:bun compile 下 `import.meta.url` 语义变化

**现象**:直接把现有 `scripts/group-chat-mcp.mjs` 作为 entry 编译(或任何 import 了引擎的 entry),`group-chat-run.mjs` 的 CLI 壳横幅打到 stdout,随后 `process.exit(0)` 杀死 server。

**根因**:`group-chat-run.mjs:693` CLI 壳守卫 `process.argv[1] === fileURLToPath(import.meta.url)`;**bun compile 下所有打包模块的 `import.meta.url` 一律指向可执行文件自身路径**(内嵌 bunfs 抹平了模块身份),而 `process.argv[1]` 也是可执行路径 → 守卫恒真 → CLI main 以空参数跑 → usage() + exit(0)。

**解法(已验证,引擎零改动)**:部署入口在静态 import 求值前不安全(import 提升在代码前),所以入口**先改写 `process.argv[1]` 为哨兵路径,再动态 import 引擎与 SDK**——守卫恒不等,CLI 壳不跑:

```js
process.argv[1] = '/nonexistent-everlasting-mcp-standalone'; // 见上注释
const { createServer } = await import('./group-chat-mcp.mjs');
```

- 动态 import 顺序不可倒:必须先设哨兵再 import。
- 引擎 `group-chat-run.mjs` **一行不改**(守卫语义在 node 直连/测试下不受影响)。
- 备选方案(未采用):bun `--define 'process.argv[1]:...'`(CLI 报 define key 含 `[1]` 非法,不支持下标键);引擎守卫加 env 豁免(动共享引擎,违背路线图「JS 单实现保留、M2 纯逻辑零改动」)。

## 引擎可打包性

- **零仓库路径依赖**:MCP 路径的转录根 = `full.session.current_cwd || entry.cwd || tmpdir`(mcp.mjs:172);`REPO_ROOT`(run.mjs:32)只作 M1 CLI 转录默认根,编译产物内为死值不触达。
- PRESETS/DEFAULT_BASE 内置常量;daemon 调用走全局 fetch(bun 原生支持);node:fs/os/path/process/url 全兼容。
- SDK(`@modelcontextprotocol/sdk` 1.30)+ zod 4 均被 bun bundle 正常收入(244 modules)。
- 记账 XDG state 路径 `~/.local/state/dev.everlasting.app/mcp-discussions.json` 与 node 直连共享——同机 node/bin 两形态并存时记账互通(良性)。

## 待实施验证(非探针范围)

- 工具调用路径(如 start_discussion 缺参 → 结构化错误)在编译产物内验证。
- `group-chat-mcp-smoke.mjs` 目前 spawn `node scripts/group-chat-mcp.mjs`,需加 bin 入参以复用既有冒烟断言。
- bun 交叉编译 flag(`--target=bun-darwin-arm64` 等)未验证,v1 只做宿主 linux-x64。
