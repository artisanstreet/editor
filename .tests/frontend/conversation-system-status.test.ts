import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../..");
const Read = (path: string) => readFileSync(resolve(workspace, path), "utf8");

describe("conversation system status", () => {
	it("renders a model transition with both engine marks and catalog model names", () => {
		const status = Read("modules/frontend/src/routes/components/conversation-status.sv");

		expect(status).toContain('{#if item.type === "model_transition"}');
		expect(status).toContain("<span>Changed</span>");
		expect(status).toContain("<span>for</span>");
		expect(status).toContain("model_name_for(item.source_engine_id, item.source_model_id)");
		expect(status).toContain("model_name_for(item.target_engine_id, item.target_model_id)");
		expect(status).toContain("candidate.native_model_id === model_id");
		expect(status).toContain("EngineMarkFor(item.source_engine_id)");
		expect(status).toContain("EngineMarkFor(item.target_engine_id)");
		expect(status).toContain('data-conversation-status="model-transition"');
	});

	it("shimmers only while compaction is active and yields the generic work status", () => {
		const status = Read("modules/frontend/src/routes/components/conversation-status.sv");
		const work_session = Read(
			"modules/frontend/src/routes/components/conversation-work-session.sv",
		);
		const shimmer = Read("modules/frontend/src/lib/components/ui/shimmer-text/shimmer-text.sv");

		expect(status).toContain('{#if item.state === "started"}');
		expect(status).toContain("<ShimmerText");
		expect(status).toContain("Compacting");
		expect(status).toContain("<span>Compacted</span>");
		expect(status).toContain(
			'data-live-work-detail={item.state === "started" ? "true" : undefined}',
		);
		expect(work_session).toContain(`querySelector('[data-live-work-detail="true"]') !== null`);
		expect(work_session).toContain("is_working && !has_live_reply && !has_live_status_detail");
		expect(work_session).toContain("{#if renders_status_line}");
		expect(shimmer).toContain("@media (prefers-reduced-motion: reduce)");
		expect(shimmer).toContain("animation: none !important");
	});

	it("never shimmers stale tool activity after its owning work settles", () => {
		const trace = Read("modules/frontend/src/routes/components/conversation-trace.sv");
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.sv");

		expect(trace).toContain("work_active && activities.some(conversation_activity_is_live)");
		expect(workspace).toContain("conversation_work_session_is_active(");
		expect(workspace).toContain(
			"work_active={block.session.ended_at === undefined && session_active}",
		);
	});

	it("owns the work-session observer in the SER component scope", () => {
		const path = "modules/frontend/src/routes/components/conversation-work-session.sv";
		const source = Read(path);
		const runner = Read("modules/frontend/src/lib/lifecycle/scoped-attachment-runner.ts");

		expect(source).toContain('<script lang="ts" effect>');
		expect(source).toContain("Effect.acquireRelease");
		expect(source).toContain("MakeScopedAttachmentRunner(RunDetailObservation)");
		expect(source).toContain("detail_runner.ReplaceUnsafe");
		expect(runner).toContain("Effect.forkScoped");
		expect(runner).toContain("Queue.unbounded");
		expect(runner).toContain("Fiber.interrupt(previous)");
	});
});
