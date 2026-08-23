import { Context, Effect, Layer, Option, Schema, Stream } from "effect";

import {
	ComposeNativeModelId,
	model_manifest,
	opencode2_big_pickle_compaction_model_id,
} from "@artisan/catalog";
import { inherited_compaction_model } from "@artisan/protocol";
import {
	EngineRegistry,
	OpenCode2AgentId,
	type EngineObservation,
	type EngineRunMetadata,
} from "@artisan/engines";

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
		readonly catalog_revision?: string;
		readonly engine_id: string;
		readonly model_id?: string;
		readonly profile_id?: string;
		readonly provider_route_id?: string;
		readonly variant_id?: string;
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
const enabled_catalog_model = (model_id: string | undefined) =>
	model_id === undefined
		? undefined
		: model_manifest.models.find(
				(model) => model.id === model_id && model.disabled === undefined,
			);

type ResolvedCompactor = {
	/** Present only for an explicit pick, whose saved model defaults apply. */
	readonly catalog_id?: string;
	readonly catalog_revision?: string;
	readonly engine_id: string;
	/** Catalog identity which must be resolved from live engine inventory. */
	readonly live_catalog_id?: string;
	readonly model_id?: string;
	readonly profile_id?: string;
	readonly provider_route_id?: string;
	readonly variant_id?: string;
};

const source_compactor = (source: ThreadCompactionRequest["source"]): ResolvedCompactor => ({
	...(source.catalog_revision === undefined ? {} : { catalog_revision: source.catalog_revision }),
	engine_id: source.engine_id,
	...(source.model_id === undefined ? {} : { model_id: source.model_id }),
	...(source.profile_id === undefined ? {} : { profile_id: source.profile_id }),
	...(source.provider_route_id === undefined
		? {}
		: { provider_route_id: source.provider_route_id }),
	...(source.variant_id === undefined ? {} : { variant_id: source.variant_id }),
});

const resolve_compactor = (
	selection: string | undefined,
	source: ThreadCompactionRequest["source"],
): ResolvedCompactor => {
	/** Inherited deliberately summarizes with the thread's own model. */
	if (selection === inherited_compaction_model) return source_compactor(source);

	/** OpenCode catalog entries are profile-scoped and therefore live-only. */
	if (selection?.startsWith("opencode2:") === true)
		return {
			catalog_id: selection,
			engine_id: "opencode2",
			live_catalog_id: selection,
		};

	/**
	 * An explicit catalog pick wins; otherwise the curated per-harness default
	 * summarizes, and only a harness without one falls back to the thread's
	 * own model.
	 */
	const explicit = enabled_catalog_model(selection);
	const curated_catalog_id = model_manifest.harnesses.find(
		(harness) => harness.id === source.engine_id,
	)?.compaction_default_model_id;
	const chosen = explicit ?? enabled_catalog_model(curated_catalog_id);

	if (
		chosen === undefined &&
		source.engine_id === "opencode2" &&
		curated_catalog_id === opencode2_big_pickle_compaction_model_id
	)
		return {
			engine_id: "opencode2",
			live_catalog_id: curated_catalog_id,
			...(source.profile_id === undefined ? {} : { profile_id: source.profile_id }),
		};

	return chosen === undefined
		? source_compactor(source)
		: {
				...(explicit === undefined ? {} : { catalog_id: explicit.id }),
				engine_id: chosen.harness,
				model_id: chosen.native_model_id,
			};
};

/**
 * Claude compaction disables its tool surface and all user customizations at
 * the CLI boundary. Codex additionally summarizes at low reasoning effort by
 * default — the template is fixed and the transcript bounded, so extra
 * thinking buys latency, not fidelity — unless an explicitly picked compactor
 * carries its own saved effort default.
 */
const compactor_run_metadata = (
	compactor: ResolvedCompactor,
	reasoning_effort: string | undefined,
): EngineRunMetadata => {
	if (compactor.engine_id === "opencode2")
		return {
			...(compactor.catalog_revision === undefined
				? {}
				: { catalog_revision: compactor.catalog_revision }),
			...(compactor.model_id === undefined
				? {}
				: { model: compactor.model_id, model_id: compactor.model_id }),
			permission_policy: {
				approval: "never",
				network_access: false,
				write_access: false,
			},
			...(compactor.profile_id === undefined ? {} : { profile_id: compactor.profile_id }),
			...(compactor.provider_route_id === undefined
				? {}
				: { provider_route_id: compactor.provider_route_id }),
			provider_options: {
				"opencode2.agent": OpenCode2AgentId("restricted", false, false),
				"opencode2.project_config": false,
				"opencode2.web_search_enabled": false,
			},
			...(compactor.variant_id === undefined ? {} : { variant_id: compactor.variant_id }),
		};

	return compactor.engine_id === "claude"
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
				...(compactor.engine_id === "codex"
					? {
							provider_options: {
								"codex.reasoning_effort": reasoning_effort ?? "low",
							},
						}
					: {}),
			};
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

		/**
		 * OpenCode selections must be refreshed in the exact profile and workspace
		 * that will execute them. This supplies the current revision and keeps the
		 * model, route, and variant tuple atomic across a portable handoff.
		 */
		const ResolveLiveCompactor = (
			request: ThreadCompactionRequest,
			compactor: ResolvedCompactor,
		) =>
			Effect.gen(function* () {
				if (compactor.engine_id !== "opencode2") return compactor;
				const engine = yield* engines.Get(compactor.engine_id);
				if (engine.Catalog === undefined)
					return yield* Effect.fail(
						new Error("OpenCode does not expose its required live model catalog"),
					);
				const profile_id =
					compactor.profile_id ??
					(request.source.engine_id === "opencode2"
						? request.source.profile_id
						: undefined) ??
					"default";
				const catalog = yield* engine.Catalog({
					profile_id,
					working_directory: request.working_directory,
					workspace_trust: "safe",
				});
				const selected = catalog.models.find((candidate) =>
					compactor.live_catalog_id === undefined
						? candidate.enabled &&
							candidate.model_id === compactor.model_id &&
							candidate.provider_route_id === compactor.provider_route_id &&
							candidate.variant_id === compactor.variant_id
						: candidate.enabled && candidate.catalog_id === compactor.live_catalog_id,
				);
				if (selected === undefined)
					return yield* Effect.fail(
						new Error("The selected OpenCode compaction model is unavailable"),
					);
				return {
					...compactor,
					catalog_revision: catalog.revision,
					model_id: selected.model_id,
					profile_id,
					provider_route_id: selected.provider_route_id,
					...(selected.variant_id === undefined
						? {}
						: { variant_id: selected.variant_id }),
				};
			});

		const RunCompactionTurn = (request: ThreadCompactionRequest, prompt: string) =>
			Effect.scoped(
				Effect.gen(function* () {
					const defaults = yield* settings.Read.pipe(Effect.option);
					const compactor = yield* ResolveLiveCompactor(
						request,
						resolve_compactor(
							Option.isSome(defaults) ? defaults.value.compaction_model : undefined,
							request.source,
						),
					);
					/** An explicit pick honors its durable per-model defaults. */
					const saved_defaults =
						compactor.catalog_id === undefined || Option.isNone(defaults)
							? undefined
							: defaults.value.models.find(
									(model) => model.model_id === compactor.catalog_id,
								);
					const engine = yield* engines.Get(compactor.engine_id);
					const artisan_run_id = yield* metadata.MakeId("compaction");
					const run = yield* engine.Open({
						_tag: "start",
						artisan_run_id,
						initial_text: prompt,
						...(compactor.model_id === undefined
							? {}
							: {
									model: ComposeNativeModelId(
										compactor.engine_id,
										compactor.model_id,
										saved_defaults?.context_window,
									),
								}),
						...compactor_run_metadata(compactor, saved_defaults?.reasoning_effort),
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
					).pipe(
						Option.map((summary) => ({
							compactor: {
								engine_id: compactor.engine_id,
								...(compactor.model_id === undefined
									? {}
									: { model_id: compactor.model_id }),
							},
							summary,
						})),
					);
				}),
			);

		const Summarize = (request: ThreadCompactionRequest) =>
			Effect.gen(function* () {
				/**
				 * An empty head is not a failure and must not be reported as one: the
				 * checkpoint's verbatim tail already holds the whole conversation, so
				 * there is nothing left for a summary to describe.
				 */
				if (request.head.length === 0) return Option.none<ThreadCompactionSummary>();
				const transcript = serialize_compaction_transcript(request.head);
				const prompt = render_compaction_prompt({
					omitted_entries: transcript.omitted_entries + request.omitted_head_entries,
					transcript: transcript.text,
				});

				/**
				 * Absence still selects the mechanical fallback rather than failing the
				 * switch, but it is recorded. Swallowing every cause silently made a
				 * genuinely broken compactor indistinguishable from one that had
				 * nothing to do, and the difference decides how much context survives
				 * every portable handoff.
				 */
				return yield* RunCompactionTurn(request, prompt).pipe(
					Effect.timeout(compaction_timeout_ms),
					Effect.tap((summary) =>
						Option.isSome(summary)
							? Effect.void
							: Effect.logWarning(
									`Thread compaction for ${request.source.engine_id} returned no usable summary; the handoff falls back to the canonical transcript`,
								),
					),
					Effect.catch((cause) =>
						Effect.logWarning(
							`Thread compaction for ${request.source.engine_id} failed; the handoff falls back to the canonical transcript: ${String(cause)}`,
						).pipe(Effect.as(Option.none<ThreadCompactionSummary>())),
					),
				);
			});

		return { Summarize };
	}),
);
