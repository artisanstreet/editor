import { Context, Effect, Layer } from "effect";

import type { ProjectRef, ThreadListItem } from "@artisan/protocol";

/** Identifies the lifecycle event that caused a metadata refinement request. */
export type ThreadMetadataRefinementTrigger =
	| "assistant_message"
	| "user_message"
	| "run_started"
	| "run_completed"
	| "run_failed";

/** Carries bounded, provider-neutral evidence for one metadata refinement. */
export interface ThreadMetadataRefinerInput {
	readonly projection: ThreadListItem;
	readonly trigger: ThreadMetadataRefinementTrigger;
	readonly recent_assistant_text: ReadonlyArray<string>;
	readonly recent_user_text: ReadonlyArray<string>;
	readonly recent_activity: ReadonlyArray<string>;
	readonly recent_files: ReadonlyArray<string>;
	readonly recent_artifacts: ReadonlyArray<string>;
}

/** Identifies a thread and its latest immutable refinement evidence. */
export interface ThreadMetadataRefinementRequest extends ThreadMetadataRefinerInput {
	readonly source_event_id: string;
	readonly thread_id: string;
}

/** Describes the metadata fields an automatic refinement may change. */
export interface ThreadMetadataRefinement {
	readonly current_goal?: string;
	readonly live_status: string;
	readonly mentioned_projects?: ReadonlyArray<ProjectRef>;
	readonly rename_suggestion?: string;
	readonly title?: string;
}

/** Computes automatic thread metadata without knowing which provider produced the thread. */
export class ThreadMetadataRefiner extends Context.Service<
	ThreadMetadataRefiner,
	{
		readonly Refine: (
			input: ThreadMetadataRefinerInput,
		) => Effect.Effect<ThreadMetadataRefinement, unknown>;
	}
>()("Artisan/ThreadMetadataRefiner") {}

const max_context_items = 8;
const max_context_text_length = 500;

const bound_context = (values: ReadonlyArray<string>) =>
	values
		.slice(-max_context_items)
		.map((value) => value.trim().slice(0, max_context_text_length))
		.filter((value) => value.length > 0);

/** Bounds provider evidence before it reaches a refiner implementation. */
export const bound_thread_metadata_refiner_input = (
	input: ThreadMetadataRefinerInput,
): ThreadMetadataRefinerInput => ({
	...input,
	recent_assistant_text: bound_context(input.recent_assistant_text),
	recent_user_text: bound_context(input.recent_user_text),
	recent_activity: bound_context(input.recent_activity),
	recent_files: bound_context(input.recent_files),
	recent_artifacts: bound_context(input.recent_artifacts),
});

/** Provides a deterministic metadata proposal from the latest bounded evidence. */
export const ThreadMetadataRefinerLive = Layer.succeed(ThreadMetadataRefiner, {
	Refine: (raw_input) =>
		Effect.gen(function* () {
			const input = yield* Effect.succeed(bound_thread_metadata_refiner_input(raw_input));
			const latest_text = input.recent_user_text.at(-1);
			const latest_file = input.recent_files.at(-1);
			const live_status =
				input.trigger === "assistant_message" ||
				input.trigger === "run_started" ||
				input.trigger === "user_message"
					? "Working"
					: input.trigger === "run_completed"
						? "Complete"
						: input.trigger === "run_failed"
							? "Failed to complete"
							: "Working";
			const title = latest_text ?? latest_file ?? input.projection.title;
			const refinement = {
				...(latest_text
					? { current_goal: latest_text }
					: input.projection.current_goal
						? { current_goal: input.projection.current_goal }
						: {}),
				live_status,
				rename_suggestion: title,
				...(input.projection.title_locked ? {} : { title }),
			};

			return refinement;
		}),
});

/** Creates a deterministic test layer from a caller-supplied refinement function. */
export const make_thread_metadata_refiner_test_layer = (
	refine: (input: ThreadMetadataRefinerInput) => Effect.Effect<ThreadMetadataRefinement, unknown>,
) => Layer.succeed(ThreadMetadataRefiner, { Refine: refine });
