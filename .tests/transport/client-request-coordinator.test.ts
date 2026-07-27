import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import type { ThreadListQueryEnvelope } from "@artisan/protocol";
import { client_error } from "../../modules/transport/src/internal/client-common";
import { make_client_request_coordinator } from "../../modules/transport/src/internal/client-request-coordinator";

const request: ThreadListQueryEnvelope = {
	kind: "thread.list.query",
	message_id: "message_send_failure",
	origin: "frontend",
	payload: {},
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-06-19T00:00:00.000Z",
};

describe("client request coordinator", () => {
	it("fails a request promptly and unregisters it when its initial send fails", async () => {
		const failure = client_error(
			"connection",
			"The transport send failed.",
			new Error("send failed"),
			true,
		);
		const program = Effect.gen(function* () {
			const coordinator = yield* make_client_request_coordinator(4, () =>
				Effect.fail(failure),
			);
			const first = yield* Effect.exit(coordinator.Request(request));
			const second = yield* Effect.exit(coordinator.Request(request));

			return { first, second };
		});
		const result = await Effect.runPromise(program);

		expect(result.first._tag).toBe("Failure");
		expect(result.second._tag).toBe("Failure");
		if (result.first._tag === "Failure" && result.second._tag === "Failure") {
			expect(result.first.cause.toString()).toContain("The transport send failed.");
			expect(result.second.cause.toString()).toContain("The transport send failed.");
			expect(result.second.cause.toString()).not.toContain("request id is already active");
		}
	});
});
