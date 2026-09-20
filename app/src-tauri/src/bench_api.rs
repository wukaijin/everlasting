//! N9 benches 的唯一入口面(任务 09-19-n9-perf-benchmark,design §1.1)。
//!
//! benches 是独立 crate:crate 内 cfg(test) 树的构造件(agent/tests_common)
//! 与 DB/LLM 基建经本模块再导出可达。benches 只准 `use everlasting_lib::bench_api`
//! ——依赖方向收口,禁止绕过本模块直捅 crate 内部类型(防腐烂约束,
//! 见 `.trellis/tasks/09-19-n9-perf-benchmark/design.md` §1.1)。

pub use crate::agent::chat_loop::run_chat_loop;
pub use crate::agent::chat_loop::suite::{CallerRole, ChatLoopDeps, ChatLoopRequest};
pub use crate::agent::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, test_messages, MockEmitter,
    TestHarness,
};
pub use crate::daemon::server::{build_router, load_daemon_state};
pub use crate::db::test_support::test_pool;
pub use crate::db::{
    create_project, create_session, init_pool, list_projects, load_session, run_migrations,
    set_config_value,
};
pub use crate::db::{finalize_turn_persist, persist_turn, MessageLatency, MessageRow};
// B6(N2 checkpoint,任务 09-20-n2-checkpoint-revert):轮末快照的
// 树构建成本。git 模块 crate 私有,经本模块再导出供 bench 面可达。
pub use crate::git::checkpoint::build_state_tree;
pub use crate::llm::provider::mock::{MockProvider, MockResponse};
pub use crate::llm::types::{ChatEvent, ChatMessage, ContentBlock, MessageContent, Role, ToolDef};
pub use crate::tools::builtin_tools;
