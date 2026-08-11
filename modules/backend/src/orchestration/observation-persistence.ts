import { Cause, Effect } from "effect";

import type { EngineObservation } from "@artisan/engines";

import { OrchestrationRepository, type PendingWork } from "../persistence/orchestration/repository";
import { ThreadContinuationRepository } from "../persistence/thread-continuation/repository";
import { AgentGraphRepository } from "./agent-graph-repository";

const AddOptional = (left: number | undefined, right: number | undefined) =>
	left === undefined && right === undefined ? undefined : (left ?? 0) + (right ?? 0);

type UsageObservation = Extract<EngineObservation, { _tag: "usage" }>;

const MergeDeltaUsage = (prior: UsageObservation, current: UsageObservation): UsageObservation => {
	const cached_input_tokens = AddOptional(prior.cached_input_tokens, current.cached_input_tokens);
	const input_tokens = AddOptional(prior.input_tokens, current.input_tokens);
	const output_tokens = AddOptional(prior.output_tokens, current.output_tokens);
	return {
		...current,
		...(cached_input_tokens === undefined ? {} : { cached_input_tokens }),
		...(current.context_tokens !== undefined
			? {}
			: prior.context_tokens === undefined
				? {}
				: { context_tokens: prior.context_tokens }),
		...(current.context_window_tokens !== undefined
			? {}
			: prior.context_window_tokens === undefined
				? {}
				: { context_window_tokens: prior.context_window_tokens }),
		...(input_tokens === undefined ? {} : { input_tokens }),
		...(output_tokens === undefined ? {} : { output_tokens }),
	};
};

const MergeObservation = (
	prior: EngineObservation,
	current: EngineObservation,
): EngineObservation | undefined => {
	if (
		prior._tag === "agent_message_delta" &&
		current._tag === "agent_message_delta" &&
		prior.artisan_run_id === current.artisan_run_id &&
		prior.item_id === current.item_id &&
		prior.turn_id === current.turn_id &&
		prior.phase === current.phase
	) {
		return { ...current, delta: prior.delta + current.delta };
	}
	if (
		prior._tag === "reasoning_summary_delta" &&
		current._tag === "reasoning_summary_delta" &&
		prior.artisan_run_id === current.artisan_run_id &&
		prior.item_id === current.item_id &&
		prior.turn_id === current.turn_id
	) {
		return { ...current, delta: prior.delta + current.delta };
	}
	if (
		prior._tag === "subagent_transcript" &&
		current._tag === "subagent_transcript" &&
		prior.content._tag === "agent_message_delta" &&
		current.content._tag === "agent_message_delta" &&
		prior.artisan_run_id === current.artisan_run_id &&
		prior.agent_native_thread_id === current.agent_native_thread_id &&
		prior.content.item_id === current.content.item_id &&
		prior.content.phase === current.content.phase
	) {
		return {
			...current,
			content: { ...current.content, delta: prior.content.delta + current.content.delta },
		};
	}
	if (
		prior._tag === "subagent_transcript" &&
		current._tag === "subagent_transcript" &&
		prior.content._tag === "reasoning_summary_delta" &&
		current.content._tag === "reasoning_summary_delta" &&
		prior.artisan_run_id === current.artisan_run_id &&
		prior.agent_native_thread_id === current.agent_native_thread_id &&
		prior.content.item_id === current.content.item_id
	) {
		return {
			...current,
			content: { ...current.content, delta: prior.content.delta + current.content.delta },
		};
	}
	return undefined;
};

/** Coalesces token-frequency text frames into one render-cadence durable update. */
export const CompactObservationBatch = (
	batch: ReadonlyArray<EngineObservation>,
): ReadonlyArray<EngineObservation> => {
	const retained_usage = new Map<
		string,
		{
			readonly index: number;
			readonly observation: UsageObservation;
		}
	>();
	for (const [index, observation] of batch.entries()) {
		if (observation._tag !== "usage") continue;
		const key = `${observation.basis}:${observation.turn_id ?? ""}`;
		const prior = retained_usage.get(key)?.observation;
		retained_usage.set(key, {
			index,
			observation:
				observation.basis !== "delta" || prior === undefined
					? observation
					: MergeDeltaUsage(prior, observation),
		});
	}
	const usage_by_last_index = new Map(
		[...retained_usage.values()].map(({ index, observation }) => [index, observation] as const),
	);
	const normalized: Array<EngineObservation> = [];
	for (const [index, observation] of batch.entries()) {
		if (observation._tag === "usage") {
			const retained = usage_by_last_index.get(index);
			if (retained !== undefined) normalized.push(retained);
			continue;
		}
		if (
			(observation._tag === "terminal_activity" && observation.state === "output") ||
			(observation._tag === "tool" && observation.action === "progress") ||
			(observation._tag === "native_action" && observation.diagnostic === true)
		) {
			continue;
		}
		normalized.push(observation);
	}
	const compacted: Array<EngineObservation> = [];
	for (const observation of normalized) {
		const prior = compacted.at(-1);
		const merged = prior === undefined ? undefined : MergeObservation(prior, observation);
		if (merged !== undefined) {
			compacted[compacted.length - 1] = merged;
			continue;
		}
		compacted.push(observation);
	}

	return compacted;
};

/**
 * Builds the durable write path for one run's observation stream. Batches
 * commit in one transaction pair; a failed batch degrades to per-observation
 * writes so one poison observation cannot discard its siblings.
 */
export const MakeObservationPersistence = (input: {
	readonly continuation_repository: typeof ThreadContinuationRepository.Service;
	readonly repository: typeof OrchestrationRepository.Service;
	readonly graph_repository: typeof AgentGraphRepository.Service;
}) => {
	const PersistIndividually = (
		work: Pick<PendingWork, "run_id">,
		batch: ReadonlyArray<EngineObservation>,
	) =>
		Effect.forEach(batch, (observation) =>
			input.repository.RecordObservation(observation).pipe(
				Effect.andThen(
					observation._tag === "subagent" || observation._tag === "subagent_transcript"
						? input.graph_repository.RecordObservedSubagent(observation)
						: Effect.void,
				),
				Effect.andThen(
					input.graph_repository.ReconcileObservedRoot(observation.artisan_run_id),
				),
				Effect.andThen(
					input.continuation_repository.RecordObservationMetadata(observation),
				),
				Effect.asVoid,
				Effect.catchCause((cause) =>
					Effect.sync(() => {
						console.error("Artisan continuation observation metadata failed", {
							failure_kind: Cause.hasInterruptsOnly(cause)
								? "interrupted"
								: "persistence",
							run_id: work.run_id,
						});
					}),
				),
			),
		);

	const PersistBatch = (
		work: Pick<PendingWork, "run_id">,
		batch: ReadonlyArray<EngineObservation>,
	) => {
		const compacted = CompactObservationBatch(batch);
		return input.repository.RecordObservations(compacted).pipe(
			Effect.andThen(
				Effect.forEach(compacted, (observation) =>
					observation._tag === "subagent" || observation._tag === "subagent_transcript"
						? input.graph_repository.RecordObservedSubagent(observation)
						: Effect.void,
				),
			),
			Effect.andThen(input.continuation_repository.RecordObservationsMetadata(compacted)),
			Effect.andThen(
				Effect.forEach(compacted, (observation) =>
					input.graph_repository.ReconcileObservedRoot(observation.artisan_run_id),
				),
			),
			Effect.asVoid,
			Effect.catchCause((cause) =>
				Effect.gen(function* () {
					const interrupted = Cause.hasInterruptsOnly(cause);

					yield* Effect.sync(() => {
						console.error("Artisan observation batch persistence failed", {
							batch_size: compacted.length,
							failure_kind: interrupted ? "interrupted" : "persistence",
							run_id: work.run_id,
						});
					});

					if (interrupted) return;

					yield* PersistIndividually(work, compacted);
				}),
			),
		);
	};

	return { PersistBatch };
};
