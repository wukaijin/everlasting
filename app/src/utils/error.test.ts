// Tests for `categoryRetryable()` / `categoryToastKey()` helpers
// — A5 R2 (2026-07-17)。
//
// 锁定契约:
// 1. categoryRetryable 5 类 × 期望值对照后端 AppError::retryable() 默认派生
// 2. categoryToastKey 把 5 类映射到 4 类 toast key(InvalidRequest → null)
// 3. 输入容错(null / undefined / 未知值)
// 4. PascalCase (`useErrorBus`) + snake_case (`message.error.category`)
//    两套 case 都接受(同一语义)

import { describe, it, expect } from "vitest";
import { categoryRetryable, categoryToastKey } from "./error";

describe("categoryRetryable — 5 类派生 (snake_case wire format)", () => {
  it("auth → false", () => {
    expect(categoryRetryable("auth")).toBe(false);
  });
  it("rate_limit → true", () => {
    expect(categoryRetryable("rate_limit")).toBe(true);
  });
  it("invalid_request → false", () => {
    expect(categoryRetryable("invalid_request")).toBe(false);
  });
  it("server → true", () => {
    expect(categoryRetryable("server")).toBe(true);
  });
  it("network → true", () => {
    expect(categoryRetryable("network")).toBe(true);
  });
});

describe("categoryRetryable — 5 类派生 (PascalCase, useErrorBus format)", () => {
  it("Auth → false", () => {
    expect(categoryRetryable("Auth")).toBe(false);
  });
  it("RateLimit → true", () => {
    expect(categoryRetryable("RateLimit")).toBe(true);
  });
  it("InvalidRequest → false", () => {
    expect(categoryRetryable("InvalidRequest")).toBe(false);
  });
  it("Server → true", () => {
    expect(categoryRetryable("Server")).toBe(true);
  });
  it("Network → true", () => {
    expect(categoryRetryable("Network")).toBe(true);
  });
});

describe("categoryRetryable — 输入容错", () => {
  it("null / undefined → false", () => {
    expect(categoryRetryable(null)).toBe(false);
    expect(categoryRetryable(undefined)).toBe(false);
  });
  it("空串 → false", () => {
    expect(categoryRetryable("")).toBe(false);
  });
  it("未知值 → false(defensive)", () => {
    expect(categoryRetryable("Bogus")).toBe(false);
    expect(categoryRetryable("weird_value")).toBe(false);
  });
});

describe("categoryToastKey — 5 类 → 4 类 toast key (snake_case)", () => {
  it("auth / rate_limit / server / network → 4 类 toast key", () => {
    expect(categoryToastKey("auth")).toBe("Auth");
    expect(categoryToastKey("rate_limit")).toBe("RateLimit");
    expect(categoryToastKey("server")).toBe("Server");
    expect(categoryToastKey("network")).toBe("Network");
  });
  it("invalid_request → null(不弹全局 toast)", () => {
    expect(categoryToastKey("invalid_request")).toBeNull();
  });
});

describe("categoryToastKey — 5 类 → 4 类 toast key (PascalCase, useErrorBus)", () => {
  it("Auth / RateLimit / Server / Network → 4 类 toast key", () => {
    expect(categoryToastKey("Auth")).toBe("Auth");
    expect(categoryToastKey("RateLimit")).toBe("RateLimit");
    expect(categoryToastKey("Server")).toBe("Server");
    expect(categoryToastKey("Network")).toBe("Network");
  });
  it("InvalidRequest → null(不弹全局 toast)", () => {
    expect(categoryToastKey("InvalidRequest")).toBeNull();
  });
});

describe("categoryToastKey — 输入容错", () => {
  it("未知 / null / undefined → null(不弹 toast)", () => {
    expect(categoryToastKey("Bogus")).toBeNull();
    expect(categoryToastKey(null)).toBeNull();
    expect(categoryToastKey(undefined)).toBeNull();
  });
});
