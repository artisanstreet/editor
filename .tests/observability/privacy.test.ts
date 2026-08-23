import { describe, expect, it } from "vitest";

import {
  ContainsTelemetryCanary,
  SanitizeSentryBreadcrumb,
  SanitizeSentryEvent,
  SanitizeTelemetryText,
} from "@artisan/observability";

const canary = "ARTISAN_TELEMETRY_FORBIDDEN_CANARY";

describe("observability privacy", () => {
  it("scrubs credentials, URLs, emails, and Windows/POSIX paths", () => {
    const input =
      "Bearer super-secret sk-12345678901234567890 at https://example.test/private?q=token from sander@example.test C:\\Users\\sander\\repo\\secret.ts /home/sander/repo/secret.ts postgres://user:pass@db/private";
    const sanitized = SanitizeTelemetryText(input);

    expect(sanitized).not.toContain("super-secret");
    expect(sanitized).not.toContain("sk-123");
    expect(sanitized).not.toContain("example.test/private");
    expect(sanitized).not.toContain("sander@example.test");
    expect(sanitized).not.toContain("Users");
    expect(sanitized).not.toContain("/home/sander");
    expect(sanitized).not.toContain("user:pass");
    expect(sanitized).toContain("[REDACTED]");
  });

  it("detects a forbidden canary at any bounded nested depth", () => {
    expect(ContainsTelemetryCanary({ nested: [{ value: canary }] })).toBe(true);
    expect(ContainsTelemetryCanary({ safe: "value" })).toBe(false);
  });

  it("drops a whole Sentry event if the canary survives anywhere", () => {
    expect(
      SanitizeSentryEvent({
        exception: { values: [{ type: "Error", value: "generic" }] },
        extra: { nested: { private: canary } },
      }),
    ).toBeNull();
  });

  it("keeps actionable frame coordinates while removing arbitrary payloads and paths", () => {
    const sanitized = SanitizeSentryEvent({
      breadcrumbs: [
        { category: "console", message: "private prompt C:\\Users\\sander\\repo" },
        {
          category: "artisan.lifecycle",
          data: { operation: "forge.startup", path: "C:\\private", token: "secret" },
          message: "startup",
        },
      ],
      debug_meta: {
        images: [
          {
            code_file: "C:\\Users\\sander\\AppData\\Artisan\\main.js",
            debug_id: "a5fcb580-fefa-5699-87cb-fa76ffc1760d",
            type: "sourcemap",
          },
          {
            code_file: "private-project.js",
            debug_id: "11111111-1111-1111-1111-111111111111",
            type: "sourcemap",
          },
          {
            code_file: "https://private.example/_app/immutable/chunks/AbCd1234.js?token=TOP_SECRET",
            debug_id: "22222222-2222-2222-2222-222222222222",
            type: "sourcemap",
          },
          {
            code_file: "/home/alice/private/_app/customer-name/secret-file.js",
            debug_id: "33333333-3333-3333-3333-333333333333",
            type: "sourcemap",
          },
        ],
      },
      environment: "production",
      exception: {
        values: [
          {
            stacktrace: {
              frames: [
                {
                  abs_path: "C:\\Users\\sander\\AppData\\Artisan\\main.js",
                  colno: 12,
                  filename: "C:\\Users\\sander\\AppData\\Artisan\\main.js",
                  function: "privateCustomerFunction",
                  in_app: true,
                  lineno: 42,
                  module: "private-customer-module",
                },
              ],
            },
            type: "DatabaseInvariantError",
            value: "failed at C:\\Users\\sander\\secret.db with Bearer secret",
          },
        ],
      },
      extra: { cause: { prompt: "private" }, env: { API_KEY: "secret" } },
      level: "error",
      platform: "node",
      request: { data: "private body", headers: { authorization: "secret" } },
      release: "artisan-editor@1.2.3+abc",
      server_name: "SANDER-PC",
      tags: {
        arch: "x64",
        artisan_code: "database_invariant",
        project_name: "private-project",
        runtime: "electron_main",
      },
      user: { email: "sander@example.test" },
    });

    expect(sanitized).not.toBeNull();
    expect(JSON.stringify(sanitized)).not.toMatch(
      /private|Users|secret\.db|Bearer|authorization|API_KEY|SANDER-PC|example\.test/iu,
    );
    expect(sanitized).toMatchObject({
      debug_meta: {
        images: [
          {
            code_file: "app:///main.js",
            debug_id: "a5fcb580-fefa-5699-87cb-fa76ffc1760d",
            type: "sourcemap",
          },
          {
            code_file: "app:///_app/immutable/chunks/AbCd1234.js",
            debug_id: "22222222-2222-2222-2222-222222222222",
            type: "sourcemap",
          },
        ],
      },
      environment: "production",
      exception: {
        values: [
          {
            stacktrace: {
              frames: [
                {
                  abs_path: "app:///main.js",
                  colno: 12,
                  filename: "app:///main.js",
                  in_app: true,
                  lineno: 42,
                },
              ],
            },
            type: "Error",
            value: "[SANITIZED]",
          },
        ],
      },
      level: "error",
      platform: "node",
      release: "artisan-editor@1.2.3+abc",
      tags: {
        arch: "x64",
        artisan_code: "database_invariant",
        runtime: "electron_main",
      },
    });
    expect(sanitized).not.toHaveProperty("request");
    expect(sanitized).not.toHaveProperty("user");
    expect(sanitized).not.toHaveProperty("extra");
    expect(sanitized).not.toHaveProperty("server_name");
    expect(sanitized?.breadcrumbs).toHaveLength(1);
  });

  it("keeps only closed custom lifecycle breadcrumbs", () => {
    expect(
      SanitizeSentryBreadcrumb({ category: "fetch", data: { url: "https://private" } }),
    ).toBeNull();
    expect(
      SanitizeSentryBreadcrumb({
        category: "artisan.lifecycle",
        data: { artisan_code: "startup_failed", operation: "forge.startup", path: "private" },
        level: "error",
        message: "startup",
      }),
    ).toEqual({
      category: "artisan.lifecycle",
      data: { artisan_code: "startup_failed", operation: "forge.startup" },
      level: "error",
    });
  });
});
