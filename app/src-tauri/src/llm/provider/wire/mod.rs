//! Provider-agnostic wire representation (PR3 of the multi-model task).
//!
//! Both the Anthropic adapter and the OpenAI adapter convert from
//! [`crate::llm::types::ChatRequest`] / [`crate::llm::types::ChatEvent`]
//! (Anthropic-shaped) to / from this intermediate [`WireRequest`] /
//! [`WireMessage`] / [`WireBlock`] form, then to / from the actual
//! provider wire format. The wire module is the single place that
//! knows how to:
//!
//! 1. Map the Anthropic-shaped `ChatMessage` / `ContentBlock` types
//!    into a provider-agnostic shape that has explicit variants for
//!    things only Anthropic supports (signature blobs, redacted
//!    thinking) and things only OpenAI supports (Reasoning).
//! 2. Run a `strip_unsupported` pass that drops blocks the target
//!    protocol cannot represent, driven by the target's
//!    [`WireCapabilities`]. This is the **silent degradation** the
//!    parent PRD §Q5 H1 decision locked in: switching from a
//!    `supports_thinking=true` Anthropic model to a non-thinking
//!    OpenAI model silently drops the thinking blocks (they stay in
//!    the DB; only the wire payload this turn omits them).
//!
//! The wire layer is **purely in-memory** — no IO, no DB. The
//! provider's `send` is the single call site that invokes it:
//!
//! ```text
//! ChatRequest  --(chat_request_to_wire)-->  WireRequest
//!                     |
//!                     v
//!           (strip_unsupported, target_caps)
//!                     |
//!                     v
//!               WireRequest
//!                     |
//!                     v
//!          (provider-wire converter)
//!                     |
//!                     v
//!         actual upstream HTTP body
//! ```
//!
//! Conversely the stream is converted block-by-block: each
//! `WireBlock` arriving from the provider's parser is mapped to a
//! [`ChatEvent`] (or None for blocks that shouldn't surface to the
//! frontend) via [`wire_block_to_chat_event`].
//!
//! Module layout (08-07-large-file-splitting, was a single wire.rs):
//! - `types` — shared wire types (`WireCapabilities` / `WireRequest` /
//!   `WireMessage` / `WireBlock` / `WireTool`)
//! - `to_wire` — Chat types → wire conversion (request building +
//!   orphan guard + `strip_unsupported` silent degradation pass)
//! - `from_wire` — wire → Chat types conversion (stream blocks →
//!   `ChatEvent`, wire messages → `ChatMessage`)

mod from_wire;
mod to_wire;
mod types;

// 部分 re-export 仅测试构建消费(openai/tests_agent_loop/tests_wire 测试模块),
// lib 构建下视为未用 —— 与拆分前 wire.rs 内 pub 项不告警的语义一致
#[allow(unused_imports)]
pub use from_wire::{
    wire_block_to_chat_event, wire_messages_to_chat_messages, wire_tools_to_tool_defs,
};
pub use to_wire::{chat_request_to_wire, strip_unsupported};
#[allow(unused_imports)]
pub use types::{WireBlock, WireCapabilities, WireImage, WireMessage, WireRequest, WireTool};
// pub(crate) 项保持原可见性再导出(外部调用点 wire::orphan_* 不变)
#[allow(unused_imports)]
pub(crate) use to_wire::{orphan_tool_call_order, orphan_tool_use_ids};
// pub(crate) re-exports for tests_wire.rs (sibling module, 08-07 split)
#[allow(unused_imports)]
pub(crate) use from_wire::wire_blocks_to_content_blocks;
#[allow(unused_imports)]
pub(crate) use to_wire::truncate;
