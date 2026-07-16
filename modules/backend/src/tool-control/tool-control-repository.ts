import { and, eq } from "drizzle-orm";
import { Context, Crypto, Data, Effect, Encoding, Layer, Option, Schema } from "effect";

import {
	AssignmentWorkspace,
	type DecideApprovalRequest,
	type ToolApprovalProjection,
	type ToolApprovalQuery,
	type ToolApprovalQueryResult,
	type ToolInvocationProjection,
	type ToolInvocationQuery,
	type ToolInvocationQueryResult,
	type InvokeRequest,
	ToolArguments,
	ToolApprovalProjection as ToolApprovalProjectionSchema,
	ToolApprovalQuery as ToolApprovalQuerySchema,
	ToolDescriptor as ToolDescriptorSchema,
	ToolInvocationProjection as ToolInvocationProjectionSchema,
	ToolInvocationQuery as ToolInvocationQuerySchema,
	ToolInvocationQueryResult as ToolInvocationQueryResultSchema,
	ToolApprovalQueryResult as ToolApprovalQueryResultSchema,
	DecideApprovalRequest as DecideApprovalRequestSchema,
	InvokeRequest as InvokeRequestSchema,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import {
	AgentRuns,
	Assignments,
	EventStreams,
	JournalEvents,
	OrchestrationGroups,
	OrchestrationRuns,
	Projects,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	ToolControlCommands,
	ToolInvocationPrivate,
	ToolInvocations,
} from "../persistence/schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { RecordThreadActivity } from "../threads/internal/thread-activity";
import { ToolRegistry } from "./tool-registry";

export class ToolControlConflict extends Data.TaggedError("ToolControlConflict")<{
	readonly reason: "changed_intent";
}> {}

export class ToolControlUnavailable extends Data.TaggedError("ToolControlUnavailable")<{
	readonly reason:
		| "erased"
		| "missing"
		| "ownership"
		| "run_inactive"
		| "tool_unavailable"
		| "workspace_mismatch";
}> {}

export class ToolControlInvariant extends Data.TaggedError("ToolControlInvariant")<{
	readonly message: string;
}> {}

export class ToolControlPersistenceFailure extends Data.TaggedError(
	"ToolControlPersistenceFailure",
)<{
	readonly reason: "unexpected";
}> {}

export type ToolControlRepositoryError =
	| ToolControlConflict
	| ToolControlUnavailable
	| ToolControlInvariant
	| ToolControlPersistenceFailure;

export interface PrepareToolInvocationResult {
	readonly approval?: ToolApprovalProjection;
	readonly invocation: ToolInvocationProjection;
	readonly status: "accepted" | "duplicate";
}

export interface DecideToolApprovalResult {
	readonly approval: ToolApprovalProjection;
	readonly invocation: ToolInvocationProjection;
	readonly status: "accepted" | "duplicate";
}

export class ToolControlRepository extends Context.Service<
	ToolControlRepository,
	{
		readonly Prepare: (
			request: InvokeRequest,
		) => Effect.Effect<PrepareToolInvocationResult, ToolControlRepositoryError>;
		readonly Decide: (
			input: DecideApprovalRequest,
		) => Effect.Effect<DecideToolApprovalResult, ToolControlRepositoryError>;
		readonly QueryApproval: (
			query: ToolApprovalQuery,
		) => Effect.Effect<ToolApprovalQueryResult, ToolControlRepositoryError>;
		readonly QueryInvocation: (
			query: ToolInvocationQuery,
		) => Effect.Effect<ToolInvocationQueryResult, ToolControlRepositoryError>;
	}
>()("Artisan/ToolControlRepository") {}

type InvocationRow = typeof ToolInvocations.$inferSelect;

const text_encoder = new TextEncoder();

function invariant(message: string) {
	return new ToolControlInvariant({ message });
}

function normalize_error(error: unknown): ToolControlRepositoryError {
	if (
		error instanceof ToolControlConflict ||
		error instanceof ToolControlUnavailable ||
		error instanceof ToolControlInvariant ||
		error instanceof ToolControlPersistenceFailure
	) {
		return error;
	}

	return new ToolControlPersistenceFailure({ reason: "unexpected" });
}

function canonical_json(value: unknown): string {
	if (Array.isArray(value)) {
		return `[${value.map(canonical_json).join(",")}]`;
	}

	if (typeof value === "object" && value !== null) {
		const record = value as Readonly<Record<string, unknown>>;

		return `{${Object.keys(record)
			.toSorted()
			.map((key) => `${JSON.stringify(key)}:${canonical_json(record[key])}`)
			.join(",")}}`;
	}

	return JSON.stringify(value);
}

function Decode<A>(schema: Schema.Codec<A, A>, value: unknown, message: string) {
	return Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(value).pipe(
		Effect.mapError(() => invariant(message)),
	);
}

function DecodeStoredJson<A>(schema: Schema.Codec<A, A>, value: string, message: string) {
	return Effect.try({
		try: () => JSON.parse(value) as unknown,
		catch: () => invariant(message),
	}).pipe(Effect.flatMap((parsed) => Decode(schema, parsed, message)));
}

/** Owns durable phase-one tool admission, approval, projection lookup, and journal publication. */
export const ToolControlRepositoryLive = Layer.effect(
	ToolControlRepository,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;
		const registry = yield* ToolRegistry;

		const Hash = (value: string) =>
			crypto.digest("SHA-256", text_encoder.encode(value)).pipe(
				Effect.map(Encoding.encodeHex),
				Effect.mapError(() => invariant("Tool control identity could not be computed")),
			);

		const DecodeInvocation = (row: InvocationRow) =>
			Effect.gen(function* () {
				const input_schema = yield* DecodeStoredJson(
					Schema.Unknown,
					row.input_schema_json,
					`Tool invocation ${row.invocation_id} has invalid descriptor data`,
				);
				const descriptor = yield* Decode(
					ToolDescriptorSchema,
					{
						approval_policy: row.approval_policy,
						effect: row.effect,
						input_schema,
						label: row.label,
						revision: row.revision,
						source: row.source,
						summary: row.summary,
						tool_id: row.tool_id,
					},
					`Tool invocation ${row.invocation_id} has invalid descriptor data`,
				);
				const descriptor_fingerprint = yield* Hash(canonical_json(descriptor));

				if (descriptor_fingerprint !== row.descriptor_fingerprint) {
					return yield* invariant(
						`Tool invocation ${row.invocation_id} descriptor is corrupt`,
					);
				}

				const context = yield* Decode(
					InvokeRequestSchema,
					{
						arguments: {},
						context: {
							agent_id: row.agent_id,
							run_id: row.run_id,
							thread_id: row.thread_id,
							...(row.workspace_id === null
								? {}
								: { workspace_id: row.workspace_id }),
						},
						request_id: row.request_id,
						tool: { revision: row.revision, tool_id: row.tool_id },
					},
					`Tool invocation ${row.invocation_id} has invalid ownership`,
				).pipe(Effect.map((value) => value.context));
				const tool = {
					approval_policy: descriptor.approval_policy,
					effect: descriptor.effect,
					label: descriptor.label,
					revision: descriptor.revision,
					source: descriptor.source,
					summary: descriptor.summary,
					tool_id: descriptor.tool_id,
				};
				const approval =
					row.decision_id === null
						? undefined
						: {
								approval_id: row.approval_id,
								context,
								decided_at: row.decided_at,
								decision: row.decision,
								decision_id: row.decision_id,
								invocation_id: row.invocation_id,
								request_id: row.request_id,
								tool: { revision: row.revision, tool_id: row.tool_id },
							};
				const common = {
					context,
					created_at: row.created_at,
					invocation_id: row.invocation_id,
					request_id: row.request_id,
					tool,
					updated_at: row.updated_at,
				};
				const projection =
					row.state === "approval_required"
						? { ...common, approval_id: row.approval_id, state: row.state }
						: row.state === "denied"
							? { ...common, approval, settled_at: row.settled_at, state: row.state }
							: row.state === "pending"
								? {
										...common,
										...(approval === undefined ? {} : { approval }),
										state: row.state,
									}
								: row.state === "running"
									? {
											...common,
											...(approval === undefined ? {} : { approval }),
											started_at: row.started_at,
											state: row.state,
										}
									: row.state === "suspended"
										? {
												...common,
												...(approval === undefined ? {} : { approval }),
												started_at: row.started_at,
												state: row.state,
												suspended_at: row.suspended_at,
											}
										: {
												...common,
												...(approval === undefined ? {} : { approval }),
												settled_at: row.settled_at,
												started_at: row.started_at,
												state: row.state,
											};

				return yield* Decode(
					ToolInvocationProjectionSchema,
					projection,
					`Tool invocation ${row.invocation_id} is corrupt`,
				);
			});

		const DecodeApproval = (row: InvocationRow) =>
			Effect.gen(function* () {
				if (row.approval_id === null || row.approval_policy !== "required") {
					return yield* invariant(
						`Tool invocation ${row.invocation_id} has invalid approval state`,
					);
				}

				const invocation = yield* DecodeInvocation(row);
				const common = {
					approval_id: row.approval_id,
					context: invocation.context,
					created_at: row.created_at,
					invocation_id: row.invocation_id,
					request_id: row.request_id,
					tool: invocation.tool,
					updated_at: row.updated_at,
				};
				const approval =
					row.state === "approval_required"
						? { ...common, state: "requested" as const }
						: row.state === "denied"
							? {
									...common,
									decided_at: row.decided_at,
									decision_id: row.decision_id,
									state: "denied" as const,
								}
							: row.state === "pending"
								? {
										...common,
										decided_at: row.decided_at,
										decision_id: row.decision_id,
										state: "approved" as const,
									}
								: row.state === "running" || row.state === "suspended"
									? {
											...common,
											decided_at: row.decided_at,
											decision_id: row.decision_id,
											started_at: row.started_at,
											state: "executing" as const,
										}
									: {
											...common,
											decided_at: row.decided_at,
											decision_id: row.decision_id,
											settled_at: row.settled_at,
											started_at: row.started_at,
											state: "settled" as const,
										};

				return yield* Decode(
					ToolApprovalProjectionSchema,
					approval,
					`Tool invocation ${row.invocation_id} approval is corrupt`,
				);
			});

		const EnsureLiveThread = (transaction: typeof database.client, thread_id: string) =>
			Effect.gen(function* () {
				const [thread] = yield* transaction
					.select()
					.from(Threads)
					.where(eq(Threads.thread_id, thread_id))
					.limit(1);
				const [claim] = yield* transaction
					.select()
					.from(ThreadErasureClaims)
					.where(eq(ThreadErasureClaims.thread_id, thread_id))
					.limit(1);
				const [tombstone] = yield* transaction
					.select()
					.from(ThreadTombstones)
					.where(eq(ThreadTombstones.thread_id, thread_id))
					.limit(1);

				if (claim || tombstone)
					return yield* new ToolControlUnavailable({ reason: "erased" });
				if (!thread) return yield* new ToolControlUnavailable({ reason: "missing" });
			});

		const Authorize = (
			transaction: typeof database.client,
			context: InvokeRequest["context"],
		) =>
			Effect.gen(function* () {
				const [ordinary] = yield* transaction
					.select()
					.from(OrchestrationRuns)
					.where(
						and(
							eq(OrchestrationRuns.run_id, context.run_id),
							eq(OrchestrationRuns.thread_id, context.thread_id),
							eq(OrchestrationRuns.agent_id, context.agent_id),
						),
					)
					.limit(1);
				const [graph] = yield* transaction
					.select()
					.from(AgentRuns)
					.where(
						and(
							eq(AgentRuns.run_id, context.run_id),
							eq(AgentRuns.agent_id, context.agent_id),
						),
					)
					.limit(1);
				const ordinary_active =
					ordinary !== undefined &&
					(ordinary.status === "running" || ordinary.status === "waiting");
				const graph_active =
					graph !== undefined &&
					graph.dispatch_status === "active" &&
					(graph.state === "running" || graph.state === "waiting");

				if (ordinary_active && graph_active)
					return yield* invariant("Tool invocation ownership is ambiguous");
				if (!ordinary_active && !graph_active) {
					if (ordinary || graph)
						return yield* new ToolControlUnavailable({ reason: "run_inactive" });
					return yield* new ToolControlUnavailable({ reason: "ownership" });
				}

				if (ordinary_active) {
					if (context.workspace_id !== undefined) {
						const [project] = yield* transaction
							.select()
							.from(Projects)
							.where(
								and(
									eq(Projects.workspace_id, context.workspace_id),
									eq(Projects.canonical_root, ordinary!.working_directory),
								),
							)
							.limit(1);
						if (!project)
							return yield* new ToolControlUnavailable({
								reason: "workspace_mismatch",
							});
					}

					return "ordinary_run" as const;
				}

				const [group] = yield* transaction
					.select()
					.from(OrchestrationGroups)
					.where(
						and(
							eq(OrchestrationGroups.group_id, graph!.group_id),
							eq(OrchestrationGroups.thread_id, context.thread_id),
						),
					)
					.limit(1);
				const [assignment] = yield* transaction
					.select()
					.from(Assignments)
					.where(
						and(
							eq(Assignments.assignment_id, graph!.assignment_id),
							eq(Assignments.group_id, graph!.group_id),
							eq(Assignments.agent_id, context.agent_id),
							eq(Assignments.active_run_id, context.run_id),
						),
					)
					.limit(1);
				if (!group || !assignment)
					return yield* new ToolControlUnavailable({ reason: "ownership" });
				if (
					!(group.state === "running" || group.state === "waiting") ||
					!(assignment.state === "running" || assignment.state === "waiting")
				)
					return yield* new ToolControlUnavailable({ reason: "run_inactive" });
				const workspace = yield* DecodeStoredJson(
					AssignmentWorkspace,
					assignment.workspace_json,
					"Tool invocation assignment workspace is corrupt",
				);
				if (
					context.workspace_id !== undefined &&
					context.workspace_id !== workspace.workspace_id
				)
					return yield* new ToolControlUnavailable({ reason: "workspace_mismatch" });

				return "graph_run" as const;
			});

		const Append = (
			transaction: typeof database.client,
			input: {
				readonly agent_id: string;
				readonly causation_id: string;
				readonly idempotency_key: string;
				readonly payload: any;
				readonly run_id: string;
				readonly thread_id: string;
				readonly occurred_at: string;
			},
		) =>
			Effect.gen(function* () {
				const stream_id = `thread:${input.thread_id}`;
				const [stream] = yield* transaction
					.select()
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);
				const stream_sequence = (stream?.last_sequence ?? 0) + 1;
				const event_id = yield* metadata.MakeId("event");

				yield* RecordThreadActivity(
					transaction,
					input.thread_id,
					input.occurred_at,
					input.payload,
				);

				if (stream) {
					yield* transaction
						.update(EventStreams)
						.set({ last_sequence: stream_sequence })
						.where(eq(EventStreams.stream_id, stream_id));
				} else {
					yield* transaction
						.insert(EventStreams)
						.values({ last_sequence: stream_sequence, stream_id });
				}

				const [event] = yield* transaction
					.insert(JournalEvents)
					.values({
						agent_id: input.agent_id,
						causation_id: input.causation_id,
						correlation_id: input.causation_id,
						event_id,
						event_type: input.payload.type,
						idempotency_key: input.idempotency_key,
						occurred_at: input.occurred_at,
						origin: "backend",
						payload_json: JSON.stringify(input.payload),
						raw_origin_json: null,
						run_id: input.run_id,
						schema_version: 1,
						stream_id,
						stream_sequence,
						thread_id: input.thread_id,
					})
					.returning({ journal_sequence: JournalEvents.sequence });

				if (!event) return yield* invariant("Tool control event was not persisted");

				return event.journal_sequence;
			});

		const Prepare = (input: InvokeRequest) =>
			Effect.gen(function* () {
				const request = yield* Decode(
					InvokeRequestSchema,
					input,
					"Tool invocation request is invalid",
				);
				const [known_command] = yield* database.client
					.select({ command_id: ToolControlCommands.command_id })
					.from(ToolControlCommands)
					.where(eq(ToolControlCommands.command_id, request.request_id))
					.limit(1)
					.pipe(Effect.mapError(normalize_error));

				const authorization = known_command
					? undefined
					: yield* registry
							.Authorize(request.tool, request.context)
							.pipe(
								Effect.mapError(
									() =>
										new ToolControlUnavailable({ reason: "tool_unavailable" }),
								),
							);

				const canonical_arguments = canonical_json(request.arguments);
				const canonical_request = canonical_json({
					arguments: request.arguments,
					context: request.context,
					request_id: request.request_id,
					tool: request.tool,
				});
				const arguments_digest = yield* Hash(canonical_arguments);
				const request_fingerprint = yield* Hash(canonical_request);
				const admission =
					authorization === undefined
						? undefined
						: {
								descriptor: authorization.descriptor,
								descriptor_fingerprint: yield* Hash(
									canonical_json(authorization.descriptor),
								),
								recovery_policy: authorization.recovery_policy,
							};
				const result = yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const [command] = yield* transaction
								.select()
								.from(ToolControlCommands)
								.where(eq(ToolControlCommands.command_id, request.request_id))
								.limit(1);
							if (command) {
								if (
									command.kind !== "invoke" ||
									command.request_fingerprint !== request_fingerprint
								)
									return yield* new ToolControlConflict({
										reason: "changed_intent",
									});
								const [row] = yield* transaction
									.select()
									.from(ToolInvocations)
									.where(eq(ToolInvocations.invocation_id, command.invocation_id))
									.limit(1);
								const [private_row] = yield* transaction
									.select()
									.from(ToolInvocationPrivate)
									.where(
										eq(
											ToolInvocationPrivate.invocation_id,
											command.invocation_id,
										),
									)
									.limit(1);
								if (!row || !private_row)
									return yield* invariant("Tool invocation replay is incomplete");

								yield* EnsureLiveThread(transaction, row.thread_id);

								const stored_arguments = yield* DecodeStoredJson(
									ToolArguments,
									private_row.arguments_json,
									"Tool invocation private arguments are corrupt",
								);
								const stored_arguments_json = canonical_json(stored_arguments);
								const stored_arguments_digest = yield* Hash(stored_arguments_json);
								const stored_request_fingerprint = yield* Hash(
									canonical_json({
										arguments: stored_arguments,
										context: {
											agent_id: row.agent_id,
											run_id: row.run_id,
											thread_id: row.thread_id,
											...(row.workspace_id === null
												? {}
												: { workspace_id: row.workspace_id }),
										},
										request_id: row.request_id,
										tool: { revision: row.revision, tool_id: row.tool_id },
									}),
								);

								if (
									private_row.arguments_json !== stored_arguments_json ||
									private_row.arguments_digest !== stored_arguments_digest ||
									private_row.request_fingerprint !== stored_request_fingerprint
								)
									return yield* invariant(
										"Tool invocation private arguments are corrupt",
									);
								if (
									row.request_id !== request.request_id ||
									row.run_id !== request.context.run_id ||
									row.agent_id !== request.context.agent_id ||
									row.thread_id !== request.context.thread_id ||
									row.workspace_id !== (request.context.workspace_id ?? null) ||
									row.tool_id !== request.tool.tool_id ||
									row.revision !== request.tool.revision ||
									private_row.request_fingerprint !== request_fingerprint ||
									private_row.arguments_digest !== arguments_digest ||
									private_row.arguments_json !== canonical_arguments
								)
									return yield* new ToolControlConflict({
										reason: "changed_intent",
									});
								const invocation = yield* DecodeInvocation(row);
								const approval =
									row.approval_id === null
										? undefined
										: yield* DecodeApproval(row);
								return {
									approval,
									invocation,
									journal_sequence: undefined,
									status: "duplicate" as const,
								};
							}

							yield* EnsureLiveThread(transaction, request.context.thread_id);

							if (admission === undefined) {
								return yield* invariant("Tool registry admission is missing");
							}

							const { descriptor, descriptor_fingerprint, recovery_policy } =
								admission;
							const owner_kind = yield* Authorize(transaction, request.context);
							const invocation_id = yield* metadata.MakeId("invocation");
							const approval_id =
								descriptor.approval_policy === "required"
									? yield* metadata.MakeId("approval")
									: null;
							const now = yield* metadata.Now;
							const state =
								descriptor.approval_policy === "required"
									? "approval_required"
									: "pending";
							const row = {
								approval_id,
								approval_policy: descriptor.approval_policy,
								agent_id: request.context.agent_id,
								created_at: now,
								current_journal_sequence: 1,
								decided_at: null,
								decision: null,
								decision_id: null,
								descriptor_fingerprint,
								effect: descriptor.effect,
								input_schema_json: canonical_json(descriptor.input_schema),
								invocation_id,
								owner_kind,
								recovery_policy,
								request_id: request.request_id,
								revision: descriptor.revision,
								run_id: request.context.run_id,
								settled_at: null,
								source: descriptor.source,
								started_at: null,
								state,
								summary: descriptor.summary,
								suspended_at: null,
								thread_id: request.context.thread_id,
								tool_id: descriptor.tool_id,
								updated_at: now,
								workspace_id: request.context.workspace_id ?? null,
								label: descriptor.label,
							};
							const invocation = yield* DecodeInvocation(row);
							const approval =
								approval_id === null ? undefined : yield* DecodeApproval(row);
							const events =
								descriptor.approval_policy === "required"
									? [
											{
												idempotency_key: `tool_approval:${approval_id}:requested`,
												payload: {
													approval,
													type: "tool.approval.updated",
												},
											},
											{
												idempotency_key: `tool_invocation:${invocation_id}:approval_required`,
												payload: {
													effect: descriptor.effect,
													invocation_id,
													label: descriptor.label,
													source: descriptor.source,
													state: "approval_required",
													summary: descriptor.summary,
													type: "capability.invocation.updated",
												},
											},
											{
												idempotency_key: `tool_invocation:${invocation_id}:approval_required:full`,
												payload: {
													invocation,
													type: "tool.invocation.updated",
												},
											},
										]
									: [
											{
												idempotency_key: `tool_invocation:${invocation_id}:started`,
												payload: {
													effect: descriptor.effect,
													invocation_id,
													label: descriptor.label,
													source: descriptor.source,
													state: "started",
													summary: descriptor.summary,
													type: "capability.invocation.updated",
												},
											},
											{
												idempotency_key: `tool_invocation:${invocation_id}:pending:full`,
												payload: {
													invocation,
													type: "tool.invocation.updated",
												},
											},
										];
							let journal_sequence = 0;
							for (const event of events)
								journal_sequence = yield* Append(transaction, {
									agent_id: row.agent_id,
									causation_id: request.request_id,
									idempotency_key: event.idempotency_key,
									occurred_at: now,
									payload: event.payload,
									run_id: row.run_id,
									thread_id: row.thread_id,
								});
							yield* transaction
								.insert(ToolInvocations)
								.values({ ...row, current_journal_sequence: journal_sequence });
							yield* transaction.insert(ToolInvocationPrivate).values({
								arguments_digest,
								arguments_json: canonical_arguments,
								invocation_id,
								request_fingerprint,
								result_digest: null,
								result_json: null,
							});
							yield* transaction.insert(ToolControlCommands).values({
								accepted_at: now,
								approval_id: null,
								command_id: request.request_id,
								decision: null,
								invocation_id,
								kind: "invoke",
								request_fingerprint,
							});

							return {
								...(approval === undefined ? {} : { approval }),
								invocation,
								journal_sequence,
								status: "accepted" as const,
							};
						}),
					),
				).pipe(Effect.mapError(normalize_error));

				if (result.status === "accepted")
					yield* notifier.Publish(result.journal_sequence!).pipe(Effect.ignoreCause);

				return {
					...(result.approval === undefined ? {} : { approval: result.approval }),
					invocation: result.invocation,
					status: result.status,
				};
			});

		const Decide = (input: DecideApprovalRequest) =>
			Effect.gen(function* () {
				const decision = yield* Decode(
					DecideApprovalRequestSchema,
					input,
					"Tool approval decision is invalid",
				);
				const request_fingerprint = yield* Hash(canonical_json(decision));
				const result = yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const [existing] = yield* transaction
								.select()
								.from(ToolControlCommands)
								.where(eq(ToolControlCommands.command_id, decision.decision_id))
								.limit(1);
							if (existing) {
								if (
									existing.kind !== "decision" ||
									existing.approval_id !== decision.approval_id ||
									existing.decision !== decision.decision ||
									existing.request_fingerprint !== request_fingerprint
								)
									return yield* new ToolControlConflict({
										reason: "changed_intent",
									});
								const [row] = yield* transaction
									.select()
									.from(ToolInvocations)
									.where(
										eq(ToolInvocations.invocation_id, existing.invocation_id),
									)
									.limit(1);
								if (!row)
									return yield* invariant("Tool approval replay is incomplete");

								yield* EnsureLiveThread(transaction, row.thread_id);

								if (
									row.approval_id !== decision.approval_id ||
									row.decision_id !== decision.decision_id ||
									row.decision !== decision.decision
								)
									return yield* invariant("Tool approval replay is corrupt");
								const approval = yield* DecodeApproval(row);
								const invocation = yield* DecodeInvocation(row);
								return {
									approval,
									invocation,
									journal_sequence: undefined,
									status: "duplicate" as const,
								};
							}

							const [row] = yield* transaction
								.select()
								.from(ToolInvocations)
								.where(
									and(
										eq(ToolInvocations.approval_id, decision.approval_id),
										eq(ToolInvocations.thread_id, decision.thread_id),
									),
								)
								.limit(1);
							if (!row)
								return yield* new ToolControlUnavailable({ reason: "missing" });
							yield* EnsureLiveThread(transaction, decision.thread_id);
							if (row.state !== "approval_required")
								return yield* new ToolControlConflict({ reason: "changed_intent" });
							const owner_kind = yield* Authorize(transaction, {
								agent_id: row.agent_id,
								run_id: row.run_id,
								thread_id: row.thread_id,
								...(row.workspace_id === null
									? {}
									: { workspace_id: row.workspace_id }),
							});

							if (owner_kind !== row.owner_kind)
								return yield* invariant("Tool invocation ownership is corrupt");

							const now = yield* metadata.Now;
							const next_state =
								decision.decision === "approved" ? "pending" : "denied";
							const next_row = {
								...row,
								decided_at: now,
								decision: decision.decision,
								decision_id: decision.decision_id,
								settled_at: decision.decision === "denied" ? now : null,
								state: next_state,
								updated_at: now,
							};
							const approval = yield* DecodeApproval(next_row);
							const invocation = yield* DecodeInvocation(next_row);
							const capability_state =
								decision.decision === "approved" ? "started" : "denied";
							const events = [
								{
									idempotency_key: `tool_approval:${row.approval_id}:${decision.decision}`,
									payload: { approval, type: "tool.approval.updated" },
								},
								{
									idempotency_key: `tool_invocation:${row.invocation_id}:${capability_state}`,
									payload: {
										effect: row.effect,
										invocation_id: row.invocation_id,
										label: row.label,
										source: row.source,
										state: capability_state,
										summary: row.summary,
										type: "capability.invocation.updated",
									},
								},
								{
									idempotency_key: `tool_invocation:${row.invocation_id}:${next_state}:full`,
									payload: { invocation, type: "tool.invocation.updated" },
								},
							];
							let journal_sequence = 0;
							for (const event of events)
								journal_sequence = yield* Append(transaction, {
									agent_id: row.agent_id,
									causation_id: decision.decision_id,
									idempotency_key: event.idempotency_key,
									occurred_at: now,
									payload: event.payload,
									run_id: row.run_id,
									thread_id: row.thread_id,
								});
							yield* transaction
								.update(ToolInvocations)
								.set({ ...next_row, current_journal_sequence: journal_sequence })
								.where(eq(ToolInvocations.invocation_id, row.invocation_id));
							yield* transaction.insert(ToolControlCommands).values({
								accepted_at: now,
								approval_id: row.approval_id,
								command_id: decision.decision_id,
								decision: decision.decision,
								invocation_id: row.invocation_id,
								kind: "decision",
								request_fingerprint,
							});

							return {
								approval,
								invocation,
								journal_sequence,
								status: "accepted" as const,
							};
						}),
					),
				).pipe(Effect.mapError(normalize_error));

				if (result.status === "accepted")
					yield* notifier.Publish(result.journal_sequence!).pipe(Effect.ignoreCause);

				return result;
			});

		const QueryInvocation: (
			query: ToolInvocationQuery,
		) => Effect.Effect<ToolInvocationQueryResult, ToolControlRepositoryError> = (query) =>
			Effect.gen(function* () {
				const decoded = yield* Decode(
					ToolInvocationQuerySchema,
					query,
					"Tool invocation query is invalid",
				);
				const [row] = yield* database.client
					.transaction((transaction) =>
						Effect.gen(function* () {
							const rows = yield* transaction
								.select()
								.from(ToolInvocations)
								.where(
									and(
										eq(ToolInvocations.invocation_id, decoded.invocation_id),
										eq(ToolInvocations.thread_id, decoded.thread_id),
									),
								)
								.limit(1);
							const [stored_row] = rows;

							if (stored_row)
								yield* EnsureLiveThread(transaction, stored_row.thread_id);

							return rows;
						}),
					)
					.pipe(Effect.mapError(normalize_error));
				return yield* Option.match(Option.fromNullishOr(row), {
					onNone: () =>
						Decode(
							ToolInvocationQueryResultSchema,
							{},
							"Tool invocation query result is corrupt",
						),
					onSome: (stored_row) =>
						DecodeInvocation(stored_row).pipe(
							Effect.flatMap((invocation) =>
								Decode(
									ToolInvocationQueryResultSchema,
									{ invocation },
									"Tool invocation query result is corrupt",
								),
							),
						),
				});
			});

		const QueryApproval: (
			query: ToolApprovalQuery,
		) => Effect.Effect<ToolApprovalQueryResult, ToolControlRepositoryError> = (query) =>
			Effect.gen(function* () {
				const decoded = yield* Decode(
					ToolApprovalQuerySchema,
					query,
					"Tool approval query is invalid",
				);
				const [row] = yield* database.client
					.transaction((transaction) =>
						Effect.gen(function* () {
							const rows = yield* transaction
								.select()
								.from(ToolInvocations)
								.where(
									and(
										eq(ToolInvocations.approval_id, decoded.approval_id),
										eq(ToolInvocations.thread_id, decoded.thread_id),
									),
								)
								.limit(1);
							const [stored_row] = rows;

							if (stored_row)
								yield* EnsureLiveThread(transaction, stored_row.thread_id);

							return rows;
						}),
					)
					.pipe(Effect.mapError(normalize_error));
				return yield* Option.match(Option.fromNullishOr(row), {
					onNone: () =>
						Decode(
							ToolApprovalQueryResultSchema,
							{},
							"Tool approval query result is corrupt",
						),
					onSome: (stored_row) =>
						DecodeApproval(stored_row).pipe(
							Effect.flatMap((approval) =>
								Decode(
									ToolApprovalQueryResultSchema,
									{ approval },
									"Tool approval query result is corrupt",
								),
							),
						),
				});
			});

		return { Decide, Prepare, QueryApproval, QueryInvocation };
	}),
);
