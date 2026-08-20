import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { DecodeInboundControlEnvelope, DecodeOutboundControlEnvelope } from "@artisan/protocol";

const sent_at = "2026-08-14T12:00:00.000Z";

const frontend = (kind: string, payload: unknown) => ({
	kind,
	message_id: `message_${kind}`,
	origin: "frontend" as const,
	payload,
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at,
});

const backend = (correlation_id: string, payload: unknown) => ({
	correlation_id,
	kind: "engine.installation.mutation.result" as const,
	message_id: "result_engine_installation",
	origin: "backend" as const,
	payload,
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at,
});

describe("engine installation protocol codec", () => {
	it("decodes correlated polling and mutation envelopes", async () => {
		const requests = [
			frontend("engine.installation.query", { check_updates: true, engine_id: "claude" }),
			frontend("engine.install.request", { engine_id: "claude" }),
			frontend("engine.authentication.request", { engine_id: "claude" }),
			frontend("engine.rollback.request", { engine_id: "claude" }),
		];

		await expect(
			Promise.all(
				requests.map((request) => Effect.runPromise(DecodeInboundControlEnvelope(request))),
			),
		).resolves.toEqual(requests);
		const decoded = await Effect.runPromise(
			DecodeOutboundControlEnvelope(
				backend("message_engine.authentication.request", {
					report: {
						activity: "authenticating",
						credentials_present: false,
						display_name: "Claude",
						engine_id: "claude",
						managed: false,
						recommended_version: "2.1.220",
						update_available: true,
					},
					status: "accepted",
				}),
			),
		);

		expect(decoded).toMatchObject({ correlation_id: "message_engine.authentication.request" });
		if (
			decoded.kind === "engine.installation.mutation.result" &&
			decoded.payload.status === "accepted"
		)
			expect(decoded.payload.report).toEqual({
				activity: "authenticating",
				credentials_present: false,
				display_name: "Claude",
				engine_id: "claude",
				managed: false,
				recommended_version: "2.1.220",
				update_available: true,
			});

		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope(
					backend("message_engine.authentication.request", {
						report: {
							activity: "authenticating",
							credentials_present: false,
							display_name: "Claude",
							engine_id: "claude",
							executable_path: "C:/artisan/toolchain/claude/claude.exe",
							home_path: "C:/artisan/toolchain/claude/home",
							managed: false,
						},
						status: "accepted",
					}),
				),
			),
		).rejects.toThrow("Unexpected key");
	});
});
