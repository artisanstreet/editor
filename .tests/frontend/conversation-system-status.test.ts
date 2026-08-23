import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { ReadStylesheets } from "./stylesheet-source";

const workspace = resolve(import.meta.dirname, "../..");
const Read = (path: string) => readFileSync(resolve(workspace, path), "utf8");

describe("conversation system status", () => {
	it("shows Steering beneath the exact user message awaiting provider acknowledgement", () => {
		const session = Read(
			"modules/frontend/src/routes/components/conversation-work-session.svelte",
		);
		const message = Read("modules/frontend/src/routes/components/conversation-message.svelte");
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");

		expect(session).not.toContain('"Steering"');
		expect(message).toContain("{#if steering_pending}");
		expect(message).toContain('data-conversation-steering-status="true"');
		expect(message.indexOf("{#if steering_pending}")).toBeGreaterThan(
			message.indexOf("</article>"),
		);
		/**
		 * The command reference identifies one canonical echo even when several
		 * steering messages target the same run.
		 */
		expect(workspace).toContain(
			"steering_pending_source_reference = pending ? source_reference : undefined",
		);
		expect(workspace).toContain("source.reference === steering_pending_source_reference");
		expect(workspace).toContain("source.event_id === steering_pending_source_reference");
	});

	it("names the wait on backgrounded delegated work instead of a thinking verb", () => {
		const session = Read(
			"modules/frontend/src/routes/components/conversation-work-session.svelte",
		);
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");

		expect(session).toContain("background_agent_names,");
		expect(workspace).toContain("conversation_background_agent_names(");
		expect(workspace).toContain("const progress_items = block.progress_items ??");
	});

	it("renders a model transition with both engine marks and catalog model names", () => {
		const status = Read("modules/frontend/src/routes/components/conversation-status.svelte");

		expect(status).toContain('{#if item.type === "model_transition"}');
		expect(status).toContain('{handing_over ? "Changing" : "Changed"}');
		expect(status).toContain("<span>for</span>");
		expect(status).toContain("PresentationForModelInCatalog");
		expect(status).toContain("effective_catalog");
		expect(status).toContain("OfflineRuntimeCatalog");
		expect(status).toContain("SessionDefaultsController");
		expect(status).toContain("source_presentation.label");
		expect(status).toContain("target_presentation.label");
		expect(status).toContain("EngineMarkClass(source_presentation.mark");
		expect(status).toContain("EngineMarkClass(target_presentation.mark");
		expect(status).toContain('data-conversation-status="model-transition"');
		expect(status).toContain("active={handing_over}");
	});

	it("shimmers only while compaction is active and yields the generic work status", () => {
		const status = Read("modules/frontend/src/routes/components/conversation-status.svelte");
		const work_session = Read(
			"modules/frontend/src/routes/components/conversation-work-session.svelte",
		);
		expect(status).toContain("<ShimmerText");
		expect(status).toContain('active={item.state === "started"}');
		expect(status).toContain("Compacting");
		expect(status).toContain('import { Separator } from "$lib/components/ui/separator"');
		expect(status).toContain(
			'data-live-work-detail={item.state === "started" ? "true" : undefined}',
		);
		expect(work_session).toContain(`querySelector('[data-live-work-detail="true"]') !== null`);
		expect(work_session.replace(/\s+/gu, " ")).toContain(
			"!superseded && !has_live_reply && !has_live_status_detail && !waiting_for_activity",
		);
		expect(work_session).toContain("{#if renders_status_line}");
		expect(work_session).toContain(
			"animate-[status-swap-enter_var(--text-swap-dur)_var(--ease-in-out)_both] my-2 flex",
		);
		/** The shimmer is decoration; the text under it has to stay readable. */
		expect(ReadStylesheets()).toMatch(
			/@utility t-shimmer-text \{[\s\S]*?prefers-reduced-motion: reduce[\s\S]*?animation: none;/u,
		);
	});

	it("never shimmers stale tool activity after its owning work settles", () => {
		const trace = Read("modules/frontend/src/routes/components/conversation-trace.svelte");
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");

		expect(trace).toContain("work_active && members.some(conversation_activity_is_live)");
		expect(workspace).toContain("work_session_is_settled(block.session.status)");
		expect(workspace).toContain("work_active={!session_settled &&");
		expect(workspace).toContain("block.session.ended_at === undefined}");
	});

	it("owns the work-session observer in the SER component scope", () => {
		const path = "modules/frontend/src/routes/components/conversation-work-session.svelte";
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

	it("does not retain settled trace DOM behind a closed disclosure", () => {
		const session = Read(
			"modules/frontend/src/routes/components/conversation-work-session.svelte",
		);
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");

		expect(workspace).toContain("has_details={visible_details.length > 0}");
		expect(session).toContain("has_details = false");
		expect(session).toContain("$state(untrack(() => has_details))");
		expect(session).toContain("work_session_initially_open({");
		expect(session).toContain("work_session_disclosure({");
		expect(session).toContain("data-open={disclosure.data_open}");
		expect(session).toContain("data-state={disclosure.data_state}");
		expect(session).toContain("{#if disclosure.details_mounted && details !== undefined}");
		expect(session).toContain("hidden={disclosure.details_hidden}");
		expect(session).toContain("has_visible_details = has_details;");
		expect(session).toContain("const ReconcileClosedDetails");
		expect(session).toContain("yield* ReconcileClosedDetails(has_details, open, is_working);");
		expect(session).toContain(
			"if (!working && !disclosure_open) has_visible_details = details_available;",
		);
	});
});
