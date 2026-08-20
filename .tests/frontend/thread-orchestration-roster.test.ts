import { describe, expect, it } from "@effect/vitest";
import { Deferred, Effect, Fiber, Layer, Option, Queue, Stream } from "effect";

import type { OrchestrationGraph } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";
import {
	RosterForOrchestrationGraphs,
	type ThreadOrchestrationState,
} from "../../modules/frontend/src/lib/orchestration/roster";
import {
	ThreadOrchestrationRoster,
	ThreadOrchestrationRosterLive,
} from "../../modules/frontend/src/lib/orchestration/service";

const timestamp = "2026-08-09T12:00:00.000Z";

const MakeGraph = (
	state: OrchestrationGraph["group"]["state"],
	worker_state: OrchestrationGraph["group"]["state"] = state,
): OrchestrationGraph => ({
	agent_instances: [
		{
			agent_id: "agent-coordinator",
			created_at: timestamp,
			display_name: "Sprocket",
			group_id: "group-active",
			role: "Coordinator",
			updated_at: timestamp,
		},
		{
			agent_id: "agent-worker",
			created_at: timestamp,
			display_name: "Gibby",
			group_id: "group-active",
			role: "Reviewer",
			updated_at: timestamp,
		},
	],
	agent_runs: [],
	artifacts: [],
	assignments: [
		{
			active_run_id: "run-worker",
			agent_id: "agent-worker",
			assignment_id: "assignment-worker",
			created_at: timestamp,
			current_attempt: 1,
			engine_id: "codex",
			expected_result: "Review findings",
			group_id: "group-active",
			heartbeat: {
				confidence: 0.8,
				current_action: "Reviewing the transport boundary",
				short_description: "Reviewing transport",
				updated_at: timestamp,
			},
			instructions: "Review the transport boundary.",
			max_attempts: 1,
			parent_node_id: "agent-coordinator",
			permission_policy: {
				approval: "on_request",
				network_access: false,
				write_access: false,
			},
			profile: "reviewer",
			role: "Reviewer",
			scope: { kind: "repo", value: "artisan-editor", write_access: false },
			state: worker_state,
			summary_contract: "Return findings.",
			updated_at: timestamp,
			workspace: {
				isolation: "shared",
				working_directory: "C:\\workspace",
				workspace_id: "workspace-active",
			},
		},
	],
	edges: [],
	group: {
		coordinator_agent_id: "agent-coordinator",
		created_at: timestamp,
		group_id: "group-active",
		max_concurrency: 2,
		state,
		thread_id: "thread-active",
		updated_at: timestamp,
		version: 1,
	},
	joins: [],
	journal_sequence: 1,
});

describe("thread orchestration roster", () => {
	it("projects active worker rows without provider identity", () => {
		const graph = MakeGraph("running");
		const entries = RosterForOrchestrationGraphs([
			{
				...graph,
				assignments: [
					{
						...graph.assignments[0]!,
						agent_id: graph.group.coordinator_agent_id,
						assignment_id: "assignment-coordinator",
					},
					...graph.assignments,
				],
			},
		]);

		expect(entries).toEqual([
			expect.objectContaining({
				display_name: "Gibby",
				engine_id: "codex",
				profile: "reviewer",
				state: "running",
				status: "Reviewing the transport boundary",
			}),
		]);
		expect(entries[0]).not.toHaveProperty("role");
		expect(JSON.stringify(entries)).not.toContain("native_thread_id");
	});

	it("hides a terminal-only graph rather than keeping a stale roster", () => {
		expect(RosterForOrchestrationGraphs([MakeGraph("complete")])).toEqual([]);
		const state: ThreadOrchestrationState = { _tag: "None" };
		expect(state).toEqual({ _tag: "None" });
	});

	it.each(["complete", "failed", "stopped", "summarized"] as const)(
		"hides a %s worker while its group remains active",
		(worker_state) => {
			expect(RosterForOrchestrationGraphs([MakeGraph("running", worker_state)])).toEqual([]);
		},
	);

	it.each(["queued", "running", "waiting", "joining", "blocked"] as const)(
		"retains a %s worker while its group remains active",
		(worker_state) => {
			expect(RosterForOrchestrationGraphs([MakeGraph("running", worker_state)])).toEqual([
				expect.objectContaining({ display_name: "Gibby", state: worker_state }),
			]);
		},
	);

	it.effect("acquires one active-thread projection from its group subscription", () =>
		Effect.scoped(
			Effect.gen(function* () {
				let listed = 0;
				const subscribed = yield* Deferred.make<void>();
				const unsubscribed = yield* Deferred.make<void>();
				const graph = MakeGraph("running");
				const client_layer = Layer.succeed(ArtisanClient, {
					...FixtureArtisanClientService,
					GetOrchestrationGraph: () => Effect.succeed(graph),
					ListOrchestrationGroups: () =>
						Effect.sync(() => {
							listed += 1;
							return {
								groups: [graph.group],
								journal_sequence: graph.journal_sequence,
							};
						}),
					SubscribeOrchestrationGraph: () =>
						Effect.gen(function* () {
							yield* Effect.addFinalizer(() =>
								Deferred.succeed(unsubscribed, undefined).pipe(Effect.ignore),
							);
							yield* Deferred.succeed(subscribed, undefined);
							return Stream.concat(
								Stream.succeed({
									graph,
									journal_sequence: graph.journal_sequence,
									type: "snapshot" as const,
								}),
								Stream.never,
							);
						}),
					SubscribeOrchestrationGroups: () =>
						Effect.succeed(
							Stream.concat(
								Stream.succeed({
									snapshot: {
										groups: [graph.group],
										journal_sequence: graph.journal_sequence,
									},
									type: "snapshot" as const,
								}),
								Stream.never,
							),
						),
				});
				const services = yield* Layer.build(
					ThreadOrchestrationRosterLive.pipe(Layer.provide(client_layer)),
				);

				const roster = yield* ThreadOrchestrationRoster.pipe(Effect.provide(services));
				const next_change = yield* roster.Changes.pipe(
					Stream.drop(1),
					Stream.filter((change) => change._tag === "Ready"),
					Stream.runHead,
					Effect.forkScoped,
				);
				const lease = yield* roster.Acquire("thread-active");
				const publication = yield* Fiber.join(next_change);

				expect(Option.getOrThrow(publication)).toMatchObject({
					_tag: "Ready",
					entries: [
						{ display_name: "Gibby", status: "Reviewing the transport boundary" },
					],
					thread_id: "thread-active",
				});
				yield* Deferred.await(subscribed);
				expect(listed).toBe(0);
				yield* lease.Select(undefined);
				yield* Deferred.await(unsubscribed);
				yield* lease.Release;
			}),
		),
	);

	it.effect("opens the authoritative subscription without a cold control snapshot", () =>
		Effect.scoped(
			Effect.gen(function* () {
				const subscribed = yield* Deferred.make<boolean>();
				const client_layer = Layer.succeed(ArtisanClient, {
					...FixtureArtisanClientService,
					SubscribeOrchestrationGroups: () =>
						Effect.gen(function* () {
							yield* Deferred.succeed(subscribed, true);
							return Stream.never;
						}),
				});
				const services = yield* Layer.build(
					ThreadOrchestrationRosterLive.pipe(Layer.provide(client_layer)),
				);
				const roster = yield* ThreadOrchestrationRoster.pipe(Effect.provide(services));
				const lease = yield* roster.Acquire("thread-cold-start");

				const did_subscribe = yield* Deferred.await(subscribed).pipe(
					Effect.timeout("1 second"),
				);
				expect(did_subscribe).toBe(true);
				yield* lease.Release;
			}),
		),
	);

	it.effect(
		"reconciles graph subscriptions without bootstrap reads or restarts for unchanged groups",
		() =>
			Effect.scoped(
				Effect.gen(function* () {
					const group_updates = yield* Queue.unbounded<{
						readonly snapshot: {
							readonly groups: ReadonlyArray<OrchestrationGraph["group"]>;
							readonly journal_sequence: number;
						};
						readonly type: "snapshot";
					}>();
					const graph_a = {
						...MakeGraph("running"),
						group: { ...MakeGraph("running").group, group_id: "group-a" },
					};
					const graph_b = {
						...MakeGraph("running"),
						group: { ...MakeGraph("running").group, group_id: "group-b" },
					};
					const graph_c = {
						...MakeGraph("running"),
						group: { ...MakeGraph("running").group, group_id: "group-c" },
					};
					const graphs = new Map([
						["group-a", graph_a],
						["group-b", graph_b],
						["group-c", graph_c],
					]);
					const started_c = yield* Deferred.make<boolean>();
					const removed_b = yield* Deferred.make<boolean>();
					const subscriptions = new Map<string, number>();
					let group_subscriptions = 0;
					let listed = 0;
					let loaded_graphs = 0;
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetOrchestrationGraph: () =>
							Effect.sync(() => {
								loaded_graphs += 1;
								return graph_a;
							}),
						ListOrchestrationGroups: () =>
							Effect.sync(() => {
								listed += 1;
								return { groups: [], journal_sequence: 0 };
							}),
						SubscribeOrchestrationGroups: () =>
							Effect.sync(() => {
								group_subscriptions += 1;
								return Stream.fromQueue(group_updates);
							}),
						SubscribeOrchestrationGraph: (group_id) =>
							Effect.gen(function* () {
								subscriptions.set(group_id, (subscriptions.get(group_id) ?? 0) + 1);
								if (group_id === "group-c")
									yield* Deferred.succeed(started_c, true);
								yield* Effect.addFinalizer(() =>
									group_id === "group-b"
										? Deferred.succeed(removed_b, true).pipe(Effect.ignore)
										: Effect.void,
								);
								return Stream.concat(
									Stream.succeed({
										graph: graphs.get(group_id)!,
										journal_sequence: graphs.get(group_id)!.journal_sequence,
										type: "snapshot" as const,
									}),
									Stream.never,
								);
							}),
					});
					const services = yield* Layer.build(
						ThreadOrchestrationRosterLive.pipe(Layer.provide(client_layer)),
					);
					const roster = yield* ThreadOrchestrationRoster.pipe(Effect.provide(services));
					const lease = yield* roster.Acquire("thread-active");
					const Snapshot = (groups: ReadonlyArray<OrchestrationGraph["group"]>) =>
						Queue.offer(group_updates, {
							snapshot: { groups, journal_sequence: 1 },
							type: "snapshot",
						});

					yield* Snapshot([graph_a.group, graph_b.group]);
					yield* Snapshot([graph_a.group, graph_b.group]);
					yield* Snapshot([graph_a.group, graph_b.group, graph_c.group]);
					const c_started = yield* Deferred.await(started_c).pipe(
						Effect.timeout("1 second"),
					);
					expect(c_started).toBe(true);

					expect(group_subscriptions).toBe(1);
					expect(listed).toBe(0);
					expect(loaded_graphs).toBe(0);
					expect(subscriptions).toEqual(
						new Map([
							["group-a", 1],
							["group-b", 1],
							["group-c", 1],
						]),
					);

					yield* Snapshot([graph_a.group, graph_c.group]);
					const b_removed = yield* Deferred.await(removed_b).pipe(
						Effect.timeout("1 second"),
					);
					expect(b_removed).toBe(true);
					expect(subscriptions).toEqual(
						new Map([
							["group-a", 1],
							["group-b", 1],
							["group-c", 1],
						]),
					);
					yield* lease.Release;
				}),
			),
	);
});
