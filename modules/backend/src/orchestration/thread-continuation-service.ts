import { Context, Crypto, Data, Effect, Encoding, Layer, Option, Schema } from "effect";

import { EngineRegistry, type Engine, type EngineResumeToken } from "@artisan/engines";

import {
	PortableHandoffLineage,
	ThreadContinuationRepository,
	type ThreadContinuationContext,
	type ThreadContinuationLaunchState,
} from "../persistence/thread-continuation/repository";
import { RuntimeMetadata } from "../runtime/metadata";
import { ThreadContinuationCompactor } from "./thread-continuation-compactor";
import {
	encode_portable_checkpoint_content,
	PortableCheckpoint,
	type PortableCheckpoint as PortableCheckpointValue,
	select_portable_checkpoint_content,
	split_portable_checkpoint_entries,
} from "./thread-continuation-model";

export class ThreadContinuationServiceFailure extends Data.TaggedError(
	"ThreadContinuationServiceFailure",
)<{
	readonly cause?: unknown;
	readonly code: string;
}> {}

export type PreparedThreadContinuation =
	| {
			readonly _tag: "fresh";
			readonly launch_state: ThreadContinuationLaunchState;
	  }
	| {
			readonly _tag: "native";
			readonly launch_state: ThreadContinuationLaunchState;
			readonly resume_token: EngineResumeToken;
			readonly source_run_id: string;
	  }
	| {
			readonly _tag: "portable";
			readonly checkpoint: PortableCheckpointValue;
			readonly handoff_id: string;
			readonly launch_state: ThreadContinuationLaunchState;
			readonly lineage: typeof PortableHandoffLineage.Type;
			readonly source_run_id: string;
	  };

export interface PrepareThreadContinuationInput {
	readonly command_id: string;
	/** Exact executable-facing model, including any provider suffix. */
	readonly target_model?: string;
	/** Catalog/native model persisted independently from executable-only suffixes. */
	readonly target_model_id?: string;
	readonly target_run_id: string;
}

/** Selects, validates, hashes, and durably prepares one continuation launch. */
export class ThreadContinuationService extends Context.Service<
	ThreadContinuationService,
	{
		readonly Prepare: (
			input: PrepareThreadContinuationInput,
		) => Effect.Effect<PreparedThreadContinuation, ThreadContinuationServiceFailure>;
	}
>()("Artisan/ThreadContinuationService") {}

const Failure = (code: string, cause?: unknown) =>
	new ThreadContinuationServiceFailure({
		code,
		...(cause === undefined ? {} : { cause }),
	});

const native_compatible = (
	engine: Engine,
	context: ThreadContinuationContext,
	source: Option.Option.Value<ThreadContinuationContext["source"]>,
	target_model: string | undefined,
) =>
	Effect.gen(function* () {
		if (
			source.engine_id !== context.target.engine_id ||
			target_model === undefined ||
			Option.isNone(source.resume_token) ||
			engine.Descriptor.capabilities.resume.state === "unsupported" ||
			engine.Descriptor.capabilities.native_continuation.state === "unsupported" ||
			engine.CheckNativeContinuation === undefined
		)
			return Option.none<EngineResumeToken>();

		const decision = yield* engine
			.CheckNativeContinuation({
				resume_token: source.resume_token.value,
				...(source.model_id === null || source.model_id === undefined
					? {}
					: { source_model: source.model_id }),
				target_model,
			})
			.pipe(Effect.option);

		return Option.isSome(decision) && decision.value.state === "compatible"
			? Option.some(source.resume_token.value)
			: Option.none<EngineResumeToken>();
	});

export const ThreadContinuationServiceLive = Layer.effect(
	ThreadContinuationService,
	Effect.gen(function* () {
		const compactor = yield* ThreadContinuationCompactor;
		const crypto = yield* Crypto.Crypto;
		const engines = yield* EngineRegistry;
		const metadata = yield* RuntimeMetadata;
		const repository = yield* ThreadContinuationRepository;

		const MakePortable = (
			input: PrepareThreadContinuationInput,
			context: ThreadContinuationContext,
			source: Option.Option.Value<ThreadContinuationContext["source"]>,
		) =>
			Effect.gen(function* () {
				const history = yield* repository.ReadCanonicalHistory(context.target.run_id);
				const split = split_portable_checkpoint_entries(history.entries);
				const generated = yield* compactor.Summarize({
					head: split.head,
					omitted_head_entries: Math.max(
						history.total_entries - history.entries.length,
						0,
					),
					source: {
						engine_id: source.engine_id,
						...(source.model_id === null || source.model_id === undefined
							? {}
							: { model_id: source.model_id }),
					},
					working_directory: source.working_directory,
				});
				const method: PortableCheckpointValue["method"] = Option.isSome(generated)
					? "compaction_model_summary"
					: "canonical_transcript_summary";
				const lineage: typeof PortableHandoffLineage.Type = Option.isSome(generated)
					? {
							compactor_engine_id: generated.value.compactor.engine_id,
							kind: "compactor",
							...(generated.value.compactor.model_id === undefined
								? {}
								: { compactor_model_id: generated.value.compactor.model_id }),
						}
					: { kind: "canonical" };
				const content = select_portable_checkpoint_content({
					canonical_entries: history.entries,
					canonical_total_entries: history.total_entries,
					...(Option.isNone(history.first_user_objective)
						? {}
						: { first_user_objective: history.first_user_objective.value.text }),
					...(Option.isNone(generated)
						? {}
						: { model_summary: { summary: generated.value.summary } }),
				});

				const created_at = yield* metadata.Now;
				const handoff_id = yield* metadata.MakeId("handoff");
				const source_value = {
					cut: {
						thread_id: context.target.thread_id,
						through_journal_sequence: context.source_cut_journal_sequence,
						through_observation_sequence: source.last_observation_sequence,
						through_run_id: source.run_id,
					},
					engine_id: source.engine_id,
					...(source.model_id === null || source.model_id === undefined
						? {}
						: { model_id: source.model_id }),
				};
				const unsigned = {
					created_at,
					method,
					omitted_entries: content.omitted_entries,
					schema_version: 1 as const,
					source: source_value,
					summary: content.summary,
					tail: content.tail,
				};
				const sha256 = yield* crypto
					.digest("SHA-256", encode_portable_checkpoint_content(unsigned))
					.pipe(Effect.map(Encoding.encodeHex));
				const checkpoint = yield* Schema.decodeUnknownEffect(PortableCheckpoint)({
					...unsigned,
					sha256,
				}).pipe(Effect.mapError((cause) => Failure("checkpoint_invalid", cause)));
				const launch_state = yield* repository.PrepareLaunch(context.target.run_id, {
					_tag: "portable",
					checkpoint,
					handoff_id,
					lineage,
					request_id: input.command_id,
					source_run_id: source.run_id,
					...(input.target_model_id === undefined
						? {}
						: { target_model_id: input.target_model_id }),
				});

				return {
					_tag: "portable" as const,
					checkpoint,
					handoff_id,
					launch_state,
					lineage,
					source_run_id: source.run_id,
				};
			});

		const Prepare = (input: PrepareThreadContinuationInput) =>
			Effect.gen(function* () {
				const context = yield* repository.ReadContext(input.target_run_id);
				if (
					context.target.run_id !== input.target_run_id ||
					context.request?.command_id !== input.command_id
				)
					return yield* Effect.fail(Failure("target_request_mismatch"));

				if (Option.isNone(context.source)) {
					return {
						_tag: "fresh" as const,
						launch_state: yield* repository.PrepareLaunch(input.target_run_id, {
							_tag: "fresh",
							request_id: input.command_id,
							...(input.target_model_id === undefined
								? {}
								: { target_model_id: input.target_model_id }),
						}),
					};
				}

				const target_engine = yield* engines
					.Get(context.target.engine_id)
					.pipe(Effect.mapError((cause) => Failure("target_engine_unavailable", cause)));
				const source = context.source.value;
				const native_resume = yield* native_compatible(
					target_engine,
					context,
					source,
					input.target_model,
				);
				if (Option.isSome(native_resume)) {
					return {
						_tag: "native" as const,
						launch_state: yield* repository.PrepareLaunch(input.target_run_id, {
							_tag: "native",
							request_id: input.command_id,
							source_run_id: source.run_id,
							...(input.target_model_id === undefined
								? {}
								: { target_model_id: input.target_model_id }),
						}),
						resume_token: native_resume.value,
						source_run_id: source.run_id,
					};
				}

				return yield* MakePortable(input, context, source);
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof ThreadContinuationServiceFailure
						? cause
						: Failure("continuation_prepare_failed", cause),
				),
			);

		return { Prepare };
	}),
);
