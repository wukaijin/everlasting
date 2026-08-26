// 08-26-custom-node-id — useRemoteConfigStore.saveNodeId / saveDisplayName
// 调用形状测试。覆盖:IPC 调用参数(camelCase 顶层 key,httpTransport 负责
// 扳 snake)、成功后 load + refreshStatus 联动刷新、空串原样透传(daemon
// 侧 = 清除)、daemon 校验错误向上抛(不刷新)。store 的其余包装
// (load/save/refreshStatus/generateCode)是纯转发,不重复测。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import { useRemoteConfigStore } from "./remoteConfig";

function makeConfig(overrides?: Record<string, unknown>) {
  return {
    remoteUrl: "wss://remote.example.com",
    sharedSecret: "s3cret",
    nodeId: null,
    displayName: null,
    ...overrides,
  };
}

describe("useRemoteConfigStore.saveNodeId", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("invokes set_tunnel_node_id then reloads config + status", async () => {
    invokeMock
      .mockResolvedValueOnce(undefined) // set_tunnel_node_id
      .mockResolvedValueOnce(makeConfig({ nodeId: "carlos-office" })) // get_remote_config
      .mockResolvedValueOnce(null); // get_tunnel_status

    const store = useRemoteConfigStore();
    await store.saveNodeId("carlos-office");

    expect(invokeMock).toHaveBeenNthCalledWith(1, "set_tunnel_node_id", {
      nodeId: "carlos-office",
    });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "get_remote_config");
    expect(invokeMock).toHaveBeenNthCalledWith(3, "get_tunnel_status");
    expect(store.config?.nodeId).toBe("carlos-office");
  });

  it("passes an empty string through (daemon-side clear semantics)", async () => {
    invokeMock
      .mockResolvedValueOnce(undefined) // set_tunnel_node_id
      .mockResolvedValueOnce(makeConfig()) // get_remote_config
      .mockResolvedValueOnce(null); // get_tunnel_status

    const store = useRemoteConfigStore();
    await store.saveNodeId("");

    expect(invokeMock).toHaveBeenNthCalledWith(1, "set_tunnel_node_id", {
      nodeId: "",
    });
    expect(store.config?.nodeId).toBeNull();
  });

  it("propagates daemon validation errors (no state refresh)", async () => {
    invokeMock.mockRejectedValueOnce(new Error("InvalidRequest"));
    const store = useRemoteConfigStore();
    await expect(store.saveNodeId("Carlos_Office")).rejects.toThrow("InvalidRequest");
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });
});

describe("useRemoteConfigStore.saveDisplayName", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("invokes set_tunnel_display_name then reloads config + status", async () => {
    invokeMock
      .mockResolvedValueOnce(undefined) // set_tunnel_display_name
      .mockResolvedValueOnce(makeConfig({ displayName: "公司台式机" })) // get_remote_config
      .mockResolvedValueOnce(null); // get_tunnel_status

    const store = useRemoteConfigStore();
    await store.saveDisplayName("公司台式机");

    expect(invokeMock).toHaveBeenNthCalledWith(1, "set_tunnel_display_name", {
      displayName: "公司台式机",
    });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "get_remote_config");
    expect(invokeMock).toHaveBeenNthCalledWith(3, "get_tunnel_status");
    expect(store.config?.displayName).toBe("公司台式机");
  });

  it("passes an empty string through (daemon-side clear semantics)", async () => {
    invokeMock
      .mockResolvedValueOnce(undefined) // set_tunnel_display_name
      .mockResolvedValueOnce(makeConfig()) // get_remote_config
      .mockResolvedValueOnce(null); // get_tunnel_status

    const store = useRemoteConfigStore();
    await store.saveDisplayName("");

    expect(invokeMock).toHaveBeenNthCalledWith(1, "set_tunnel_display_name", {
      displayName: "",
    });
    expect(store.config?.displayName).toBeNull();
  });

  it("propagates daemon validation errors (no state refresh)", async () => {
    invokeMock.mockRejectedValueOnce(new Error("InvalidRequest"));
    const store = useRemoteConfigStore();
    await expect(store.saveDisplayName("x".repeat(65))).rejects.toThrow("InvalidRequest");
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });
});
