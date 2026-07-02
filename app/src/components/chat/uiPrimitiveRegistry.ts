// uiPrimitiveRegistry.ts — component registry for B9 generative UI
// primitives (Child A of 07-02-b9-generative-ui, 2026-07-02).
//
// Maps `primitive.type` → Vue component. `<UiCard>` resolves the
// renderer here; an unknown type degrades to the fallback (not a
// crash). Adding a new primitive type = adding one entry — the
// dispatch logic in UiCard never changes.
//
// MVP (Child A): every type maps to `<MockPrimitive>` so the pipeline
// can be validated end-to-end before real renderers exist. Child B
// (code_block → hljs) and Child C (diff → reuses DiffView) each
// replace their entry with the real component; MockPrimitive stays
// as the fallback for unknown types.

import type { Component } from "vue";

import CodeBlockPrimitive from "./primitives/CodeBlockPrimitive.vue";
import MockPrimitive from "./primitives/MockPrimitive.vue";

/** `type` → component. Child B replaced `code_block`; Child C will
 *  replace `diff`. */
export const UI_PRIMITIVE_REGISTRY: Record<string, Component> = {
  diff: MockPrimitive,
  code_block: CodeBlockPrimitive,
};

/** Fallback for types not in the registry (e.g. a hallucinated type
 *  that slipped past backend validation, or a stale message from
 *  before a type was renamed). Renders as a degraded card rather
 *  than crashing the message stream. */
export const UI_PRIMITIVE_FALLBACK: Component = MockPrimitive;

/** Resolve a primitive type to its renderer, or the fallback. */
export function resolveUiPrimitive(type: string): Component {
  return UI_PRIMITIVE_REGISTRY[type] ?? UI_PRIMITIVE_FALLBACK;
}
