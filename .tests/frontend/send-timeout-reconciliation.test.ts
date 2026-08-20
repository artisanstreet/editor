import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { request_deadline_ms_for } from "../../modules/transport/src/internal/client-request-coordinator";

const workspace = resolve(import.meta.dirname, "../..");
const Read = (path: string) => readFileSync(resolve(workspace, path), "utf8");
const route_path = "modules/frontend/src/routes/components/thread-route.svelte";

/**
 * A send that Forge accepted came back as "did not answer the command request
 * before its deadline", and stayed invisible until a reload proved it had
 * landed. Two independent faults produced that: the deadline was a local read's,
 * and giving up on the request was treated as knowing the command failed.
 */
describe("durable send reconciliation", () => {
	it("gives a durable command room to finish rather than a local read's budget", () => {
		/**
		 * Abandoning a read costs a retry. Abandoning a command decides nothing —
		 * Forge may accept it anyway — and a message carries attachment bytes and a
		 * durable journal write, so it cannot share the projection-read budget.
		 */
		expect(request_deadline_ms_for("command")).toBe(30_000);
		expect(request_deadline_ms_for("command")).toBeGreaterThan(
			request_deadline_ms_for("thread.list.query"),
		);
	});

	/**
	 * Thread creation is the same kind of durable write and was left on the read
	 * budget, which is a deadline for the reply rather than for the work: Forge
	 * accepts a `thread.create` in 2ms at the median and 233ms at its worst, so
	 * every miss here is a late answer, never an unmade thread.
	 *
	 * What that cost was visible in the database: four threads created 4 seconds
	 * apart, each titled "New thread", each with no message and no run — one per
	 * press of send, the client giving up at 2s while the thread it had just made
	 * stayed behind and the first message went nowhere.
	 */
	it("gives thread creation the same room, since abandoning it strands a thread", () => {
		expect(request_deadline_ms_for("thread.create.request")).toBe(30_000);
		expect(request_deadline_ms_for("thread.create.request")).toBeGreaterThan(
			request_deadline_ms_for("thread.list.query"),
		);
	});

	it("names every command so a failed request can still be looked up", () => {
		const route = Read(route_path);

		/**
		 * Left to the transport, the id exists but never reaches this route, so a
		 * send whose request failed has nothing to search the transcript by.
		 */
		expect(route).toContain(
			'const submitted_command_id = command_id ?? (yield* snowflake_id.Make("command"));',
		);
		expect(route).toContain(
			"const sent = client.Command({ ...result.command, command_id: submitted_command_id });",
		);
	});

	it("resolves a retryable failure against the transcript before reporting it", () => {
		const route = Read(route_path);
		const recover = route.slice(
			route.indexOf("const RecoverAcceptedSend"),
			route.indexOf("const SendMessage"),
		);

		/** A rejection is a decision; only a retryable failure is genuinely unknown. */
		expect(recover).toContain("error.retryable && expects_user_message");
		expect(recover).toContain("HasAcceptedUserMessage(candidate, command_id)");
		/** An absent message keeps the failure — recovery must not invent acceptance. */
		expect(recover).toContain("if (Option.isNone(accepted)) return yield* Effect.fail(error);");
		/** What the transcript proved is adopted, so the reader sees it without a reload. */
		expect(recover).toContain("yield* ReplaceSnapshot(accepted.value);");
	});

	/**
	 * The happy path must stay exactly as fast: reconciliation is a failure
	 * handler, never a step between the receipt and the caller.
	 */
	it("keeps the accepted path free of any lookup", () => {
		const route = Read(route_path);
		const send_message = route.slice(
			route.indexOf("const SendMessage"),
			route.indexOf("const UpdateSessionPolicy"),
		);

		expect(send_message).not.toContain("GetConversation");
		expect(send_message).not.toContain("Effect.retry");
		expect(send_message).not.toContain("Effect.sleep");
		expect(send_message).toContain("RecoverAcceptedSend(submitted_command_id");
	});
});
