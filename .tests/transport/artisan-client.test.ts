import { Cause, Deferred, Effect, Fiber, Option, Schedule, Stream } from "effect";
import { describe, expect, it } from "vitest";

import { ArtisanClientError } from "@artisan/transport";

import { make_transport_test_harness, wait_for } from "./message-channel-harness";

describe("ArtisanClient over MessagePorts", () => {
	it("keeps an asset source failure retryable and leaves the session available", async () => {
		const harness = await make_transport_test_harness({
			binary_stream_errors: { "asset:unavailable": "source_error" },
		});

		try {
			await expect(
				Effect.runPromise(
					Effect.scoped(
						Effect.gen(function* () {
							const stream = yield* harness.client.OpenAsset("unavailable");
							return yield* stream.pipe(Stream.runCollect);
						}),
					),
				),
			).rejects.toMatchObject({ code: "stream_closed", retryable: true });
			const chunks = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.OpenAsset("asset_1");
						return yield* stream.pipe(Stream.runCollect);
					}),
				),
			);

			expect([...chunks]).toEqual([Uint8Array.of(1, 2), Uint8Array.of(3, 4, 5)]);
			expect(harness.connector_snapshot().connections).toBe(1);
		} finally {
			await harness.dispose();
		}
	});

	it("hides envelopes across commands, queries, subscriptions, ACKs, and heartbeat", async () => {
		const harness = await make_transport_test_harness({
			protocol: { heartbeat_after_welcome: true },
		});

		try {
			const output = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const thread = yield* harness.client.CreateThread({
							title: "Transport seed",
						});
						const updates = yield* harness.client.SubscribeThreadList;
						const updates_fiber = yield* updates.pipe(
							Stream.take(2),
							Stream.runCollect,
							Effect.forkScoped,
						);
						const event_fiber = yield* harness.client.Events.pipe(
							Stream.take(1),
							Stream.runCollect,
							Effect.forkScoped,
						);
						const receipt = yield* harness.client.Command({
							command_id: "command_with_spaces",
							payload: {
								title: "Transport title with spaces",
								type: "thread.rename",
							},
							thread_id: thread.thread_id,
						});
						const thread_lists = yield* Effect.all(
							Array.from({ length: 12 }, () => harness.client.ListThreads),
							{ concurrency: "unbounded" },
						);
						const work = yield* harness.client.GetThreadWork(thread.thread_id);
						const terminals = yield* harness.client.ListTerminals(
							thread.thread_id,
							"workspace_1",
						);
						const events = yield* Fiber.join(event_fiber);
						const updates_result = yield* Fiber.join(updates_fiber);

						return {
							events: [...events],
							receipt,
							terminals,
							thread_lists,
							updates: [...updates_result],
							work_is_none: Option.isNone(work),
						};
					}),
				),
			);

			await wait_for(() => harness.protocol_snapshot().acknowledgements.length >= 1);
			await wait_for(() => harness.protocol_snapshot().pongs.length === 1);

			expect(output.receipt).toEqual({
				command_id: "command_with_spaces",
				journal_sequence: 2,
				status: "accepted",
			});
			expect(output.events).toMatchObject([
				{
					journal_sequence: 2,
					payload: {
						change: "rename",
						thread: { title: "Transport title with spaces" },
						type: "thread.metadata.updated",
					},
				},
			]);
			expect(output.updates).toMatchObject([
				{
					threads: [
						{
							thread_id: expect.any(String),
							title: "Transport seed",
						},
					],
					type: "snapshot",
				},
				{
					thread: { title: "Transport title with spaces" },
					type: "upsert",
				},
			]);
			expect(output.thread_lists).toHaveLength(12);
			expect(output.thread_lists.every((threads) => threads.length === 1)).toBe(true);
			expect(output.terminals).toEqual([]);
			expect(output.work_is_none).toBe(true);
			expect(harness.protocol_snapshot().acknowledgements.at(-1)).toMatchObject({
				kind: "ack",
				payload: {
					event_cursors: [expect.objectContaining({ sequence: 2 })],
					journal_sequence: 2,
				},
			});
		} finally {
			await harness.dispose();
		}
	});

	it("exposes typed workspace queries and mutations with attributed envelopes", async () => {
		const harness = await make_transport_test_harness();

		try {
			const read = await Effect.runPromise(
				harness.client.ReadWorkspaceFile({
					path: "src/main.ts",
					workspace_id: "workspace_fixture",
				}),
			);
			const listed = await Effect.runPromise(
				harness.client.ListWorkspaceChanges({
					thread_id: "thread_fixture",
					workspace_id: "workspace_fixture",
				}),
			);
			const replace = await Effect.runPromise(
				harness.client.ReplaceWorkspaceFile({
					agent_id: "agent_fixture",
					change_id: "change_replace",
					command_id: "workspace_replace_1",
					content: "updated workspace content\n",
					expected_before: read.identity,
					path: "src/main.ts",
					run_id: "run_fixture",
					thread_id: "thread_fixture",
					workspace_id: "workspace_fixture",
				}),
			);
			const review_input = {
				change_id: "change_fixture",
				command_id: "workspace_review_1",
				reviewer_kind: "user" as const,
				thread_id: "thread_fixture",
			};
			const review = await Effect.runPromise(
				harness.client.ReviewWorkspaceChange(review_input),
			);
			const rollback = await Effect.runPromise(
				harness.client.RollbackWorkspaceChange({
					change_id: "change_fixture",
					command_id: "workspace_rollback_1",
					expected_after: listed.changes[0]!.after_identity,
					thread_id: "thread_fixture",
				}),
			);
			const rejected = await Effect.runPromise(
				harness.client
					.ReviewWorkspaceChange({
						change_id: "different_change",
						command_id: review_input.command_id,
						reviewer_kind: review_input.reviewer_kind,
						thread_id: review_input.thread_id,
					})
					.pipe(Effect.flip),
			);

			expect(read).toMatchObject({
				content: "fixture workspace content\n",
				path: "src/main.ts",
				workspace_id: "workspace_fixture",
			});
			expect(listed).toMatchObject({
				changes: [{ change_id: "change_fixture" }],
				journal_sequence: 0,
			});
			expect([replace, review, rollback]).toMatchObject([
				{ command_id: "workspace_replace_1", status: "accepted" },
				{ command_id: "workspace_review_1", status: "accepted" },
				{ command_id: "workspace_rollback_1", status: "accepted" },
			]);
			expect(rejected).toBeInstanceOf(ArtisanClientError);
			expect(rejected).toMatchObject({
				code: "protocol",
				protocol_code: "command.id_conflict",
				retryable: false,
			});

			const snapshot = harness.protocol_snapshot();
			expect(snapshot.workspace_file_read_attempts[0]).toMatchObject({
				kind: "workspace.file.read.query",
				payload: { path: "src/main.ts", workspace_id: "workspace_fixture" },
			});
			expect(snapshot.workspace_file_replace_attempts[0]).toMatchObject({
				agent_id: "agent_fixture",
				kind: "workspace.file.replace",
				message_id: "workspace_replace_1",
				run_id: "run_fixture",
				thread_id: "thread_fixture",
			});
			expect(snapshot.workspace_change_review_attempts).toHaveLength(2);
			expect(snapshot.workspace_change_rollback_attempts[0]).toMatchObject({
				kind: "workspace.change.rollback",
				message_id: "workspace_rollback_1",
				thread_id: "thread_fixture",
			});
		} finally {
			await harness.dispose();
		}
	});

	it("retries the exact workspace mutation envelope after a dropped receipt", async () => {
		const harness = await make_transport_test_harness({
			drop_first_command_receipt: true,
			client: { reconnect_delay_ms: 5 },
		});

		try {
			const output = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const event_fiber = yield* harness.client.Events.pipe(
							Stream.filter(
								(event) =>
									event.payload.type === "workspace.change.updated" &&
									event.correlation_id === "workspace_retry_1",
							),
							Stream.take(1),
							Stream.runCollect,
							Effect.forkScoped,
						);
						const receipt = yield* harness.client.ReplaceWorkspaceFile({
							agent_id: "agent_fixture",
							change_id: "change_retry",
							command_id: "workspace_retry_1",
							content: "retry content\n",
							expected_before: {
								algorithm: "sha256",
								byte_count: 26,
								content_hash:
									"2222222222222222222222222222222222222222222222222222222222222222",
							},
							path: "src/main.ts",
							run_id: "run_fixture",
							thread_id: "thread_fixture",
							workspace_id: "workspace_fixture",
						});

						return { events: [...(yield* Fiber.join(event_fiber))], receipt };
					}),
				),
			);

			await wait_for(() => harness.connector_snapshot().connections >= 2);
			const snapshot = harness.protocol_snapshot();
			const attempts = snapshot.workspace_file_replace_attempts;

			expect(output.receipt).toMatchObject({
				command_id: "workspace_retry_1",
				status: "duplicate",
			});
			expect(attempts).toHaveLength(2);
			expect(attempts[1]).toEqual(attempts[0]);
			expect(snapshot.workspace_change_events).toHaveLength(1);
			expect(output.events).toMatchObject([
				{
					correlation_id: "workspace_retry_1",
					payload: { action: "recorded", type: "workspace.change.updated" },
				},
			]);
			expect(output.events[0]!.journal_sequence).toBe(output.receipt.journal_sequence);
		} finally {
			await harness.dispose();
		}
	});

	it("reads and durably updates retention policy without exposing an internal thread id", async () => {
		const harness = await make_transport_test_harness();

		try {
			const initial = await Effect.runPromise(harness.client.GetThreadRetentionPolicy);
			const receipt = await Effect.runPromise(
				harness.client.UpdateThreadRetentionPolicy({
					command_id: "retention_policy_1",
					enabled: false,
					inactivity_days: 30,
				}),
			);
			const updated = await Effect.runPromise(harness.client.GetThreadRetentionPolicy);
			const conflict = await Effect.runPromise(
				harness.client
					.UpdateThreadRetentionPolicy({
						command_id: "retention_policy_1",
						enabled: true,
						inactivity_days: 7,
					})
					.pipe(Effect.flip),
			);
			const attempts = harness.protocol_snapshot().retention_update_attempts;

			expect(initial).toEqual({ enabled: true, inactivity_days: 7 });
			expect(receipt).toEqual({
				command_id: "retention_policy_1",
				journal_sequence: 1,
				status: "accepted",
			});
			expect(updated).toEqual({ enabled: false, inactivity_days: 30 });
			expect(conflict).toMatchObject({
				code: "protocol",
				protocol_code: "command.id_conflict",
				retryable: false,
			});
			expect(attempts).toHaveLength(2);
			expect(attempts[0]).not.toHaveProperty("thread_id");
			expect(attempts.map((attempt) => attempt.kind)).toEqual([
				"thread.retention.update",
				"thread.retention.update",
			]);
		} finally {
			await harness.dispose();
		}
	});

	it("retries the exact retention update after its durable receipt is lost", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 5 },
			drop_first_command_receipt: true,
		});

		try {
			const receipt = await Effect.runPromise(
				harness.client.UpdateThreadRetentionPolicy({
					command_id: "retention_retry_1",
					enabled: true,
					inactivity_days: 14,
				}),
			);

			await wait_for(() => harness.connector_snapshot().connections >= 2);

			const attempts = harness.protocol_snapshot().retention_update_attempts;

			expect(receipt).toEqual({
				command_id: "retention_retry_1",
				journal_sequence: 1,
				status: "duplicate",
			});
			expect(attempts).toHaveLength(2);
			expect(attempts[1]).toEqual(attempts[0]);
			expect(await Effect.runPromise(harness.client.GetThreadRetentionPolicy)).toEqual({
				enabled: true,
				inactivity_days: 14,
			});
		} finally {
			await harness.dispose();
		}
	});

	it("reads and mutates every global guidance operation", async () => {
		const harness = await make_transport_test_harness();

		try {
			const initial = await Effect.runPromise(harness.client.GetGlobalGuidance);
			const update = await Effect.runPromise(
				harness.client.UpdateGlobalGuidance({
					command_id: "guidance_update_1",
					content: "Updated guidance\n",
				}),
			);
			const selection = await Effect.runPromise(
				harness.client.SelectGlobalGuidance({
					command_id: "guidance_selection_1",
					content_hash: "2".repeat(64),
					provider: "claude",
				}),
			);
			const drift = await Effect.runPromise(
				harness.client.ResolveGlobalGuidanceDrift({
					action: "ignore",
					command_id: "guidance_drift_1",
					observed_hash: "3".repeat(64),
					provider: "codex",
				}),
			);
			const retry = await Effect.runPromise(
				harness.client.RetryGlobalGuidanceSync({
					command_id: "guidance_retry_1",
					provider: "codex",
				}),
			);
			const conflict = await Effect.runPromise(
				harness.client
					.UpdateGlobalGuidance({
						command_id: "guidance_update_1",
						content: "Changed intent\n",
					})
					.pipe(Effect.flip),
			);
			const snapshot = harness.protocol_snapshot();

			expect(initial.metadata.canonical.status).toBe("ready");
			expect(update).toMatchObject({
				command_id: "guidance_update_1",
				status: "accepted",
			});
			expect(selection).toMatchObject({ status: "accepted" });
			expect(drift).toMatchObject({ status: "accepted" });
			expect(retry).toMatchObject({ status: "accepted" });
			expect(conflict).toMatchObject({
				code: "protocol",
				protocol_code: "command.id_conflict",
				retryable: false,
			});
			expect(snapshot.guidance_query_attempts).toHaveLength(1);
			expect(snapshot.guidance_update_attempts).toHaveLength(2);
			expect(snapshot.guidance_selection_attempts).toHaveLength(1);
			expect(snapshot.guidance_drift_attempts).toHaveLength(1);
			expect(snapshot.guidance_retry_attempts).toHaveLength(1);
			expect(snapshot.guidance_snapshot.content).toBe("Updated guidance\n");
		} finally {
			await harness.dispose();
		}
	});

	it("retries an exact guidance update after its durable receipt is lost", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 5 },
			drop_first_command_receipt: true,
		});

		try {
			const receipt = await Effect.runPromise(
				harness.client.UpdateGlobalGuidance({
					command_id: "guidance_retry_update_1",
					content: "Retry guidance\n",
				}),
			);

			await wait_for(() => harness.connector_snapshot().connections >= 2);

			const attempts = harness.protocol_snapshot().guidance_update_attempts;

			expect(receipt).toMatchObject({
				command_id: "guidance_retry_update_1",
				status: "duplicate",
			});
			expect(attempts).toHaveLength(2);
			expect(attempts[1]).toEqual(attempts[0]);
		} finally {
			await harness.dispose();
		}
	});

	it("reads and mutates every Model Behaviour operation", async () => {
		const harness = await make_transport_test_harness();

		try {
			const initial = await Effect.runPromise(harness.client.GetModelBehaviour);
			const update = await Effect.runPromise(
				harness.client.UpdateModelBehaviour({
					command_id: "model_behaviour_update_1",
					setting_id: "auto_compaction_trigger_tokens",
					value: { type: "integer", value: 250_000 },
				}),
			);
			const drift = await Effect.runPromise(
				harness.client.ResolveModelBehaviourDrift({
					action: "ignore",
					command_id: "model_behaviour_drift_1",
					observed_hash: "3".repeat(64),
					provider_id: "codex",
					setting_id: "auto_compaction_trigger_tokens",
				}),
			);
			const retry = await Effect.runPromise(
				harness.client.RetryModelBehaviourSync({
					command_id: "model_behaviour_retry_1",
					provider_id: "codex",
					setting_id: "auto_compaction_trigger_tokens",
				}),
			);
			const conflict = await Effect.runPromise(
				harness.client
					.UpdateModelBehaviour({
						command_id: "model_behaviour_update_1",
						setting_id: "auto_compaction_trigger_tokens",
						value: { type: "integer", value: 300_000 },
					})
					.pipe(Effect.flip),
			);
			const current = await Effect.runPromise(harness.client.GetModelBehaviour);
			const snapshot = harness.protocol_snapshot();

			expect(initial.settings[0]!.value).toEqual({ type: "provider_default" });
			expect(update).toMatchObject({
				command_id: "model_behaviour_update_1",
				status: "accepted",
			});
			expect(drift).toMatchObject({ status: "accepted" });
			expect(retry).toMatchObject({ status: "accepted" });
			expect(conflict).toMatchObject({
				code: "protocol",
				protocol_code: "command.id_conflict",
				retryable: false,
			});
			expect(current.settings[0]!.value).toEqual({ type: "integer", value: 250_000 });
			expect(snapshot.model_behaviour_query_attempts).toHaveLength(2);
			expect(snapshot.model_behaviour_update_attempts).toHaveLength(2);
			expect(snapshot.model_behaviour_drift_attempts).toHaveLength(1);
			expect(snapshot.model_behaviour_retry_attempts).toHaveLength(1);
			expect(JSON.stringify(snapshot.model_behaviour_snapshot)).not.toContain(
				"config.toml =",
			);
		} finally {
			await harness.dispose();
		}
	});

	it("retries an exact Model Behaviour update after its durable receipt is lost", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 5 },
			drop_first_command_receipt: true,
		});

		try {
			const receipt = await Effect.runPromise(
				harness.client.UpdateModelBehaviour({
					command_id: "model_behaviour_retry_update_1",
					setting_id: "auto_compaction_trigger_tokens",
					value: { type: "integer", value: 250_000 },
				}),
			);

			await wait_for(() => harness.connector_snapshot().connections >= 2);

			const attempts = harness.protocol_snapshot().model_behaviour_update_attempts;

			expect(receipt).toMatchObject({
				command_id: "model_behaviour_retry_update_1",
				status: "duplicate",
			});
			expect(attempts).toHaveLength(2);
			expect(attempts[1]).toEqual(attempts[0]);
		} finally {
			await harness.dispose();
		}
	});

	it("delivers one ordered thread-list removal and omits erased threads from queries", async () => {
		const harness = await make_transport_test_harness();

		try {
			const output = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.SubscribeThreadList;
						const updates_fiber = yield* stream.pipe(
							Stream.take(3),
							Stream.runCollect,
							Effect.forkScoped,
						);

						const thread = yield* harness.client.CreateThread({
							title: "Erase through projection",
						});
						yield* harness.erase_thread(thread.thread_id);

						return {
							thread_id: thread.thread_id,
							updates: [...(yield* Fiber.join(updates_fiber))],
						};
					}),
				),
			);

			expect(output.updates).toMatchObject([
				{ type: "snapshot" },
				{ thread: { thread_id: output.thread_id }, type: "upsert" },
				{
					journal_sequence: 2,
					thread_id: output.thread_id,
					type: "remove",
				},
			]);
			expect(await Effect.runPromise(harness.client.ListThreads)).toEqual([]);
		} finally {
			await harness.dispose();
		}
	});

	it("queries a graph and delivers its ordered snapshot and replacement patch", async () => {
		const harness = await make_transport_test_harness();

		try {
			const output = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const graph = yield* harness.client.GetOrchestrationGraph("group_graph");
						const updates =
							yield* harness.client.SubscribeOrchestrationGraph("group_graph");
						const updates_fiber = yield* updates.pipe(
							Stream.take(3),
							Stream.runCollect,
							Effect.forkScoped,
						);
						const first_receipt = yield* harness.client.Command({
							command_id: "rename_graph_agent",
							payload: {
								agent_id: graph.group.coordinator_agent_id,
								display_name: "Patchwork",
								group_id: "group_graph",
								type: "agent_instance.rename",
							},
							thread_id: graph.group.thread_id,
						});
						const second_receipt = yield* harness.client.Command({
							command_id: "rename_graph_agent_again",
							payload: {
								agent_id: graph.group.coordinator_agent_id,
								display_name: "Final Name",
								group_id: "group_graph",
								type: "agent_instance.rename",
							},
							thread_id: graph.group.thread_id,
						});

						return {
							first_receipt,
							graph,
							second_receipt,
							updates: [...(yield* Fiber.join(updates_fiber))],
						};
					}),
				),
			);

			expect(output.graph).toMatchObject({
				group: { group_id: "group_graph", version: 1 },
				journal_sequence: 0,
			});
			expect(output.first_receipt).toMatchObject({
				journal_sequence: 1,
				status: "accepted",
			});
			expect(output.second_receipt).toMatchObject({
				journal_sequence: 2,
				status: "accepted",
			});
			expect(output.updates).toMatchObject([
				{
					graph: {
						agent_instances: [{ display_name: "Coordinator" }],
						group: { group_id: "group_graph", version: 1 },
					},
					journal_sequence: 0,
					type: "snapshot",
				},
				{
					graph: {
						agent_instances: [{ display_name: "Patchwork" }],
						group: { group_id: "group_graph", version: 2 },
					},
					journal_sequence: 1,
					type: "patch",
				},
				{
					graph: {
						agent_instances: [{ display_name: "Final Name" }],
						group: { group_id: "group_graph", version: 3 },
					},
					journal_sequence: 2,
					type: "patch",
				},
			]);
			await wait_for(() => harness.protocol_snapshot().acknowledgements.length === 2);
			expect(harness.protocol_snapshot().acknowledgements[1]).toMatchObject({
				payload: {
					event_cursors: [{ sequence: 2, stream_id: "orchestration:group_graph" }],
					journal_sequence: 2,
				},
			});
		} finally {
			await harness.dispose();
		}
	});

	it("retries an active graph subscription exactly and stops after scoped teardown", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 5 },
		});

		try {
			const output = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const graph =
							yield* harness.client.GetOrchestrationGraph("group_reconnect");
						const updates =
							yield* harness.client.SubscribeOrchestrationGraph("group_reconnect");
						const initial_fiber = yield* updates.pipe(
							Stream.take(2),
							Stream.runCollect,
							Effect.forkScoped,
						);

						yield* harness.client.Command({
							command_id: "rename_before_graph_reconnect",
							payload: {
								agent_id: graph.group.coordinator_agent_id,
								display_name: "Reconnect Name",
								group_id: "group_reconnect",
								type: "agent_instance.rename",
							},
							thread_id: graph.group.thread_id,
						});
						const initial = [...(yield* Fiber.join(initial_fiber))];

						yield* Effect.sync(harness.close_current_connection);
						yield* Effect.promise(() =>
							wait_for(() => harness.connector_snapshot().connections >= 2),
						);

						const resumed = yield* updates.pipe(
							Stream.take(1),
							Stream.runCollect,
							Effect.timeout("1 second"),
						);

						return { initial, resumed: [...resumed] };
					}),
				),
			);

			expect(output.initial.map((update) => update.type)).toEqual(["snapshot", "patch"]);
			expect(output.resumed).toMatchObject([
				{
					graph: {
						agent_instances: [{ display_name: "Reconnect Name" }],
						group: { version: 2 },
					},
					journal_sequence: 1,
					type: "snapshot",
				},
			]);

			const attempts = harness.protocol_snapshot().subscriptions;

			expect(attempts).toHaveLength(2);
			expect(attempts[1]).toEqual(attempts[0]);
			await wait_for(() => harness.protocol_snapshot().active_subscriptions === 0);

			harness.close_current_connection();
			await wait_for(() => harness.connector_snapshot().connections >= 3);
			await new Promise((resolve) => setTimeout(resolve, 20));

			expect(harness.protocol_snapshot().subscriptions).toHaveLength(2);
		} finally {
			await harness.dispose();
		}
	});

	it("retries the exact accepted command envelope after receipt delivery is lost", async () => {
		const harness = await make_transport_test_harness({
			drop_first_command_receipt: true,
			client: { reconnect_delay_ms: 5 },
		});

		try {
			const thread = await Effect.runPromise(
				harness.client.CreateThread({ title: "Retry seed" }),
			);
			const receipt = await Effect.runPromise(
				harness.client.Command({
					command_id: "durable_command_1",
					payload: { title: "Accepted before disconnect", type: "thread.rename" },
					thread_id: thread.thread_id,
				}),
			);

			await wait_for(() => harness.connector_snapshot().connections >= 2);

			const attempts = harness.protocol_snapshot().command_attempts;

			expect(receipt).toEqual({
				command_id: "durable_command_1",
				journal_sequence: 2,
				status: "duplicate",
			});
			expect(harness.connector_snapshot().dropped_command_receipts).toBe(1);
			expect(attempts).toHaveLength(2);
			expect(attempts[1]).toEqual(attempts[0]);
			expect(attempts.map((attempt) => attempt.message_id)).toEqual([
				"durable_command_1",
				"durable_command_1",
			]);
		} finally {
			await harness.dispose();
		}
	});

	it("resumes from the last applied journal and stream cursors after reconnect", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 5 },
		});

		try {
			const first_event = Effect.runPromise(
				harness.client.Events.pipe(Stream.take(1), Stream.runCollect),
			);

			const first_thread = await Effect.runPromise(
				harness.client.CreateThread({ title: "First cursor" }),
			);
			await first_event;
			await wait_for(() => harness.protocol_snapshot().acknowledgements.length === 1);

			harness.close_current_connection();
			await wait_for(() => harness.protocol_snapshot().hellos.length >= 2);

			const reconnect_hello = harness.protocol_snapshot().hellos[1];

			expect(reconnect_hello?.payload).toMatchObject({
				event_cursors: [{ sequence: 1, stream_id: `thread:${first_thread.thread_id}` }],
				last_journal_sequence: 1,
			});

			const second_event = Effect.runPromise(
				harness.client.Events.pipe(Stream.take(1), Stream.runCollect),
			);

			const second_thread = await Effect.runPromise(
				harness.client.CreateThread({ title: "Second cursor" }),
			);
			const second = await second_event;

			expect([...second]).toMatchObject([
				{ journal_sequence: 2, thread_id: second_thread.thread_id },
			]);
			await wait_for(() => harness.protocol_snapshot().acknowledgements.length === 2);

			const cursors = await Effect.runPromise(harness.client.Cursors);

			expect(cursors).toEqual({
				event_cursors: [
					{ sequence: 1, stream_id: `thread:${first_thread.thread_id}` },
					{ sequence: 1, stream_id: `thread:${second_thread.thread_id}` },
				],
				last_journal_sequence: 2,
			});
		} finally {
			await harness.dispose();
		}
	});

	it("rejects a concurrent command-id collision without replacing pending work", async () => {
		const harness = await make_transport_test_harness();

		try {
			const thread = await Effect.runPromise(
				harness.client.CreateThread({ title: "Concurrent seed" }),
			);
			const exits = await Effect.runPromise(
				Effect.all(
					Array.from({ length: 2 }, () =>
						harness.client
							.Command({
								command_id: "concurrent_command_id",
								payload: { title: "One durable command", type: "thread.rename" },
								thread_id: thread.thread_id,
							})
							.pipe(Effect.exit),
					),
					{ concurrency: "unbounded" },
				),
			);

			expect(exits.map((exit) => exit._tag).sort()).toEqual(["Failure", "Success"]);
			expect(harness.protocol_snapshot().command_attempts).toHaveLength(1);
			const failures = exits.flatMap((exit) => {
				if (exit._tag === "Success") {
					return [];
				}

				const failure = Cause.findErrorOption(exit.cause);

				return Option.isSome(failure) ? [failure.value] : [];
			});

			expect(failures).toMatchObject([{ code: "correlation_conflict" }]);
		} finally {
			await harness.dispose();
		}
	});

	it("closes a connection that reuses an outbound query correlation", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 5 },
			protocol: { duplicate_query_result: true },
		});

		try {
			const error = Effect.runPromise(
				harness.client.Errors.pipe(
					Stream.take(1),
					Stream.runCollect,
					Effect.timeout("1 second"),
				),
			);

			expect(await Effect.runPromise(harness.client.ListThreads)).toEqual([]);
			expect([...(await error)]).toMatchObject([{ code: "correlation_conflict" }]);
			await wait_for(() => harness.connector_snapshot().connections >= 2);
		} finally {
			await harness.dispose();
		}
	});

	it("forgets interrupted query correlations and does not poison later responses", async () => {
		const harness = await make_transport_test_harness({
			client: { max_pending_requests: 1 },
			protocol: { query_delay_ms: 50 },
		});

		try {
			await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const query = yield* harness.client.ListThreads.pipe(Effect.forkScoped);

						yield* Effect.promise(() =>
							wait_for(
								() =>
									harness
										.protocol_snapshot()
										.received_kinds.filter(
											(kind) => kind === "thread.list.query",
										).length === 1,
							),
						);
						yield* Fiber.interrupt(query);
					}),
				),
			);
			const bounded_failure = await Effect.runPromise(
				harness.client.ListThreads.pipe(Effect.flip),
			);

			expect(bounded_failure).toMatchObject({ code: "request_overflow" });
			await wait_for(
				() =>
					harness
						.protocol_snapshot()
						.received_kinds.filter((kind) => kind === "thread.list.query").length === 1,
			);
			const threads = await Effect.runPromise(
				harness.client.ListThreads.pipe(
					Effect.retry({
						schedule: Schedule.spaced("5 millis"),
						while: (error) => error.code === "request_overflow",
					}),
					Effect.timeout("1 second"),
				),
			);

			expect(threads).toEqual([]);
			expect(harness.connector_snapshot().connections).toBe(1);
		} finally {
			await harness.dispose();
		}
	});

	it("tears down scoped subscriptions and never retries them after reconnect", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 5 },
		});

		try {
			const snapshot = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const updates = yield* harness.client.SubscribeThreadList;

						return yield* updates.pipe(Stream.take(1), Stream.runCollect);
					}),
				),
			);

			expect([...snapshot]).toMatchObject([{ type: "snapshot" }]);
			await wait_for(() => harness.protocol_snapshot().active_subscriptions === 0);

			const subscribe_count = harness
				.protocol_snapshot()
				.received_kinds.filter((kind) => kind === "subscribe").length;

			harness.close_current_connection();
			await wait_for(() => harness.connector_snapshot().connections >= 2);
			await new Promise((resolve) => setTimeout(resolve, 20));

			expect(
				harness.protocol_snapshot().received_kinds.filter((kind) => kind === "subscribe")
					.length,
			).toBe(subscribe_count);
			expect(harness.protocol_snapshot().active_subscriptions).toBe(0);
		} finally {
			await harness.dispose();
		}
	});

	it("closes only an overflowing projection subscription and unsubscribes it", async () => {
		const harness = await make_transport_test_harness({
			client: { subscription_capacity: 1 },
		});

		try {
			const output = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const updates = yield* harness.client.SubscribeThreadList;
						const error_fiber = yield* harness.client.Errors.pipe(
							Stream.take(1),
							Stream.runCollect,
							Effect.forkScoped,
						);

						yield* harness.client.CreateThread({ title: "Overflow projection" });

						return {
							errors: [...(yield* Fiber.join(error_fiber))],
							updates_exit: yield* updates.pipe(Stream.runCollect, Effect.exit),
						};
					}),
				),
			);

			expect(output.errors).toMatchObject([{ code: "subscription_overflow" }]);
			expect(output.updates_exit._tag).toBe("Failure");
			await wait_for(() => harness.protocol_snapshot().active_subscriptions === 0);
			expect(harness.connector_snapshot().connections).toBe(1);
		} finally {
			await harness.dispose();
		}
	});

	it("fails the client before ACKing an event that cannot enter a stalled Events observer", async () => {
		const harness = await make_transport_test_harness({
			client: { event_capacity: 1 },
		});

		try {
			const output = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const hold = yield* Deferred.make<void>();
						const started = yield* Deferred.make<void>();
						const error = yield* harness.client.Errors.pipe(
							Stream.take(1),
							Stream.runCollect,
							Effect.forkScoped,
						);
						const observer = yield* harness.client.Events.pipe(
							Stream.runForEach(() =>
								Deferred.succeed(started, undefined).pipe(
									Effect.andThen(Deferred.await(hold)),
								),
							),
							Effect.forkScoped,
						);
						yield* Effect.yieldNow;

						yield* harness.client.CreateThread({ title: "Queued event 1" });
						yield* Deferred.await(started).pipe(Effect.timeout("1 second"));

						let attempted_events = 1;
						for (const index of [2, 3, 4, 5, 6, 7, 8]) {
							if (error.pollUnsafe() !== undefined) break;

							attempted_events = index;
							yield* harness.client
								.CreateThread({ title: `Queued event ${index}` })
								.pipe(Effect.timeout("1 second"), Effect.exit);
							yield* Effect.yieldNow;
						}
						yield* Deferred.succeed(hold, undefined);

						return {
							attempted_events,
							error: yield* Fiber.join(error),
							observer_error: yield* Fiber.join(observer).pipe(Effect.flip),
						};
					}),
				),
			);

			expect(output.error).toMatchObject([{ code: "event_overflow" }]);
			expect(output.observer_error).toMatchObject({ code: "event_overflow" });
			expect(harness.protocol_snapshot().acknowledgements.length).toBeLessThan(
				output.attempted_events,
			);
		} finally {
			await harness.dispose();
		}
	});

	it("keeps transport live when no Events observer is attached", async () => {
		const harness = await make_transport_test_harness({
			client: { event_capacity: 1 },
		});

		try {
			for (const index of [1, 2]) {
				await Effect.runPromise(
					harness.client.CreateThread({ title: `Unobserved event ${index}` }),
				);
			}

			await wait_for(() => harness.protocol_snapshot().acknowledgements.length === 2);
			expect(await Effect.runPromise(harness.client.ListThreads)).toHaveLength(2);
			expect(await Effect.runPromise(harness.client.Cursors)).toMatchObject({
				last_journal_sequence: 2,
			});

			const observed = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const observer = yield* harness.client.Events.pipe(
							Stream.take(1),
							Stream.runCollect,
							Effect.forkScoped,
						);
						yield* Effect.yieldNow;
						const thread = yield* harness.client.CreateThread({
							title: "Observed only after subscribing",
						});
						return {
							events: yield* Fiber.join(observer),
							thread_id: thread.thread_id,
						};
					}),
				),
			);
			expect([...observed.events]).toMatchObject([
				{ journal_sequence: 3, thread_id: observed.thread_id },
			]);
		} finally {
			await harness.dispose();
		}
	});

	it("keeps control responsive while the isolated server logical stream queue saturates", async () => {
		const flood = Array.from({ length: 128 }, (_, index) => Uint8Array.of(index % 256));
		const harness = await make_transport_test_harness({
			binary_streams: { "asset:flood": flood },
			client: { stream_capacity: 1 },
			server: { stream_outbound_capacity: 1 },
		});

		try {
			const output = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream_exit = yield* Effect.gen(function* () {
							const stream = yield* harness.client.OpenAsset("flood");

							return yield* stream.pipe(Stream.runCollect);
						}).pipe(Effect.timeout("1 second"), Effect.exit, Effect.forkScoped);
						const receipt = yield* harness.client
							.Command({
								command_id: "control_during_flood",
								payload: { type: "run.cancel" },
								thread_id: "thread_control",
							})
							.pipe(Effect.timeout("1 second"));
						const threads = yield* harness.client.ListThreads.pipe(
							Effect.timeout("1 second"),
						);

						return {
							receipt,
							stream_exit: yield* Fiber.join(stream_exit),
							threads,
						};
					}),
				),
			);

			expect(output.receipt.status).toBe("accepted");
			expect(output.threads).toHaveLength(1);
			expect(output.stream_exit._tag).toBe("Failure");

			if (output.stream_exit._tag === "Failure") {
				const failure = Cause.findErrorOption(output.stream_exit.cause);

				expect(Option.isSome(failure) ? failure.value : undefined).toMatchObject({
					code: expect.stringMatching(/^stream_(?:gap|overflow)$/),
				});
			}

			await new Promise((resolve) => setTimeout(resolve, 20));

			expect(harness.connector_snapshot().connections).toBe(1);
		} finally {
			await harness.dispose();
		}
	});

	it("preserves Uint8Array frame boundaries and deterministically disposes scopes", async () => {
		const harness = await make_transport_test_harness();
		const chunks = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const stream = yield* harness.client.OpenAsset("asset_1");

					return yield* stream.pipe(Stream.runCollect);
				}),
			),
		);

		expect([...chunks]).toEqual([Uint8Array.of(1, 2), Uint8Array.of(3, 4, 5)]);

		await harness.dispose();
		await wait_for(
			() =>
				harness.connector_snapshot().active_sessions === 0 &&
				harness.protocol_snapshot().active_connections === 0,
		);
		expect(harness.connector_snapshot().server_failures).toBe(0);

		const connections = harness.connector_snapshot().connections;
		const failure = await Effect.runPromise(harness.client.ListThreads.pipe(Effect.flip));

		expect(failure).toBeInstanceOf(ArtisanClientError);
		expect(failure).toMatchObject({ code: "disposed" });

		await new Promise((resolve) => setTimeout(resolve, 30));

		expect(harness.connector_snapshot().connections).toBe(connections);
	});
});
