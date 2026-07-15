import { Effect, Fiber, Stream } from "effect";
import { describe, expect, it } from "vitest";

import { make_transport_test_harness, wait_for } from "./message-channel-harness";

describe("ArtisanClient preview target controls", () => {
	it("queries targets and sends typed durable commands", async () => {
		const harness = await make_transport_test_harness();

		try {
			const query = await Effect.runPromise(
				harness.client.GetPreviewTargets({
					project_id: "project_fixture",
					workspace_id: "workspace_fixture",
				}),
			);
			const receipts = await Effect.runPromise(
				Effect.gen(function* () {
					const register = yield* harness.client.RegisterPreviewTarget({
						command_id: "preview_register",
						project_id: "project_fixture",
						target_id: "target_fixture",
						thread_id: "thread_fixture",
						url: "http://127.0.0.1:4173",
						workspace_id: "workspace_fixture",
					});
					const probe = yield* harness.client.ProbePreviewTarget({
						command_id: "preview_probe",
						project_id: "project_fixture",
						target_id: "target_fixture",
						thread_id: "thread_fixture",
						workspace_id: "workspace_fixture",
					});
					const remove = yield* harness.client.RemovePreviewTarget({
						command_id: "preview_remove",
						project_id: "project_fixture",
						target_id: "target_fixture",
						thread_id: "thread_fixture",
						workspace_id: "workspace_fixture",
					});

					return [register, probe, remove] as const;
				}),
			);

			expect(query).toEqual({
				project_id: "project_fixture",
				targets: [],
				workspace_id: "workspace_fixture",
			});
			expect(receipts).toMatchObject([
				{ command_id: "preview_register", status: "accepted" },
				{ command_id: "preview_probe", status: "accepted" },
				{ command_id: "preview_remove", status: "accepted" },
			]);
			expect(harness.protocol_snapshot().command_attempts).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						message_id: "preview_register",
						payload: expect.objectContaining({ type: "preview.target.register" }),
					}),
					expect.objectContaining({
						message_id: "preview_probe",
						payload: expect.objectContaining({ type: "preview.target.probe" }),
					}),
					expect.objectContaining({
						message_id: "preview_remove",
						payload: expect.objectContaining({ type: "preview.target.remove" }),
					}),
				]),
			);
		} finally {
			await harness.dispose();
		}
	});

	it("retries the exact query envelope after reconnect", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 5 },
			protocol: { drop_first_preview_targets_result: true },
		});

		try {
			const result = await Effect.runPromise(
				harness.client.GetPreviewTargets({
					project_id: "project_fixture",
					workspace_id: "workspace_fixture",
				}),
			);

			await wait_for(
				() => harness.protocol_snapshot().preview_targets_query_attempts.length === 2,
			);
			const attempts = harness.protocol_snapshot().preview_targets_query_attempts;

			expect(result.targets).toEqual([]);
			expect(attempts[1]).toEqual(attempts[0]);
		} finally {
			await harness.dispose();
		}
	});

	it("retries the exact preview command and replays its canonical event", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 5 },
			drop_first_command_receipt: true,
		});

		try {
			const output = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const event_fiber = yield* harness.client.Events.pipe(
							Stream.filter(
								(event) => event.correlation_id === "preview_register_reconnect",
							),
							Stream.take(1),
							Stream.runCollect,
							Effect.forkScoped,
						);
						const receipt = yield* harness.client.RegisterPreviewTarget({
							command_id: "preview_register_reconnect",
							project_id: "project_fixture",
							target_id: "target_fixture",
							thread_id: "thread_fixture",
							url: "http://127.0.0.1:4173",
							workspace_id: "workspace_fixture",
						});
						const events = yield* Fiber.join(event_fiber);
						const cursors = yield* harness.client.Cursors;
						const targets = yield* harness.client.GetPreviewTargets({
							project_id: "project_fixture",
							workspace_id: "workspace_fixture",
						});

						return { cursors, events: [...events], receipt, targets };
					}),
				),
			);
			const attempts = harness
				.protocol_snapshot()
				.command_attempts.filter(
					(command) => command.message_id === "preview_register_reconnect",
				);

			expect(output.receipt.status).toBe("duplicate");
			expect(attempts).toHaveLength(2);
			expect(attempts[1]).toEqual(attempts[0]);
			expect(output.events).toMatchObject([
				{
					correlation_id: "preview_register_reconnect",
					payload: {
						action: "registered",
						target: { state: "registered", target_id: "target_fixture" },
						type: "preview.target.updated",
					},
					sequence: 1,
					stream_id: "thread:thread_fixture",
				},
			]);
			expect(output.cursors).toEqual({
				event_cursors: [{ sequence: 1, stream_id: "thread:thread_fixture" }],
				last_journal_sequence: 1,
			});
			expect(output.targets.targets).toMatchObject([
				{ state: "registered", target_id: "target_fixture" },
			]);
		} finally {
			await harness.dispose();
		}
	});

	it("rejects malformed preview commands before sending them", async () => {
		const harness = await make_transport_test_harness();

		try {
			const error = await Effect.runPromise(
				harness.client
					.RegisterPreviewTarget({
						command_id: "preview_invalid",
						project_id: "project_fixture",
						target_id: "target_fixture",
						thread_id: "thread_fixture",
						url: "not-a-url",
						workspace_id: "workspace_fixture",
					})
					.pipe(Effect.flip),
			);

			expect(error).toMatchObject({
				code: "malformed",
				message: "The preview target command is invalid.",
			});
			expect(harness.protocol_snapshot().command_attempts).toEqual([]);
		} finally {
			await harness.dispose();
		}
	});
});
