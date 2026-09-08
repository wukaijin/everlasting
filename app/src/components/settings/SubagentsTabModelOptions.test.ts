// SubagentsTab / ProjectSubagentsTab — provider/model 禁用过滤的行级
// 回显语义(09-09-gc-disabled-model-leak)。
//
// 契约(09-07 provider-model-disable PRD R2 + 09-09 修正):模型下拉选项 =
// 启用模型 ∪ **本行**当前已解析 id。原实现把所有行的已选值收进一个全局
// pinned Set —— 任一行指向禁用模型,每行下拉都会提供它,别行可把禁用
// 模型新选进来;本文件锁死该回归点:
//   - 行 A(指向禁用模型 m-dis):A 自己的下拉回显 m-dis(可见可切走);
//   - 行 B(inherit):B 的下拉不提供 m-dis。
//
// 选项断言走键盘打开(Enter ∈ reka OPEN_KEYS;jsdom pointer capture
// stub 见 beforeAll —— ScheduledTasksTab.test.ts 同款),SelectContent
// teleport 到 document.body,全局查 [role=option]。

import { describe, it, expect, beforeAll, afterEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createPinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

// ProjectSubagentsTab 经 projects.projectById(props.projectId).path 发起
// 拉取;SubagentsTab 只用 showToast。ScheduledTasksTab.test.ts 同款 mock。
const showToastMock = vi.fn();
vi.mock("../../stores/projects", () => ({
  useProjectsStore: () => ({
    showToast: showToastMock,
    projectById: (id: string) =>
      id === "p1" ? { id: "p1", name: "alpha", path: "/tmp/alpha" } : undefined,
  }),
}));

import SubagentsTab from "./SubagentsTab.vue";
import ProjectSubagentsTab from "./ProjectSubagentsTab.vue";
import type { SubagentWithModelRow } from "../../stores/subagents";

/** 目录:md = 禁用(模型级),me = 启用。 */
const CATALOG = [
  {
    id: "md",
    providerId: "p1",
    modelName: "gpt-4",
    displayName: "GPT-4",
    maxTokens: null,
    thinkingEffort: null,
    supportsThinking: false,
    supportsImages: false,
    contextWindow: 128000,
    disabled: true,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    providerDisplayName: "OpenAI",
    providerProtocol: "openai",
  },
  {
    id: "me",
    providerId: "p2",
    modelName: "claude-3-5",
    displayName: "Claude 3.5",
    maxTokens: null,
    thinkingEffort: null,
    supportsThinking: true,
    supportsImages: true,
    contextWindow: 200000,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    providerDisplayName: "Anthropic",
    providerProtocol: "anthropic",
  },
];

function agentRow(
  name: string,
  resolvedModelId: string | null,
  resolvedModelDisplay: string | null,
  source: SubagentWithModelRow["source"] = "builtin",
): SubagentWithModelRow {
  return {
    name,
    description: "",
    source,
    tools: [],
    resolvedModelId,
    resolvedModelDisplay,
    declaredModelId: null,
    hasDbOverride: resolvedModelId !== null,
    writable: source !== "builtin",
  };
}

/** a-row 指向禁用模型 md;b-row inherit(null)。按名排序 a 在前。 */
function rowsFor(source: SubagentWithModelRow["source"]): SubagentWithModelRow[] {
  return [
    agentRow("a-row", "md", "GPT-4", source),
    agentRow("b-row", null, null, source),
  ];
}

function stubBackend(source: SubagentWithModelRow["source"]) {
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "list_models") return CATALOG;
    if (cmd === "get_default_model") return null;
    if (cmd === "list_subagents_with_model") return rowsFor(source);
    return null;
  });
}

/** 键盘打开第 rowIdx 行模型下拉,快照弹层 option 文本。行序 = sortedRows
 * (按名排序,a-row=0)。 */
async function openRowOptions(
  w: ReturnType<typeof mount>,
  rowIdx: number,
): Promise<string[]> {
  const triggers = w.findAll('[aria-label="Model"]');
  expect(triggers.length).toBeGreaterThan(rowIdx);
  triggers[rowIdx].element.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
  );
  await flushPromises();
  await flushPromises();
  return Array.from(document.querySelectorAll('[role="option"]')).map(
    (el) => el.textContent ?? "",
  );
}

beforeAll(() => {
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("SubagentsTab — 禁用模型行级回显(09-09)", () => {
  it("行指向禁用模型:本行下拉回显它(可见可切走)", async () => {
    stubBackend("builtin");
    const w = mount(SubagentsTab, { global: { plugins: [createPinia()] } });
    await flushPromises();
    const options = await openRowOptions(w, 0); // a-row → md
    expect(options.some((t) => t.includes("GPT-4"))).toBe(true);
    expect(options.some((t) => t.includes("Claude 3.5"))).toBe(true);
    w.unmount();
  });

  it("inherit 行的下拉不提供禁用模型(跨行泄漏修复)", async () => {
    stubBackend("builtin");
    const w = mount(SubagentsTab, { global: { plugins: [createPinia()] } });
    await flushPromises();
    const options = await openRowOptions(w, 1); // b-row → null
    expect(options.some((t) => t.includes("Claude 3.5"))).toBe(true);
    expect(options.some((t) => t.includes("GPT-4"))).toBe(false);
    w.unmount();
  });
});

describe("ProjectSubagentsTab — 禁用模型行级回显(09-09)", () => {
  function mountProject() {
    return mount(ProjectSubagentsTab, {
      props: { projectId: "p1" },
      global: { plugins: [createPinia()] },
    });
  }

  it("行指向禁用模型:本行下拉回显它;inherit 行不提供", async () => {
    stubBackend("project");
    const w = mountProject();
    await flushPromises();

    const own = await openRowOptions(w, 0); // a-row → md
    expect(own.some((t) => t.includes("GPT-4"))).toBe(true);

    // 两次挂载避免两个弹层同时在 body 里互相污染全局查询。
    w.unmount();
    document.body.innerHTML = "";
    const w2 = mountProject();
    await flushPromises();
    const other = await openRowOptions(w2, 1); // b-row → null
    expect(other.some((t) => t.includes("GPT-4"))).toBe(false);
    expect(other.some((t) => t.includes("Claude 3.5"))).toBe(true);
    w2.unmount();
  });
});
