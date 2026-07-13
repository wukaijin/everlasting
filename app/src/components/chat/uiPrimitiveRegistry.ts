// uiPrimitiveRegistry.ts — component registry for B9 generative UI
// primitives (Child A of 07-02-b9-generative-ui, 2026-07-02; B9+ D3
// added the `button` entry on 2026-07-13).
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
// as the fallback for unknown types. B9+ D3 adds the `button` entry
// for `<ButtonPrimitive>` (D-Q2a/b: predefined action enum dispatch).

import type { Component } from "vue";

import ButtonPrimitive from "./primitives/ButtonPrimitive.vue";
import CodeBlockPrimitive from "./primitives/CodeBlockPrimitive.vue";
import DiffPrimitive from "./primitives/DiffPrimitive.vue";
import MockPrimitive from "./primitives/MockPrimitive.vue";

/** `type` → component. Child B replaced `code_block`, Child C replaced
 *  `diff`. B9+ D3 (2026-07-13) replaced `button` with the real
 *  renderer. MockPrimitive stays as the fallback for unknown types. */
export const UI_PRIMITIVE_REGISTRY: Record<string, Component> = {
  diff: DiffPrimitive,
  code_block: CodeBlockPrimitive,
  // B9+ D3: action dispatcher (apply_diff / copy / dismiss).
  button: ButtonPrimitive,
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
