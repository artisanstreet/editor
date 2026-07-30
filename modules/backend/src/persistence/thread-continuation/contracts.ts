import { Context, Data, Effect, Option, Schema } from "effect";

import type { EngineObservation, EngineResumeToken } from "@artisan/engines";

import {
	PortableCheckpoint,
	type PortableCheckpoint as PortableCheckpointValue,
} from "../../orchestration/thread-continuation-model";

export const EngineResumeTokenSchema = Schema.Struct({
	native_thread_id: Schema.NonEmptyString,
	opaque_checkpoint: Schema.optional(Schema.String),
});

export const FailureCode = Schema.String.check(Schema.isPattern(/^[a-z][a-z0-9_]{0,63}$/));

export const PortableHandoffLineage = Schema.Union([
	Schema.Struct({ kind: Schema.Literal("canonical") }),
	Schema.Struct({
		compactor_engine_id: Schema.NonEmptyString,
		compactor_model_id: Schema.optional(Schema.NonEmptyString),
		kind: Schema.Literal("compactor"),
	}),
]);

export const lineage_matches_method = (
	method: PortableCheckpointValue["method"],
	lineage: typeof PortableHandoffLineage.Type,
) =>
	(method === "canonical_transcript_summary" && lineage.kind === "canonical") ||
	(method === "compaction_model_summary" && lineage.kind === "compactor");

export type ThreadContinuationContext = {
	readonly first_target_journal_sequence: number;
	readonly source_cut_journal_sequence: number;
	readonly request:
		| { readonly command_id: string; readonly message_id: string; readonly text: string }
		| undefined;
	readonly source: Option.Option<{
		readonly engine_id: string;
		readonly last_native_turn_id: string | null | undefined;
		readonly last_observation_sequence: number;
		readonly model_id: string | null | undefined;
		readonly native_thread_id: string | null;
		readonly resume_token: Option.Option<EngineResumeToken>;
		readonly status: string;
		readonly working_directory: string;
		readonly run_id: string;
	}>;
	readonly target: {
		readonly engine_id: string;
		readonly model_id: string | null;
		readonly run_id: string;
		readonly thread_id: string;
	};
};

export class ThreadContinuationFailure extends Data.TaggedError("ThreadContinuationFailure")<{
	readonly code: string;
}> {}

export class ThreadContinuationConflict extends Data.TaggedError("ThreadContinuationConflict")<{
	readonly target_run_id: string;
}> {}

export type ContinuationError = ThreadContinuationFailure | ThreadContinuationConflict;
export type ThreadContinuationLaunchState = "prepared" | "opening" | "bound" | "failed";
export const ThreadContinuationLaunchState = Schema.Union([
	Schema.Literal("prepared"),
	Schema.Literal("opening"),
	Schema.Literal("bound"),
	Schema.Literal("failed"),
]);

export type CanonicalHistoryEntry = {
	readonly journal_sequence: number;
	readonly logical_sequence: number;
	readonly role: "user" | "assistant";
	readonly run_id: string;
	readonly text: string;
};

export type CanonicalHistory = {
	readonly entries: ReadonlyArray<CanonicalHistoryEntry>;
	readonly first_user_objective: Option.Option<CanonicalHistoryEntry>;
	readonly total_entries: number;
};

export type ContinuationLaunch =
	| {
			readonly _tag: "fresh";
			readonly request_id: string;
			readonly target_model_id?: string;
	  }
	| {
			readonly _tag: "native";
			readonly request_id: string;
			readonly source_run_id: string;
			readonly target_model_id?: string;
	  }
	| {
			readonly _tag: "portable";
			readonly handoff_id: string;
			readonly request_id: string;
			readonly source_run_id: string;
			readonly target_model_id?: string;
			readonly checkpoint: typeof PortableCheckpoint.Type;
			readonly lineage: typeof PortableHandoffLineage.Type;
	  };

/** Private SQLite ownership for crash-safe engine continuation lineage. */
export class ThreadContinuationRepository extends Context.Service<
	ThreadContinuationRepository,
	{
		readonly IsDispatchReady: (
			target_run_id: string,
		) => Effect.Effect<boolean, ContinuationError>;
		readonly ReadContext: (
			target_run_id: string,
		) => Effect.Effect<ThreadContinuationContext, ContinuationError>;
		readonly ReadCanonicalHistory: (
			target_run_id: string,
		) => Effect.Effect<CanonicalHistory, ContinuationError>;
		readonly RecordObservationMetadata: (
			observation: EngineObservation,
		) => Effect.Effect<void, ContinuationError>;
		readonly PrepareLaunch: (
			target_run_id: string,
			launch: ContinuationLaunch,
		) => Effect.Effect<ThreadContinuationLaunchState, ContinuationError>;
		readonly MarkOpening: (target_run_id: string) => Effect.Effect<void, ContinuationError>;
		readonly BindTarget: (input: {
			readonly command_id: string;
			readonly model_id?: string;
			readonly native_thread_id: string;
			readonly resume_token: EngineResumeToken;
			readonly target_run_id: string;
		}) => Effect.Effect<void, ContinuationError>;
		readonly FailLaunch: (
			target_run_id: string,
			failure_code: string,
		) => Effect.Effect<void, ContinuationError>;
		readonly ReconcileStranded: () => Effect.Effect<ReadonlyArray<string>, ContinuationError>;
	}
>()("Artisan/ThreadContinuationRepository") {}
