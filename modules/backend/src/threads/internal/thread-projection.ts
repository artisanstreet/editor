import { Effect, Schema } from "effect";

import {
	ProjectAffinityScore,
	ProjectRef,
	ThreadListItem,
	ThreadProjectRehomeSuggestion,
	type ProjectRef as Project,
	type ThreadProjectRehomeSuggestion as RehomeSuggestion,
} from "@artisan/protocol";

import { JournalInvariantError } from "../../persistence/journal-store";
import { Threads } from "../../persistence/tables";

const DecodeJson = <A>(
	thread_id: string,
	field: string,
	json: string,
	schema: Schema.ConstraintDecoder<A, never>,
) =>
	Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(json).pipe(
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: `Thread ${thread_id} ${field} JSON is malformed`,
				}),
		),
		Effect.flatMap(
			Schema.decodeUnknownEffect(schema, {
				onExcessProperty: "error",
			}),
		),
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: `Thread ${thread_id} ${field} does not match the protocol schema`,
				}),
		),
	);

/** Decodes one complete persisted thread row into the canonical sidebar projection. */
/**
 * The launch policy lives with the thread's coordinator, not the thread row, so
 * callers that can cheaply join it pass it in. Callers that cannot simply omit
 * it: a list item without an engine is an ordinary state, not an error.
 */
export interface ThreadCoordinatorLaunch {
	readonly engine_id: string;
	readonly policy_model: string | null;
}

export const DecodeThreadProjection = (
	thread: typeof Threads.$inferSelect,
	launch?: ThreadCoordinatorLaunch | undefined,
) =>
	Effect.gen(function* () {
		const linked_projects = yield* DecodeJson(
			thread.thread_id,
			"linked projects",
			thread.linked_projects_json,
			Schema.Array(ProjectRef),
		);
		const project_affinity_scores = yield* DecodeJson(
			thread.thread_id,
			"affinity scores",
			thread.project_affinity_scores_json,
			Schema.Array(ProjectAffinityScore),
		);
		const primary_project: Project | undefined = yield* thread.primary_project_json === null
			? Effect.succeed(undefined)
			: DecodeJson(
					thread.thread_id,
					"primary project",
					thread.primary_project_json,
					ProjectRef,
				);
		const rehome_suggestion: RehomeSuggestion | undefined =
			yield* thread.rehome_suggestion_json === null
				? Effect.succeed(undefined)
				: DecodeJson(
						thread.thread_id,
						"rehome suggestion",
						thread.rehome_suggestion_json,
						ThreadProjectRehomeSuggestion,
					);
		const primary_project_id = primary_project?.project_id ?? null;

		if (primary_project_id !== thread.primary_project_id) {
			return yield* new JournalInvariantError({
				message: `Thread ${thread.thread_id} primary project identity is inconsistent`,
			});
		}

		return yield* Schema.decodeUnknownEffect(ThreadListItem, {
			onExcessProperty: "error",
		})({
			activity_version: thread.activity_version,
			affinity_version: thread.affinity_version,
			created_at: thread.created_at,
			last_activity_at: thread.last_activity_at,
			reader_activity_at: thread.reader_activity_at,
			reader_acknowledged_activity_at: thread.reader_acknowledged_activity_at,
			linked_projects,
			live_status: thread.live_status,
			metadata_version: thread.metadata_version,
			pinned: thread.pinned,
			project_affinity_scores,
			project_locked: thread.project_locked,
			thread_id: thread.thread_id,
			title: thread.title,
			title_locked: thread.title_locked,
			title_source: thread.title_source,
			updated_at: thread.updated_at,
			...(launch === undefined ? {} : { engine_id: launch.engine_id }),
			...(launch?.policy_model == null ? {} : { model_id: launch.policy_model }),
			...(thread.archived_at === null ? {} : { archived_at: thread.archived_at }),
			...(thread.current_goal === null ? {} : { current_goal: thread.current_goal }),
			...(thread.last_assistant_message === null
				? {}
				: { last_assistant_message: thread.last_assistant_message }),
			...(primary_project === undefined ? {} : { primary_project }),
			...(thread.rename_suggestion === null
				? {}
				: { rename_suggestion: thread.rename_suggestion }),
			...(rehome_suggestion === undefined ? {} : { rehome_suggestion }),
			...(thread.summary_title === null ? {} : { summary_title: thread.summary_title }),
		}).pipe(
			Effect.mapError(
				() =>
					new JournalInvariantError({
						message: `Thread ${thread.thread_id} projection is invalid`,
					}),
			),
		);
	});
