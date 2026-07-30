import { Context, Effect, Layer, Option, Schema, Stream } from "effect";

import { model_manifest } from "@artisan/catalog";
import { EngineRegistry, type EngineObservation, type EngineRunMetadata } from "@artisan/engines";

import { SessionDefaultsService } from "../settings/session-defaults-service";
import { RuntimeMetadata } from "../runtime/metadata";
import {
	PortableCheckpointSummary,
	render_compaction_prompt,
	serialize_compaction_transcript,
	type CanonicalTranscriptEntry,
} from "./thread-continuation-model";

/** Bounds one compaction turn so a stuck provider cannot stall a switch. */
const compaction_timeout_ms = 5 * 60 * 1_000;

export interface ThreadCompactionRequest {
	/** Ordered canonical entries older than the checkpoint's verbatim tail. */
	readonly head: ReadonlyArray<CanonicalTranscriptEntry>;
	/** Entries already missing from `head` because the canonical read is bounded. */
	readonly omitted_head_entries: number;
	readonly source: {
		readonly engine_id: string;
		readonly model_id?: string;
	};
	readonly working_directory: string;
}

export interface ThreadCompactionSummary {
	readonly compactor: {
		readonly engine_id: string;
		readonly model_id?: string;
	};
	readonly summary: string;
}

/**
 * Generates a portable handoff summary by running one constrained turn on the
 * configured compaction model. Absence of a result deliberately selects the
 * canonical mechanical fallback; this service never fails a switch.
 */
export class ThreadContinuationCompactor extends Context.Service<
	ThreadContinuationCompactor,
	{
		readonly Summarize: (
			request: ThreadCompactionRequest,
		) => Effect.Effect<Option.Option<ThreadCompactionSummary>>;
	}
>()("Artisan/ThreadContinuationCompactor") {}

/**
 * Resolves the compaction engine and model: the operator's configured
 * compaction model when it names an enabled catalog entry by its unique
 * catalog id, otherwise the source thread's own engine and model.
 */
const resolve_compactor = (
	configured_model_id: string | undefined,
	source: ThreadCompactionRequest["source"],
) => {
	const configured =
		configured_model_id === undefined
			? undefined
			: model_manifest.models.find(
					(model) => model.id === configured_model_id && model.disabled === undefined,
				);

	return configured === undefined
		? {
				engine_id: source.engine_id,
				...(source.model_id === undefined ? {} : { model_id: source.model_id }),
			}
		: { engine_id: configured.harness, model_id: configured.native_model_id };
};

/** Claude compaction disables its tool surface and all user customizations at the CLI boundary. */
const compactor_run_metadata = (engine_id: string): EngineRunMetadata =>
	engine_id === "claude"
		? {
				provider_options: {
					"claude.disable_tools": true,
					"claude.permission_mode": "plan",
					"claude.safe_mode": true,
				},
			}
		: {
				permission_policy: {
					approval: "never",
					network_access: false,
					write_access: false,
				},
			};

/** A pending question or approval can only hang a print-mode turn; abort it. */
const observation_blocks_compaction = (observation: EngineObservation) =>
	(observation._tag === "approval" || observation._tag === "question") &&
	observation.state === "requested";

export const ThreadContinuationCompactorLive = Layer.effect(
	ThreadContinuationCompactor,
	Effect.gen(function* () {
		const engines = yield* EngineRegistry;
		const metadata = yield* RuntimeMetadata;
		const settings = yield* SessionDefaultsService;

		const RunCompactionTurn = (request: ThreadCompactionRequest, prompt: string) =>
			Effect.scoped(
				Effect.gen(function* () {
					const defaults = yield* settings.Read.pipe(Effect.option);
					const compactor = resolve_compactor(
						Option.isSome(defaults) ? defaults.value.compaction_model_id : undefined,
						request.source,
					);
					const engine = yield* engines.Get(compactor.engine_id);
					const artisan_run_id = yield* metadata.MakeId("compaction");
					const run = yield* engine.Open({
						_tag: "start",
						artisan_run_id,
						initial_text: prompt,
						...(compactor.model_id === undefined ? {} : { model: compactor.model_id }),
						...compactor_run_metadata(compactor.engine_id),
						working_directory: request.working_directory,
					});
					const message = yield* Stream.runFoldEffect(
						run.Events,
						() => undefined as string | undefined,
						(latest, observation) =>
							observation_blocks_compaction(observation)
								? Effect.fail(new Error("Compaction turn requested interaction"))
								: Effect.succeed(
										observation._tag === "agent_message_completed" &&
											observation.phase !== "commentary"
											? observation.message
											: latest,
									),
					);
					const terminal = yield* run.Closed;
					if (terminal !== "completed" || message === undefined) return Option.none();

					return Schema.decodeUnknownOption(PortableCheckpointSummary)(
						message.trim(),
					).pipe(Option.map((summary) => ({ compactor, summary })));
				}),
			);

		const Summarize = (request: ThreadCompactionRequest) =>
			Effect.gen(function* () {
				if (request.head.length === 0) return Option.none<ThreadCompactionSummary>();
				const transcript = serialize_compaction_transcript(request.head);
				const prompt = render_compaction_prompt({
					omitted_entries: transcript.omitted_entries + request.omitted_head_entries,
					transcript: transcript.text,
				});

				return yield* RunCompactionTurn(request, prompt).pipe(
					Effect.timeout(compaction_timeout_ms),
					Effect.catch(() => Effect.succeed(Option.none<ThreadCompactionSummary>())),
				);
			});

		return { Summarize };
	}),
);
