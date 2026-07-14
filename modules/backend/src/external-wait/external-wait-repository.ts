import {
	and,
	asc,
	eq,
	exists,
	gt,
	inArray,
	isNotNull,
	isNull,
	lte,
	or,
	sql,
	type SQL,
} from "drizzle-orm";
import { Context, Crypto, Data, DateTime, Effect, Encoding, Layer, Option, Schema } from "effect";

import {
	AssignmentWorkspace,
	ExternalWaitGate,
	ExternalWaitOwner,
	ExternalWaitRequest,
	ExternalWaitSnapshot,
	ExternalWaitState,
	ExternalWaitTarget,
	ExternalWaitTrigger,
	Identifier,
	IsoDateTime,
	ProjectRef,
	CommandPayload,
	type ExternalWaitSnapshot as ExternalWaitSnapshotValue,
} from "@artisan/protocol";
import { EngineResumeToken } from "@artisan/engines";

import { ExternalWaitBaseline, serialize_external_wait_baseline } from "./external-wait-policy";
import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { JournalStoreFailure } from "../persistence/journal-store";
import {
	AgentInstances,
	AgentRuns,
	Assignments,
	EventStreams,
	ExternalWaitOperations,
	ExternalWaits,
	ExternalWaitWakeOutbox,
	JournalCommands,
	JournalEvents,
	OrchestrationCoordinators,
	OrchestrationGroups,
	OrchestrationMessages,
	OrchestrationOutbox,
	OrchestrationRuns,
	ProjectHostedOrigins,
	Projects,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
} from "../persistence/schema";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

const Fingerprint = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/u));

const SourceCommand = Schema.Struct({
	message_id: Identifier,
	sent_at: IsoDateTime,
});

const MaximumGeneration = Schema.Int.check(
	Schema.isGreaterThanOrEqualTo(1),
	Schema.isLessThanOrEqualTo(10),
);

const RegisterInput = Schema.Struct({
	baseline: ExternalWaitBaseline,
	maximum_generation: Schema.optional(MaximumGeneration),
	owner: ExternalWaitOwner,
	project_id: Identifier,
	request: ExternalWaitRequest,
	request_fingerprint: Fingerprint,
	source_command: SourceCommand,
	target: ExternalWaitTarget,
	thread_id: Identifier,
	wait_id: Identifier,
});

const RequestReplayInput = Schema.Struct({
	request_fingerprint: Fingerprint,
	source_command: SourceCommand,
	thread_id: Identifier,
	wait_id: Identifier,
});

const ObservationClaim = Schema.Struct({
	lease_owner: Identifier,
	now: IsoDateTime,
	wait_id: Identifier,
});

const ObservationRecord = Schema.Struct({
	lease_owner: Identifier,
	next_observation_at: IsoDateTime,
	now: IsoDateTime,
	state: ExternalWaitState,
	wait_id: Identifier,
});

const WakeMutationInput = Schema.Struct({
	now: IsoDateTime,
	trigger: ExternalWaitTrigger,
	wait_id: Identifier,
});

const WakeInput = Schema.Struct({
	lease_owner: Identifier,
	now: IsoDateTime,
	trigger: ExternalWaitTrigger,
	wait_id: Identifier,
});

const WakeClaim = Schema.Struct({
	lease_owner: Identifier,
	now: IsoDateTime,
	outbox_id: Identifier,
});

const WakeMaterialization = Schema.Struct({
	lease_owner: Identifier,
	native_resume_supported: Schema.Boolean,
	now: IsoDateTime,
	outbox_id: Identifier,
});

const SourceClosure = Schema.Struct({
	now: IsoDateTime,
	wait_id: Identifier,
});

const SourceRunClosure = Schema.Struct({
	now: IsoDateTime,
	owner_tag: Schema.Literals(["thread_run", "assignment_run"]),
	source_run_id: Identifier,
});

const TerminalSourceClosureRecovery = Schema.Struct({ now: IsoDateTime });

const CancelInput = Schema.Struct({
	now: IsoDateTime,
	reason: Schema.Literals(["user", "thread_terminal", "project_removed", "superseded"]),
	source_command: SourceCommand,
	thread_id: Identifier,
	wait_id: Identifier,
});

const ManualResumeInput = Schema.Struct({
	now: IsoDateTime,
	source_command: SourceCommand,
	thread_id: Identifier,
	wait_id: Identifier,
});

const DiscoverInput = Schema.Struct({ now: IsoDateTime });
const QueryInput = Schema.Struct({ thread_id: Identifier });

export type ExternalWaitRegistration = typeof RegisterInput.Type;

export class ExternalWaitConflict extends Data.TaggedError("ExternalWaitConflict")<{
	readonly reason: "changed_intent" | "source_run_claimed" | "source_command_claimed";
}> {}

export class ExternalWaitUnavailable extends Data.TaggedError("ExternalWaitUnavailable")<{
	readonly reason: "erased" | "lease_lost" | "missing" | "ownership" | "project_missing";
}> {}

export class ExternalWaitInvariant extends Data.TaggedError("ExternalWaitInvariant")<{
	readonly message: string;
}> {}

export type ExternalWaitRepositoryError =
	| ExternalWaitConflict
	| ExternalWaitInvariant
	| ExternalWaitUnavailable
	| JournalStoreFailure;

export interface ExternalWaitAcceptance {
	readonly snapshot: ExternalWaitSnapshotValue;
	readonly status: "accepted" | "duplicate";
}

export interface ExternalWaitObservationClaim {
	readonly baseline: typeof ExternalWaitBaseline.Type;
	readonly lease_expires_at: string;
	readonly snapshot: ExternalWaitSnapshotValue;
	readonly timeout_at: string;
}

export interface ExternalWaitWake {
	readonly follow_up_command_id: string;
	readonly follow_up_run_id: string;
	readonly outbox_id: string;
	readonly trigger_fingerprint: string;
}

export interface ExternalWaitWakeDiscovery {
	readonly outbox_id: string;
	readonly thread_id: string;
}

export interface ExternalWaitManualResumeAcceptance extends ExternalWaitAcceptance {
	readonly wake: ExternalWaitWake;
}

export interface ExternalWaitWakeClaim extends ExternalWaitWake {
	readonly lease_expires_at: string;
	readonly owner: typeof ExternalWaitOwner.Type;
	readonly snapshot: ExternalWaitSnapshotValue;
	readonly trigger: typeof ExternalWaitTrigger.Type;
}

export interface ExternalWaitMaterialization {
	readonly follow_up_run_id: string;
	readonly mode: "native_resume" | "linked_run";
	readonly owner: typeof ExternalWaitOwner.Type;
	readonly snapshot: ExternalWaitSnapshotValue;
	readonly status: "created" | "duplicate";
}

/** Owns private external-review evidence, public projections, and durable wake delivery. */
export class ExternalWaitRepository extends Context.Service<
	ExternalWaitRepository,
	{
		readonly Register: (
			input: ExternalWaitRegistration,
		) => Effect.Effect<ExternalWaitAcceptance, ExternalWaitRepositoryError>;
		readonly Replay: (
			input: ExternalWaitRegistration,
		) => Effect.Effect<Option.Option<ExternalWaitAcceptance>, ExternalWaitRepositoryError>;
		readonly ReplayRequest: (
			input: typeof RequestReplayInput.Type,
		) => Effect.Effect<Option.Option<ExternalWaitAcceptance>, ExternalWaitRepositoryError>;
		readonly Query: (input: typeof QueryInput.Type) => Effect.Effect<
			{
				readonly snapshots: ReadonlyArray<ExternalWaitSnapshotValue>;
				readonly truncated: boolean;
			},
			ExternalWaitRepositoryError
		>;
		readonly DiscoverDueObservations: (
			input: typeof DiscoverInput.Type,
		) => Effect.Effect<ReadonlyArray<string>, ExternalWaitRepositoryError>;
		readonly ClaimObservation: (
			input: typeof ObservationClaim.Type,
		) => Effect.Effect<
			Option.Option<ExternalWaitObservationClaim>,
			ExternalWaitRepositoryError
		>;
		readonly ReleaseObservation: (
			input: typeof ObservationRecord.Type,
		) => Effect.Effect<Option.Option<ExternalWaitSnapshotValue>, ExternalWaitRepositoryError>;
		readonly RecordObservation: (
			input: typeof ObservationRecord.Type,
		) => Effect.Effect<Option.Option<ExternalWaitSnapshotValue>, ExternalWaitRepositoryError>;
		readonly MarkSourceClosed: (
			input: typeof SourceClosure.Type,
		) => Effect.Effect<Option.Option<ExternalWaitSnapshotValue>, ExternalWaitRepositoryError>;
		readonly MarkSourceClosedForRun: (
			input: typeof SourceRunClosure.Type,
		) => Effect.Effect<Option.Option<ExternalWaitSnapshotValue>, ExternalWaitRepositoryError>;
		readonly ReconcileSourceClosures: (
			input: typeof TerminalSourceClosureRecovery.Type,
		) => Effect.Effect<ReadonlyArray<string>, ExternalWaitRepositoryError>;
		readonly CreateWake: (
			input: typeof WakeInput.Type,
		) => Effect.Effect<ExternalWaitWake, ExternalWaitRepositoryError>;
		readonly DiscoverWakes: (
			input: typeof DiscoverInput.Type,
		) => Effect.Effect<ReadonlyArray<ExternalWaitWakeDiscovery>, ExternalWaitRepositoryError>;
		readonly ClaimWake: (
			input: typeof WakeClaim.Type,
		) => Effect.Effect<Option.Option<ExternalWaitWakeClaim>, ExternalWaitRepositoryError>;
		readonly ReleaseWake: (
			input: typeof WakeClaim.Type,
		) => Effect.Effect<Option.Option<ExternalWaitWake>, ExternalWaitRepositoryError>;
		readonly MaterializeWake: (
			input: typeof WakeMaterialization.Type,
		) => Effect.Effect<ExternalWaitMaterialization, ExternalWaitRepositoryError>;
		readonly Cancel: (
			input: typeof CancelInput.Type,
		) => Effect.Effect<Option.Option<ExternalWaitAcceptance>, ExternalWaitRepositoryError>;
		readonly ManualResume: (
			input: typeof ManualResumeInput.Type,
		) => Effect.Effect<ExternalWaitManualResumeAcceptance, ExternalWaitRepositoryError>;
	}
>()("Artisan/ExternalWaitRepository") {}

type CommandKind = "request" | "cancel" | "manual_resume";
type WaitRow = typeof ExternalWaits.$inferSelect;
type WakeRow = typeof ExternalWaitWakeOutbox.$inferSelect;
type ValidRegistration = typeof RegisterInput.Type & { readonly baseline_fingerprint: string };

const observation_lease_seconds = 30;
const observation_interval_seconds = 15;
const timeout_days = 7;
const default_maximum_generation = 3;

function invariant(message: string) {
	return new ExternalWaitInvariant({ message });
}

function normalize_error(error: unknown): ExternalWaitRepositoryError {
	if (
		error instanceof ExternalWaitConflict ||
		error instanceof ExternalWaitInvariant ||
		error instanceof ExternalWaitUnavailable ||
		error instanceof JournalStoreFailure
	) {
		return error;
	}

	return new JournalStoreFailure({ cause: error });
}

function json(value: unknown) {
	return JSON.stringify(value);
}

function fingerprint_frame(value: string) {
	return new TextEncoder().encode(value);
}

function wait_event_key(wait_id: string, version: number) {
	return `external_wait:${wait_id}:${version}`;
}

function command_payload(kind: CommandKind, wait_id: string, fingerprint: string) {
	return json({ fingerprint, kind, type: "external_wait.command", wait_id });
}

function command_payload_type(kind: CommandKind) {
	return `external_wait.${kind}`;
}

function canonical_gates(gates: ReadonlyArray<typeof ExternalWaitGate.Type>) {
	return json(
		gates
			.map((gate) =>
				gate._tag === "selected_checks_terminal"
					? { ...gate, check_names: [...gate.check_names].sort() }
					: gate,
			)
			.sort((left, right) => left._tag.localeCompare(right._tag)),
	);
}

function registration_matches_snapshot(
	registration: ValidRegistration,
	snapshot: ExternalWaitSnapshotValue,
) {
	return (
		snapshot.baseline_fingerprint === registration.baseline_fingerprint &&
		snapshot.project_id === registration.project_id &&
		snapshot.thread_id === registration.thread_id &&
		snapshot.wait_id === registration.wait_id &&
		snapshot.workspace_id === registration.request.workspace_id &&
		json(snapshot.owner) === json(registration.owner) &&
		json(snapshot.target) === json(registration.target) &&
		canonical_gates(snapshot.gates) === canonical_gates(registration.request.gates) &&
		(registration.maximum_generation === undefined ||
			snapshot.maximum_generation === registration.maximum_generation)
	);
}

function ParseInstant(value: string, label: string) {
	return Option.match(DateTime.make(value), {
		onNone: () => Effect.fail(invariant(`${label} is invalid`)),
		onSome: Effect.succeed,
	});
}

function AddDuration(value: string, duration: Parameters<typeof DateTime.add>[1], label: string) {
	return Effect.gen(function* () {
		const date_time = yield* ParseInstant(value, label);

		return DateTime.formatIso(DateTime.add(date_time, duration));
	});
}

function HasExpired(expires_at: string, now: string, label: string) {
	return Effect.gen(function* () {
		const expiry = yield* ParseInstant(expires_at, label);
		const current = yield* ParseInstant(now, "External wait clock");

		return DateTime.toEpochMillis(expiry) <= DateTime.toEpochMillis(current);
	});
}

/** Supplies the SQLite-backed external-wait repository. */
export const ExternalWaitRepositoryLive = Layer.effect(
	ExternalWaitRepository,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const DecodeJson = <A>(
			schema: Schema.ConstraintDecoder<A, never>,
			value: string,
			label: string,
		) =>
			Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(value).pipe(
				Effect.flatMap(Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })),
				Effect.mapError(() => invariant(`Stored ${label} is corrupt`)),
			);

		const HashText = (value: string): Effect.Effect<string, ExternalWaitRepositoryError> =>
			crypto
				.digest("SHA-256", fingerprint_frame(value))
				.pipe(Effect.map(Encoding.encodeHex), Effect.mapError(normalize_error));

		const BaselineFingerprint = (
			baseline: typeof ExternalWaitBaseline.Type,
		): Effect.Effect<string, ExternalWaitRepositoryError> =>
			HashText(serialize_external_wait_baseline(baseline));

		const DecodeState = (row: WaitRow) =>
			DecodeJson(ExternalWaitState, row.state_json, "external wait state").pipe(
				Effect.flatMap((state) =>
					state._tag === row.state
						? Effect.succeed(state)
						: Effect.fail(invariant("Stored external wait state tag is corrupt")),
				),
			);

		const DecodeSnapshot = (row: WaitRow) =>
			Effect.gen(function* () {
				const baseline = yield* DecodeJson(
					ExternalWaitBaseline,
					row.baseline_json,
					"external wait baseline",
				);
				const gates = yield* DecodeJson(
					Schema.Array(ExternalWaitGate),
					row.gates_json,
					"external wait gates",
				);
				const owner = yield* DecodeJson(
					ExternalWaitOwner,
					row.owner_json,
					"external wait owner",
				);
				const state = yield* DecodeState(row);
				const target = yield* DecodeJson(
					ExternalWaitTarget,
					row.target_json,
					"external wait target",
				);
				const baseline_fingerprint = yield* BaselineFingerprint(baseline);
				const baseline_target = {
					branch: baseline.branch,
					expected_head_commit: baseline.expected_head_commit,
					pull_request_number: baseline.pull_request_number,
					pull_request_origin: baseline.pull_request_origin,
					repository: baseline.repository,
				};

				if (
					baseline_fingerprint !== row.baseline_fingerprint ||
					json(baseline_target) !== json(target) ||
					canonical_gates(baseline.gates) !== canonical_gates(gates)
				) {
					return yield* invariant("Stored external wait baseline identity is corrupt");
				}

				return yield* Schema.decodeUnknownEffect(ExternalWaitSnapshot, {
					onExcessProperty: "error",
				})({
					baseline_fingerprint: row.baseline_fingerprint,
					created_at: row.created_at,
					gates,
					generation: row.generation,
					journal_sequence: row.journal_sequence,
					maximum_generation: row.maximum_generation,
					owner,
					project_id: row.project_id,
					state,
					target,
					thread_id: row.thread_id,
					updated_at: row.updated_at,
					version: row.version,
					wait_id: row.wait_id,
					workspace_id: row.workspace_id,
				}).pipe(
					Effect.mapError(() => invariant("Stored external wait projection is corrupt")),
				);
			});

		const DecodeBaseline = (row: WaitRow) =>
			DecodeJson(ExternalWaitBaseline, row.baseline_json, "external wait baseline");

		const DecodeOperationSnapshot = (value: string) =>
			DecodeJson(ExternalWaitSnapshot, value, "external wait operation result");

		const ValidateRegistration = (
			input: ExternalWaitRegistration,
		): Effect.Effect<ValidRegistration, ExternalWaitRepositoryError> =>
			Schema.decodeUnknownEffect(RegisterInput, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(() => new ExternalWaitConflict({ reason: "changed_intent" })),
				Effect.flatMap((decoded) =>
					BaselineFingerprint(decoded.baseline).pipe(
						Effect.map((baseline_fingerprint) => ({
							...decoded,
							baseline_fingerprint,
						})),
					),
				),
				Effect.flatMap((decoded) => {
					const baseline_target = {
						branch: decoded.baseline.branch,
						expected_head_commit: decoded.baseline.expected_head_commit,
						pull_request_number: decoded.baseline.pull_request_number,
						pull_request_origin: decoded.baseline.pull_request_origin,
						repository: decoded.baseline.repository,
					};
					const owner_matches = decoded.owner.run_id === decoded.request.source_run_id;
					const target_matches = json(baseline_target) === json(decoded.target);
					const request_matches =
						decoded.baseline.expected_head_commit ===
							decoded.request.expected_head_commit &&
						decoded.baseline.pull_request_number ===
							decoded.request.pull_request_number &&
						canonical_gates(decoded.baseline.gates) ===
							canonical_gates(decoded.request.gates);

					return owner_matches && target_matches && request_matches
						? Effect.succeed(decoded)
						: Effect.fail(new ExternalWaitConflict({ reason: "changed_intent" }));
				}),
			);

		const AppendEvent = (
			transaction: typeof database.client,
			snapshot: ExternalWaitSnapshotValue,
			causation_id: string,
			correlation_id: string,
		) =>
			Effect.gen(function* () {
				const stream_id = `thread:${snapshot.thread_id}`;
				const [stream] = yield* transaction
					.select()
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);
				const stream_sequence = (stream?.last_sequence ?? 0) + 1;

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
						causation_id,
						correlation_id,
						event_id: yield* metadata.MakeId("event"),
						event_type: "external_wait.updated",
						idempotency_key: wait_event_key(snapshot.wait_id, snapshot.version),
						occurred_at: snapshot.updated_at,
						origin: "backend",
						payload_json: json({ snapshot, type: "external_wait.updated" }),
						raw_origin_json: null,
						schema_version: 1,
						stream_id,
						stream_sequence,
						thread_id: snapshot.thread_id,
					})
					.returning({ sequence: JournalEvents.sequence });

				if (!event) {
					return yield* invariant("External wait event was not persisted");
				}

				return event.sequence;
			});

		const PersistVisibleUpdate = (
			transaction: typeof database.client,
			row: WaitRow,
			state: typeof ExternalWaitState.Type,
			now: string,
			next_observation_at: string,
			causation_id: string,
			guard?: SQL,
		) =>
			Effect.gen(function* () {
				const provisional = yield* DecodeSnapshot({
					...row,
					journal_sequence: 1,
					state: state._tag,
					state_json: json(state),
					updated_at: now,
					version: row.version + 1,
				});
				const journal_sequence = yield* AppendEvent(
					transaction,
					provisional,
					causation_id,
					row.wait_id,
				);
				const snapshot = { ...provisional, journal_sequence };
				const predicate = guard
					? and(
							eq(ExternalWaits.wait_id, row.wait_id),
							eq(ExternalWaits.version, row.version),
							guard,
						)
					: and(
							eq(ExternalWaits.wait_id, row.wait_id),
							eq(ExternalWaits.version, row.version),
						);
				const changed = yield* transaction
					.update(ExternalWaits)
					.set({
						journal_sequence,
						next_observation_at,
						observer_lease_expires_at: null,
						observer_lease_owner: null,
						state: state._tag,
						state_json: json(state),
						updated_at: now,
						version: snapshot.version,
					})
					.where(predicate)
					.returning({ wait_id: ExternalWaits.wait_id });

				if (!changed[0]) {
					return yield* new ExternalWaitUnavailable({ reason: "lease_lost" });
				}

				yield* transaction
					.update(JournalEvents)
					.set({ payload_json: json({ snapshot, type: "external_wait.updated" }) })
					.where(eq(JournalEvents.sequence, journal_sequence));

				return snapshot;
			});

		const StoreOperation = (
			transaction: typeof database.client,
			input: {
				readonly kind: CommandKind;
				readonly request_fingerprint: string;
				readonly snapshot: ExternalWaitSnapshotValue;
				readonly source_command: typeof SourceCommand.Type;
				readonly thread_id: string;
				readonly wait_id: string;
			},
		) =>
			transaction.insert(ExternalWaitOperations).values({
				journal_sequence: input.snapshot.journal_sequence,
				kind: input.kind,
				operation_id: input.source_command.message_id,
				request_fingerprint: input.request_fingerprint,
				result_snapshot_json: json(input.snapshot),
				sent_at: input.source_command.sent_at,
				source_command_id: input.source_command.message_id,
				thread_id: input.thread_id,
				wait_id: input.wait_id,
			});

		const SourceCommandMatches = (
			row: typeof JournalCommands.$inferSelect,
			input: {
				readonly fingerprint: string;
				readonly kind: CommandKind;
				readonly source_command: typeof SourceCommand.Type;
				readonly thread_id: string;
				readonly wait_id: string;
			},
		) =>
			row.message_id === input.source_command.message_id &&
			row.schema_version === 1 &&
			row.thread_id === input.thread_id &&
			row.run_id === null &&
			row.agent_id === null &&
			row.causation_id === null &&
			row.origin === "frontend" &&
			row.raw_origin_json === null &&
			row.sent_at === input.source_command.sent_at &&
			row.payload_type === command_payload_type(input.kind) &&
			row.payload_json === command_payload(input.kind, input.wait_id, input.fingerprint) &&
			row.status === "accepted";

		const ReplayOperation = (
			transaction: typeof database.client,
			source_command: typeof SourceCommand.Type,
			expected: {
				readonly fingerprint: string;
				readonly kind: CommandKind;
				readonly thread_id: string;
				readonly wait_id: string;
			},
		) =>
			Effect.gen(function* () {
				const [[operation], [command]] = yield* Effect.all([
					transaction
						.select()
						.from(ExternalWaitOperations)
						.where(
							eq(ExternalWaitOperations.source_command_id, source_command.message_id),
						)
						.limit(1),
					transaction
						.select()
						.from(JournalCommands)
						.where(eq(JournalCommands.message_id, source_command.message_id))
						.limit(1),
				]);

				if (!operation) {
					return command
						? yield* new ExternalWaitConflict({ reason: "source_command_claimed" })
						: Option.none<ExternalWaitSnapshotValue>();
				}

				if (
					operation.kind !== expected.kind ||
					operation.thread_id !== expected.thread_id ||
					operation.wait_id !== expected.wait_id ||
					operation.request_fingerprint !== expected.fingerprint ||
					operation.sent_at !== source_command.sent_at
				) {
					return yield* new ExternalWaitConflict({ reason: "changed_intent" });
				}

				if (
					!command ||
					!SourceCommandMatches(command, {
						fingerprint: expected.fingerprint,
						kind: expected.kind,
						source_command,
						thread_id: expected.thread_id,
						wait_id: expected.wait_id,
					})
				) {
					return yield* invariant("Stored external wait source command is corrupt");
				}

				const snapshot = yield* DecodeOperationSnapshot(operation.result_snapshot_json);

				if (
					snapshot.wait_id !== expected.wait_id ||
					snapshot.thread_id !== expected.thread_id ||
					snapshot.journal_sequence !== operation.journal_sequence
				) {
					return yield* invariant("Stored external wait operation result is corrupt");
				}

				return Option.some(snapshot);
			});

		const ReplayRegistration = (
			transaction: typeof database.client,
			registration: ValidRegistration,
		) =>
			ReplayOperation(transaction, registration.source_command, {
				fingerprint: registration.request_fingerprint,
				kind: "request",
				thread_id: registration.thread_id,
				wait_id: registration.wait_id,
			}).pipe(
				Effect.flatMap((replay) =>
					Option.match(replay, {
						onNone: () => Effect.succeed(Option.none<ExternalWaitSnapshotValue>()),
						onSome: (snapshot) =>
							registration_matches_snapshot(registration, snapshot)
								? Effect.succeed(Option.some(snapshot))
								: Effect.fail(
										new ExternalWaitConflict({ reason: "changed_intent" }),
									),
					}),
				),
			);

		const EnsureOwnership = (transaction: typeof database.client, input: ValidRegistration) =>
			Effect.gen(function* () {
				const [[thread], [project], origins, [erasure], [tombstone]] = yield* Effect.all([
					transaction
						.select()
						.from(Threads)
						.where(eq(Threads.thread_id, input.thread_id))
						.limit(1),
					transaction
						.select()
						.from(Projects)
						.where(eq(Projects.project_id, input.project_id))
						.limit(1),
					transaction
						.select()
						.from(ProjectHostedOrigins)
						.where(eq(ProjectHostedOrigins.project_id, input.project_id)),
					transaction
						.select({ thread_id: ThreadErasureClaims.thread_id })
						.from(ThreadErasureClaims)
						.where(eq(ThreadErasureClaims.thread_id, input.thread_id))
						.limit(1),
					transaction
						.select({ thread_id: ThreadTombstones.thread_id })
						.from(ThreadTombstones)
						.where(eq(ThreadTombstones.thread_id, input.thread_id))
						.limit(1),
				]);

				if (!thread || erasure || tombstone) {
					return yield* new ExternalWaitUnavailable({ reason: "erased" });
				}

				if (!project || project.workspace_id !== input.request.workspace_id) {
					return yield* new ExternalWaitUnavailable({ reason: "project_missing" });
				}

				if (origins.length !== 1) {
					return yield* invariant("Stored project hosted origin is corrupt");
				}

				const origin = origins[0]!;

				if (
					origin.provider_id !== input.target.repository.provider_id ||
					origin.canonical_host !== input.target.repository.host ||
					origin.owner !== input.target.repository.owner ||
					origin.name !== input.target.repository.name
				) {
					return yield* new ExternalWaitUnavailable({ reason: "ownership" });
				}

				const linked_projects = yield* DecodeJson(
					Schema.Array(ProjectRef),
					thread.linked_projects_json,
					"thread project links",
				);
				const primary_project =
					thread.primary_project_id === null
						? undefined
						: thread.primary_project_json === null
							? yield* invariant("Stored primary thread project is missing")
							: yield* DecodeJson(
									ProjectRef,
									thread.primary_project_json,
									"primary thread project",
								);

				if (
					primary_project !== undefined &&
					primary_project.project_id !== thread.primary_project_id
				) {
					return yield* invariant("Stored primary thread project identity is corrupt");
				}

				const attached = [
					...(primary_project === undefined ? [] : [primary_project]),
					...linked_projects,
				].find((candidate) => candidate.project_id === project.project_id);

				if (!attached) {
					return yield* new ExternalWaitUnavailable({ reason: "ownership" });
				}

				if (
					attached.display_name !== project.display_name ||
					attached.root_path !== project.canonical_root
				) {
					return yield* invariant("Stored thread project reference is corrupt");
				}

				if (input.owner._tag === "thread_run") {
					const [run] = yield* transaction
						.select()
						.from(OrchestrationRuns)
						.where(eq(OrchestrationRuns.run_id, input.request.source_run_id))
						.limit(1);

					if (
						!run ||
						input.owner.run_id !== run.run_id ||
						run.thread_id !== input.thread_id ||
						run.agent_id !== input.owner.agent_id ||
						run.engine_id !== input.owner.engine_id ||
						run.working_directory !== project.canonical_root ||
						!["running", "waiting"].includes(run.status)
					) {
						return yield* new ExternalWaitUnavailable({ reason: "ownership" });
					}

					return {
						Transition: transaction
							.update(OrchestrationRuns)
							.set({
								status: "waiting_external",
								updated_at: input.source_command.sent_at,
							})
							.where(
								and(
									eq(OrchestrationRuns.run_id, run.run_id),
									inArray(OrchestrationRuns.status, ["running", "waiting"]),
								),
							)
							.returning({ run_id: OrchestrationRuns.run_id })
							.pipe(Effect.map((rows) => rows.length === 1)),
					};
				}

				const [[run], [assignment], [group], [agent]] = yield* Effect.all([
					transaction
						.select()
						.from(AgentRuns)
						.where(eq(AgentRuns.run_id, input.request.source_run_id))
						.limit(1),
					transaction
						.select()
						.from(Assignments)
						.where(eq(Assignments.assignment_id, input.owner.assignment_id))
						.limit(1),
					transaction
						.select()
						.from(OrchestrationGroups)
						.where(eq(OrchestrationGroups.group_id, input.owner.group_id))
						.limit(1),
					transaction
						.select()
						.from(AgentInstances)
						.where(eq(AgentInstances.agent_id, input.owner.agent_id))
						.limit(1),
				]);

				if (
					!run ||
					!assignment ||
					!group ||
					!agent ||
					input.owner.run_id !== run.run_id ||
					group.thread_id !== input.thread_id ||
					group.state !== "running" ||
					agent.group_id !== group.group_id ||
					run.group_id !== group.group_id ||
					run.assignment_id !== assignment.assignment_id ||
					run.agent_id !== input.owner.agent_id ||
					run.engine_id !== input.owner.engine_id ||
					assignment.group_id !== group.group_id ||
					assignment.agent_id !== input.owner.agent_id ||
					assignment.active_run_id !== run.run_id ||
					run.dispatch_status !== "active" ||
					!["running", "waiting"].includes(run.state) ||
					!["running", "waiting"].includes(assignment.state)
				) {
					return yield* new ExternalWaitUnavailable({ reason: "ownership" });
				}

				const workspace = yield* DecodeJson(
					AssignmentWorkspace,
					assignment.workspace_json,
					"assignment workspace",
				);

				if (
					workspace.workspace_id !== input.request.workspace_id ||
					workspace.working_directory !== project.canonical_root
				) {
					return yield* new ExternalWaitUnavailable({ reason: "ownership" });
				}

				return {
					Transition: Effect.all([
						transaction
							.update(AgentRuns)
							.set({
								dispatch_status: "waiting_external",
								state: "waiting_external",
								updated_at: input.source_command.sent_at,
							})
							.where(
								and(
									eq(AgentRuns.run_id, run.run_id),
									eq(AgentRuns.dispatch_status, "active"),
									inArray(AgentRuns.state, ["running", "waiting"]),
								),
							)
							.returning({ run_id: AgentRuns.run_id }),
						transaction
							.update(Assignments)
							.set({
								state: "waiting_external",
								updated_at: input.source_command.sent_at,
							})
							.where(
								and(
									eq(Assignments.assignment_id, assignment.assignment_id),
									eq(Assignments.active_run_id, run.run_id),
									inArray(Assignments.state, ["running", "waiting"]),
								),
							)
							.returning({ assignment_id: Assignments.assignment_id }),
					]).pipe(
						Effect.map(
							([runs, assignments]) => runs.length === 1 && assignments.length === 1,
						),
					),
				};
			});

		const Register = (
			input: ExternalWaitRegistration,
		): Effect.Effect<ExternalWaitAcceptance, ExternalWaitRepositoryError> =>
			ValidateRegistration(input).pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const replay = yield* ReplayRegistration(transaction, decoded);

								if (Option.isSome(replay)) {
									return {
										snapshot: replay.value,
										status: "duplicate" as const,
									};
								}

								const [source_claim] = yield* transaction
									.select({
										source_run_id: ExternalWaits.source_run_id,
										wait_id: ExternalWaits.wait_id,
									})
									.from(ExternalWaits)
									.where(
										or(
											eq(ExternalWaits.wait_id, decoded.wait_id),
											eq(
												ExternalWaits.source_run_id,
												decoded.request.source_run_id,
											),
										),
									)
									.limit(1);

								if (source_claim) {
									return yield* new ExternalWaitConflict({
										reason:
											source_claim.wait_id === decoded.wait_id
												? "changed_intent"
												: "source_run_claimed",
									});
								}

								const ownership = yield* EnsureOwnership(transaction, decoded);
								const prior = yield* transaction
									.select({
										generation: ExternalWaits.generation,
										maximum_generation: ExternalWaits.maximum_generation,
										settled_at: ExternalWaitWakeOutbox.updated_at,
									})
									.from(ExternalWaitWakeOutbox)
									.innerJoin(
										ExternalWaits,
										eq(ExternalWaitWakeOutbox.wait_id, ExternalWaits.wait_id),
									)
									.where(
										and(
											eq(ExternalWaitWakeOutbox.state, "settled"),
											eq(
												ExternalWaitWakeOutbox.follow_up_run_id,
												decoded.request.source_run_id,
											),
										),
									)
									.limit(2);

								if (prior.length > 1) {
									return yield* invariant(
										"Source run has multiple settled external wakes",
									);
								}

								const previous = prior[0];
								const requested_maximum = decoded.maximum_generation;

								if (
									previous &&
									requested_maximum !== undefined &&
									requested_maximum !== previous.maximum_generation
								) {
									return yield* new ExternalWaitConflict({
										reason: "changed_intent",
									});
								}

								if (
									previous &&
									decoded.source_command.sent_at < previous.settled_at
								) {
									return yield* new ExternalWaitConflict({
										reason: "changed_intent",
									});
								}

								const selected_maximum = previous
									? previous.maximum_generation
									: (requested_maximum ?? default_maximum_generation);
								const derived_generation = previous ? previous.generation + 1 : 1;
								const exhausted = derived_generation > selected_maximum;
								const generation = exhausted
									? selected_maximum
									: derived_generation;
								const next_observation_at = yield* AddDuration(
									decoded.source_command.sent_at,
									{ seconds: observation_interval_seconds },
									"External wait clock",
								);
								const timeout_at = yield* AddDuration(
									decoded.source_command.sent_at,
									{ days: timeout_days },
									"External wait clock",
								);
								const state = exhausted
									? ({ _tag: "exhausted" } as const)
									: ({ _tag: "waiting" } as const);
								const provisional = yield* Schema.decodeUnknownEffect(
									ExternalWaitSnapshot,
								)({
									baseline_fingerprint: decoded.baseline_fingerprint,
									created_at: decoded.source_command.sent_at,
									gates: decoded.request.gates,
									generation,
									journal_sequence: 1,
									maximum_generation: selected_maximum,
									owner: decoded.owner,
									project_id: decoded.project_id,
									state,
									target: decoded.target,
									thread_id: decoded.thread_id,
									updated_at: decoded.source_command.sent_at,
									version: 1,
									wait_id: decoded.wait_id,
									workspace_id: decoded.request.workspace_id,
								});
								const journal_sequence = yield* AppendEvent(
									transaction,
									provisional,
									decoded.source_command.message_id,
									decoded.wait_id,
								);
								const snapshot = { ...provisional, journal_sequence };
								const accepted_at = yield* metadata.Now;

								yield* transaction.insert(JournalCommands).values({
									accepted_at,
									message_id: decoded.source_command.message_id,
									origin: "frontend",
									payload_json: command_payload(
										"request",
										decoded.wait_id,
										decoded.request_fingerprint,
									),
									payload_type: command_payload_type("request"),
									schema_version: 1,
									sent_at: decoded.source_command.sent_at,
									status: "accepted",
									thread_id: decoded.thread_id,
								});
								yield* transaction.insert(ExternalWaits).values({
									baseline_fingerprint: decoded.baseline_fingerprint,
									baseline_json: serialize_external_wait_baseline(
										decoded.baseline,
									),
									created_at: snapshot.created_at,
									gates_json: json(snapshot.gates),
									generation,
									journal_sequence,
									maximum_generation: selected_maximum,
									next_observation_at,
									owner_json: json(snapshot.owner),
									project_id: snapshot.project_id,
									request_fingerprint: decoded.request_fingerprint,
									source_run_id: decoded.request.source_run_id,
									state: state._tag,
									state_json: json(state),
									target_json: json(snapshot.target),
									thread_id: snapshot.thread_id,
									timeout_at,
									updated_at: snapshot.updated_at,
									version: snapshot.version,
									wait_id: snapshot.wait_id,
									workspace_id: snapshot.workspace_id,
								});
								yield* StoreOperation(transaction, {
									kind: "request",
									request_fingerprint: decoded.request_fingerprint,
									snapshot,
									source_command: decoded.source_command,
									thread_id: decoded.thread_id,
									wait_id: decoded.wait_id,
								});

								if (!exhausted && !(yield* ownership.Transition)) {
									return yield* new ExternalWaitUnavailable({
										reason: "ownership",
									});
								}

								return { snapshot, status: "accepted" as const };
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
				Effect.tap((result) =>
					result.status === "accepted"
						? notifier.Publish(result.snapshot.journal_sequence)
						: Effect.void,
				),
			);

		const Replay = (input: ExternalWaitRegistration) =>
			ValidateRegistration(input).pipe(
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						ReplayRegistration(transaction, decoded).pipe(
							Effect.map((result) =>
								Option.map(result, (snapshot) => ({
									snapshot,
									status: "duplicate" as const,
								})),
							),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ReplayRequest = (input: typeof RequestReplayInput.Type) =>
			Schema.decodeUnknownEffect(RequestReplayInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						ReplayOperation(transaction, decoded.source_command, {
							fingerprint: decoded.request_fingerprint,
							kind: "request",
							thread_id: decoded.thread_id,
							wait_id: decoded.wait_id,
						}).pipe(
							Effect.map((result) =>
								Option.map(result, (snapshot) => ({
									snapshot,
									status: "duplicate" as const,
								})),
							),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const Query = (input: typeof QueryInput.Type) =>
			Schema.decodeUnknownEffect(QueryInput, { onExcessProperty: "error" })(input).pipe(
				Effect.flatMap((decoded) =>
					database.client
						.select()
						.from(ExternalWaits)
						.where(eq(ExternalWaits.thread_id, decoded.thread_id))
						.orderBy(asc(ExternalWaits.updated_at), asc(ExternalWaits.wait_id))
						.limit(65)
						.pipe(
							Effect.flatMap((rows) =>
								Effect.forEach(rows.slice(0, 64), DecodeSnapshot).pipe(
									Effect.map((snapshots) => ({
										snapshots,
										truncated: rows.length === 65,
									})),
								),
							),
						),
				),
				Effect.mapError(normalize_error),
			);

		const DiscoverDueObservations = (input: typeof DiscoverInput.Type) =>
			Schema.decodeUnknownEffect(DiscoverInput, { onExcessProperty: "error" })(input).pipe(
				Effect.flatMap((decoded) =>
					database.client
						.select({ wait_id: ExternalWaits.wait_id })
						.from(ExternalWaits)
						.where(
							and(
								eq(ExternalWaits.state, "waiting"),
								lte(ExternalWaits.next_observation_at, decoded.now),
								or(
									isNull(ExternalWaits.observer_lease_owner),
									lte(ExternalWaits.observer_lease_expires_at, decoded.now),
								),
							),
						)
						.orderBy(asc(ExternalWaits.next_observation_at), asc(ExternalWaits.wait_id))
						.limit(64),
				),
				Effect.map((rows) => rows.map((row) => row.wait_id)),
				Effect.mapError(normalize_error),
			);

		const ClaimObservation = (input: typeof ObservationClaim.Type) =>
			Schema.decodeUnknownEffect(ObservationClaim, { onExcessProperty: "error" })(input).pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const [row] = yield* transaction
									.select()
									.from(ExternalWaits)
									.where(eq(ExternalWaits.wait_id, decoded.wait_id))
									.limit(1);

								if (
									!row ||
									row.state !== "waiting" ||
									row.next_observation_at > decoded.now
								) {
									return Option.none<ExternalWaitObservationClaim>();
								}

								if (
									(row.observer_lease_owner === null) !==
									(row.observer_lease_expires_at === null)
								) {
									return yield* invariant("Stored observation lease is corrupt");
								}

								const lease_available =
									row.observer_lease_owner === null ||
									row.observer_lease_owner === decoded.lease_owner ||
									(row.observer_lease_expires_at !== null &&
										(yield* HasExpired(
											row.observer_lease_expires_at,
											decoded.now,
											"Observation lease",
										)));

								if (!lease_available) {
									return Option.none<ExternalWaitObservationClaim>();
								}

								const lease_expires_at = yield* AddDuration(
									decoded.now,
									{ seconds: observation_lease_seconds },
									"Observation lease",
								);
								const claimed = yield* transaction
									.update(ExternalWaits)
									.set({
										observer_lease_expires_at: lease_expires_at,
										observer_lease_owner: decoded.lease_owner,
									})
									.where(
										and(
											eq(ExternalWaits.wait_id, row.wait_id),
											eq(ExternalWaits.state, "waiting"),
											lte(ExternalWaits.next_observation_at, decoded.now),
											or(
												isNull(ExternalWaits.observer_lease_owner),
												lte(
													ExternalWaits.observer_lease_expires_at,
													decoded.now,
												),
												eq(
													ExternalWaits.observer_lease_owner,
													decoded.lease_owner,
												),
											),
										),
									)
									.returning({ wait_id: ExternalWaits.wait_id });

								if (!claimed[0]) {
									return Option.none<ExternalWaitObservationClaim>();
								}

								return Option.some({
									baseline: yield* DecodeBaseline(row),
									lease_expires_at,
									snapshot: yield* DecodeSnapshot(row),
									timeout_at: row.timeout_at,
								});
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ReleaseObservation = (input: typeof ObservationRecord.Type) =>
			Schema.decodeUnknownEffect(ObservationRecord, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const [row] = yield* transaction
									.select()
									.from(ExternalWaits)
									.where(eq(ExternalWaits.wait_id, decoded.wait_id))
									.limit(1);

								if (!row) {
									return Option.none<ExternalWaitSnapshotValue>();
								}

								if (
									row.state !== "waiting" ||
									row.observer_lease_owner !== decoded.lease_owner ||
									row.observer_lease_expires_at === null ||
									(yield* HasExpired(
										row.observer_lease_expires_at,
										decoded.now,
										"Observation lease",
									))
								) {
									return yield* new ExternalWaitUnavailable({
										reason: "lease_lost",
									});
								}

								const released = yield* transaction
									.update(ExternalWaits)
									.set({
										next_observation_at: decoded.next_observation_at,
										observer_lease_expires_at: null,
										observer_lease_owner: null,
									})
									.where(
										and(
											eq(ExternalWaits.wait_id, row.wait_id),
											eq(ExternalWaits.state, "waiting"),
											eq(
												ExternalWaits.observer_lease_owner,
												decoded.lease_owner,
											),
											eq(
												ExternalWaits.observer_lease_expires_at,
												row.observer_lease_expires_at,
											),
										),
									)
									.returning({ wait_id: ExternalWaits.wait_id });

								if (!released[0]) {
									return yield* new ExternalWaitUnavailable({
										reason: "lease_lost",
									});
								}

								return Option.some(yield* DecodeSnapshot(row));
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const RecordObservation = (input: typeof ObservationRecord.Type) =>
			Effect.gen(function* () {
				const decoded = yield* Schema.decodeUnknownEffect(ObservationRecord, {
					onExcessProperty: "error",
				})(input);

				if (decoded.state._tag === "waiting") {
					return yield* ReleaseObservation(decoded);
				}

				if (decoded.state._tag !== "suspended") {
					return yield* new ExternalWaitUnavailable({ reason: "ownership" });
				}

				const snapshot = yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const [row] = yield* transaction
								.select()
								.from(ExternalWaits)
								.where(eq(ExternalWaits.wait_id, decoded.wait_id))
								.limit(1);

							if (!row) {
								return Option.none<ExternalWaitSnapshotValue>();
							}

							if (
								row.state !== "waiting" ||
								row.observer_lease_owner !== decoded.lease_owner ||
								row.observer_lease_expires_at === null ||
								(yield* HasExpired(
									row.observer_lease_expires_at,
									decoded.now,
									"Observation lease",
								))
							) {
								return yield* new ExternalWaitUnavailable({ reason: "lease_lost" });
							}

							return Option.some(
								yield* PersistVisibleUpdate(
									transaction,
									row,
									decoded.state,
									decoded.now,
									decoded.next_observation_at,
									decoded.wait_id,
									and(
										eq(ExternalWaits.state, "waiting"),
										eq(ExternalWaits.observer_lease_owner, decoded.lease_owner),
										eq(
											ExternalWaits.observer_lease_expires_at,
											row.observer_lease_expires_at,
										),
									),
								),
							);
						}),
					),
				);

				yield* Option.match(snapshot, {
					onNone: () => Effect.void,
					onSome: (value) => notifier.Publish(value.journal_sequence),
				});

				return snapshot;
			}).pipe(Effect.mapError(normalize_error));

		const MarkSourceClosedWhere = (predicate: SQL, now: string) =>
			RetrySqliteWrite(
				database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [changed] = yield* transaction
							.update(ExternalWaits)
							.set({ source_closed_at: now })
							.where(and(predicate, isNull(ExternalWaits.source_closed_at)))
							.returning();

						if (changed) {
							return Option.some(yield* DecodeSnapshot(changed));
						}

						const [existing] = yield* transaction
							.select()
							.from(ExternalWaits)
							.where(predicate)
							.limit(1);

						return existing
							? Option.some(yield* DecodeSnapshot(existing))
							: Option.none<ExternalWaitSnapshotValue>();
					}),
				),
			).pipe(Effect.mapError(normalize_error));

		const MarkSourceClosed = (input: typeof SourceClosure.Type) =>
			Schema.decodeUnknownEffect(SourceClosure, { onExcessProperty: "error" })(input).pipe(
				Effect.flatMap((decoded) =>
					MarkSourceClosedWhere(eq(ExternalWaits.wait_id, decoded.wait_id), decoded.now),
				),
				Effect.mapError(normalize_error),
			);

		const MarkSourceClosedForRun = (input: typeof SourceRunClosure.Type) =>
			Schema.decodeUnknownEffect(SourceRunClosure, { onExcessProperty: "error" })(input).pipe(
				Effect.flatMap((decoded) =>
					MarkSourceClosedWhere(
						sql`
							${ExternalWaits.source_run_id} = ${decoded.source_run_id}
							AND json_extract(${ExternalWaits.owner_json}, '$._tag') = ${decoded.owner_tag}
						`,
						decoded.now,
					),
				),
				Effect.mapError(normalize_error),
			);

		const ReconcileSourceClosures = (input: typeof TerminalSourceClosureRecovery.Type) =>
			Schema.decodeUnknownEffect(TerminalSourceClosureRecovery, {
				onExcessProperty: "error",
			})(input).pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) => {
							const TerminalOrdinarySource = transaction
								.select({ run_id: OrchestrationRuns.run_id })
								.from(OrchestrationRuns)
								.where(
									and(
										eq(OrchestrationRuns.run_id, ExternalWaits.source_run_id),
										inArray(OrchestrationRuns.status, [
											"interrupted",
											"completed",
											"cancelled",
											"failed",
											"closed",
										]),
									),
								);
							const TerminalGraphSource = transaction
								.select({ run_id: AgentRuns.run_id })
								.from(AgentRuns)
								.where(
									and(
										eq(AgentRuns.run_id, ExternalWaits.source_run_id),
										inArray(AgentRuns.state, ["complete", "failed", "stopped"]),
									),
								);
							const HasTerminalSourceRun = or(
								and(
									sql`json_extract(${ExternalWaits.owner_json}, '$._tag') = 'thread_run'`,
									exists(TerminalOrdinarySource),
								),
								and(
									sql`json_extract(${ExternalWaits.owner_json}, '$._tag') = 'assignment_run'`,
									exists(TerminalGraphSource),
								),
							);

							return transaction
								.update(ExternalWaits)
								.set({ source_closed_at: decoded.now })
								.where(
									and(
										isNull(ExternalWaits.source_closed_at),
										HasTerminalSourceRun,
									),
								)
								.returning({ wait_id: ExternalWaits.wait_id })
								.pipe(Effect.map((rows) => rows.map((row) => row.wait_id).sort()));
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const DecodeWake = (row: WakeRow) =>
			Effect.gen(function* () {
				const trigger = yield* DecodeJson(
					ExternalWaitTrigger,
					row.trigger_json,
					"external wait wake trigger",
				);
				const expected_fingerprint = yield* HashText(
					json({ trigger, wait_id: row.wait_id }),
				);
				const expected = {
					follow_up_command_id: `external_wait_command_${expected_fingerprint}`,
					follow_up_run_id: `external_wait_run_${expected_fingerprint}`,
					outbox_id: `external_wait_outbox_${expected_fingerprint}`,
					trigger_fingerprint: expected_fingerprint,
				};

				if (
					row.trigger_fingerprint !== expected.trigger_fingerprint ||
					row.follow_up_command_id !== expected.follow_up_command_id ||
					row.follow_up_run_id !== expected.follow_up_run_id ||
					row.outbox_id !== expected.outbox_id
				) {
					return yield* invariant("Stored external wait wake identity is corrupt");
				}

				return { trigger, wake: expected };
			});

		const MakeWake = (
			transaction: typeof database.client,
			decoded: typeof WakeMutationInput.Type,
			options: {
				readonly allow_suspended: boolean;
				readonly allow_woken_existing: boolean;
				readonly observer_lease_owner?: string;
			},
		) =>
			Effect.gen(function* () {
				const [row] = yield* transaction
					.select()
					.from(ExternalWaits)
					.where(eq(ExternalWaits.wait_id, decoded.wait_id))
					.limit(1);

				if (!row) {
					return yield* new ExternalWaitUnavailable({ reason: "missing" });
				}

				const state = yield* DecodeState(row);
				const [existing] = yield* transaction
					.select()
					.from(ExternalWaitWakeOutbox)
					.where(eq(ExternalWaitWakeOutbox.wait_id, decoded.wait_id))
					.limit(1);

				if (existing) {
					if (
						state._tag !== "wake_pending" &&
						!(options.allow_woken_existing && state._tag === "woken")
					) {
						return yield* new ExternalWaitUnavailable({ reason: "ownership" });
					}

					const stored = yield* DecodeWake(existing);
					const requested_fingerprint = yield* HashText(
						json({ trigger: decoded.trigger, wait_id: decoded.wait_id }),
					);

					if (stored.wake.trigger_fingerprint !== requested_fingerprint) {
						return yield* new ExternalWaitUnavailable({ reason: "ownership" });
					}

					return {
						published_snapshot: Option.none<ExternalWaitSnapshotValue>(),
						result_snapshot: yield* DecodeSnapshot(row),
						wake: stored.wake,
					};
				}

				if (
					options.observer_lease_owner !== undefined &&
					(row.observer_lease_owner !== options.observer_lease_owner ||
						row.observer_lease_expires_at === null ||
						row.observer_lease_expires_at <= decoded.now)
				) {
					return yield* new ExternalWaitUnavailable({ reason: "lease_lost" });
				}

				if (
					state._tag !== "waiting" &&
					!(options.allow_suspended && state._tag === "suspended")
				) {
					return yield* new ExternalWaitUnavailable({ reason: "ownership" });
				}

				const trigger_fingerprint = yield* HashText(
					json({ trigger: decoded.trigger, wait_id: decoded.wait_id }),
				);
				const wake = {
					follow_up_command_id: `external_wait_command_${trigger_fingerprint}`,
					follow_up_run_id: `external_wait_run_${trigger_fingerprint}`,
					outbox_id: `external_wait_outbox_${trigger_fingerprint}`,
					trigger_fingerprint,
				};
				const source_state =
					options.observer_lease_owner === undefined
						? eq(ExternalWaits.state, row.state)
						: and(
								eq(ExternalWaits.state, row.state),
								eq(
									ExternalWaits.observer_lease_owner,
									options.observer_lease_owner,
								),
								eq(
									ExternalWaits.observer_lease_expires_at,
									row.observer_lease_expires_at!,
								),
								gt(ExternalWaits.observer_lease_expires_at, decoded.now),
							);
				const snapshot = yield* PersistVisibleUpdate(
					transaction,
					row,
					{ _tag: "wake_pending", trigger: decoded.trigger },
					decoded.now,
					row.next_observation_at,
					decoded.wait_id,
					source_state,
				);

				yield* transaction.insert(ExternalWaitWakeOutbox).values({
					created_at: decoded.now,
					follow_up_command_id: wake.follow_up_command_id,
					follow_up_run_id: wake.follow_up_run_id,
					outbox_id: wake.outbox_id,
					state: "pending",
					trigger_fingerprint,
					trigger_json: json(decoded.trigger),
					updated_at: decoded.now,
					wait_id: decoded.wait_id,
				});

				return {
					published_snapshot: Option.some(snapshot),
					result_snapshot: snapshot,
					wake,
				};
			});

		const CreateWake = (input: typeof WakeInput.Type) =>
			Schema.decodeUnknownEffect(WakeInput, { onExcessProperty: "error" })(input).pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							MakeWake(transaction, decoded, {
								allow_suspended: false,
								allow_woken_existing: true,
								observer_lease_owner: decoded.lease_owner,
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
				Effect.tap((result) =>
					Option.match(result.published_snapshot, {
						onNone: () => Effect.void,
						onSome: (snapshot) => notifier.Publish(snapshot.journal_sequence),
					}),
				),
				Effect.map((result) => result.wake),
			);

		const DiscoverWakes = (input: typeof DiscoverInput.Type) =>
			Schema.decodeUnknownEffect(DiscoverInput, { onExcessProperty: "error" })(input).pipe(
				Effect.flatMap((decoded) =>
					database.client
						.select({
							outbox_id: ExternalWaitWakeOutbox.outbox_id,
							thread_id: ExternalWaits.thread_id,
						})
						.from(ExternalWaitWakeOutbox)
						.innerJoin(
							ExternalWaits,
							eq(ExternalWaitWakeOutbox.wait_id, ExternalWaits.wait_id),
						)
						.where(
							and(
								eq(ExternalWaits.state, "wake_pending"),
								isNotNull(ExternalWaits.source_closed_at),
								lte(ExternalWaits.source_closed_at, decoded.now),
								or(
									eq(ExternalWaitWakeOutbox.state, "pending"),
									and(
										eq(ExternalWaitWakeOutbox.state, "claimed"),
										lte(ExternalWaitWakeOutbox.lease_expires_at, decoded.now),
									),
								),
							),
						)
						.orderBy(
							asc(ExternalWaitWakeOutbox.updated_at),
							asc(ExternalWaitWakeOutbox.outbox_id),
						)
						.limit(64),
				),
				Effect.mapError(normalize_error),
			);

		const ClaimWake = (input: typeof WakeClaim.Type) =>
			Schema.decodeUnknownEffect(WakeClaim, { onExcessProperty: "error" })(input).pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const [outbox] = yield* transaction
									.select()
									.from(ExternalWaitWakeOutbox)
									.where(eq(ExternalWaitWakeOutbox.outbox_id, decoded.outbox_id))
									.limit(1);

								if (
									!outbox ||
									outbox.state === "settled" ||
									outbox.state === "cancelled"
								) {
									return Option.none<ExternalWaitWakeClaim>();
								}

								const [wait] = yield* transaction
									.select()
									.from(ExternalWaits)
									.where(eq(ExternalWaits.wait_id, outbox.wait_id))
									.limit(1);

								if (
									!wait ||
									wait.source_closed_at === null ||
									wait.source_closed_at > decoded.now ||
									wait.state !== "wake_pending"
								) {
									return Option.none<ExternalWaitWakeClaim>();
								}

								const state = yield* DecodeState(wait);

								if (state._tag !== "wake_pending") {
									return yield* invariant("Stored wake-pending state is corrupt");
								}

								if (
									(outbox.lease_owner === null) !==
									(outbox.lease_expires_at === null)
								) {
									return yield* invariant("Stored wake lease is corrupt");
								}

								const lease_available =
									outbox.state === "pending" ||
									outbox.lease_owner === decoded.lease_owner ||
									(outbox.lease_expires_at !== null &&
										(yield* HasExpired(
											outbox.lease_expires_at,
											decoded.now,
											"Wake lease",
										)));

								if (!lease_available) {
									return Option.none<ExternalWaitWakeClaim>();
								}

								const lease_expires_at = yield* AddDuration(
									decoded.now,
									{ seconds: observation_lease_seconds },
									"Wake lease",
								);
								const claimed = yield* transaction
									.update(ExternalWaitWakeOutbox)
									.set({
										lease_expires_at,
										lease_owner: decoded.lease_owner,
										state: "claimed",
										updated_at: decoded.now,
									})
									.where(
										and(
											eq(ExternalWaitWakeOutbox.outbox_id, outbox.outbox_id),
											or(
												eq(ExternalWaitWakeOutbox.state, "pending"),
												and(
													eq(ExternalWaitWakeOutbox.state, "claimed"),
													or(
														eq(
															ExternalWaitWakeOutbox.lease_owner,
															decoded.lease_owner,
														),
														lte(
															ExternalWaitWakeOutbox.lease_expires_at,
															decoded.now,
														),
													),
												),
											),
										),
									)
									.returning({ outbox_id: ExternalWaitWakeOutbox.outbox_id });

								if (!claimed[0]) {
									return Option.none<ExternalWaitWakeClaim>();
								}

								const stored = yield* DecodeWake(outbox);

								return Option.some({
									...stored.wake,
									lease_expires_at,
									owner: yield* DecodeJson(
										ExternalWaitOwner,
										wait.owner_json,
										"external wait owner",
									),
									snapshot: yield* DecodeSnapshot(wait),
									trigger: stored.trigger,
								});
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ReleaseWake = (input: typeof WakeClaim.Type) =>
			Schema.decodeUnknownEffect(WakeClaim, { onExcessProperty: "error" })(input).pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const [outbox] = yield* transaction
									.select()
									.from(ExternalWaitWakeOutbox)
									.where(eq(ExternalWaitWakeOutbox.outbox_id, decoded.outbox_id))
									.limit(1);

								if (!outbox) {
									return Option.none<ExternalWaitWake>();
								}

								if (
									outbox.state !== "claimed" ||
									outbox.lease_owner !== decoded.lease_owner ||
									outbox.lease_expires_at === null ||
									(yield* HasExpired(
										outbox.lease_expires_at,
										decoded.now,
										"Wake lease",
									))
								) {
									return yield* new ExternalWaitUnavailable({
										reason: "lease_lost",
									});
								}

								const released = yield* transaction
									.update(ExternalWaitWakeOutbox)
									.set({
										lease_expires_at: null,
										lease_owner: null,
										state: "pending",
										updated_at: decoded.now,
									})
									.where(
										and(
											eq(ExternalWaitWakeOutbox.outbox_id, outbox.outbox_id),
											eq(ExternalWaitWakeOutbox.state, "claimed"),
											eq(
												ExternalWaitWakeOutbox.lease_owner,
												decoded.lease_owner,
											),
											eq(
												ExternalWaitWakeOutbox.lease_expires_at,
												outbox.lease_expires_at,
											),
										),
									)
									.returning();

								if (!released[0]) {
									return yield* new ExternalWaitUnavailable({
										reason: "lease_lost",
									});
								}

								return Option.some((yield* DecodeWake(released[0])).wake);
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ContinuationText = (trigger: typeof ExternalWaitTrigger.Type) =>
			trigger._tag === "checks_terminal"
				? "External checks reached a terminal state. Continue the task."
				: trigger._tag === "review_changed"
					? "External review state changed. Continue the task."
					: "Continue the task.";

		const DecodeResumeToken = (native_thread_id: string | null, value: string | null) => {
			if (value === null) {
				return native_thread_id === null
					? Effect.succeed(Option.none<typeof EngineResumeToken.Type>())
					: Effect.fail(
							invariant("Stored native thread is missing its engine resume token"),
						);
			}

			return DecodeJson(EngineResumeToken, value, "engine resume token").pipe(
				Effect.flatMap((token) =>
					native_thread_id === token.native_thread_id
						? Effect.succeed(Option.some(token))
						: Effect.fail(
								invariant(
									"Stored engine resume token does not match its native thread",
								),
							),
				),
			);
		};

		const resume_tokens_match = (
			left: Option.Option<typeof EngineResumeToken.Type>,
			right: Option.Option<typeof EngineResumeToken.Type>,
		) =>
			Option.isNone(left)
				? Option.isNone(right)
				: Option.isSome(right) &&
					left.value.native_thread_id === right.value.native_thread_id &&
					left.value.opaque_checkpoint === right.value.opaque_checkpoint;

		const MaterializeWake = (input: typeof WakeMaterialization.Type) =>
			Schema.decodeUnknownEffect(WakeMaterialization, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const [outbox] = yield* transaction
									.select()
									.from(ExternalWaitWakeOutbox)
									.where(eq(ExternalWaitWakeOutbox.outbox_id, decoded.outbox_id))
									.limit(1);

								if (!outbox) {
									return yield* new ExternalWaitUnavailable({
										reason: "missing",
									});
								}

								const [wait] = yield* transaction
									.select()
									.from(ExternalWaits)
									.where(eq(ExternalWaits.wait_id, outbox.wait_id))
									.limit(1);

								if (!wait) {
									return yield* new ExternalWaitUnavailable({
										reason: "missing",
									});
								}

								const stored = yield* DecodeWake(outbox);
								const owner = yield* DecodeJson(
									ExternalWaitOwner,
									wait.owner_json,
									"external wait owner",
								);

								if (outbox.state === "settled") {
									const snapshot = yield* DecodeSnapshot(wait);

									if (
										snapshot.state._tag !== "woken" ||
										snapshot.state.follow_up_run_id !==
											outbox.follow_up_run_id ||
										snapshot.state.mode !== outbox.mode ||
										json(snapshot.state.trigger) !== json(stored.trigger) ||
										outbox.mode === null
									) {
										return yield* invariant(
											"Stored settled wake projection is corrupt",
										);
									}

									const continuation_matches =
										owner._tag === "thread_run"
											? yield* Effect.gen(function* () {
													const [
														[continuation],
														[source],
														[command],
														[message],
														[run_outbox],
													] = yield* Effect.all([
														transaction
															.select()
															.from(OrchestrationRuns)
															.where(
																eq(
																	OrchestrationRuns.run_id,
																	outbox.follow_up_run_id,
																),
															)
															.limit(1),
														transaction
															.select()
															.from(OrchestrationRuns)
															.where(
																eq(
																	OrchestrationRuns.run_id,
																	owner.run_id,
																),
															)
															.limit(1),
														transaction
															.select()
															.from(JournalCommands)
															.where(
																eq(
																	JournalCommands.message_id,
																	outbox.follow_up_command_id,
																),
															)
															.limit(1),
														transaction
															.select()
															.from(OrchestrationMessages)
															.where(
																eq(
																	OrchestrationMessages.command_id,
																	outbox.follow_up_command_id,
																),
															)
															.limit(1),
														transaction
															.select()
															.from(OrchestrationOutbox)
															.where(
																eq(
																	OrchestrationOutbox.command_id,
																	outbox.follow_up_command_id,
																),
															)
															.limit(1),
													]);

													if (
														!continuation ||
														!source ||
														!command ||
														!message ||
														!run_outbox
													) {
														return false;
													}

													const source_token = yield* DecodeResumeToken(
														source.native_thread_id,
														source.native_resume_json,
													);
													const continuation_token =
														yield* DecodeResumeToken(
															continuation.native_thread_id,
															continuation.native_resume_json,
														);
													const expected_payload = json({
														type: "thread.send_message",
														engine_id: source.engine_id,
														text: ContinuationText(stored.trigger),
														working_directory: source.working_directory,
													});
													const token_matches =
														outbox.mode === "native_resume"
															? Option.isSome(source_token) &&
																resume_tokens_match(
																	source_token,
																	continuation_token,
																)
															: Option.isNone(continuation_token);

													return (
														source.status === "closed" &&
														source.thread_id === wait.thread_id &&
														source.agent_id === owner.agent_id &&
														source.engine_id === owner.engine_id &&
														continuation.thread_id === wait.thread_id &&
														continuation.agent_id === owner.agent_id &&
														continuation.engine_id ===
															owner.engine_id &&
														continuation.working_directory ===
															source.working_directory &&
														continuation.open_mode ===
															(outbox.mode === "native_resume"
																? "resume"
																: "start") &&
														token_matches &&
														command.agent_id === owner.agent_id &&
														command.assigned_run_id ===
															continuation.run_id &&
														command.causation_id === outbox.outbox_id &&
														command.origin === "backend" &&
														command.payload_json === expected_payload &&
														command.payload_type ===
															"thread.send_message" &&
														command.run_id === source.run_id &&
														command.schema_version === 1 &&
														command.thread_id === wait.thread_id &&
														message.agent_id === owner.agent_id &&
														message.message_id ===
															outbox.follow_up_command_id &&
														message.run_id === continuation.run_id &&
														message.text ===
															ContinuationText(stored.trigger) &&
														message.thread_id === wait.thread_id &&
														run_outbox.agent_id === owner.agent_id &&
														run_outbox.kind ===
															(outbox.mode === "native_resume"
																? "resume"
																: "start") &&
														run_outbox.payload_json ===
															expected_payload &&
														run_outbox.run_id === continuation.run_id &&
														run_outbox.thread_id === wait.thread_id
													);
												})
											: yield* Effect.gen(function* () {
													const [[continuation], [source]] =
														yield* Effect.all([
															transaction
																.select()
																.from(AgentRuns)
																.where(
																	eq(
																		AgentRuns.run_id,
																		outbox.follow_up_run_id,
																	),
																)
																.limit(1),
															transaction
																.select()
																.from(AgentRuns)
																.where(
																	eq(
																		AgentRuns.run_id,
																		owner.run_id,
																	),
																)
																.limit(1),
														]);

													if (!continuation || !source) {
														return false;
													}

													const source_token = yield* DecodeResumeToken(
														source.native_thread_id,
														source.native_resume_json,
													);
													const continuation_token =
														yield* DecodeResumeToken(
															continuation.native_thread_id,
															continuation.native_resume_json,
														);
													const token_matches =
														outbox.mode === "native_resume"
															? Option.isSome(source_token) &&
																resume_tokens_match(
																	source_token,
																	continuation_token,
																)
															: Option.isNone(continuation_token);

													return (
														source.state === "stopped" &&
														source.dispatch_status === "terminal" &&
														source.assignment_id ===
															owner.assignment_id &&
														source.group_id === owner.group_id &&
														source.agent_id === owner.agent_id &&
														source.engine_id === owner.engine_id &&
														continuation.assignment_id ===
															owner.assignment_id &&
														continuation.group_id === owner.group_id &&
														continuation.agent_id === owner.agent_id &&
														continuation.engine_id ===
															owner.engine_id &&
														continuation.attempt === source.attempt &&
														continuation.continuation_index ===
															source.continuation_index + 1 &&
														continuation.continuation_text ===
															ContinuationText(stored.trigger) &&
														continuation.open_mode ===
															(outbox.mode === "native_resume"
																? "resume"
																: "start") &&
														continuation.profile === source.profile &&
														token_matches
													);
												});

									if (!continuation_matches) {
										return yield* invariant(
											"Stored settled wake continuation is missing",
										);
									}

									return {
										follow_up_run_id: outbox.follow_up_run_id,
										mode: outbox.mode,
										owner,
										snapshot,
										status: "duplicate" as const,
									};
								}

								if (
									outbox.state !== "claimed" ||
									outbox.lease_owner !== decoded.lease_owner ||
									outbox.lease_expires_at === null ||
									(yield* HasExpired(
										outbox.lease_expires_at,
										decoded.now,
										"Wake lease",
									))
								) {
									return yield* new ExternalWaitUnavailable({
										reason: "lease_lost",
									});
								}

								const wait_state = yield* DecodeState(wait);

								if (wait_state._tag !== "wake_pending") {
									return yield* new ExternalWaitUnavailable({
										reason: "lease_lost",
									});
								}

								const [[erasure], [tombstone]] = yield* Effect.all([
									transaction
										.select({ thread_id: ThreadErasureClaims.thread_id })
										.from(ThreadErasureClaims)
										.where(eq(ThreadErasureClaims.thread_id, wait.thread_id))
										.limit(1),
									transaction
										.select({ thread_id: ThreadTombstones.thread_id })
										.from(ThreadTombstones)
										.where(eq(ThreadTombstones.thread_id, wait.thread_id))
										.limit(1),
								]);

								if (erasure || tombstone) {
									return yield* new ExternalWaitUnavailable({ reason: "erased" });
								}

								const text = ContinuationText(stored.trigger);
								const materialization =
									owner._tag === "thread_run"
										? yield* Effect.gen(function* () {
												const [[source], [coordinator]] = yield* Effect.all(
													[
														transaction
															.select()
															.from(OrchestrationRuns)
															.where(
																eq(
																	OrchestrationRuns.run_id,
																	owner.run_id,
																),
															)
															.limit(1),
														transaction
															.select()
															.from(OrchestrationCoordinators)
															.where(
																eq(
																	OrchestrationCoordinators.thread_id,
																	wait.thread_id,
																),
															)
															.limit(1),
													],
												);

												if (
													!source ||
													!coordinator ||
													source.status !== "waiting_external" ||
													source.thread_id !== wait.thread_id ||
													source.agent_id !== owner.agent_id ||
													source.engine_id !== owner.engine_id ||
													coordinator.active_run_id !== source.run_id ||
													coordinator.agent_id !== owner.agent_id ||
													coordinator.engine_id !== owner.engine_id
												) {
													return yield* new ExternalWaitUnavailable({
														reason: "ownership",
													});
												}

												const source_token = yield* DecodeResumeToken(
													source.native_thread_id,
													source.native_resume_json,
												);
												const coordinator_token = yield* DecodeResumeToken(
													coordinator.native_thread_id,
													coordinator.native_resume_json,
												);

												if (
													!resume_tokens_match(
														source_token,
														coordinator_token,
													)
												) {
													return yield* invariant(
														"Stored coordinator resume token does not match its source run",
													);
												}
												const mode =
													decoded.native_resume_supported &&
													Option.isSome(source_token)
														? ("native_resume" as const)
														: ("linked_run" as const);
												const payload = yield* Schema.decodeUnknownEffect(
													CommandPayload,
													{ onExcessProperty: "error" },
												)({
													engine_id: source.engine_id,
													text,
													type: "thread.send_message",
													working_directory: source.working_directory,
												}).pipe(
													Effect.mapError(() =>
														invariant(
															"External wake continuation payload is invalid",
														),
													),
												);

												yield* transaction
													.insert(OrchestrationRuns)
													.values({
														agent_id: source.agent_id,
														created_at: decoded.now,
														engine_id: source.engine_id,
														native_resume_json:
															mode === "native_resume"
																? source.native_resume_json
																: null,
														native_thread_id:
															mode === "native_resume"
																? source.native_thread_id
																: null,
														open_mode:
															mode === "native_resume"
																? "resume"
																: "start",
														run_id: outbox.follow_up_run_id,
														status: "queued",
														thread_id: source.thread_id,
														updated_at: decoded.now,
														working_directory: source.working_directory,
													});
												yield* transaction.insert(JournalCommands).values({
													accepted_at: decoded.now,
													agent_id: source.agent_id,
													assigned_run_id: outbox.follow_up_run_id,
													causation_id: outbox.outbox_id,
													message_id: outbox.follow_up_command_id,
													origin: "backend",
													payload_json: json(payload),
													payload_type: payload.type,
													raw_origin_json: null,
													run_id: source.run_id,
													schema_version: 1,
													sent_at: decoded.now,
													status: "accepted",
													thread_id: source.thread_id,
												});
												yield* transaction
													.insert(OrchestrationMessages)
													.values({
														agent_id: source.agent_id,
														command_id: outbox.follow_up_command_id,
														created_at: decoded.now,
														delivery: "queued",
														message_id: outbox.follow_up_command_id,
														run_id: outbox.follow_up_run_id,
														text,
														thread_id: source.thread_id,
													});
												yield* transaction
													.insert(OrchestrationOutbox)
													.values({
														agent_id: source.agent_id,
														command_id: outbox.follow_up_command_id,
														created_at: decoded.now,
														kind:
															mode === "native_resume"
																? "resume"
																: "start",
														payload_json: json(payload),
														run_id: outbox.follow_up_run_id,
														status: "pending",
														thread_id: source.thread_id,
														updated_at: decoded.now,
													});
												yield* transaction
													.update(OrchestrationCoordinators)
													.set({
														active_run_id: outbox.follow_up_run_id,
														native_resume_json:
															mode === "native_resume"
																? source.native_resume_json
																: null,
														native_thread_id:
															mode === "native_resume"
																? source.native_thread_id
																: null,
														updated_at: decoded.now,
													})
													.where(
														and(
															eq(
																OrchestrationCoordinators.thread_id,
																source.thread_id,
															),
															eq(
																OrchestrationCoordinators.active_run_id,
																source.run_id,
															),
														),
													);
												yield* transaction
													.update(OrchestrationRuns)
													.set({
														status: "closed",
														updated_at: decoded.now,
													})
													.where(
														and(
															eq(
																OrchestrationRuns.run_id,
																source.run_id,
															),
															eq(
																OrchestrationRuns.status,
																"waiting_external",
															),
														),
													);

												return mode;
											})
										: yield* Effect.gen(function* () {
												const [[source], [assignment], [group], [agent]] =
													yield* Effect.all([
														transaction
															.select()
															.from(AgentRuns)
															.where(
																eq(AgentRuns.run_id, owner.run_id),
															)
															.limit(1),
														transaction
															.select()
															.from(Assignments)
															.where(
																eq(
																	Assignments.assignment_id,
																	owner.assignment_id,
																),
															)
															.limit(1),
														transaction
															.select()
															.from(OrchestrationGroups)
															.where(
																eq(
																	OrchestrationGroups.group_id,
																	owner.group_id,
																),
															)
															.limit(1),
														transaction
															.select()
															.from(AgentInstances)
															.where(
																eq(
																	AgentInstances.agent_id,
																	owner.agent_id,
																),
															)
															.limit(1),
													]);

												if (
													!source ||
													!assignment ||
													!group ||
													!agent ||
													source.state !== "waiting_external" ||
													source.dispatch_status !== "waiting_external" ||
													source.assignment_id !==
														assignment.assignment_id ||
													source.group_id !== group.group_id ||
													source.agent_id !== owner.agent_id ||
													source.engine_id !== owner.engine_id ||
													assignment.active_run_id !== source.run_id ||
													assignment.state !== "waiting_external" ||
													group.state !== "running" ||
													group.thread_id !== wait.thread_id ||
													agent.group_id !== group.group_id
												) {
													return yield* new ExternalWaitUnavailable({
														reason: "ownership",
													});
												}

												const source_token = yield* DecodeResumeToken(
													source.native_thread_id,
													source.native_resume_json,
												);
												const mode =
													decoded.native_resume_supported &&
													Option.isSome(source_token)
														? ("native_resume" as const)
														: ("linked_run" as const);

												yield* transaction.insert(AgentRuns).values({
													agent_id: source.agent_id,
													assignment_id: source.assignment_id,
													attempt: source.attempt,
													completed_at: null,
													continuation_index:
														source.continuation_index + 1,
													continuation_text: text,
													created_at: decoded.now,
													dispatch_status: "queued",
													engine_id: source.engine_id,
													group_id: source.group_id,
													last_observation_sequence: 0,
													native_identity_json: null,
													native_resume_json:
														mode === "native_resume"
															? source.native_resume_json
															: null,
													native_thread_id:
														mode === "native_resume"
															? source.native_thread_id
															: null,
													open_mode:
														mode === "native_resume"
															? "resume"
															: "start",
													owner_instance_id: null,
													profile: source.profile,
													raw_origin_json: null,
													run_id: outbox.follow_up_run_id,
													state: "queued",
													updated_at: decoded.now,
												});
												yield* transaction
													.update(Assignments)
													.set({
														active_run_id: outbox.follow_up_run_id,
														state: "queued",
														updated_at: decoded.now,
													})
													.where(
														and(
															eq(
																Assignments.assignment_id,
																assignment.assignment_id,
															),
															eq(
																Assignments.active_run_id,
																source.run_id,
															),
															eq(
																Assignments.state,
																"waiting_external",
															),
														),
													);
												yield* transaction
													.update(AgentRuns)
													.set({
														completed_at: decoded.now,
														dispatch_status: "terminal",
														state: "stopped",
														updated_at: decoded.now,
													})
													.where(
														and(
															eq(AgentRuns.run_id, source.run_id),
															eq(
																AgentRuns.dispatch_status,
																"waiting_external",
															),
															eq(AgentRuns.state, "waiting_external"),
														),
													);

												return mode;
											});

								const changed = yield* transaction
									.update(ExternalWaitWakeOutbox)
									.set({
										lease_expires_at: null,
										lease_owner: null,
										mode: materialization,
										state: "settled",
										updated_at: decoded.now,
									})
									.where(
										and(
											eq(ExternalWaitWakeOutbox.outbox_id, outbox.outbox_id),
											eq(ExternalWaitWakeOutbox.state, "claimed"),
											eq(
												ExternalWaitWakeOutbox.lease_owner,
												decoded.lease_owner,
											),
											eq(
												ExternalWaitWakeOutbox.lease_expires_at,
												outbox.lease_expires_at,
											),
										),
									)
									.returning({ outbox_id: ExternalWaitWakeOutbox.outbox_id });

								if (!changed[0]) {
									return yield* new ExternalWaitUnavailable({
										reason: "lease_lost",
									});
								}

								const snapshot = yield* PersistVisibleUpdate(
									transaction,
									wait,
									{
										_tag: "woken",
										follow_up_run_id: outbox.follow_up_run_id,
										mode: materialization,
										trigger: stored.trigger,
									},
									decoded.now,
									wait.next_observation_at,
									outbox.outbox_id,
									eq(ExternalWaits.state, "wake_pending"),
								);

								return {
									follow_up_run_id: outbox.follow_up_run_id,
									mode: materialization,
									owner,
									snapshot,
									status: "created" as const,
								};
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
				Effect.tap((result) =>
					result.status === "created"
						? notifier.Publish(result.snapshot.journal_sequence)
						: Effect.void,
				),
			);

		const CommandFingerprint = (kind: CommandKind, wait_id: string, detail: unknown) =>
			HashText(json({ detail, kind, wait_id }));

		const StoreCommand = (
			transaction: typeof database.client,
			input: {
				readonly fingerprint: string;
				readonly kind: CommandKind;
				readonly source_command: typeof SourceCommand.Type;
				readonly thread_id: string;
				readonly wait_id: string;
			},
		) =>
			Effect.gen(function* () {
				const accepted_at = yield* metadata.Now;

				yield* transaction.insert(JournalCommands).values({
					accepted_at,
					message_id: input.source_command.message_id,
					origin: "frontend",
					payload_json: command_payload(input.kind, input.wait_id, input.fingerprint),
					payload_type: command_payload_type(input.kind),
					schema_version: 1,
					sent_at: input.source_command.sent_at,
					status: "accepted",
					thread_id: input.thread_id,
				});
			});

		const Cancel = (input: typeof CancelInput.Type) =>
			Schema.decodeUnknownEffect(CancelInput, { onExcessProperty: "error" })(input).pipe(
				Effect.flatMap((decoded) =>
					CommandFingerprint("cancel", decoded.wait_id, decoded.reason).pipe(
						Effect.flatMap((request_fingerprint) =>
							RetrySqliteWrite(
								database.client.transaction((transaction) =>
									Effect.gen(function* () {
										const replay = yield* ReplayOperation(
											transaction,
											decoded.source_command,
											{
												fingerprint: request_fingerprint,
												kind: "cancel",
												thread_id: decoded.thread_id,
												wait_id: decoded.wait_id,
											},
										);

										if (Option.isSome(replay)) {
											return {
												acceptance: Option.some({
													snapshot: replay.value,
													status: "duplicate" as const,
												}),
												published: false,
											};
										}

										const [row] = yield* transaction
											.select()
											.from(ExternalWaits)
											.where(
												and(
													eq(ExternalWaits.wait_id, decoded.wait_id),
													eq(ExternalWaits.thread_id, decoded.thread_id),
												),
											)
											.limit(1);

										if (!row) {
											return {
												acceptance: Option.none<ExternalWaitAcceptance>(),
												published: false,
											};
										}

										const state = yield* DecodeState(row);

										if (state._tag === "woken" || state._tag === "exhausted") {
											return yield* new ExternalWaitUnavailable({
												reason: "ownership",
											});
										}

										if (
											state._tag === "cancelled" &&
											state.reason !== decoded.reason
										) {
											return yield* new ExternalWaitConflict({
												reason: "changed_intent",
											});
										}

										let snapshot: ExternalWaitSnapshotValue;
										let published = false;

										if (state._tag === "cancelled") {
											snapshot = yield* DecodeSnapshot(row);
										} else {
											const [outbox] = yield* transaction
												.select()
												.from(ExternalWaitWakeOutbox)
												.where(
													eq(
														ExternalWaitWakeOutbox.wait_id,
														decoded.wait_id,
													),
												)
												.limit(1);

											if (outbox) {
												const cancelled = yield* transaction
													.update(ExternalWaitWakeOutbox)
													.set({
														lease_expires_at: null,
														lease_owner: null,
														state: "cancelled",
														updated_at: decoded.now,
													})
													.where(
														and(
															eq(
																ExternalWaitWakeOutbox.outbox_id,
																outbox.outbox_id,
															),
															inArray(ExternalWaitWakeOutbox.state, [
																"pending",
																"claimed",
															]),
														),
													)
													.returning({
														outbox_id: ExternalWaitWakeOutbox.outbox_id,
													});

												if (!cancelled[0]) {
													return yield* new ExternalWaitUnavailable({
														reason: "lease_lost",
													});
												}
											}

											snapshot = yield* PersistVisibleUpdate(
												transaction,
												row,
												{ _tag: "cancelled", reason: decoded.reason },
												decoded.now,
												row.next_observation_at,
												decoded.source_command.message_id,
												inArray(ExternalWaits.state, [
													"waiting",
													"suspended",
													"wake_pending",
												]),
											);
											published = true;
										}

										yield* StoreCommand(transaction, {
											fingerprint: request_fingerprint,
											kind: "cancel",
											source_command: decoded.source_command,
											thread_id: decoded.thread_id,
											wait_id: decoded.wait_id,
										});
										yield* StoreOperation(transaction, {
											kind: "cancel",
											request_fingerprint,
											snapshot,
											source_command: decoded.source_command,
											thread_id: decoded.thread_id,
											wait_id: decoded.wait_id,
										});

										return {
											acceptance: Option.some({
												snapshot,
												status: "accepted" as const,
											}),
											published,
										};
									}),
								),
							),
						),
					),
				),
				Effect.mapError(normalize_error),
				Effect.tap((result) =>
					result.published && Option.isSome(result.acceptance)
						? notifier.Publish(result.acceptance.value.snapshot.journal_sequence)
						: Effect.void,
				),
				Effect.map((result) => result.acceptance),
			);

		const ManualResume = (input: typeof ManualResumeInput.Type) =>
			Schema.decodeUnknownEffect(ManualResumeInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.flatMap((decoded) =>
					CommandFingerprint("manual_resume", decoded.wait_id, null).pipe(
						Effect.flatMap((request_fingerprint) =>
							RetrySqliteWrite(
								database.client.transaction((transaction) =>
									Effect.gen(function* () {
										const replay = yield* ReplayOperation(
											transaction,
											decoded.source_command,
											{
												fingerprint: request_fingerprint,
												kind: "manual_resume",
												thread_id: decoded.thread_id,
												wait_id: decoded.wait_id,
											},
										);

										if (Option.isSome(replay)) {
											const [outbox] = yield* transaction
												.select()
												.from(ExternalWaitWakeOutbox)
												.where(
													eq(
														ExternalWaitWakeOutbox.wait_id,
														decoded.wait_id,
													),
												)
												.limit(1);

											if (!outbox) {
												return yield* invariant(
													"Manual resume operation has no wake outbox",
												);
											}

											return {
												acceptance: {
													snapshot: replay.value,
													status: "duplicate" as const,
													wake: (yield* DecodeWake(outbox)).wake,
												},
												published_snapshot:
													Option.none<ExternalWaitSnapshotValue>(),
											};
										}

										const [row] = yield* transaction
											.select()
											.from(ExternalWaits)
											.where(
												and(
													eq(ExternalWaits.wait_id, decoded.wait_id),
													eq(ExternalWaits.thread_id, decoded.thread_id),
												),
											)
											.limit(1);

										if (!row) {
											return yield* new ExternalWaitUnavailable({
												reason: "missing",
											});
										}

										const mutation = yield* MakeWake(
											transaction,
											{
												now: decoded.now,
												trigger: { _tag: "manual_resume" },
												wait_id: decoded.wait_id,
											},
											{
												allow_suspended: true,
												allow_woken_existing: false,
											},
										);

										yield* StoreCommand(transaction, {
											fingerprint: request_fingerprint,
											kind: "manual_resume",
											source_command: decoded.source_command,
											thread_id: decoded.thread_id,
											wait_id: decoded.wait_id,
										});
										yield* StoreOperation(transaction, {
											kind: "manual_resume",
											request_fingerprint,
											snapshot: mutation.result_snapshot,
											source_command: decoded.source_command,
											thread_id: decoded.thread_id,
											wait_id: decoded.wait_id,
										});

										return {
											acceptance: {
												snapshot: mutation.result_snapshot,
												status: "accepted" as const,
												wake: mutation.wake,
											},
											published_snapshot: mutation.published_snapshot,
										};
									}),
								),
							),
						),
					),
				),
				Effect.mapError(normalize_error),
				Effect.tap((result) =>
					Option.match(result.published_snapshot, {
						onNone: () => Effect.void,
						onSome: (snapshot) => notifier.Publish(snapshot.journal_sequence),
					}),
				),
				Effect.map((result) => result.acceptance),
			);

		return {
			Cancel,
			ClaimObservation,
			ClaimWake,
			CreateWake,
			DiscoverDueObservations,
			DiscoverWakes,
			ManualResume,
			MarkSourceClosed,
			MarkSourceClosedForRun,
			Query,
			ReconcileSourceClosures,
			RecordObservation,
			Register,
			ReleaseObservation,
			ReleaseWake,
			Replay,
			ReplayRequest,
			MaterializeWake,
		};
	}),
);
