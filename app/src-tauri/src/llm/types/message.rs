//! Message content primitives: `Role`, `CacheControl`, `ContentBlock`,
//! and the string-or-array `MessageContent` wrapper with its custom Serde impls.
//!
//! Split out of `llm/types.rs` (2026-08-08 batch3). These types are tightly
//! coupled — `MessageContent::Blocks` wraps `Vec<ContentBlock>`, and the
//! manual Serde impls are the only way to round-trip the string-or-array shape.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::chat::AttachmentRef;

// ---------------------------------------------------------------------------
// Role
// ---------------------------------------------------------------------------

/// Conversation role. In the Anthropic Messages API, `tool_result` content
/// blocks are placed inside a `role: "user"` message, so we don't need a
/// separate `Tool` role.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

// ---------------------------------------------------------------------------
// CacheControl — Anthropic prompt cache breakpoint marker
// ---------------------------------------------------------------------------

/// A `cache_control` hint attached to a content block. Anthropic's
/// Messages API reads this field to decide where to put a cache
/// breakpoint — the LAST block in a request that carries this
/// marker is the cache boundary; everything before it becomes
/// eligible for a cache hit on the next turn (within the 5-min
/// TTL).
///
/// The B5 memory refactor (2026-06-11) attaches `Ephemeral` to
/// the first content block of the synthetic "instructions" user
/// message so the 4 instruction files (CLAUDE.md / AGENTS.md ×
/// user / project) are cached on turn 1 and read from cache on
/// turns 2..MAX_TURNS. Without this marker, Anthropic would
/// 100% miss every turn and re-bill the full instructions
/// payload.
///
/// Today only `Ephemeral` exists (5-min TTL, 1.25× write /
/// 0.1× read pricing). A future `Persistent` (1-hour TTL) variant
/// can land here without a schema break — the tagged-enum shape
/// is forward-compatible.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum CacheControl {
    Ephemeral,
}

// ---------------------------------------------------------------------------
// ContentBlock — structured message content
// ---------------------------------------------------------------------------

/// One content block inside a message.
///
/// Serde is implemented MANUALLY (not derived): the `ToolResult`
/// variant needs field-shape branching that derive cannot express —
/// when `resolved` carries pre-send base64 images (request-copy-only
/// form; see the variant docs), `content` must serialize as the
/// Anthropic-documented **block array** (`[{type:"image"},…,
/// {type:"text"}]`) instead of a plain string. Every other path must
/// stay byte-identical to the historical derive output (locked by
/// fixture tests in `tests_types.rs`).
#[derive(Debug, Clone, PartialEq)]
pub enum ContentBlock {
    Text {
        text: String,
        /// Optional Anthropic prompt-cache breakpoint. When `Some`,
        /// the wire layer preserves this block as a separate
        /// content block (does NOT concatenate it with adjacent
        /// text blocks) and the Anthropic adapter emits
        /// `cache_control: {"type": "ephemeral"}` next to the
        /// block. See [`CacheControl`] for the cost model.
        cache_control: Option<CacheControl>,
    },
    /// Anthropic extended-thinking content block. `thinking` is the streamed
    /// (or summarized, depending on `display`) summary text the model
    /// produces while reasoning; `signature` is the opaque, encrypted blob
    /// the model emits at the end of the block and which MUST be echoed
    /// back verbatim in subsequent turns — otherwise the API returns 400.
    Thinking { thinking: String, signature: String },
    /// Anthropic `redacted_thinking` block: emitted when the server
    /// encrypts part of a thinking block (e.g. for safety reasons). The
    /// `data` field is opaque, undisplayable, and MUST be echoed back
    /// verbatim in subsequent turns.
    RedactedThinking { data: String },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// `images`/`resolved` (08-21-b1-image-followups R4): tool-result
    /// images, two mutually-exclusive-by-construction forms:
    /// - `images: Some(refs)` — the **persisted** form (DB rows,
    ///   frontend rehydrate). File refs into the session attachments
    ///   dir; never base64 on disk. Serialized as an `images` field
    ///   next to the string `content`.
    /// - `resolved: Some(base64)` — the **request-copy-only** form set
    ///   by the pre-send resolve pass (same lifecycle as
    ///   `ImageRef → Image` for user images). Serialized as the
    ///   Anthropic-documented tool_result content **block array**
    ///   (image blocks first, text block last); `images` is NOT
    ///   emitted in this form. DB rows never carry `resolved`.
    /// - both `None` — the overwhelming majority: plain-text tool
    ///   results, serialized byte-identically to the pre-R4 derive
    ///   shape.
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
        images: Option<Vec<AttachmentRef>>,
        resolved: Option<Vec<ImageSource>>,
    },
    /// B1 (2026-08-16): stable image **reference** — a file name inside
    /// the session's attachments directory. This is the form that
    /// lives in history / metadata / group-chat rewrites: lightweight
    /// to clone and serializable without dragging megabytes of base64
    /// through C3 compaction estimates, `role_history` clones, or SSE
    /// payloads. Never sent as-is: the request builder resolves it to
    /// [`ContentBlock::Image`] right before `provider.send` (one disk
    /// read per turn; see `agent` image resolve pass). Serde tag
    /// `"image_ref"` is internal-only — it never appears on a provider
    /// wire.
    ImageRef { file: String, media_type: String },
    /// B1: resolved pre-send image (base64). Exists only in the
    /// request copy between the resolve pass and `provider.send`.
    /// Serializes as the Anthropic-native image block
    /// (`{"type":"image","source":{"type":"base64",…}}`) because the
    /// Anthropic adapter serde-serializes the reconstructed
    /// `ChatRequest` verbatim; the OpenAI adapter maps it to
    /// `image_url` with a data URL. When the model's
    /// `supports_images` cap is false, the wire strip pass replaces
    /// this block with a text placeholder instead of dropping it (the
    /// model must know an image was attached but not delivered).
    Image { source: ImageSource },
}

/// B1 (2026-08-16): Anthropic-shaped image source payload. The
/// `source_type` is always `"base64"` today; kept as a data field
/// (not an enum) so the serde shape matches Anthropic's wire exactly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

// ---------------------------------------------------------------------------
// ContentBlock manual Serde — see the enum docs for why derive is not
// used (ToolResult content string-or-array branching). Every non-ToolResult
// variant, and ToolResult with no images, must remain byte-identical to the
// historical derive output (tag = "type", snake_case, is_error/cache_control
// skip-if-default).
// ---------------------------------------------------------------------------

impl Serialize for ContentBlock {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(None)?;
        match self {
            ContentBlock::Text {
                text,
                cache_control,
            } => {
                map.serialize_entry("type", "text")?;
                map.serialize_entry("text", text)?;
                if let Some(cc) = cache_control {
                    map.serialize_entry("cache_control", cc)?;
                }
            }
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                map.serialize_entry("type", "thinking")?;
                map.serialize_entry("thinking", thinking)?;
                map.serialize_entry("signature", signature)?;
            }
            ContentBlock::RedactedThinking { data } => {
                map.serialize_entry("type", "redacted_thinking")?;
                map.serialize_entry("data", data)?;
            }
            ContentBlock::ToolUse { id, name, input } => {
                map.serialize_entry("type", "tool_use")?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("name", name)?;
                map.serialize_entry("input", input)?;
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                images,
                resolved,
            } => {
                map.serialize_entry("type", "tool_result")?;
                map.serialize_entry("tool_use_id", tool_use_id)?;
                match resolved {
                    // Request-copy HTTP form: content becomes the
                    // Anthropic-documented block array (images first,
                    // text last). `images` refs are NOT emitted in
                    // this form (the API rejects unknown fields).
                    Some(imgs) if !imgs.is_empty() => {
                        let mut arr: Vec<serde_json::Value> = Vec::with_capacity(imgs.len() + 1);
                        for src in imgs {
                            arr.push(serde_json::json!({
                                "type": "image",
                                "source": {
                                    "type": src.source_type,
                                    "media_type": src.media_type,
                                    "data": src.data,
                                },
                            }));
                        }
                        if !content.is_empty() {
                            arr.push(serde_json::json!({ "type": "text", "text": content }));
                        }
                        map.serialize_entry("content", &arr)?;
                    }
                    // Persisted/plain form: string content (+ refs
                    // when present, for DB rows and the frontend).
                    _ => {
                        map.serialize_entry("content", content)?;
                        if let Some(refs) = images {
                            map.serialize_entry("images", refs)?;
                        }
                    }
                }
                if *is_error {
                    map.serialize_entry("is_error", &true)?;
                }
            }
            ContentBlock::ImageRef { file, media_type } => {
                map.serialize_entry("type", "image_ref")?;
                map.serialize_entry("file", file)?;
                map.serialize_entry("media_type", media_type)?;
            }
            ContentBlock::Image { source } => {
                map.serialize_entry("type", "image")?;
                map.serialize_entry("source", source)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ContentBlock {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let val = serde_json::Value::deserialize(d)?;
        let obj = val
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("content block must be an object"))?;
        let tag = obj
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| serde::de::Error::custom("content block missing `type` tag"))?;
        let field = |name: &str| {
            obj.get(name)
                .cloned()
                .ok_or_else(|| serde::de::Error::custom(format!("missing field `{name}`")))
        };
        match tag {
            "text" => Ok(ContentBlock::Text {
                text: serde_json::from_value(field("text")?).map_err(serde::de::Error::custom)?,
                cache_control: match obj.get("cache_control") {
                    Some(v) => {
                        serde_json::from_value(v.clone()).map_err(serde::de::Error::custom)?
                    }
                    None => None,
                },
            }),
            "thinking" => Ok(ContentBlock::Thinking {
                thinking: serde_json::from_value(field("thinking")?)
                    .map_err(serde::de::Error::custom)?,
                signature: serde_json::from_value(field("signature")?)
                    .map_err(serde::de::Error::custom)?,
            }),
            "redacted_thinking" => Ok(ContentBlock::RedactedThinking {
                data: serde_json::from_value(field("data")?).map_err(serde::de::Error::custom)?,
            }),
            "tool_use" => Ok(ContentBlock::ToolUse {
                id: serde_json::from_value(field("id")?).map_err(serde::de::Error::custom)?,
                name: serde_json::from_value(field("name")?).map_err(serde::de::Error::custom)?,
                input: serde_json::from_value(field("input")?).map_err(serde::de::Error::custom)?,
            }),
            "tool_result" => {
                let tool_use_id: String = serde_json::from_value(field("tool_use_id")?)
                    .map_err(serde::de::Error::custom)?;
                // content: string (persisted/plain form) or block
                // array (HTTP form — parsed back into `resolved`
                // for symmetry; only tests ingest this form).
                let (content, resolved): (String, Option<Vec<ImageSource>>) = match obj
                    .get("content")
                {
                    Some(serde_json::Value::String(s)) => (s.clone(), None),
                    Some(serde_json::Value::Array(blocks)) => {
                        let mut imgs = Vec::new();
                        let mut text = String::new();
                        for b in blocks {
                            match b.get("type").and_then(|v| v.as_str()) {
                                Some("image") => {
                                    let src: ImageSource = b
                                        .get("source")
                                        .map(|v| serde_json::from_value(v.clone()))
                                        .transpose()
                                        .map_err(serde::de::Error::custom)?
                                        .ok_or_else(|| {
                                            serde::de::Error::custom("image block missing source")
                                        })?;
                                    imgs.push(src);
                                }
                                Some("text") => {
                                    if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                                        text.push_str(t);
                                    }
                                }
                                _ => {}
                            }
                        }
                        let resolved = if imgs.is_empty() { None } else { Some(imgs) };
                        (text, resolved)
                    }
                    _ => return Err(serde::de::Error::custom("tool_result missing `content`")),
                };
                Ok(ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error: obj
                        .get("is_error")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    images: match obj.get("images") {
                        Some(v) => {
                            serde_json::from_value(v.clone()).map_err(serde::de::Error::custom)?
                        }
                        None => None,
                    },
                    resolved,
                })
            }
            "image_ref" => Ok(ContentBlock::ImageRef {
                file: serde_json::from_value(field("file")?).map_err(serde::de::Error::custom)?,
                media_type: serde_json::from_value(field("media_type")?)
                    .map_err(serde::de::Error::custom)?,
            }),
            "image" => Ok(ContentBlock::Image {
                source: serde_json::from_value(field("source")?)
                    .map_err(serde::de::Error::custom)?,
            }),
            other => Err(serde::de::Error::custom(format!(
                "unknown content block type `{other}`"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// MessageContent — string-or-array wrapper
// ---------------------------------------------------------------------------

/// Message content that serializes as a plain string (step 1 compat) or an
/// array of [`ContentBlock`] (step 2+ tool calling; step 6+ thinking).
#[derive(Debug, Clone, PartialEq)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Extract all *visible* text from this content — used for the
    /// denormalized `text` column in the DB and for the session-list
    /// preview. **Thinking text is intentionally excluded** so that the
    /// sidebar preview only shows user-typed / assistant-said text and the
    /// persisted `text` field stays a useful search/index surface.
    pub fn to_text(&self) -> String {
        match self {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    /// Convenience: create a single-text-block content.
    #[allow(dead_code)]
    pub fn from_text(s: impl Into<String>) -> Self {
        MessageContent::Text(s.into())
    }
}

impl Serialize for MessageContent {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            MessageContent::Text(t) => s.serialize_str(t),
            MessageContent::Blocks(blocks) => blocks.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for MessageContent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let val = serde_json::Value::deserialize(d)?;
        match val {
            serde_json::Value::String(s) => Ok(MessageContent::Text(s)),
            other => {
                let blocks: Vec<ContentBlock> =
                    serde_json::from_value(other).map_err(serde::de::Error::custom)?;
                Ok(MessageContent::Blocks(blocks))
            }
        }
    }
}
