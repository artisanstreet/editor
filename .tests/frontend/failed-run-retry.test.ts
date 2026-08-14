import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(path, "utf8");

describe("failed root run retry", () => {
	it("keeps queued work busy and submits the explicit durable retry command", () => {
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const send_message = route.slice(
			route.indexOf("const SendMessage"),
			route.indexOf("const UpdateSessionPolicy"),
		);

		expect(route).toMatch(
			/work\?\.status === "queued"[\s\S]*work\?\.status === "running"[\s\S]*work\?\.status === "waiting"/u,
		);
		expect(route).toContain(
			'client.Command({ payload: { run_id, type: "run.retry" }, thread_id })',
		);
		expect(send_message).toContain("yield* RefreshInteractionContext.pipe(Effect.ignore);");
		expect(send_message).not.toContain("yield* Effect.forkIn(");
		expect(route).toContain("onretry={RetryRun}");
	});

	it("offers retry only for the exact authoritative failed session", () => {
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");
		const session = Read(
			"modules/frontend/src/routes/components/conversation-work-session.svelte",
		);

		expect(workspace).toContain('active_run_status === "failed"');
		expect(workspace).toContain("active_run_id === block.session.run_id");
		expect(workspace).toContain('active_run_status === "queued"');
		expect(session).toContain("retry_available");
		expect(session).toMatch(/onSuccess:[\s\S]*retrying = false;/u);
		expect(session).toContain(
			'retrying ? "Retrying…" : retry_failed ? "Retry again" : "Retry"',
		);
	});

	it("keeps a safe failure explanation visible outside collapsed details", () => {
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");
		const session = Read(
			"modules/frontend/src/routes/components/conversation-work-session.svelte",
		);

		expect(session).toContain(
			'import ConversationErrorCard from "./conversation-error-card.svelte";',
		);
		expect(session).toContain("<ConversationErrorCard error={failure} />");
		expect(session).toContain("code: artisan_error_codes.run_failed,");
		expect(session).toContain("No detailed reason was recorded for this failed run.");
		expect(session).toContain("{@render details(is_failed)}");
		expect(workspace).toContain("{#snippet details(session_failed: boolean)}");
		expect(workspace).toContain("failed={session_failed}");
	});
});
