import { Context, Data, Effect, Layer, Schedule, Schema, Semaphore } from "effect";

import {
	Identifier,
	RawOrigin,
	type EventEnvelope,
	FilesystemMutationEvent,
	GitWorkspaceObservedEvent,
	ProcessOwnershipEvent,
} from "@artisan/protocol";

import {
	JournalStore,
	JournalStoreFailure,
	type JournalStoreError,
} from "../persistence/journal-store";

const EvidenceWriteContentionSchedule = Schedule.exponential("5 millis").pipe(
	Schedule.upTo({ duration: "1 second", times: 8 }),
);

const WorkspaceEvidenceTrace = Schema.Struct({
	agent_id: Schema.optional(Identifier),
	operation_id: Identifier,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
});

/** Carries the explicit attribution that controlled workspace tools must supply. */
export type WorkspaceEvidenceTrace = typeof WorkspaceEvidenceTrace.Type;

const FilesystemMutationEvidenceInput = Schema.Struct({
	...WorkspaceEvidenceTrace.fields,
	destination_path: FilesystemMutationEvent.fields.destination_path,
	operation: FilesystemMutationEvent.fields.operation,
	path: FilesystemMutationEvent.fields.path,
});

/** Supplies the complete intent for one attributed filesystem mutation. */
export type FilesystemMutationEvidenceInput = typeof FilesystemMutationEvidenceInput.Type;

const ProcessOwnershipEvidenceInput = Schema.Struct({
	...WorkspaceEvidenceTrace.fields,
	source: ProcessOwnershipEvent.fields.source,
	working_directory: ProcessOwnershipEvent.fields.working_directory,
});

/** Supplies the complete intent for one attributed process ownership observation. */
export type ProcessOwnershipEvidenceInput = typeof ProcessOwnershipEvidenceInput.Type;

const GitWorkspaceObservedEvidenceInput = Schema.Struct({
	...WorkspaceEvidenceTrace.fields,
	branch: GitWorkspaceObservedEvent.fields.branch,
	changed_file_count: GitWorkspaceObservedEvent.fields.changed_file_count,
	has_diff: GitWorkspaceObservedEvent.fields.has_diff,
	root_path: GitWorkspaceObservedEvent.fields.root_path,
	worktree_path: GitWorkspaceObservedEvent.fields.worktree_path,
});

/** Supplies the complete intent for one attributed Git workspace observation. */
export type GitWorkspaceObservedEvidenceInput = typeof GitWorkspaceObservedEvidenceInput.Type;

/** Returns the durable event produced by an evidence-recording attempt. */
export interface WorkspaceEvidenceAcceptance {
	readonly event: EventEnvelope;
	readonly status: "accepted" | "duplicate";
}

/** Reports operation-id reuse that no longer represents the same attributed evidence. */
export class WorkspaceEvidenceConflict extends Data.TaggedError("WorkspaceEvidenceConflict")<{
	readonly operation_id: string;
}> {}

/** Reports workspace evidence that failed canonical boundary validation. */
export class WorkspaceEvidenceInvalid extends Data.TaggedError("WorkspaceEvidenceInvalid")<{
	readonly message: string;
}> {}

/** Represents failures while recording explicit workspace evidence. */
export type WorkspaceEvidenceRecorderError =
	| JournalStoreError
	| WorkspaceEvidenceConflict
	| WorkspaceEvidenceInvalid;

/** Owns production attribution and durable publication of controlled workspace evidence. */
export class WorkspaceEvidenceRecorder extends Context.Service<
	WorkspaceEvidenceRecorder,
	{
		readonly RecordFilesystemMutation: (
			input: FilesystemMutationEvidenceInput,
		) => Effect.Effect<WorkspaceEvidenceAcceptance, WorkspaceEvidenceRecorderError>;
		readonly RecordGitWorkspaceObserved: (
			input: GitWorkspaceObservedEvidenceInput,
		) => Effect.Effect<WorkspaceEvidenceAcceptance, WorkspaceEvidenceRecorderError>;
		readonly RecordProcessOwnership: (
			input: ProcessOwnershipEvidenceInput,
		) => Effect.Effect<WorkspaceEvidenceAcceptance, WorkspaceEvidenceRecorderError>;
	}
>()("Artisan/WorkspaceEvidenceRecorder") {}

type WorkspaceEvidenceInput =
	| FilesystemMutationEvidenceInput
	| GitWorkspaceObservedEvidenceInput
	| ProcessOwnershipEvidenceInput;

function causation_id(operation_id: string) {
	return `workspace_evidence:${operation_id}:causation`;
}

function correlation_id(operation_id: string) {
	return `workspace_evidence:${operation_id}:correlation`;
}

function payload_from_input(input: WorkspaceEvidenceInput) {
	if ("operation" in input) {
		return {
			...(input.destination_path === undefined
				? {}
				: { destination_path: input.destination_path }),
			operation: input.operation,
			path: input.path,
			type: "filesystem.mutation" as const,
		};
	}

	if ("source" in input) {
		return {
			source: input.source,
			type: "process.ownership" as const,
			working_directory: input.working_directory,
		};
	}

	return {
		...(input.branch === undefined ? {} : { branch: input.branch }),
		changed_file_count: input.changed_file_count,
		has_diff: input.has_diff,
		root_path: input.root_path,
		type: "git.workspace.observed" as const,
		worktree_path: input.worktree_path,
	};
}

function has_same_raw_origin(left: RawOrigin | undefined, right: RawOrigin | undefined) {
	return left?.provider === right?.provider && left?.reference === right?.reference;
}

function has_same_intent(event: EventEnvelope, input: WorkspaceEvidenceInput) {
	const payload = payload_from_input(input);

	return (
		event.agent_id === input.agent_id &&
		event.causation_id === causation_id(input.operation_id) &&
		event.correlation_id === correlation_id(input.operation_id) &&
		JSON.stringify(event.payload) === JSON.stringify(payload) &&
		has_same_raw_origin(event.raw_origin, input.raw_origin) &&
		event.run_id === input.run_id &&
		event.thread_id === input.thread_id
	);
}

/** Builds the production recorder over the journal and a runtime-local publication lock. */
export const WorkspaceEvidenceRecorderLive = Layer.effect(
	WorkspaceEvidenceRecorder,
	Effect.gen(function* () {
		const journal = yield* JournalStore;
		const lock = yield* Semaphore.make(1);

		const RecordUnlocked = (input: WorkspaceEvidenceInput) =>
			Effect.gen(function* () {
				const correlation = correlation_id(input.operation_id);
				const existing = yield* journal.ReadCorrelatedEvents(correlation);

				if (existing.length > 0) {
					const [event] = existing;

					/** Legacy runtimes could append the same exact evidence more than once. */
					if (existing.some((candidate) => !has_same_intent(candidate, input))) {
						return yield* Effect.fail(
							new WorkspaceEvidenceConflict({ operation_id: input.operation_id }),
						);
					}

					return { event: event!, status: "duplicate" as const };
				}

				const event = yield* journal.AppendEvent({
					...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
					causation_id: causation_id(input.operation_id),
					correlation_id: correlation,
					idempotency_key: correlation,
					payload: payload_from_input(input),
					...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
					...(input.run_id === undefined ? {} : { run_id: input.run_id }),
					thread_id: input.thread_id,
				});

				return { event, status: "accepted" as const };
			});

		const Record = (input: WorkspaceEvidenceInput) =>
			Semaphore.withPermit(lock)(
				RecordUnlocked(input).pipe(
					Effect.retry({
						schedule: EvidenceWriteContentionSchedule,
						while: (error) => error instanceof JournalStoreFailure,
					}),
				),
			);
		const Decode = <A>(schema: Schema.Codec<A, A>, input: unknown) =>
			Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(
					() =>
						new WorkspaceEvidenceInvalid({
							message: "Workspace evidence does not match its canonical schema",
						}),
				),
			);

		return {
			RecordFilesystemMutation: (input) =>
				Decode(FilesystemMutationEvidenceInput, input).pipe(Effect.flatMap(Record)),
			RecordGitWorkspaceObserved: (input) =>
				Decode(GitWorkspaceObservedEvidenceInput, input).pipe(Effect.flatMap(Record)),
			RecordProcessOwnership: (input) =>
				Decode(ProcessOwnershipEvidenceInput, input).pipe(Effect.flatMap(Record)),
		};
	}),
);
