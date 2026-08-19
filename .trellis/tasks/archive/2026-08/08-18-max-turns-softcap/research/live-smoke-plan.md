# Live 冒烟程序 — MAX_TURNS 软卡(AC5)

> **执行结果:2026-08-19 PASS**(session 75ebc475…,跑完已删)。
>
> - daemon 以 `EVERLASTING_SOFTCAP_TURN_BOUNDARY=2` 起服(新 release 二进制);
> - 双工具轮消息 → turn3 顶部撞线:`get_pending_interaction` 返回
>   `kind=turn_limit_softcap` / `tool_use_id=turn_limit_softcap_3` /
>   选项 `[继续(+200 轮), 压缩后续跑, 停止]`(compaction_on 缺省开 → 三支);
> - HTTP `resolve_tool_question` 答「停止」→ pending 清空、loop 干净收尾;
> - DB 断言(`session_audit_events`,`payload_json` 列):
>   `{"action":"asked","budget":2,"turn":3}` → `{"action":"stopped","budget":2,"turn":3}`;
> - 消息尾部 seq4=user(tool results)——无孤儿 tool_use;
> - 冒烟后 daemon 已重启回**无 env** 的干净状态。
>
> 经验:模型可能一轮并行双工具或提前总结(两次未触发后第三次用
> "禁止并行/禁止跳过第二步"强指令成功);audit 断言必须在
> `delete_session` 之前(FOREIGN KEY ON DELETE CASCADE 会连带删行)。
> UI 浮动卡的手动三分支点选留待日常使用验证(dist 未重建,不在本冒烟范围)。

> 前置:check 子代理完成(避免 cargo 构建锁竞争);daemon 空闲确认
> (最近 session 活动 2026-08-17,无 in-flight)。

## 程序

1. **带 env 重启 daemon**(新二进制 + 撞线边界=2):
   ```bash
   cd /usr/local/code/github/everlasting
   EVERLASTING_SOFTCAP_TURN_BOUNDARY=2 ./scripts/daemon.sh restart
   ```
   (restart 会先编 release;env 经脚本继承进 daemon 进程。)

2. **建临时 session + 触发两轮工具调用**:
   ```bash
   # project 复用本仓库(已在列表);create_session 同 turn-smoke.sh §2
   # 消息:"请用 list_dir 工具查看当前目录,然后用 read_file 查看任意一个
   # 文件的开头几行。" —— 期望 turn1/turn2 各含 tool_use。
   curl -sf -X POST $BASE/api/v1/agent/chat -d @- <<EOF
   {"request_id":"softcap-smoke-1","session_id":"$SID",
    "messages":[{"role":"user","content":"…"}]}
   EOF
   ```
   budget=2:turn1 tool → turn2 tool → turn3 顶部 `3 > 2` → 询问卡注册。

3. **轮询 pending(期望 kind=turn_limit_softcap)**:
   ```bash
   curl -sf -X POST $BASE/api/v1/question/get_pending_interaction \
     -H 'Content-Type: application/json' -d "{\"session_id\":\"$SID\"}"
   # 期望:{"kind":"turn_limit_softcap","payload":{…tool_use_id:
   # "turn_limit_softcap_3", questions:[继续(+200 轮)/压缩后续跑/停止]}}
   ```

4. **答「停止」**(脚本侧模拟用户点卡;UI 手动点选同路径):
   ```bash
   curl -sf -X POST $BASE/api/v1/question/resolve_tool_question \
     -H 'Content-Type: application/json' -d "{
       \"session_id\":\"$SID\",\"tool_use_id\":\"turn_limit_softcap_3\",
       \"answer\":[{\"question\":\"\",\"header\":null,
         \"options\":[\"停止\"],\"multi_select\":false,\"custom\":null}]}"
   ```

5. **DB 断言 + 清理**:
   ```bash
   sqlite3 -readonly $DB "SELECT kind, payload FROM audit_events
     WHERE session_id='$SID' AND kind='turn_limit_softcap';"
   # 期望两行:asked → stopped(payload 含 turn=3, budget=2)
   # 清理:delete_session(同 turn-smoke cleanup)
   ```

## 已知注意

- boundary env 只在 loop 初始化读一次(修过的 QA 陷阱),「继续」后
  下一问在 boundary+200,不会每轮重问。
- 若模型 turn1 即 end_turn(没调工具),轮数不消耗完,不会触发询问
  ——换更明确的工具指令重试。
- 冒烟只验 stop 分支;continue/compact 分支 mock 端到端已覆盖
  (softcap.rs 用例 1/5),UI 手动三分支点选留给用户日常使用验证。
