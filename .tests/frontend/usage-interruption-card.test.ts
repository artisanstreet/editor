import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../..");
const Read = (path: string) => readFileSync(resolve(workspace, path), "utf8");

describe("usage interruption card", () => {
  it("renders the durable interruption in transcript position, not as a trace diagnostic", () => {
    const item = Read("modules/frontend/src/routes/components/conversation-item.svelte");
    const route = Read("modules/frontend/src/routes/components/thread-route.svelte");

    expect(item).toContain('item.type === "usage_interruption"');
    expect(item).toContain("<ConversationUsageInterruptionCard");
    expect(route).toContain("onusageinterruptionresolve={ResolveUsageInterruption}");
  });

  it("uses a scoped absolute-time countdown and revisioned commands", () => {
    const card = Read(
      "modules/frontend/src/routes/components/conversation-usage-interruption-card.svelte",
    );
    const route = Read("modules/frontend/src/routes/components/thread-route.svelte");

    expect(card).toContain("Clock.currentTimeMillis");
    expect(card).toContain('Effect.sleep("1 second")');
    expect(card).toContain("Effect.forkScoped");
    expect(card).not.toContain("setInterval");
    expect(card).toContain('return "now"');
    expect(card).toContain("interruption.resets_at");
    expect(card).not.toContain("interruption.resume_not_before");
    expect(card).toContain("when available");
    expect(card).toContain("Switch to {alternative.display_name}");
    expect(card).toContain("DisplayModel(interruption.source_model_id)");
    expect(card).toContain('value.limit_scope === "model"');
    expect(card).toContain("usage limit was depleted");
    expect(route).toContain('type: "usage.interruption.resolve"');
    expect(route).toContain("expected_revision");
    expect(route).toContain("yield* Resync.pipe(Effect.ignore)");
  });

  it("persists the global default through the session-defaults controller", () => {
    const controller = Read("modules/frontend/src/lib/settings/session-defaults-controller.ts");
    const settings = Read("modules/frontend/src/routes/components/settings/usage-recovery.svelte");

    expect(controller).toContain("SetAutoContinueUsageLimits");
    expect(controller).toContain("auto_continue_usage_limits");
    expect(settings).toContain("Automatically continue after usage resets");
    expect(settings).toContain("SetAutoContinueUsageLimits");
  });
});
