import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const source = (path: string) => readFileSync(new URL(`../../${path}`, import.meta.url), "utf8");

describe("frontend observability source contract", () => {
  it("emits the protocol's closed editor-session intent without blocking runtime startup", () => {
    const bootstrap = source("modules/frontend/src/lib/telemetry/product-bootstrap.ts");
    expect(bootstrap).toContain('event: "editor_session_started"');
    expect(bootstrap).toContain('surface === "desktop" ? "desktop_renderer" : "browser_renderer"');
    expect(bootstrap).toContain("Effect.forkScoped");
    expect(bootstrap).not.toContain('kind: "editor_session_started"');
    expect(bootstrap).not.toContain("Layer.scopedDiscard");
  });

  it("starts consent lookup in a scoped background fiber and disables browser defaults", () => {
    const monitoring = source("modules/frontend/src/lib/telemetry/renderer-sentry.ts");
    expect(monitoring).toContain("Effect.forkIn(StartMonitoring, layer_scope)");
    expect(monitoring).toContain('Stream.filter((next) => next.crash_reports === "enabled")');
    expect(monitoring).toContain("Stream.runHead");
    expect(monitoring).toContain("const InitializeWithRetry:");
    expect(monitoring).toContain("Effect.suspend(() =>");
    expect(monitoring).toContain("Effect.sleep(Duration.seconds(2))");
    expect(monitoring).toContain("Cause.hasInterrupts(cause)");
    expect(monitoring).toContain("defaultIntegrations: false");
    expect(monitoring).toContain("beforeBreadcrumb: () => null");
    expect(monitoring).toContain("sendClientReports: false");
    expect(monitoring).not.toContain("Layer.scoped(");
  });

  it("serializes preference refresh with consent mutations", () => {
    const controller = source("modules/frontend/src/lib/settings/telemetry-controller.ts");
    expect(controller).toContain("const Refresh = mutation_lock.withPermit(");
  });
});
