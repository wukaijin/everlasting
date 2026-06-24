# LangGraph `Send` 原语与并行节点执行 — 调研笔记

> 调研日期：2026-06-24
> 范围：LangGraph（langchain-ai/langgraph main 分支，2026-06）`Send` 原语 + 并行节点执行模型
> 目标：为 everlasting 第二档 ⑩/⑫ 多 worker 并行扩展（自研 agent harness，参考 LangGraph `Send` 模式）做对照调研，回答 7 个具体问题
> 方法：① GitHub 源码直读（`libs/langgraph/langgraph/types.py` Send/TimeoutPolicy）；② 官方 docs.langchain.com 一手抓取；③ DeepWiki + Substack 互证；④ 二手 Medium / dev.to 仅作交叉验证
> 一手优先：源码 + 官方 docs > DeepWiki（自动生成但引用准确）> Medium

---

## 0. TL;DR

| 维度 | 结论 |
|---|---|
| **Send 是什么** | 一个**消息包**（node 名 + arg payload + 可选 timeout），由 conditional edge 返回多个 → Pregel 引擎在下一个 superstep 内**并行**调起 N 个目标节点实例 |
| **结果怎么回来** | 每条分支返回 `dict` 状态增量 → 通过 **reducer**（如 `operator.add`）合并回父 graph 的共享 state。**没有显式的 join 节点**，reducer 即 join |
| **并行模型** | Google Pregel **BSP（Bulk Synchronous Parallel）** 模型；同一 superstep 内 N 个 actor 真并行（asyncio 协程），跨 superstep 用 channel 同步 |
| **状态隔离** | Send 的 `arg` 是该分支的**私有入参**（通常只是父 state 的一个切片）；reducer 决定**写入方向**——多写同 key 需 reducer 合并，否则末值胜出（LastValue 默认） |
| **错误处理** | **整个 superstep 原子事务**：任一分支 raise → 整步回滚、无 partial 结果写入；但 checkpoint 持久化已完成分支的中间结果，resume 时跳过 |
| **取消 / 超时** | Send 自带 `timeout: TimeoutPolicy` 字段（`run_timeout` 硬墙钟 / `idle_timeout` 静默上限 / `refresh_on="auto"|"heartbeat"`）；依赖 asyncio cancellation（同步阻塞代码不响应） |
| **vs Subagent** | `Send` 是**低级** fan-out 工具（同节点多次 + 合并），**不是** subagent；LangGraph 的 "subagent" 由 **subgraph（StateGraph 嵌套）** 表达。两者组合：`Send("subgraph_node", payload)` 即可 fan-out N 个独立 subgraph |

---

## 1. 通信模式（Communication Pattern）

### 1.1 Send 的 wire shape

`Send` 是 langgraph 自定义的小数据类，源码（`libs/langgraph/langgraph/types.py`）：

```python
class Send:
    """A message or packet to send to a specific node in the graph.

    The `Send` class is used within a `StateGraph`'s conditional edges to
    dynamically invoke a node with a custom state at the next step.

    Importantly, the sent state can differ from the core graph's state,
    allowing for flexible and dynamic workflow management.

    One such example is a "map-reduce" workflow where your graph invokes
    the same node multiple times in parallel with different states,
    before aggregating the results back into the main graph's state.
    """
    __slots__ = ("node", "arg", "timeout")

    node: str
    arg: Any
    timeout: TimeoutPolicy | None

    def __init__(
        self,
        /,
        node: str,
        arg: Any,
        *,
        timeout: float | timedelta | TimeoutPolicy | None = None,
    ) -> None:
        self.node = node
        self.arg = arg
        self.timeout = TimeoutPolicy.coerce(timeout)
```

`__slots__ = ("node", "arg", "timeout")` —— 三字段，无 method。来源：[langchain-ai/langgraph types.py](https://github.com/langchain-ai/langgraph/blob/main/libs/langgraph/langgraph/types.py)（Send 类，line 437-496 区间）；API reference：[reference.langchain.com/python/langgraph/types/Send](https://reference.langchain.com/python/langgraph/types/Send)。

**Send 不是结果，是入站消息**：它描述"调哪个节点 + 带什么入参 + 多长超时"。真正的"结果"是分支节点返回的 `dict` 状态更新。

### 1.2 父节点怎么"发送"多个 Send

`Send` **只能** 在 conditional edge 函数里 return（不能在普通 node 里）。经典 map-reduce 模板（[Send API reference example](https://reference.langchain.com/python/langgraph/types/Send)）：

```python
from typing import Annotated
from langgraph.types import Send
from langgraph.graph import END, START
from langgraph.graph import StateGraph
import operator

class OverallState(TypedDict):
    subjects: list[str]
    jokes: Annotated[list[str], operator.add]   # reducer = merge

def continue_to_jokes(state: OverallState):
    # fan-out：动态产 N 个 Send
    return [Send("generate_joke", {"subject": s}) for s in state["subjects"]]

builder = StateGraph(OverallState)
builder.add_node("generate_joke", lambda state: {"jokes": [f"Joke about {state['subject']}"]})
builder.add_conditional_edges(START, continue_to_jokes)   # ← 关键：用 conditional edge 触发
builder.add_edge("generate_joke", END)
graph = builder.compile()

graph.invoke({"subjects": ["cats", "dogs"]})
# → {'subjects': ['cats', 'dogs'], 'jokes': ['Joke about cats', 'Joke about dogs']}
```

### 1.3 父节点怎么"消费"结果

**没有显式 join**。分支返回的 `dict` 增量直接进 channel，reducer 在 channel 层自动合并：

```python
jokes: Annotated[list[str], operator.add]
```

每条分支返回 `{"jokes": [one_joke]}` → `operator.add` 把所有 `one_joke` 串成一个 list。来源：[docs.langchain.com/oss/python/langgraph/use-graph-api](https://docs.langchain.com/oss/python/langgraph/use-graph-api)（"consuming results through a reducer" 段）。

父节点**看不到**中间事件，只看到 superstep 结束后的合并 state。Pregel 引擎在 channel 层做了 BSP 同步屏障（§2.2）。

### 1.4 中间事件可见性

**默认父节点看不到中间事件**，但 LangGraph 提供 `stream_mode="updates"` / `events"` 把每条分支的中间产出推到订阅者。这是**观察钩子**，不影响数据合并逻辑。来源：[docs.langchain.com/oss/python/langgraph/pregel](https://docs.langchain.com/oss/python/langgraph/pregel) "streaming" 段。

---

## 2. 并发模型（Concurrency Model）

### 2.1 BSP / Pregel 模型

LangGraph runtime 叫 **Pregel**，灵感来自 Google Pregel（图计算 BSP 模型）。每个 superstep 三阶段：

1. **Plan** — 选 actor：上一步写了 channel X → 订阅 X 的 actor 入选
2. **Execution** — 入选 actor **全部并行**（asyncio gather），互相不可见（"channel updates are invisible to actors until the next step"）
3. **Update** — channel 应用 reducer / LastValue 更新

来源：[docs.langchain.com/oss/python/langgraph/pregel](https://docs.langchain.com/oss/python/langgraph/pregel)（"Plan / Execution / Update" 三段）；[deepwiki.com Pregel Execution Engine](https://deepwiki.com/langchain-ai/langgraph/3.3-pregel-execution-engine)。

### 2.2 Send 怎么触发并行

当 conditional edge 返回 `list[Send]` 时，Pregel 在**下一个 superstep** 把这些 Send 解读为"调这些目标节点的 N 个独立任务"，并在 Plan 阶段全部入选。**N 个 Send 真并行**（asyncio 协程 + `gather`），它们都在同一个 superstep 的 Execution 阶段运行，superstep 结束前互相不可见。

如果 Send 都指向**同一节点**（典型 map-reduce），该节点被实例化 N 次，每次收到自己的 `arg`。如果 Send 指向**不同节点**（不常见），就是普通 fan-out。

### 2.3 fan-out / fan-in 是隐式的

**没有显式的 FanOut / FanIn 节点**。fan-out 是 conditional edge return `list[Send]` 的语义效果；fan-in 是 reducer 在 channel 层做的合并。

如果想"先并行、再串行下一步"，就在分支节点后加普通 `add_edge("branch_node", "next_node")` —— 所有分支完成后才进入 next_node（因为 next_node 订阅了 channel）。

### 2.4 channel 类型

来自 [docs.langchain.com/oss/python/langgraph/pregel](https://docs.langchain.com/oss/python/langgraph/pregel)：

| Channel | 用途 |
|---|---|
| `LastValue`（默认） | 末值胜出，单写者 |
| `Topic` | 累积 / 去重（按 value 哈希） |
| `BinaryOperatorAggregate` | 应用二元算子（如 `operator.add`），多写者并行合并 |
| `DeltaChannel` | 存增量（大型频繁写场景） |

---

## 3. 生命周期 / 取消（Lifecycle / Cancel）

### 3.1 Send 自带 TimeoutPolicy

源码（types.py 305-361 行）：

```python
@dataclass(**_DC_KWARGS)  # kw_only=True, slots=True, frozen=True
class TimeoutPolicy:
    """Configuration for timing out node attempts.

    !!! note "Cooperative cancellation"
        Timeouts rely on asyncio cancellation. If your node uses synchronous
        time.sleep() or other CPU-bound work that blocks the GIL, the timeout
        will not be fired until after the event loop has been released.

    !!! note "Inline callback dispatch"
        Under `refresh_on="auto"`, an internal handler refreshes the timeout on
        any callback event that occurs in the execution of the node or its
        nested descendants.
    """

    run_timeout: float | timedelta | None = None
    """Hard wall-clock cap (in seconds) for a single node attempt.
    This timeout is never refreshed by progress signals or `runtime.heartbeat()`.
    """

    idle_timeout: float | timedelta | None = None
    """Maximum time (in seconds) a single node attempt may go without observable progress."""

    refresh_on: Literal["auto", "heartbeat"] = "auto"
    """Which signals refresh `idle_timeout`.
    `"auto"` refreshes on standard graph progress signals and explicit heartbeats.
    `"heartbeat"` refreshes only on explicit `runtime.heartbeat()` calls.
    """
```

使用：

```python
Send("slow_node", payload, timeout=30.0)              # 数字 → run_timeout
Send("slow_node", payload, timeout=TimeoutPolicy(
    run_timeout=60, idle_timeout=10, refresh_on="heartbeat"
))
```

### 3.2 取消语义

- **超时就取消**：依赖 asyncio `Task.cancel()`，分支节点 await 链路上任何一处抛 `CancelledError` → 父节点看到 `TimeoutError`。
- **同步阻塞代码不响应超时**（"cooperative cancellation"）：`time.sleep(60)` 会卡 60 秒，超时只在事件循环再次获得控制权时生效。这是 Python asyncio 的固有限制，不是 LangGraph 的 bug。
- **`refresh_on="heartbeat"`** 适合长任务：节点内周期调 `runtime.heartbeat()` 重置 idle 计时器；`run_timeout` 是硬墙钟，**永远不刷新**。

### 3.3 手动取消（外部 interrupt）

Pregel 提供 `stream` / `invoke` 的 `cancel()`（返回 future），调用即中止当前 superstep。这是图级取消，会中断所有未完成的 Send 分支。

---

## 4. 状态隔离（State Isolation）

### 4.1 Send 的 arg 是私有入参

每条分支**只看到自己 `Send(..., arg=...)` 传入的那个 dict**，看不到父 graph 的完整 state。来源：[docs.langchain.com/oss/python/langgraph/use-graph-api](https://docs.langchain.com/oss/python/langgraph/use-graph-api) "State isolation" 段：

> Each Send branch receives only the state fields explicitly passed in the `Send` call. In the example above, `generate_joke` receives only `{"subject": s}` rather than the full `OverallState`.

实际使用模式：conditional edge 函数**显式切片**：

```python
def continue_to_jokes(state: OverallState):
    return [Send("generate_joke", {"subject": s}) for s in state["subjects"]]
```

`generate_joke` 的入参类型只需 `TypedDict("subject": str)`，跟父 `OverallState` 解耦。这是"写时显式声明"的隔离模型。

### 4.2 分支能不能写共享 state？

**能**，但走 channel → reducer：

1. 分支返回 `{"jokes": [...]}`
2. Pregel 把这 dict 喂给 channel 的 reducer（`operator.add`）
3. channel 状态被多个分支并发写入、reducer 串行归并

**默认 channel `LastValue` 不能多写**——两个分支都写 `{"x": 1}` 和 `{"x": 2}`，**后写胜出**（顺序取决于 superstep 调度顺序）。需要多写合并必须用 `Annotated[..., operator.add]` 或显式 `BinaryOperatorAggregate`。

来源：[docs.langchain.com/oss/python/langgraph/pregel](https://docs.langchain.com/oss/python/langgraph/pregel) "Channel Synchronization" 段。

### 4.3 分支之间互不可见

BSP 模型保证：**同一 superstep 内**，所有分支的写入在 Update 阶段才可见，分支读取的 state 是上一步的快照。两条分支不能"中途互相通信"，必须等 superstep 边界。

---

## 5. 失败可见性（Failure Visibility）

### 5.1 原子事务语义

来自 [docs.langchain.com/oss/python/langgraph/use-graph-api](https://docs.langchain.com/oss/python/langgraph/use-graph-api)：

> LangGraph executes nodes within supersteps, meaning that while parallel branches are executed in parallel, the entire superstep is transactional. **If any of these branches raises an exception, none of the updates are applied to the state (the entire superstep errors).**

也就是：**任一分支 raise → 整个 superstep 回滚 + 异常向上抛**。即使 99 条分支成功，成功的 partial 结果也**不写入 state**。

### 5.2 Checkpoint 救场

> When using a checkpointer, results from successful nodes within a superstep are saved, and don't repeat when resumed.

checkpoint（Sqlite/Postgres）会持久化 superstep 内**已完成分支的中间结果**，图 resume 时这些分支不再跑、只重跑失败的。这是从"原子回滚"中恢复的标准 BSP trick。

### 5.3 父节点看到了什么

- **异常路径**：raise → Pregel 抛异常 → 父 conditional edge / 上游 node 拿到的就是原始异常对象
- **正常路径**：父只看 superstep 后的合并 state；看不到哪条分支失败、谁还在跑

如果父需要"partial result + error info"语义（即便一条分支失败也想拿到其它分支的产出），**Send 不能直接满足**——必须自己包：分支节点 catch 异常并返回 `{"results": [...], "errors": [...]}`，让 reducer 把 errors 也合并进 state。

### 5.4 RetryPolicy

Pregel 支持节点级 retry 配置（`add_node(..., retry=RetryPolicy(max_attempts=3))`）。retry 只对失败分支重跑，不影响成功分支。

---

## 6. vs Subagent（Comparison to "Subagent"）

### 6.1 Send ≠ Subagent

| 抽象 | Send | Subgraph |
|---|---|---|
| 定位 | **低级 fan-out 工具**，单一节点的 N 次并行 | **完整子图嵌套**，可独立 state schema + 独立生命周期 |
| 状态 | arg 私有，channel 共享 | 独立 state schema，可与父 schema 完全不同 |
| 复杂度 | 一行 `list[Send]` | `builder.compile()` + `add_node("name", subgraph_app)` |
| 适用 | map-reduce、批量独立子任务 | 完整 multi-agent delegation（带自己的 LLM 调用、工具、持久化） |

Send 的 docstring 自己写：

> One such example is a "map-reduce" workflow where your graph invokes the **same node multiple times in parallel** with different states, before aggregating the results back into the main graph's state.

Send 是 **map-reduce 的 map 步**。

### 6.2 Send + Subgraph = "dispatch subagent"

要实现"dispatch a subagent"，**Send 套 subgraph**：

```python
subgraph_app = builder.compile()  # 独立 graph，独立的 state schema

def dispatch(state):
    return [
        Send("subagent_node", {"task": t})    # 调 subgraph
        for t in state["tasks"]
    ]

parent.add_node("subagent_node", subgraph_app)
parent.add_conditional_edges("plan", dispatch)
```

每条 Send 分支启动一个**独立的 subgraph 实例**，有自己完整的 Pregel 执行（多次 superstep、自己的 stream、自己的 state）。父 graph 通过 channel / reducer 看到的是 subgraph 的**最终 state**（取决于 subgraph 的输出 schema）。

这就是 LangGraph "multi-agent" 的标准模式：subgraph = 完整 agent 逻辑；Send = fan-out 触发；reducer = 合并所有 subgraph 的 final state。

来源：[docs.langchain.com/oss/python/langgraph/use-subgraphs](https://docs.langchain.com/oss/python/langgraph/use-subgraphs)；[deepwiki.com Sub-Graphs for Modularity](https://deepwiki.com/langchain-ai/langchain-academy/7.2-parallelization-techniques)。

### 6.3 LangGraph 没有更高层 "dispatch_subagent" 原语

LangGraph **没有** 类似 Claude Code `Task` tool 那样的 LLM 触发型 dispatcher 原语。"dispatch subagent" 是应用层用 `Send + subgraph + conditional edge` 自己组合出来的。`create_react_agent`（prebuilt）是另一个独立 axis —— 它封装 ReAct loop，但不包含并行 fan-out。

---

## 7. 一手参考清单

### 7.1 GitHub 源码
- **Send / TimeoutPolicy 类**：[langchain-ai/langgraph `libs/langgraph/langgraph/types.py`](https://github.com/langchain-ai/langgraph/blob/main/libs/langgraph/langgraph/types.py) —— Send 类 line 437-496，TimeoutPolicy line 305-361
- **Pregel runtime**：[langchain-ai/langgraph `libs/langgraph/langgraph/pregel/main.py`](https://github.com/langchain-ai/langgraph/tree/main/libs/langgraph/langgraph/pregel)
- **仓库入口**：[github.com/langchain-ai/langgraph](https://github.com/langchain-ai/langgraph)

### 7.2 官方文档
- **Send API reference**：[reference.langchain.com/python/langgraph/types/Send](https://reference.langchain.com/python/langgraph/types/Send)
- **Graph API overview**：[docs.langchain.com/oss/python/langgraph/use-graph-api](https://docs.langchain.com/oss/python/langgraph/use-graph-api) —— Send 用于 map-reduce + state reducer + superstep 原子性
- **Pregel runtime**：[docs.langchain.com/oss/python/langgraph/pregel](https://docs.langchain.com/oss/python/langgraph/pregel) —— BSP 三阶段 + channel 类型
- **Subgraphs**：[docs.langchain.com/oss/python/langgraph/use-subgraphs](https://docs.langchain.com/oss/python/langgraph/use-subgraphs) —— 嵌套图 + 独立 state schema

### 7.3 二手交叉验证
- [deepwiki.com Pregel Execution Engine](https://deepwiki.com/langchain-ai/langgraph/3.3-pregel-execution-engine) —— 自动生成的代码解读，引用源码准确
- [deepwiki.com Map-Reduce Pattern](https://deepwiki.com/langchain-ai/langchain-academy/7.1-map-reduce-pattern) —— Send API 三机制：fan-out + reducer + 独立 arg
- [aipractitioner.substack.com Scaling LangGraph Agents](https://aipractitioner.substack.com/p/scaling-langgraph-agents-parallelization) —— superstep 定义 + transactional 行为
- [medium.com Parallel Nodes with Deferred Execution](https://medium.com/@gmurro/parallel-nodes-in-langgraph-managing-concurrent-branches-with-the-deferred-execution-d7e94d03ef78) —— 不同长度并行分支的合并策略

### 7.4 JS 版（langgraph-js 对照）
- **TS API reference**：[langchain-ai.github.io/langgraphjs/reference/classes/langgraph.Send.html](https://langchain-ai.github.io/langgraphjs/reference/classes/langgraph.Send.html)
- TS 版的 `Send(node, args)` 是双参（无 timeout 字段），与 Python 版不同步——这是 API surface 的小差异

---

## 8. 对 everlasing 的可借鉴要点

> 这一节是给后续 ⑩/⑫ 多 worker 并行扩展的设计输入，不属于本调研事实部分。

1. **Send 是"声明式 fan-out 消息"，不是结果**。我们的 `dispatch_subagent` tool 可以参考这个模型：tool 不直接 await worker，而是把"调哪个 worker + 带什么入参"以数据结构的方式塞回主 LLM 的 tool_result 流转，由专门的 worker 调度层异步执行。
2. **reducer 即 join**。如果想让多条 worker 的结果合并回主 LLM 的 tool_result 流（而不是当前架构的"text-only summary string"），可以用类似 `Annotated[list[ToolResult], merge_results]` 的模式：每条 worker 完成后 emit 一个 `ToolResult`，main LLM 下一次 chat 调用时看到的就是**结构化 list**而不是单 string。
3. **per-Send timeout 字段**值得抄：everlasting 现在用 `SUBAGENT_MAX_TURNS`（硬墙钟按 turn 数）防失控，langgraph 的 `TimeoutPolicy(run_timeout, idle_timeout, refresh_on)` 更精细——可以在 worker 粒度给每个 subagent 配不同超时策略。
4. **partial result + checkpoint resume** 是 Send 当前缺的能力。everlasting 如果想"一条 worker 失败不影响其它 worker 结果写入"，需要在应用层自己 catch + 持久化，不依赖框架。这块 langgraph 的 atomic-superstep 模型不适合做"乐观并行收集"。
5. **真正的"subagent" = subgraph 嵌套**。everlasting 当前架构下 worker 是单独进程（subagent store + transcript），不是 state machine 子图。如果未来想把 worker 也抽象成"独立 agent 编排图"，subgraph 模式值得参考，但成本高、收益需要评估。