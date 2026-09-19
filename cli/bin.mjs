#!/usr/bin/env node
// evl — Everlasting daemon CLI(daemon HTTP API 薄壳;零运行时依赖,Node ≥20)。
//
// 设计/契约:.trellis/tasks/09-19-everlasting-cli/{prd,design}.md
// - stdout 只出数据(text 或单行 JSON);session id/工具行/权限交互/verbose 全 stderr。
// - 退出码:0 done / 1 脚本错 / 2 chat kind=error / 3 SIGINT cancelled /
//   7 timeout / 64 用法错。
// - LLM 委派方一律 `evl chat "<任务>" --output json`。
import process from 'node:process';
import { readFileSync } from 'node:fs';
// package.json 用 fs 读而非 `import ... with { type: 'json' }`:import
// attributes 的 `with` 语法 Node 20.10 才稳定,engines 承诺 >=20。
const pkg = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8'));
import { parseCli, resolveBaseUrl, UsageError, COMMANDS } from './lib/args.mjs';
import { EXIT } from './lib/api.mjs';
import { runStatus } from './lib/commands/status.mjs';
import { runSessions, runProjects, runModels, runUsage } from './lib/commands/list.mjs';
import { runChat } from './lib/chat.mjs';

const HELP_TOP = `evl — Everlasting daemon CLI(daemon HTTP API 薄壳;零运行时依赖)

用法:
  evl <command> [flags]
  evl <command> --help

命令:
  status     daemon health + 版本
  chat       委派一轮 agent loop(LLM 委派主入口,配 --output json)
  sessions   列出 session(id/busy/stop_reason;默认跨全部 project,--project 收窄)
  projects   列出 project(含隐藏)
  models     列出 model(标注默认;模型引用只认 UUID)
  usage      token 用量窗口(--provider <id> 过滤)

全局 flags:
  --base-url <url>      daemon 地址(env EVERLASTING_BASE,默认 http://127.0.0.1:7456)
  --output text|json    输出格式;json = 单行 JSON(stdout 只出数据,人向信息在 stderr)
  --quiet               抑制 stderr 人向提示(错误仍出)
  --verbose             stderr 打 HTTP 请求/响应摘要与 SSE 事件名(调试)
  --timeout <s>         chat 等终态超时秒数(默认 540;不变量:宿主 bash 超时 ≥ --timeout + 60)
  --no-color            预留(当前输出无色)
  --non-interactive     强制非交互语义(等价非 TTY:静默 + ask 自动 deny + 默认 mode=plan)
  -h, --help            用法;--version 版本

chat 专属 flags:
  --mode plan|edit|yolo 权限模式(默认双模:TTY=edit,非 TTY=plan fail-closed;非法值退出 64)
  --session <id>        续聊既有 session(默认新建并保留,session id 打在 stderr)
  --ephemeral           发完即删 session(与 --session 互斥)
  --model <id>          新建 session 指定 model(UUID;续聊忽略)
  --project <path>      project 解析覆盖(默认按 CWD 匹配,无则创建)

退出码:
  0 done | 1 脚本错(网络/不可达/SSE 流断) | 2 chat kind=error
  3 SIGINT cancelled(cancel_chat 已发,session 保留) | 7 timeout(已发 cancel) | 64 用法错误

LLM 调用方速记:
  evl chat "<任务>" --output json   # stdout 单行 JSON 终态,退出码判定成败
  非交互默认 --mode plan(只读);写任务显式 --mode edit(ask 全拒继续)或 --mode yolo(自动批)

示例:
  evl status
  evl chat "列出当前目录文件并总结" --mode plan --output json
  evl chat "继续" --session <id>
  evl sessions --output json | jq '.[] | select(.busy)'
`;

const HELP = {
  status: `evl status — daemon health + 版本

用法: evl status [全局 flags]

输出: text = 人类可读行;json = health 对象单行 JSON。
daemon 不在时:非零退出 + OS 错误翻译 + ./scripts/daemon.sh bg 提示。

示例:
  evl status
  evl status --output json
`,
  chat: `evl chat — 委派一轮 agent loop(daemon :7456 的 HTTP 薄壳,fire-and-forget + SSE 终态)

用法: evl chat "<message>" [flags]

时序(design §4):project 解析(CWD 匹配,无则创建;--project 覆盖)→
session(默认新建并保留;--session 续聊)→ mode → SSE 先挂 → agent/chat →
request_id 过滤消费 → 终态。

权限(design §5):
  --mode plan   只读:写工具过滤,shell 只读沙盒面,零权限 ask
  --mode edit   默认:写操作触发 ask(TTY 三键应答;非交互立即主动 deny)
  --mode yolo   自动批(硬拒规则仍生效,root guard)
  默认双模:TTY=edit,非 TTY=plan(fail-closed)。--mode 对 session 是持久
  覆盖(落 sessions.mode),续聊命中时 stderr 提示。

输出(stdout 只出数据):
  text  = assistant 全文(多工具轮按轮间空行拼接;不截断)
  json  = 单行终态对象,形状恒定:
          {text, usage, session_id, request_id, stop_reason,
           permission_denials, text_chars, error?}
          error 分支 text="" + error:{kind,message} + usage 可为 null。

flags:
  --mode plan|edit|yolo / --session <id> / --ephemeral / --model <id> / --project <path>
  --timeout <s>(默认 540;到点先 cancel_chat 再退 7;宿主 bash 超时须 ≥ 此值+60)
  消息以 - 开头时用 -- 终止符:evl chat -- "-开头的内容"

SIGINT: 首次 cancel_chat 并等取消终态(退出 3,session 保留);二次立即硬退。

示例:
  evl chat "只回一句问候,不要调用任何工具" --output json < /dev/null
  evl chat "列出当前目录文件并总结" --mode plan --output json
  evl chat "继续" --session <id>
  evl chat "改一下 README 标题" --mode yolo          # 自动批写操作
`,
  sessions: `evl sessions — 列出 session(id/标题/时间/busy/stop_reason)

默认:遍历全部 project 合并(list_sessions 必填 project_id,纯运输层聚合),
按 updated_at 降序;--project <path> 收窄到单 project。

输出: text 表;json = SessionSummary 投影数组单行 JSON
  [{id, title, project_path, session_type, updated_at, busy, stop_reason}]

用途: 宿主杀掉 evl 后 loop 仍在跑的兜底排查(busy=true → GUI Stop 或等自然结束)。

示例:
  evl sessions
  evl sessions --output json | jq '.[] | select(.busy)'
  evl sessions --project /path/to/repo
`,
  projects: `evl projects — 列出 project(含隐藏)

输出: text 表;json = [{id, name, path, hidden, git_branch, updated_at}] 单行 JSON。

示例:
  evl projects
  evl projects --output json
`,
  models: `evl models — 列出 model + 标注默认

输出: text 表(default 列 * 标默认);json = {default: <id|null>, models: [...]} 单行 JSON。
模型引用只认 UUID(daemon 语义);--chat 的 --model 传这里查到的 id。

示例:
  evl models
  evl models --output json | jq '.default'
`,
  usage: `evl usage — token 用量窗口

输出: text = 窗口 + provider 合计 + top sessions 表;
      json = UsageWindowReport 原样单行 JSON(camelCase)。

flags:
  --provider <id>   只看该 provider(默认 null = 全部)

示例:
  evl usage
  evl usage --output json | jq '.providers[].totals'
`,
};

function printHelp(command) {
  process.stdout.write(HELP[command] ?? HELP_TOP);
}

async function main(argv) {
  const parsed = parseCli(argv);
  const { command, positionals, flags } = parsed;

  if (flags.version) {
    process.stdout.write(`${pkg.name} ${pkg.version}\n`);
    return EXIT.ok;
  }
  if (flags.help) {
    printHelp(command);
    return EXIT.ok;
  }
  if (command == null) {
    // 缺参:usage 打 stderr + 64(design §6)
    process.stderr.write(HELP_TOP);
    throw new UsageError('缺少命令');
  }

  const base = resolveBaseUrl(flags.baseUrl, process.env.EVERLASTING_BASE);
  const io = { stdout: process.stdout, stderr: process.stderr, stdin: process.stdin };

  switch (command) {
    case 'status':
      return runStatus({ base, flags, io });
    case 'sessions':
      return runSessions({ base, flags, io });
    case 'projects':
      return runProjects({ base, flags, io });
    case 'models':
      return runModels({ base, flags, io });
    case 'usage':
      return runUsage({ base, flags, io });
    case 'chat': {
      const message = positionals.join(' ').trim();
      if (message === '') {
        throw new UsageError('chat 缺少消息文本(用法:evl chat "<message>" [flags])');
      }
      return runChat({ base, flags, message, io });
    }
    default:
      throw new UsageError(`未知命令: ${command}(可用:${COMMANDS.join(' | ')})`);
  }
}

const topAwait = await main(process.argv.slice(2)).catch((e) => {
  const isUsage = e instanceof UsageError;
  process.stderr.write(
    `evl: ${e.message}${isUsage ? '\n(用法:evl --help;子命令:evl <command> --help)' : ''}\n`
  );
  return isUsage ? EXIT.usage : (e.exitCode ?? EXIT.scriptError);
});
process.exitCode = topAwait ?? EXIT.ok;
