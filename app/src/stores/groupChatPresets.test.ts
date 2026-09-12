// Tests for `stores/groupChatPresets.ts` — mergedPresets 合并视图
// (GCE-P1, task `09-12-gc-preset-settings`,design §6 前端单测行;
// 覆盖层 GCE-P1b, task `09-12-gc-preset-override`)。
//
// 契约:
//   1. 内置四档在前,键序 = JSON 声明序(review / fe_review / arch /
//      retro),`builtin: true`,model 字段保持名字(内置档现状不动)。
//   2. 用户行按 name 字典序追加,`builtin: false`,UUID 直进 model 字段
//      (moderator_model = moderatorModelId;participants[].model =
//      modelId)—— resolveModelRef byId 首趟直配的机制前提。
//   3. key 不撞:内置 = JSON key,用户 = 行 id(UUID);同一 Record 无
//      覆盖。
//   4. 覆盖行(builtinKey 非空)原位顶替对应内置槽:key/name 保持内置
//      key、overriddenBy = 行 id、阵容 = 行内容;不占用户档新键。未知
//      builtinKey(未来 JSON 删 key 的存量脏行)跳过。
//   5. load / ensureLoaded 幂等语义 + create/update/remove 重拉列表;
//      create 载荷恒带 builtinKey(null = 普通行)。
import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import { useGroupChatPresetsStore, type GcPresetRow } from "./groupChatPresets";
import { GC_PRESETS, resolveModelRef } from "../utils/groupChatPresets";

function row(overrides: Partial<GcPresetRow> = {}): GcPresetRow {
  return {
    id: "uuid-row-1",
    name: "我的评审团",
    description: "自定义阵容",
    moderatorModelId: "uuid-m1",
    participants: [
      { name: "架构", modelId: "uuid-m2", persona: "arch" },
      { name: "后端", modelId: "uuid-m3", persona: "backend" },
    ],
    createdAt: "2026-09-12T00:00:00Z",
    updatedAt: "2026-09-12T00:00:00Z",
    ...overrides,
  };
}

beforeEach(() => {
  setActivePinia(createPinia());
  invokeMock.mockReset();
  invokeMock.mockResolvedValue([]);
});

describe("groupChatPresets store — mergedPresets 合并视图", () => {
  it("内置四档在前:builtin=true、键序 = JSON 声明序、model 字段保持名字", () => {
    const store = useGroupChatPresetsStore();
    const merged = store.mergedPresets;
    const keys = Object.keys(merged);
    expect(keys).toEqual(Object.keys(GC_PRESETS.presets));
    expect(keys).toEqual(["review", "fe_review", "arch", "retro"]);
    for (const key of keys) {
      expect(merged[key]).toMatchObject({
        key,
        builtin: true,
        name: key,
        description: GC_PRESETS.presets[key]!.description,
        moderator_model: GC_PRESETS.presets[key]!.moderator_model,
      });
    }
  });

  it("用户行按 name 字典序追加:builtin=false、UUID 直进 model 字段、key = 行 id 不撞内置", () => {
    const store = useGroupChatPresetsStore();
    // 故意乱序插入,验证 getter 里的防御性 name 排序(后端本就 ORDER BY name)。
    // 名字用 ASCII 前缀保证任意 collation 下字典序稳定(CJK 默认按码点排,
    // 拼音序不可依赖)。
    store.rows = [
      row({ id: "uuid-b", name: "beta 阵容" }),
      row({
        id: "uuid-a",
        name: "alpha 阵容",
        moderatorModelId: "uuid-x1",
        participants: [
          { name: "产品", modelId: "uuid-x2", persona: "product" },
          { name: "局外", modelId: "uuid-x3", persona: "outsider" },
        ],
      }),
    ];
    const keys = Object.keys(store.mergedPresets);
    // 内置在前(声明序),用户行 name 序追加在后。
    expect(keys).toEqual([
      "review",
      "fe_review",
      "arch",
      "retro",
      "uuid-a",
      "uuid-b",
    ]);
    const a = store.mergedPresets["uuid-a"]!;
    expect(a.builtin).toBe(false);
    expect(a.name).toBe("alpha 阵容");
    expect(a.moderator_model).toBe("uuid-x1");
    expect(a.participants).toEqual([
      { name: "产品", model: "uuid-x2", persona: "product" },
      { name: "局外", model: "uuid-x3", persona: "outsider" },
    ]);
  });

  it("机制前提:UUID 进 model 字段后 resolveModelRef byId 首趟直配(零新分支)", () => {
    const store = useGroupChatPresetsStore();
    store.rows = [row({ id: "uuid-a", name: "甲" })];
    const def = store.mergedPresets["uuid-a"]!;
    const catalog = [
      { id: "uuid-m1", modelName: "m1", displayName: "M1" },
      { id: "uuid-m2", modelName: "m2", displayName: "M2" },
    ];
    expect(def.moderator_model).toBe("uuid-m1");
    expect(resolveModelRef(catalog, def.moderator_model)).toBe("uuid-m1");
    expect(resolveModelRef(catalog, def.participants[0]!.model)).toBe("uuid-m2");
  });

  // ---------------------------------------------------------------------
  // GCE-P1b:覆盖行原位顶替内置槽(design §4.1 核心机制)。
  // ---------------------------------------------------------------------

  it("覆盖行原位顶替内置槽:阵容 = 行内容、key/name 保持内置 key、overriddenBy = 行 id、不追加用户档键", () => {
    const store = useGroupChatPresetsStore();
    const archDef = GC_PRESETS.presets["arch"]!;
    store.rows = [
      row({
        id: "uuid-ov-1",
        name: "arch 本机覆盖", // display 名与链接键分离;merged 不带出。
        moderatorModelId: "uuid-x1",
        participants: [
          { name: "修复者", modelId: "uuid-x2", persona: "product" },
          { name: "局外", modelId: "uuid-x3", persona: "outsider" },
        ],
        builtinKey: "arch",
      }),
    ];
    const merged = store.mergedPresets;
    // 键集合不变:内置四 key,覆盖行不追加新键(无 uuid-ov-1)。
    expect(Object.keys(merged)).toEqual(["review", "fe_review", "arch", "retro"]);
    expect(merged["uuid-ov-1"]).toBeUndefined();
    const arch = merged["arch"]!;
    expect(arch.builtin).toBe(true);
    expect(arch.key).toBe("arch");
    expect(arch.name).toBe("arch");
    expect(arch.overriddenBy).toBe("uuid-ov-1");
    // 阵容 = 行内容(UUID 借道 byId 首趟),≠ JSON def。
    expect(arch.moderator_model).toBe("uuid-x1");
    expect(arch.participants).toEqual([
      { name: "修复者", model: "uuid-x2", persona: "product" },
      { name: "局外", model: "uuid-x3", persona: "outsider" },
    ]);
    expect(arch.moderator_model).not.toBe(archDef.moderator_model);
    // 未覆盖槽原样(JSON def)。
    expect(merged["retro"]!.moderator_model).toBe(GC_PRESETS.presets["retro"]!.moderator_model);
  });

  it("覆盖行与普通行共存:普通行照旧追加,覆盖只动对应槽", () => {
    const store = useGroupChatPresetsStore();
    store.rows = [
      row({ id: "uuid-plain", name: "普通档" }),
      row({ id: "uuid-ov", name: "覆盖档", builtinKey: "retro" }),
    ];
    const keys = Object.keys(store.mergedPresets);
    expect(keys).toEqual(["review", "fe_review", "arch", "retro", "uuid-plain"]);
    expect(store.mergedPresets["retro"]!.overriddenBy).toBe("uuid-ov");
    expect(store.mergedPresets["uuid-plain"]!.builtin).toBe(false);
  });

  it("未知 builtinKey(JSON 删 key 的存量脏行)→ 跳过,不当用户档追加", () => {
    const store = useGroupChatPresetsStore();
    store.rows = [row({ id: "uuid-dirty", name: "脏行", builtinKey: "nope" })];
    const merged = store.mergedPresets;
    expect(Object.keys(merged)).toEqual(["review", "fe_review", "arch", "retro"]);
    expect(merged["nope"]).toBeUndefined();
    expect(merged["uuid-dirty"]).toBeUndefined();
    // 悬空脏行不顶替任何槽:四档全为 JSON 原样,无 overriddenBy。
    for (const key of ["review", "fe_review", "arch", "retro"] as const) {
      expect(merged[key]!.overriddenBy).toBeUndefined();
      expect(merged[key]!.moderator_model).toBe(GC_PRESETS.presets[key]!.moderator_model);
    }
  });

  it("覆盖行 create 载荷透传 builtinKey(builtinKey ?? null)", async () => {
    const store = useGroupChatPresetsStore();
    invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "list_group_chat_presets"
        ? [row({ id: "uuid-ov", name: "覆盖档", builtinKey: "arch" })]
        : cmd === "create_group_chat_preset"
          ? row({ id: "uuid-ov", name: "覆盖档", builtinKey: "arch" })
          : null,
    );
    await store.create({
      name: "覆盖档",
      description: "",
      moderatorModelId: "uuid-m1",
      participants: [],
      builtinKey: "arch",
    });
    expect(invokeMock).toHaveBeenCalledWith("create_group_chat_preset", {
      name: "覆盖档",
      description: "",
      moderatorModelId: "uuid-m1",
      participants: [],
      builtinKey: "arch",
    });
    expect(store.rows[0]!.builtinKey).toBe("arch");
  });

  it("load 拉全量并置 loaded;ensureLoaded 未加载才拉(幂等)", async () => {
    const store = useGroupChatPresetsStore();
    invokeMock.mockResolvedValue([row()]);
    await store.ensureLoaded();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("list_group_chat_presets", {});
    expect(store.loaded).toBe(true);
    expect(store.rows).toHaveLength(1);
    // 已加载 → 零 IPC。
    await store.ensureLoaded();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    // load() 总是重拉(管理面显式刷新入口)。
    await store.load();
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it("create / update / remove 走对应命令并重拉列表", async () => {
    const store = useGroupChatPresetsStore();
    invokeMock.mockResolvedValue([row()]);
    await store.ensureLoaded();
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "list_group_chat_presets"
        ? [row(), row({ id: "uuid-new", name: "新阵容" })]
        : cmd === "create_group_chat_preset"
          ? row({ id: "uuid-new", name: "新阵容" })
          : null,
    );
    await store.create({
      name: "新阵容",
      description: "",
      moderatorModelId: "uuid-m1",
      participants: [
        { name: "架构", modelId: "uuid-m2", persona: "arch" },
        { name: "后端", modelId: "uuid-m3", persona: "backend" },
      ],
    });
    expect(invokeMock).toHaveBeenCalledWith("create_group_chat_preset", {
      name: "新阵容",
      description: "",
      moderatorModelId: "uuid-m1",
      participants: [
        { name: "架构", modelId: "uuid-m2", persona: "arch" },
        { name: "后端", modelId: "uuid-m3", persona: "backend" },
      ],
      // 普通行恒显式 null(Rust Option,None = 无 builtin_key 列)。
      builtinKey: null,
    });
    expect(invokeMock).toHaveBeenCalledWith("list_group_chat_presets", {});
    expect(store.rows).toHaveLength(2);

    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "list_group_chat_presets" ? [row()] : cmd === "update_group_chat_preset" ? row() : null,
    );
    await store.update("uuid-row-1", {
      name: "改名",
      description: "",
      moderatorModelId: "uuid-m1",
      participants: [],
    });
    expect(invokeMock).toHaveBeenCalledWith(
      "update_group_chat_preset",
      expect.objectContaining({ id: "uuid-row-1", name: "改名" }),
    );

    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "list_group_chat_presets" ? [] : { ok: true },
    );
    await store.remove("uuid-row-1");
    expect(invokeMock).toHaveBeenCalledWith("delete_group_chat_preset", {
      id: "uuid-row-1",
    });
    expect(store.rows).toEqual([]);
  });

  it("update / remove 在途置行级 spinner,finally 必删", async () => {
    const store = useGroupChatPresetsStore();
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "update_group_chat_preset") await gate;
      if (cmd === "list_group_chat_presets") return [];
      return null;
    });
    const pending = store.update("uuid-row-1", {
      name: "x",
      description: "",
      moderatorModelId: "m",
      participants: [],
    });
    // invoke 已进入(gate 前)→ spinner 在;await 一轮微任务确保进入。
    await Promise.resolve();
    await Promise.resolve();
    expect(store.spinnerById.has("uuid-row-1")).toBe(true);
    release();
    await pending;
    expect(store.spinnerById.has("uuid-row-1")).toBe(false);
  });

  it("服务端错误经 extractErrorMessage 重抛(可读 message)", async () => {
    const store = useGroupChatPresetsStore();
    invokeMock.mockImplementation(async () => {
      const err = new Error("预设名称「review」与内置预设冲突,请换一个名称");
      throw err;
    });
    await expect(
      store.create({
        name: "review",
        description: "",
        moderatorModelId: "m",
        participants: [],
      }),
    ).rejects.toThrow("与内置预设冲突");
  });
});
