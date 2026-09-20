# 群聊评审结论存档(2026-09-21,session a4c6582c)

- 转录:`~/.local/share/dev.everlasting.app/discussions/2026-09-21-评审 everlasting 任务 09-21-error-bus-catego-a4c6582c.md`
- 阵容:moderator MiniMax-M3 + 架构/glm-5.3 + 产品/GLM-5.3-Flash + 后端/deepseek-flash;63 条消息,~16 分钟。
- 结论 7 条全 verified,无最终反对意见。

## 必须吸收的裁决(已回填 prd/design/implement)

1. **[设计缺陷] D2.2 终端兜底「→ Server」是新引入回归**:status=0 的 unknown-cmd 路径(`http.ts:420`,`http.test.ts:135-183` 记录 handoff_session / list_queued_messages 两次生产前科)将从静默丢弃翻转为弹假「服务端错误」。修正为分档:status=0→InvalidRequest、非 canonical 4xx→InvalidRequest、≥500→Server;「Server」仅保留为构造上不可达的防御默认。implement 第 4 步「404→Server 兜底」期望值删除改写。
2. **测试策略补强**:implement 第 10 步第 3 例的手写形状对象测不出 PR1×PR2 接缝,须补真 TransportError 实例过 handle 的接缝集成用例(顺带把「形状识别先于 instanceof Error」变成可失败断言);补构造函数收窄不变量用例(脏 body:undefined/string/{}/脏 category/kind/retryable 恒过形状门);Error 分支对 `name==="TransportError"` 单独打标 `[errorBus:transport-shape-miss]` 作运行时观测兜底。
3. **类型单一事实源**:category 字段类型 = `error.ts` 新增导出的 PascalCase 五值 union,transport 与 useErrorBus 只导入不本地定义;现有双 case union 降级为 helper 入参容忍类型。useErrorBus 侧 re-export 推 P2/chore。
4. **401 Auth toast × onAuthFailed 跳转叠加**是本任务新引入,留第 13 步冒烟验收(仅存配对→pairing 页,toast+落页无双跳),不划 P2。
5. main.ts:23-27 注释在 PR2 后变假,须同步改写。
6. design D4 补两行:fetch 裸 TypeError(daemon 掉线)不经 TransportError、Network 路由在全局兜底层不可达;string 行为收益面仅限非 AppCommandError 字符串。
7. design D1.3 补「裸 string 不进 errors FIFO」;P2 横扫任务验收写成用户可见标准(「每个用户主动操作的失败有可见反馈」)。

## 维持原设计(评审确认)

- D1 三级 console 分级 + 不新增 Local category(console 三级是与 category 正交的严重度通道)。
- 未捕 transport rejection 翻转为按 category toast:**双弹疑虑被结构性否定**(handle 唯一消费方是 main.ts;被 catch 的 rejection 不触发 unhandledrejection;useToast max3+5s dedupe 有界)——该翻转是 A5 防静默承诺的首次兑现。
- 兼容面:extractErrorMessage 对 TransportError 输出逐字节不变(AC4 成立)、401 流只读 status 不受影响、transport-parity/http.test 的 toMatchObject 子集匹配不破、errors FIFO 全仓无消费方。
- Q4 范围切分(P2 移出)成立,前提 = 裁决 1 留本任务。

## Open questions 闭环

- 「全仓 reject("字符串字面量") 零命中」单人 grep:**已二次复核**(09-21 会话,`rg 'reject\("'`/`rg "reject\('"` 于 app/src 全部 -g '*.ts' -g '*.vue' 均零命中;非测试代码无 Promise.reject)——裸 string warn 降级确无掩盖面。
- 「文档未随共识更新」:本批回填完成即闭环。
