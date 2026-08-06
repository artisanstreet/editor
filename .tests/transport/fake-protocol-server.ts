import { createHash } from "node:crypto";

import { Deferred, Effect, Layer, Queue, Stream } from "effect";

import { ProtocolServer, type ProtocolConnection } from "@artisan/backend";
import {
	DecodeInboundControlEnvelope,
	type AckEnvelope,
	type CommandEnvelope,
	type EventEnvelope,
	type GlobalGuidanceDriftResolutionEnvelope,
	type GlobalGuidanceQueryEnvelope,
	type GlobalGuidanceRetryEnvelope,
	type GlobalGuidanceSelectionEnvelope,
	type GlobalGuidanceSnapshot,
	type GlobalGuidanceUpdateEnvelope,
	type HeartbeatPongEnvelope,
	type HelloEnvelope,
	type InboundControlEnvelope,
	type ModelBehaviourDriftResolutionEnvelope,
	type ModelBehaviourQueryEnvelope,
	type ModelBehaviourRetryEnvelope,
	type ModelBehaviourSnapshot,
	type ModelBehaviourUpdateEnvelope,
	type OrchestrationGraph,
	type OutboundControlEnvelope,
	type StreamCursor,
	type SubscribeEnvelope,
	type ThreadListItem,
	type ThreadRetentionPolicy,
	type ThreadRetentionUpdateEnvelope,
	type WorkspaceChange,
	type WorkspaceChangeListQueryEnvelope,
	type WorkspaceChangeReviewEnvelope,
	type WorkspaceChangeRollbackEnvelope,
	type WorkspaceFileReadQueryEnvelope,
	type WorkspaceFileReadQueryResult,
	type WorkspaceFileReplaceEnvelope,
} from "@artisan/protocol";

interface StoredCommand {
	readonly envelope: CommandEnvelope;
	readonly event: EventEnvelope;
	readonly fingerprint: string;
}

interface StoredRetentionUpdate {
	readonly envelope: ThreadRetentionUpdateEnvelope;
	readonly fingerprint: string;
	readonly journal_sequence: number;
}

interface StoredGuidanceMutation {
	readonly fingerprint: string;
	readonly journal_sequence: number;
}

interface StoredModelBehaviourMutation {
	readonly fingerprint: string;
	readonly journal_sequence: number;
}

interface StoredWorkspaceMutation {
	readonly fingerprint: string;
	readonly journal_sequence: number;
}

type LocalSubscription =
	| {
			readonly _tag: "orchestration.graph";
			readonly group_id: string;
			readonly stream_id: string;
			sequence: number;
	  }
	| {
			readonly _tag: "thread.list";
			readonly stream_id: string;
			sequence: number;
	  };

interface LiveConnection {
	readonly deliver_event: (event: EventEnvelope) => Effect.Effect<void>;
}

/** Configures deterministic protocol behavior used by transport integration tests. */
export interface FakeProtocolOptions {
	readonly baseline_journal_sequence?: number;
	readonly duplicate_query_result?: boolean;
	readonly heartbeat_after_welcome?: boolean;
	readonly query_delay_ms?: number;
	/**
	 * Delays `subscription.started` per subscription type, so a test can hold a
	 * connection in its readiness window while other subscriptions have
	 * already started — the exact shape of a mid-readiness death.
	 */
	readonly subscription_started_delay_ms?: Readonly<Record<string, number>>;
}

/** Exposes immutable observations from the durable fake protocol server. */
export interface FakeProtocolSnapshot {
	readonly acknowledgements: ReadonlyArray<AckEnvelope>;
	readonly active_connections: number;
	readonly active_subscriptions: number;
	readonly command_attempts: ReadonlyArray<CommandEnvelope>;
	readonly hellos: ReadonlyArray<HelloEnvelope>;
	readonly opened_connections: number;
	readonly pongs: ReadonlyArray<HeartbeatPongEnvelope>;
	readonly received_kinds: ReadonlyArray<InboundControlEnvelope["kind"]>;
	readonly retention_update_attempts: ReadonlyArray<ThreadRetentionUpdateEnvelope>;
	readonly guidance_drift_attempts: ReadonlyArray<GlobalGuidanceDriftResolutionEnvelope>;
	readonly guidance_query_attempts: ReadonlyArray<GlobalGuidanceQueryEnvelope>;
	readonly guidance_retry_attempts: ReadonlyArray<GlobalGuidanceRetryEnvelope>;
	readonly guidance_selection_attempts: ReadonlyArray<GlobalGuidanceSelectionEnvelope>;
	readonly guidance_snapshot: GlobalGuidanceSnapshot;
	readonly guidance_update_attempts: ReadonlyArray<GlobalGuidanceUpdateEnvelope>;
	readonly model_behaviour_drift_attempts: ReadonlyArray<ModelBehaviourDriftResolutionEnvelope>;
	readonly model_behaviour_query_attempts: ReadonlyArray<ModelBehaviourQueryEnvelope>;
	readonly model_behaviour_retry_attempts: ReadonlyArray<ModelBehaviourRetryEnvelope>;
	readonly model_behaviour_snapshot: ModelBehaviourSnapshot;
	readonly model_behaviour_update_attempts: ReadonlyArray<ModelBehaviourUpdateEnvelope>;
	readonly workspace_change_events: ReadonlyArray<EventEnvelope>;
	readonly workspace_change_list_attempts: ReadonlyArray<WorkspaceChangeListQueryEnvelope>;
	readonly workspace_change_review_attempts: ReadonlyArray<WorkspaceChangeReviewEnvelope>;
	readonly workspace_change_rollback_attempts: ReadonlyArray<WorkspaceChangeRollbackEnvelope>;
	readonly workspace_file_read_attempts: ReadonlyArray<WorkspaceFileReadQueryEnvelope>;
	readonly workspace_file_replace_attempts: ReadonlyArray<WorkspaceFileReplaceEnvelope>;
	readonly subscriptions: ReadonlyArray<SubscribeEnvelope>;
}

/** Supplies a Layer and observations for one durable fake backend. */
export interface FakeProtocolHarness {
	readonly EraseThread: (thread_id: string) => Effect.Effect<void>;
	readonly layer: Layer.Layer<ProtocolServer>;
	readonly snapshot: () => FakeProtocolSnapshot;
	/**
	 * Discards the durable journal, simulating a backend whose history a
	 * resuming client is now ahead of — the poisoned-resume scenario.
	 */
	readonly WipeJournal: Effect.Effect<void>;
}

function ordered_cursors(events: ReadonlyArray<EventEnvelope>): ReadonlyArray<StreamCursor> {
	const cursors = new Map<string, number>();

	for (const event of events) {
		cursors.set(event.stream_id, Math.max(cursors.get(event.stream_id) ?? 0, event.sequence));
	}

	return [...cursors]
		.map(([stream_id, sequence]) => ({ sequence, stream_id }))
		.sort((left, right) => left.stream_id.localeCompare(right.stream_id));
}

function command_fingerprint(command: CommandEnvelope) {
	return JSON.stringify(command);
}

function guidance_hash(content: string) {
	return createHash("sha256").update(content).digest("hex");
}

/** Creates a durable in-memory ProtocolServer with connection-local projections. */
export function make_fake_protocol_server(options: FakeProtocolOptions = {}): FakeProtocolHarness {
	const baseline_journal_sequence = options.baseline_journal_sequence ?? 0;
	const acknowledgements: Array<AckEnvelope> = [];
	const command_attempts: Array<CommandEnvelope> = [];
	const commands = new Map<string, StoredCommand>();
	const events: Array<EventEnvelope> = [];
	const graphs = new Map<string, OrchestrationGraph>();
	const hellos: Array<HelloEnvelope> = [];
	const live_connections = new Set<LiveConnection>();
	const pongs: Array<HeartbeatPongEnvelope> = [];
	const received_kinds: Array<InboundControlEnvelope["kind"]> = [];
	const retention_update_attempts: Array<ThreadRetentionUpdateEnvelope> = [];
	const retention_updates = new Map<string, StoredRetentionUpdate>();
	const guidance_mutations = new Map<string, StoredGuidanceMutation>();
	const guidance_drift_attempts: Array<GlobalGuidanceDriftResolutionEnvelope> = [];
	const guidance_query_attempts: Array<GlobalGuidanceQueryEnvelope> = [];
	const guidance_retry_attempts: Array<GlobalGuidanceRetryEnvelope> = [];
	const guidance_selection_attempts: Array<GlobalGuidanceSelectionEnvelope> = [];
	const guidance_update_attempts: Array<GlobalGuidanceUpdateEnvelope> = [];
	const model_behaviour_mutations = new Map<string, StoredModelBehaviourMutation>();
	const model_behaviour_drift_attempts: Array<ModelBehaviourDriftResolutionEnvelope> = [];
	const model_behaviour_query_attempts: Array<ModelBehaviourQueryEnvelope> = [];
	const model_behaviour_retry_attempts: Array<ModelBehaviourRetryEnvelope> = [];
	const model_behaviour_update_attempts: Array<ModelBehaviourUpdateEnvelope> = [];
	const workspace_changes = new Map<string, StoredWorkspaceMutation>();
	const workspace_change_list_attempts: Array<WorkspaceChangeListQueryEnvelope> = [];
	const workspace_change_review_attempts: Array<WorkspaceChangeReviewEnvelope> = [];
	const workspace_change_rollback_attempts: Array<WorkspaceChangeRollbackEnvelope> = [];
	const workspace_file_read_attempts: Array<WorkspaceFileReadQueryEnvelope> = [];
	const workspace_file_replace_attempts: Array<WorkspaceFileReplaceEnvelope> = [];
	const subscription_attempts: Array<SubscribeEnvelope> = [];
	const threads = new Map<string, ThreadListItem>();
	const thread_creations = new Map<string, ThreadListItem>();
	let retention_policy: ThreadRetentionPolicy = { enabled: true, inactivity_days: 7 };
	let active_connections = 0;
	let active_subscriptions = 0;
	let id_sequence = 0;
	let opened_connections = 0;
	let thread_sequence = 0;
	let guidance_snapshot: GlobalGuidanceSnapshot = {
		candidates: [],
		content: "Initial guidance\n",
		metadata: {
			canonical: {
				byte_count: 17,
				content_hash: "1111111111111111111111111111111111111111111111111111111111111111",
				selected_provider: "codex",
				status: "ready",
				updated_at: "2026-07-10T08:00:00.000Z",
			},
			providers: [
				{
					applied_byte_count: 17,
					applied_hash:
						"1111111111111111111111111111111111111111111111111111111111111111",
					path: "C:/guidance/codex.md",
					provider: "codex",
					status: "synced",
					updated_at: "2026-07-10T08:00:00.000Z",
				},
				{
					provider: "claude",
					status: "unsupported",
					updated_at: "2026-07-10T08:00:00.000Z",
				},
			],
		},
	};
	let model_behaviour_snapshot: ModelBehaviourSnapshot = {
		capabilities: [
			{
				control: {
					kind: "integer",
					maximum: 2_000_000,
					minimum: 16_384,
					step: 128,
					unit: "tokens",
				},
				description:
					"Token threshold that triggers automatic history compaction; this does not change model context capacity.",
				display_name: "Auto-compaction trigger",
				provider_support: [
					{
						activation_timing: "new_threads",
						details: "Codex reads this global value when a thread starts.",
						native_key: "model_auto_compact_token_limit",
						provider_id: "codex",
						state: "supported",
					},
					{
						activation_timing: "new_threads",
						details: "Claude Code has no equivalent supported mapping.",
						provider_id: "claude",
						state: "unsupported",
					},
				],
				scope: "global_default",
				setting_id: "auto_compaction_trigger_tokens",
			},
		],
		providers: [
			{
				native_key: "model_auto_compact_token_limit",
				provider_id: "codex",
				setting_id: "auto_compaction_trigger_tokens",
				status: "provider_default",
				updated_at: "1970-01-01T00:00:00.000Z",
			},
			{
				provider_id: "claude",
				setting_id: "auto_compaction_trigger_tokens",
				status: "unsupported",
				updated_at: "1970-01-01T00:00:00.000Z",
			},
		],
		registry_version: 1,
		settings: [
			{
				setting_id: "auto_compaction_trigger_tokens",
				updated_at: "1970-01-01T00:00:00.000Z",
				value: { type: "provider_default" },
				version: 0,
			},
		],
	};

	const next_id = (prefix: string) => {
		id_sequence += 1;

		return `${prefix}_${id_sequence}`;
	};
	const now = () => new Date(Date.UTC(2026, 6, 10, 8, 0, id_sequence)).toISOString();
	const workspace_file_content = "fixture workspace content\n";
	const workspace_file_identity = {
		algorithm: "sha256" as const,
		byte_count: Buffer.byteLength(workspace_file_content),
		content_hash: "2222222222222222222222222222222222222222222222222222222222222222",
	};
	const workspace_file_result: WorkspaceFileReadQueryResult = {
		content: workspace_file_content,
		identity: workspace_file_identity,
		path: "src/main.ts",
		workspace_id: "workspace_fixture",
	};
	let workspace_change: WorkspaceChange = {
		after_identity: workspace_file_identity,
		agent_id: "agent_fixture",
		before_identity: {
			...workspace_file_identity,
			content_hash: "3333333333333333333333333333333333333333333333333333333333333333",
		},
		change_id: "change_fixture",
		created_at: "2026-07-10T08:00:00.000Z",
		path: "src/main.ts",
		review_state: "needs_review",
		rollback_state: "available",
		run_id: "run_fixture",
		source_command_id: "replace_fixture",
		thread_id: "thread_fixture",
		updated_at: "2026-07-10T08:00:00.000Z",
		version: 1,
		workspace_id: "workspace_fixture",
	};
	const backend_trace = () => ({
		message_id: next_id("backend_message"),
		origin: "backend" as const,
		protocol_version: 1 as const,
		schema_version: 1 as const,
		sent_at: now(),
	});
	const get_graph = (group_id: string) => {
		const existing = graphs.get(group_id);

		if (existing) {
			return existing;
		}

		const timestamp = now();
		const graph: OrchestrationGraph = {
			agent_instances: [
				{
					agent_id: `coordinator:${group_id}`,
					created_at: timestamp,
					display_name: "Coordinator",
					group_id,
					role: "coordinator",
					updated_at: timestamp,
				},
			],
			agent_runs: [],
			artifacts: [],
			assignments: [],
			edges: [],
			group: {
				coordinator_agent_id: `coordinator:${group_id}`,
				created_at: timestamp,
				group_id,
				max_concurrency: 2,
				state: "running",
				thread_id: `thread:${group_id}`,
				updated_at: timestamp,
				version: 1,
			},
			joins: [],
			journal_sequence: events.at(-1)?.journal_sequence ?? baseline_journal_sequence,
		};

		graphs.set(group_id, graph);

		return graph;
	};
	const rename_graph_agent = (
		group_id: string,
		agent_id: string,
		display_name: string,
		journal_sequence: number,
	) => {
		const graph = get_graph(group_id);
		const timestamp = now();
		const agent_instances = graph.agent_instances.map((agent) =>
			agent.agent_id === agent_id ? { ...agent, display_name, updated_at: timestamp } : agent,
		);
		const updated: OrchestrationGraph = {
			...graph,
			agent_instances,
			group: {
				...graph.group,
				updated_at: timestamp,
				version: graph.group.version + 1,
			},
			journal_sequence,
		};

		graphs.set(group_id, updated);

		return updated;
	};

	const open = Effect.gen(function* () {
		const outbound = yield* Effect.acquireRelease(
			Queue.unbounded<OutboundControlEnvelope>(),
			Queue.shutdown,
		);
		const closed = yield* Deferred.make<void>();
		const subscriptions = new Map<string, LocalSubscription>();
		let negotiated = false;
		let locally_closed = false;

		opened_connections += 1;
		active_connections += 1;

		const enqueue = (envelope: OutboundControlEnvelope) =>
			Queue.offer(outbound, envelope).pipe(Effect.asVoid);

		const close = Effect.gen(function* () {
			if (locally_closed) {
				return;
			}

			locally_closed = true;
			live_connections.delete(live_connection);
			active_connections -= 1;
			active_subscriptions -= subscriptions.size;
			subscriptions.clear();
			yield* Queue.shutdown(outbound);
			yield* Deferred.succeed(closed, undefined);
		});

		const deliver_event = (event: EventEnvelope) =>
			Effect.gen(function* () {
				if (!negotiated || locally_closed) {
					return;
				}

				yield* enqueue(event);

				for (const [subscription_id, subscription] of subscriptions) {
					if (
						subscription._tag === "thread.list" &&
						(event.payload.type === "thread.created" ||
							event.payload.type === "thread.metadata.updated")
					) {
						subscription.sequence += 1;
						yield* enqueue({
							...backend_trace(),
							journal_sequence: event.journal_sequence,
							kind: "thread.list.upsert",
							payload: threads.get(event.thread_id)!,
							sequence: subscription.sequence,
							stream_id: subscription.stream_id,
							subscription_id,
						});
					}

					if (
						subscription._tag === "thread.list" &&
						event.payload.type === "thread.erased"
					) {
						subscription.sequence += 1;
						yield* enqueue({
							...backend_trace(),
							journal_sequence: event.journal_sequence,
							kind: "thread.list.remove",
							payload: { thread_id: event.thread_id },
							sequence: subscription.sequence,
							stream_id: subscription.stream_id,
							subscription_id,
						});
					}

					if (
						subscription._tag === "orchestration.graph" &&
						event.payload.type === "agent_instance.renamed" &&
						event.payload.group_id === subscription.group_id
					) {
						subscription.sequence += 1;
						yield* enqueue({
							...backend_trace(),
							journal_sequence: event.journal_sequence,
							kind: "orchestration.graph.patch",
							payload: { graph: get_graph(subscription.group_id) },
							sequence: subscription.sequence,
							stream_id: subscription.stream_id,
							subscription_id,
						});
					}
				}
			});
		const live_connection: LiveConnection = { deliver_event };

		yield* Effect.addFinalizer(() => close);

		const reject_before_hello = (input: InboundControlEnvelope) =>
			enqueue({
				...backend_trace(),
				correlation_id: input.message_id,
				kind: "protocol.error",
				payload: {
					code: "protocol.handshake_required",
					message: "A hello envelope is required before domain traffic.",
					retryable: false,
				},
			});

		const handle_hello = (hello: HelloEnvelope) =>
			Effect.gen(function* () {
				hellos.push(hello);

				if (!hello.payload.supported_protocol_versions.includes(1)) {
					yield* enqueue({
						message_id: next_id("protocol_error"),
						origin: "backend",
						schema_version: 1,
						sent_at: now(),
						correlation_id: hello.message_id,
						kind: "protocol.error",
						payload: {
							code: "protocol.unsupported_version",
							message: "No offered protocol version is supported.",
							retryable: false,
						},
					});

					return;
				}

				negotiated = true;
				live_connections.add(live_connection);
				const replay =
					hello.payload.resume_mode === "fresh"
						? []
						: events.filter(
								(event) =>
									event.journal_sequence > hello.payload.last_journal_sequence,
							);
				const current_cursors = ordered_cursors(events);
				const journal_sequence =
					events.at(-1)?.journal_sequence ?? baseline_journal_sequence;

				yield* enqueue({
					...backend_trace(),
					correlation_id: hello.message_id,
					kind: "welcome",
					payload: {
						connection_id: next_id("protocol_connection"),
						current_event_cursors: current_cursors,
						heartbeat_interval_ms: 100,
						heartbeat_timeout_ms: 500,
						journal_sequence,
						stream_ticket: next_id("stream_ticket"),
					},
				});
				yield* Effect.forEach(replay, enqueue, { discard: true });
				yield* enqueue({
					...backend_trace(),
					correlation_id: hello.message_id,
					kind: "replay.complete",
					payload: {
						current_event_cursors: current_cursors,
						journal_sequence,
					},
				});

				if (options.heartbeat_after_welcome) {
					yield* enqueue({
						...backend_trace(),
						kind: "heartbeat.ping",
						payload: { nonce: next_id("heartbeat_nonce") },
					});
				}
			});

		const handle_command = (command: CommandEnvelope) =>
			Effect.gen(function* () {
				command_attempts.push(command);
				const fingerprint = command_fingerprint(command);
				const previous = commands.get(command.message_id);

				if (previous && previous.fingerprint !== fingerprint) {
					yield* enqueue({
						...backend_trace(),
						causation_id: command.message_id,
						correlation_id: command.message_id,
						kind: "command.receipt",
						payload: {
							error: {
								code: "command.id_conflict",
								message: "The command id was reused with different content.",
								retryable: false,
							},
							status: "rejected",
						},
						thread_id: command.thread_id,
					});

					return;
				}

				if (previous) {
					yield* enqueue({
						...backend_trace(),
						causation_id: command.message_id,
						correlation_id: command.message_id,
						kind: "command.receipt",
						payload: {
							journal_sequence: previous.event.journal_sequence,
							status: "duplicate",
						},
						thread_id: command.thread_id,
					});

					return;
				}

				const journal_sequence = baseline_journal_sequence + events.length + 1;
				let event: EventEnvelope;

				if (command.payload.type === "agent_instance.rename") {
					const stream_id = `orchestration:${command.payload.group_id}`;
					const sequence =
						events.filter((candidate) => candidate.stream_id === stream_id).length + 1;

					rename_graph_agent(
						command.payload.group_id,
						command.payload.agent_id,
						command.payload.display_name,
						journal_sequence,
					);
					event = {
						...backend_trace(),
						causation_id: command.message_id,
						correlation_id: command.message_id,
						journal_sequence,
						kind: "event",
						payload: {
							agent_id: command.payload.agent_id,
							display_name: command.payload.display_name,
							group_id: command.payload.group_id,
							type: "agent_instance.renamed",
						},
						sequence,
						stream_id,
						thread_id: command.thread_id,
					};
				} else if (command.payload.type === "thread.rename") {
					const current = threads.get(command.thread_id);
					const stream_id = `thread:${command.thread_id}`;
					const sequence =
						events.filter((candidate) => candidate.stream_id === stream_id).length + 1;
					const updated: ThreadListItem = {
						...(current ?? {
							activity_version: 0,
							affinity_version: 0,
							created_at: backend_trace().sent_at,
							last_activity_at: backend_trace().sent_at,
							linked_projects: [],
							live_status: "Idle",
							metadata_version: 0,
							pinned: false,
							project_affinity_scores: [],
							project_locked: false,
							thread_id: command.thread_id,
							title_locked: false,
							title_source: "initial",
							updated_at: backend_trace().sent_at,
						}),
						activity_version: (current?.activity_version ?? 0) + 1,
						current_goal: command.payload.title,
						metadata_version: (current?.metadata_version ?? 0) + 1,
						title: command.payload.title,
						title_locked: true,
						title_source: "manual",
						updated_at: backend_trace().sent_at,
					};

					threads.set(command.thread_id, updated);
					event = {
						...backend_trace(),
						causation_id: command.message_id,
						correlation_id: command.message_id,
						journal_sequence,
						kind: "event",
						payload: {
							activity_kind: "renamed",
							change: "rename",
							thread: updated,
							type: "thread.metadata.updated",
						},
						sequence,
						stream_id,
						thread_id: command.thread_id,
					};
				} else {
					const title =
						command.payload.type === "thread.create"
							? command.payload.title
							: `Command ${command.message_id}`;

					event = {
						...backend_trace(),
						causation_id: command.message_id,
						correlation_id: command.message_id,
						journal_sequence,
						kind: "event",
						payload: { title, type: "thread.created" },
						sequence: 1,
						stream_id: `thread:${command.thread_id}`,
						thread_id: command.thread_id,
					};
					const item: ThreadListItem = {
						activity_version: 0,
						affinity_version: 0,
						created_at: event.sent_at,
						current_goal: title,
						last_activity_at: event.sent_at,
						live_status: "Idle",
						linked_projects: [],
						metadata_version: 0,
						pinned: false,
						project_affinity_scores: [],
						project_locked: false,
						thread_id: command.thread_id,
						title,
						title_locked: false,
						title_source: "initial",
						updated_at: event.sent_at,
					};

					threads.set(command.thread_id, item);
				}

				events.push(event);
				commands.set(command.message_id, { envelope: command, event, fingerprint });
				yield* enqueue({
					...backend_trace(),
					causation_id: command.message_id,
					correlation_id: command.message_id,
					kind: "command.receipt",
					payload: { journal_sequence, status: "accepted" },
					thread_id: command.thread_id,
				});
				yield* Effect.forEach(
					live_connections,
					(connection) => connection.deliver_event(event),
					{ discard: true },
				);
			});

		const handle_retention_update = (update: ThreadRetentionUpdateEnvelope) =>
			Effect.gen(function* () {
				retention_update_attempts.push(update);
				const fingerprint = JSON.stringify(update);
				const previous = retention_updates.get(update.message_id);
				const internal_thread_id = "settings/thread-retention";

				if (previous && previous.fingerprint !== fingerprint) {
					yield* enqueue({
						...backend_trace(),
						causation_id: update.message_id,
						correlation_id: update.message_id,
						kind: "command.receipt",
						payload: {
							error: {
								code: "command.id_conflict",
								message: "The command id was reused with different content.",
								retryable: false,
							},
							status: "rejected",
						},
						thread_id: internal_thread_id,
					});

					return;
				}

				if (previous) {
					yield* enqueue({
						...backend_trace(),
						causation_id: update.message_id,
						correlation_id: update.message_id,
						kind: "command.receipt",
						payload: {
							journal_sequence: previous.journal_sequence,
							status: "duplicate",
						},
						thread_id: internal_thread_id,
					});

					return;
				}

				const journal_sequence = baseline_journal_sequence + events.length + 1;
				const stream_id = `thread:${internal_thread_id}`;
				const sequence = events.filter((event) => event.stream_id === stream_id).length + 1;
				const event: EventEnvelope = {
					...backend_trace(),
					causation_id: update.message_id,
					correlation_id: update.message_id,
					journal_sequence,
					kind: "event",
					payload: {
						policy: update.payload,
						type: "thread.retention.updated",
					},
					sequence,
					stream_id,
					thread_id: internal_thread_id,
				};

				retention_policy = update.payload;
				events.push(event);
				retention_updates.set(update.message_id, {
					envelope: update,
					fingerprint,
					journal_sequence,
				});
				yield* enqueue({
					...backend_trace(),
					causation_id: update.message_id,
					correlation_id: update.message_id,
					kind: "command.receipt",
					payload: { journal_sequence, status: "accepted" },
					thread_id: internal_thread_id,
				});
				yield* Effect.forEach(
					live_connections,
					(connection) => connection.deliver_event(event),
					{ discard: true },
				);
			});

		const handle_guidance_query = (query: GlobalGuidanceQueryEnvelope) =>
			Effect.gen(function* () {
				guidance_query_attempts.push(query);

				yield* enqueue({
					...backend_trace(),
					correlation_id: query.message_id,
					kind: "guidance.query.result",
					payload: guidance_snapshot,
				});
			});

		const handle_guidance_mutation = (
			mutation:
				| GlobalGuidanceDriftResolutionEnvelope
				| GlobalGuidanceRetryEnvelope
				| GlobalGuidanceSelectionEnvelope
				| GlobalGuidanceUpdateEnvelope,
		) =>
			Effect.gen(function* () {
				if (mutation.kind === "guidance.update") {
					guidance_update_attempts.push(mutation);
				} else if (mutation.kind === "guidance.selection") {
					guidance_selection_attempts.push(mutation);
				} else if (mutation.kind === "guidance.drift.resolve") {
					guidance_drift_attempts.push(mutation);
				} else {
					guidance_retry_attempts.push(mutation);
				}

				const fingerprint = JSON.stringify({
					kind: mutation.kind,
					payload: mutation.payload,
				});
				const previous = guidance_mutations.get(mutation.message_id);

				if (previous && previous.fingerprint !== fingerprint) {
					yield* enqueue({
						...backend_trace(),
						causation_id: mutation.message_id,
						correlation_id: mutation.message_id,
						kind: "command.receipt",
						payload: {
							error: {
								code: "command.id_conflict",
								message: "The command id was reused with different content.",
								retryable: false,
							},
							status: "rejected",
						},
						thread_id: "settings/guidance",
					});

					return;
				}

				if (previous) {
					yield* enqueue({
						...backend_trace(),
						causation_id: mutation.message_id,
						correlation_id: mutation.message_id,
						kind: "command.receipt",
						payload: {
							journal_sequence: previous.journal_sequence,
							status: "duplicate",
						},
						thread_id: "settings/guidance",
					});

					return;
				}

				const journal_sequence = baseline_journal_sequence + events.length + 1;
				const stream_id = "settings:guidance";
				const sequence = events.filter((event) => event.stream_id === stream_id).length + 1;
				const content =
					mutation.kind === "guidance.update"
						? mutation.payload.content
						: guidance_snapshot.content;
				const content_hash = guidance_hash(content);
				const selected_provider =
					mutation.kind === "guidance.selection"
						? mutation.payload.provider
						: guidance_snapshot.metadata.canonical.selected_provider;
				const updated_at = now();

				guidance_snapshot = {
					...guidance_snapshot,
					content,
					metadata: {
						...guidance_snapshot.metadata,
						canonical: {
							...guidance_snapshot.metadata.canonical,
							byte_count: Buffer.byteLength(content),
							content_hash,
							selected_provider,
							status: "ready",
							updated_at,
						},
					},
				};
				const event: EventEnvelope = {
					...backend_trace(),
					causation_id: mutation.message_id,
					correlation_id: mutation.message_id,
					journal_sequence,
					kind: "event",
					payload: {
						byte_count: Buffer.byteLength(content),
						content_hash,
						selected_provider,
						type: "guidance.canonical.updated",
					},
					sequence,
					stream_id,
					thread_id: "settings/guidance",
				};

				events.push(event);
				guidance_mutations.set(mutation.message_id, { fingerprint, journal_sequence });
				yield* enqueue({
					...backend_trace(),
					causation_id: mutation.message_id,
					correlation_id: mutation.message_id,
					kind: "command.receipt",
					payload: { journal_sequence, status: "accepted" },
					thread_id: "settings/guidance",
				});
				yield* Effect.forEach(
					live_connections,
					(connection) => connection.deliver_event(event),
					{ discard: true },
				);
			});

		const handle_model_behaviour_query = (query: ModelBehaviourQueryEnvelope) =>
			Effect.gen(function* () {
				model_behaviour_query_attempts.push(query);

				yield* enqueue({
					...backend_trace(),
					correlation_id: query.message_id,
					kind: "model_behaviour.query.result",
					payload: model_behaviour_snapshot,
				});
			});

		const handle_model_behaviour_mutation = (
			mutation:
				| ModelBehaviourDriftResolutionEnvelope
				| ModelBehaviourRetryEnvelope
				| ModelBehaviourUpdateEnvelope,
		) =>
			Effect.gen(function* () {
				if (mutation.kind === "model_behaviour.update") {
					model_behaviour_update_attempts.push(mutation);
				} else if (mutation.kind === "model_behaviour.drift.resolve") {
					model_behaviour_drift_attempts.push(mutation);
				} else {
					model_behaviour_retry_attempts.push(mutation);
				}

				const fingerprint = JSON.stringify({
					kind: mutation.kind,
					payload: mutation.payload,
				});
				const previous = model_behaviour_mutations.get(mutation.message_id);

				if (previous && previous.fingerprint !== fingerprint) {
					yield* enqueue({
						...backend_trace(),
						causation_id: mutation.message_id,
						correlation_id: mutation.message_id,
						kind: "command.receipt",
						payload: {
							error: {
								code: "command.id_conflict",
								message: "The command id was reused with different content.",
								retryable: false,
							},
							status: "rejected",
						},
						thread_id: "settings/model-behaviour",
					});

					return;
				}

				if (previous) {
					yield* enqueue({
						...backend_trace(),
						causation_id: mutation.message_id,
						correlation_id: mutation.message_id,
						kind: "command.receipt",
						payload: {
							journal_sequence: previous.journal_sequence,
							status: "duplicate",
						},
						thread_id: "settings/model-behaviour",
					});

					return;
				}

				const journal_sequence = events.length + model_behaviour_mutations.size + 1;
				const updated_at = now();

				if (mutation.kind === "model_behaviour.update") {
					const [setting] = model_behaviour_snapshot.settings;
					const applied_hash = createHash("sha256")
						.update(JSON.stringify(mutation.payload.value))
						.digest("hex");

					if (setting) {
						model_behaviour_snapshot = {
							...model_behaviour_snapshot,
							providers: model_behaviour_snapshot.providers.map((provider) =>
								provider.provider_id === "codex" &&
								provider.setting_id === mutation.payload.setting_id
									? {
											...provider,
											applied_hash,
											observed_hash: applied_hash,
											status:
												mutation.payload.value.type === "provider_default"
													? ("provider_default" as const)
													: ("synced" as const),
											updated_at,
										}
									: provider,
							),
							settings: [
								{
									...setting,
									updated_at,
									value: mutation.payload.value,
									version: setting.version + 1,
								},
							],
						};
					}
				} else {
					const is_drift_resolution = mutation.kind === "model_behaviour.drift.resolve";
					const observed_hash = is_drift_resolution
						? mutation.payload.observed_hash
						: undefined;
					const provider_status =
						is_drift_resolution && mutation.payload.action === "ignore"
							? ("drift_ignored" as const)
							: ("synced" as const);
					const providers = model_behaviour_snapshot.providers.map((provider) =>
						provider.provider_id === mutation.payload.provider_id &&
						provider.setting_id === mutation.payload.setting_id
							? {
									...provider,
									...(observed_hash === undefined ? {} : { observed_hash }),
									...(provider_status === "drift_ignored" &&
									observed_hash !== undefined
										? { ignored_drift_hash: observed_hash }
										: {}),
									status: provider_status,
									updated_at,
								}
							: provider,
					);

					model_behaviour_snapshot = { ...model_behaviour_snapshot, providers };
				}

				model_behaviour_mutations.set(mutation.message_id, {
					fingerprint,
					journal_sequence,
				});
				yield* enqueue({
					...backend_trace(),
					causation_id: mutation.message_id,
					correlation_id: mutation.message_id,
					kind: "command.receipt",
					payload: { journal_sequence, status: "accepted" },
					thread_id: "settings/model-behaviour",
				});
			});

		const handle_workspace_read = (query: WorkspaceFileReadQueryEnvelope) =>
			Effect.gen(function* () {
				workspace_file_read_attempts.push(query);
				yield* enqueue({
					...backend_trace(),
					correlation_id: query.message_id,
					kind: "workspace.file.read.query.result",
					payload: {
						...workspace_file_result,
						path: query.payload.path,
						workspace_id: query.payload.workspace_id,
					},
				});
			});
		const handle_workspace_change_list = (query: WorkspaceChangeListQueryEnvelope) =>
			Effect.gen(function* () {
				workspace_change_list_attempts.push(query);
				const matches =
					workspace_change.thread_id === query.payload.thread_id &&
					(query.payload.workspace_id === undefined ||
						workspace_change.workspace_id === query.payload.workspace_id);

				yield* enqueue({
					...backend_trace(),
					correlation_id: query.message_id,
					kind: "workspace.change.list.query.result",
					payload: {
						changes: matches ? [workspace_change] : [],
						journal_sequence: events.length,
					},
				});
			});

		type WorkspaceMutationEnvelope =
			| WorkspaceChangeReviewEnvelope
			| WorkspaceChangeRollbackEnvelope
			| WorkspaceFileReplaceEnvelope;
		const handle_workspace_mutation = (mutation: WorkspaceMutationEnvelope) =>
			Effect.gen(function* () {
				if (mutation.kind === "workspace.file.replace") {
					workspace_file_replace_attempts.push(mutation);
				} else if (mutation.kind === "workspace.change.review") {
					workspace_change_review_attempts.push(mutation);
				} else {
					workspace_change_rollback_attempts.push(mutation);
				}

				const fingerprint = JSON.stringify({
					agent_id: "agent_id" in mutation ? mutation.agent_id : undefined,
					kind: mutation.kind,
					message_id: mutation.message_id,
					origin: mutation.origin,
					payload: mutation.payload,
					raw_origin: "raw_origin" in mutation ? mutation.raw_origin : undefined,
					run_id: "run_id" in mutation ? mutation.run_id : undefined,
					sent_at: mutation.sent_at,
					thread_id: mutation.thread_id,
				});
				const previous = workspace_changes.get(mutation.message_id);

				if (previous && previous.fingerprint !== fingerprint) {
					yield* enqueue({
						...backend_trace(),
						causation_id: mutation.message_id,
						correlation_id: mutation.message_id,
						kind: "command.receipt",
						payload: {
							error: {
								code: "command.id_conflict",
								message: "The command id was reused with different content.",
								retryable: false,
							},
							status: "rejected",
						},
						thread_id: mutation.thread_id,
					});

					return;
				}

				if (previous) {
					yield* enqueue({
						...backend_trace(),
						causation_id: mutation.message_id,
						correlation_id: mutation.message_id,
						kind: "command.receipt",
						payload: {
							journal_sequence: previous.journal_sequence,
							status: "duplicate",
						},
						thread_id: mutation.thread_id,
					});

					return;
				}

				const action =
					mutation.kind === "workspace.file.replace"
						? ("recorded" as const)
						: mutation.kind === "workspace.change.review"
							? ("reviewed" as const)
							: ("rolled_back" as const);
				const next_change: WorkspaceChange =
					mutation.kind === "workspace.file.replace"
						? {
								...workspace_change,
								after_identity: {
									algorithm: "sha256",
									byte_count: Buffer.byteLength(mutation.payload.content),
									content_hash: createHash("sha256")
										.update(mutation.payload.content)
										.digest("hex"),
								},
								agent_id: mutation.agent_id,
								before_identity: mutation.payload.expected_before,
								change_id: mutation.payload.change_id,
								created_at: mutation.sent_at,
								path: mutation.payload.path,
								...(mutation.raw_origin === undefined
									? {}
									: { raw_origin: mutation.raw_origin }),
								review_state: "needs_review",
								rollback_state: "available",
								run_id: mutation.run_id,
								source_command_id: mutation.message_id,
								thread_id: mutation.thread_id,
								updated_at: mutation.sent_at,
								version: 1,
								workspace_id: mutation.payload.workspace_id,
							}
						: mutation.kind === "workspace.change.review"
							? {
									...workspace_change,
									change_id: mutation.payload.change_id,
									review_state: "reviewed",
									reviewed_at: mutation.sent_at,
									updated_at: mutation.sent_at,
									version: workspace_change.version + 1,
								}
							: {
									...workspace_change,
									change_id: mutation.payload.change_id,
									review_state: "rolled_back",
									rollback_state: "consumed",
									rolled_back_at: mutation.sent_at,
									updated_at: mutation.sent_at,
									version: workspace_change.version + 1,
								};
				const journal_sequence = baseline_journal_sequence + events.length + 1;
				const stream_id = `thread:${mutation.thread_id}`;
				const sequence = events.filter((event) => event.stream_id === stream_id).length + 1;
				const event: EventEnvelope = {
					...backend_trace(),
					...(mutation.kind === "workspace.file.replace"
						? {
								agent_id: mutation.agent_id,
								...(mutation.raw_origin === undefined
									? {}
									: { raw_origin: mutation.raw_origin }),
								run_id: mutation.run_id,
							}
						: {}),
					causation_id: mutation.message_id,
					correlation_id: mutation.message_id,
					journal_sequence,
					kind: "event",
					payload: {
						action,
						change: next_change,
						type: "workspace.change.updated",
					},
					sequence,
					stream_id,
					thread_id: mutation.thread_id,
				};

				workspace_change = next_change;
				events.push(event);
				workspace_changes.set(mutation.message_id, { fingerprint, journal_sequence });
				yield* enqueue({
					...backend_trace(),
					causation_id: mutation.message_id,
					correlation_id: mutation.message_id,
					kind: "command.receipt",
					payload: {
						journal_sequence,
						status: "accepted",
					},
					thread_id: mutation.thread_id,
				});
				yield* Effect.forEach(
					live_connections,
					(connection) => connection.deliver_event(event),
					{ discard: true },
				);
			});

		const handle_subscribe = (subscribe: SubscribeEnvelope) =>
			Effect.gen(function* () {
				subscription_attempts.push(subscribe);

				if (subscriptions.has(subscribe.subscription_id)) {
					return;
				}

				active_subscriptions += 1;

				/**
				 * A configured delay detaches the answer so the connection keeps
				 * serving other envelopes while this subscription's start is
				 * outstanding. A killed connection shuts its outbound queue, so
				 * a late answer dies quietly instead of leaking.
				 */
				const answer = (envelopes: ReadonlyArray<OutboundControlEnvelope>) => {
					const delay = options.subscription_started_delay_ms?.[subscribe.payload.type];
					const deliver = Effect.forEach(envelopes, enqueue, { discard: true });

					return delay === undefined
						? deliver
						: Effect.sleep(`${delay} millis`).pipe(
								Effect.andThen(deliver),
								Effect.forkDetach,
								Effect.asVoid,
							);
				};

				if (subscribe.payload.type === "orchestration.graph") {
					const graph = get_graph(subscribe.payload.group_id);
					const stream_id = `projection:orchestration.graph:${subscribe.payload.group_id}:${subscribe.subscription_id}`;

					subscriptions.set(subscribe.subscription_id, {
						_tag: "orchestration.graph",
						group_id: subscribe.payload.group_id,
						sequence: 0,
						stream_id,
					});
					yield* answer([
						{
							...backend_trace(),
							correlation_id: subscribe.message_id,
							kind: "subscription.started",
							payload: { stream_id },
							subscription_id: subscribe.subscription_id,
						},
						{
							...backend_trace(),
							journal_sequence: graph.journal_sequence,
							kind: "orchestration.graph.snapshot",
							payload: { graph },
							sequence: 0,
							stream_id,
							subscription_id: subscribe.subscription_id,
						},
					]);

					return;
				}

				const stream_id = `projection:thread.list:${subscribe.subscription_id}`;

				subscriptions.set(subscribe.subscription_id, {
					_tag: "thread.list",
					sequence: 0,
					stream_id,
				});
				yield* answer([
					{
						...backend_trace(),
						correlation_id: subscribe.message_id,
						kind: "subscription.started",
						payload: { stream_id },
						subscription_id: subscribe.subscription_id,
					},
					{
						...backend_trace(),
						journal_sequence:
							events.at(-1)?.journal_sequence ?? baseline_journal_sequence,
						kind: "thread.list.snapshot",
						payload: { threads: [...threads.values()] },
						sequence: 0,
						stream_id,
						subscription_id: subscribe.subscription_id,
					},
				]);
			});

		const handle = (input: InboundControlEnvelope) =>
			Effect.gen(function* () {
				received_kinds.push(input.kind);

				if (!negotiated && input.kind !== "hello") {
					yield* reject_before_hello(input);

					return;
				}

				switch (input.kind) {
					case "hello":
						yield* handle_hello(input);

						return;
					case "command":
						yield* handle_command(input);

						return;
					case "thread.create.request": {
						const previous = thread_creations.get(input.message_id);
						if (previous) {
							yield* enqueue({
								...backend_trace(),
								correlation_id: input.message_id,
								kind: "thread.create.result",
								payload: previous,
							});
							return;
						}

						thread_sequence += 1;
						const thread_id = `1378080849396121${thread_sequence}`;
						const trace = backend_trace();
						const item: ThreadListItem = {
							activity_version: 0,
							affinity_version: 0,
							created_at: trace.sent_at,
							current_goal: input.payload.title,
							last_activity_at: trace.sent_at,
							linked_projects: [],
							live_status: "Idle",
							metadata_version: 0,
							pinned: false,
							project_affinity_scores: [],
							project_locked: false,
							thread_id,
							title: input.payload.title,
							title_locked: false,
							title_source: "initial",
							updated_at: trace.sent_at,
						};
						const event: EventEnvelope = {
							...trace,
							causation_id: input.message_id,
							correlation_id: input.message_id,
							journal_sequence: baseline_journal_sequence + events.length + 1,
							kind: "event",
							payload: { title: input.payload.title, type: "thread.created" },
							sequence: 1,
							stream_id: `thread:${thread_id}`,
							thread_id,
						};

						threads.set(thread_id, item);
						thread_creations.set(input.message_id, item);
						events.push(event);
						yield* enqueue({
							...backend_trace(),
							correlation_id: input.message_id,
							kind: "thread.create.result",
							payload: item,
						});
						yield* Effect.forEach(
							live_connections,
							(connection) => connection.deliver_event(event),
							{ discard: true },
						);
						return;
					}
					case "thread.list.query":
						if (options.query_delay_ms) {
							yield* Effect.sleep(options.query_delay_ms);
						}

						const result: OutboundControlEnvelope = {
							...backend_trace(),
							correlation_id: input.message_id,
							kind: "thread.list.query.result",
							payload: {
								journal_sequence:
									events.at(-1)?.journal_sequence ?? baseline_journal_sequence,
								threads: [...threads.values()],
							},
						};

						yield* enqueue(result);

						if (options.duplicate_query_result) {
							yield* enqueue(result);
						}

						return;
					case "thread.retention.query":
						yield* enqueue({
							...backend_trace(),
							correlation_id: input.message_id,
							kind: "thread.retention.query.result",
							payload: retention_policy,
						});

						return;
					case "thread.retention.update":
						yield* handle_retention_update(input);

						return;
					case "guidance.query":
						yield* handle_guidance_query(input);

						return;
					case "guidance.update":
					case "guidance.selection":
					case "guidance.drift.resolve":
					case "guidance.sync.retry":
						yield* handle_guidance_mutation(input);

						return;
					case "model_behaviour.query":
						yield* handle_model_behaviour_query(input);

						return;
					case "model_behaviour.update":
					case "model_behaviour.drift.resolve":
					case "model_behaviour.sync.retry":
						yield* handle_model_behaviour_mutation(input);

						return;
					case "workspace.file.read.query":
						yield* handle_workspace_read(input);

						return;
					case "workspace.change.list.query":
						yield* handle_workspace_change_list(input);

						return;
					case "workspace.file.replace":
					case "workspace.change.review":
					case "workspace.change.rollback":
						yield* handle_workspace_mutation(input);

						return;
					case "thread.work.query":
						yield* enqueue({
							...backend_trace(),
							correlation_id: input.message_id,
							kind: "thread.work.query.result",
							payload: {},
						});

						return;
					case "terminal.list.query":
						yield* enqueue({
							...backend_trace(),
							correlation_id: input.message_id,
							kind: "terminal.list.query.result",
							payload: { terminals: [] },
						});

						return;
					case "subscribe":
						yield* handle_subscribe(input);

						return;
					case "unsubscribe":
						if (subscriptions.delete(input.subscription_id)) {
							active_subscriptions -= 1;
						}
						yield* enqueue({
							...backend_trace(),
							correlation_id: input.message_id,
							kind: "subscription.stopped",
							payload: {},
							subscription_id: input.subscription_id,
						});

						return;
					case "ack":
						acknowledgements.push(input);

						return;
					case "heartbeat.pong":
						pongs.push(input);

						return;
					case "replay":
						yield* Effect.forEach(
							events.filter(
								(event) =>
									event.journal_sequence > input.payload.after_journal_sequence,
							),
							enqueue,
							{ discard: true },
						);

						return;
					case "orchestration.graph.query":
						yield* enqueue({
							...backend_trace(),
							correlation_id: input.message_id,
							kind: "orchestration.graph.query.result",
							payload: { graph: get_graph(input.payload.group_id) },
						});

						return;
				}
			});

		const receive = (input: unknown) =>
			DecodeInboundControlEnvelope(input).pipe(
				Effect.flatMap(handle),
				Effect.catch(() => Effect.void),
			);
		const connection: ProtocolConnection = {
			Close: close,
			Closed: Deferred.await(closed),
			Outbound: Stream.fromQueue(outbound),
			Receive: receive,
		};

		return connection;
	});
	const layer = Layer.succeed(ProtocolServer, { Open: open });
	const EraseThread = (thread_id: string) =>
		Effect.gen(function* () {
			if (!threads.delete(thread_id)) {
				return;
			}

			const journal_sequence = baseline_journal_sequence + events.length + 1;
			const stream_id = `thread:${thread_id}`;
			const sequence = events.filter((event) => event.stream_id === stream_id).length + 1;
			const event_id = `thread_erased_${thread_id}`;
			const event: EventEnvelope = {
				...backend_trace(),
				causation_id: event_id,
				correlation_id: event_id,
				journal_sequence,
				kind: "event",
				payload: { type: "thread.erased" },
				sequence,
				stream_id,
				thread_id,
			};

			events.push(event);
			yield* Effect.forEach(
				live_connections,
				(connection) => connection.deliver_event(event),
				{ discard: true },
			);
		});
	const WipeJournal = Effect.sync(() => {
		events.length = 0;
	});
	const snapshot = (): FakeProtocolSnapshot => ({
		acknowledgements: [...acknowledgements],
		active_connections,
		active_subscriptions,
		command_attempts: [...command_attempts],
		hellos: [...hellos],
		opened_connections,
		pongs: [...pongs],
		received_kinds: [...received_kinds],
		retention_update_attempts: [...retention_update_attempts],
		guidance_drift_attempts: [...guidance_drift_attempts],
		guidance_query_attempts: [...guidance_query_attempts],
		guidance_retry_attempts: [...guidance_retry_attempts],
		guidance_selection_attempts: [...guidance_selection_attempts],
		guidance_snapshot,
		guidance_update_attempts: [...guidance_update_attempts],
		model_behaviour_drift_attempts: [...model_behaviour_drift_attempts],
		model_behaviour_query_attempts: [...model_behaviour_query_attempts],
		model_behaviour_retry_attempts: [...model_behaviour_retry_attempts],
		model_behaviour_snapshot: {
			...model_behaviour_snapshot,
			capabilities: [...model_behaviour_snapshot.capabilities],
			providers: [...model_behaviour_snapshot.providers],
			settings: [...model_behaviour_snapshot.settings],
		},
		model_behaviour_update_attempts: [...model_behaviour_update_attempts],
		workspace_change_events: events.filter(
			(event) => event.payload.type === "workspace.change.updated",
		),
		workspace_change_list_attempts: [...workspace_change_list_attempts],
		workspace_change_review_attempts: [...workspace_change_review_attempts],
		workspace_change_rollback_attempts: [...workspace_change_rollback_attempts],
		workspace_file_read_attempts: [...workspace_file_read_attempts],
		workspace_file_replace_attempts: [...workspace_file_replace_attempts],
		subscriptions: [...subscription_attempts],
	});

	return { EraseThread, layer, snapshot, WipeJournal };
}
