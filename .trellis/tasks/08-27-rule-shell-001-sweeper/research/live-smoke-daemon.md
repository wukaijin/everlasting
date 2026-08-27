# Live smoke — daemon 活体验证记录(2026-08-27)

> 结论:**通过**。真实 daemon(release 构建)上,完整生产链路走通:
> 真实 LLM 调工具 → 权限询问交互 → shell 执行 → sweeper 定时清扫。

## 最终证据(daemon.log)

```
03:52:45.559 INFO background_shell::in_memory: background_shell: task finished
         session_id=a4470b2c… shell_id=bsh_57f0a3b4f95347b085bf86280ae428ac
         outcome=Completed exit_code=Some(0)
03:53:03.422 INFO daemon::server: swept completed background shell entries count=1
```

时序核验:完成 03:52:45.5 → 清扫 03:53:03.4,间隔 17.9s,落在临时参数
(retention 15s / interval 5s)的 [15s, 20s] 窗口内,与设计精确吻合。
生产常量(1h/5min)已还原并重建重启,36 模块单测复绿。

## 方法(复现要点)

1. 临时将 `SHELL_RETENTION_MS`/`SWEEP_INTERVAL_MS` 调小(15s/5s)→
   `./scripts/daemon.sh restart`(release 重建 ~6min)。
2. `turn-smoke.sh --keep --message "调用 run_background_shell 后台跑 sleep 3"`。
3. **权限门是关键坑**(下节)——用 SSE + `permission_response` 模拟前端批准。
4. 观察日志两线:`background_shell: task finished` → `swept completed … count=1`。
5. 还原常量 → 模块测试 → `daemon.sh restart`,烟测 session 手动删除。

## 踩坑记录(对未来烟测有复用价值)

- **turn-smoke 的轮询会在 turn 流式进行中提前命中 turn_trace 行**:
  --keep 模式下 turn 未结束脚本就退出;非 --keep 模式下脚本退出即删
  session,`delete_session` 会**取消进行中的 turn**("destructive op:
  cancelled in-flight chat")。多轮工具 turn(先 `load_tool_schemas` 再目标
  工具)必踩。对策:--keep + 手动收尾。
- **daemon 无头环境工具调用会被权限门拦住**:`run_background_shell` 走
  Tier 4 交互询问,120s 无应答即 deny(日志 `Tier 4 timed out`),工具根本
  不执行。三次绕行尝试:
  - `set_session_mode(yolo)` → **root 被拒**("Cannot enable Yolo as root",
    安全护栏,合理);
  - `grant_tool_permission(match_kind="tool")` → HTTP 200 但无效。**已查实
    非 bug**:`check/permission.rs` Tier 4 里 `run_background_shell` 分类为
    `ToolKind::Shell`,该分支只消费 **prefix 授权**(command 首词匹配,
    如 `match_kind="prefix", match_value="sleep"`);tool 级授权仅
    WebFetch/GitMutation 分支消费。正确姿势是 prefix 授权或走下面的
    SSE 应答。
  - **正解:模拟前端真实交互** —— 订阅 `/api/v1/stream` 抓
    `event: permission:ask` 的 `rid`,在 120s 窗口内 POST
    `/api/v1/permissions/permission_response {decision:"allow_once"}`。
    注意 rid 抓取→应答必须压在同一脚本内(窗口只 120s,跨工具调用回合
    会超时,实测踩过一次)。
  - 残留观察(P3 级 API footgun):`grant_tool_permission` 对 shell 类工具
    接受永不生效的 tool 级授权,写入成功无任何警告(前端 UI 只发 prefix,
    故仅裸 API 调用方会踩)。
- 模型第一轮倾向先调 `load_tool_schemas`(延迟加载),目标工具在第二轮
  才发出;提示词里写"不要调用 load_tool_schemas"未必有效,预留轮次时间。
