import { Data, Duration, Effect, Option, Ref, Schedule } from "effect";

import type {
	ThreadListItem,
	ThreadSendMessageCommand,
	ThreadSessionSnapshot,
	ThreadWorkItem,
} from "@artisan/protocol";
import type { ArtisanCommandInput } from "@artisan/transport/client";
import { MakeUserMessageContent, type ComposerSubmission } from "../composer/image-attachments";

/** Describes the routed thread state required to construct one safe user action. */
export interface ThreadInteractionContext {
	readonly session: ThreadSessionSnapshot;
	readonly thread: ThreadListItem | undefined;
	readonly thread_id: string;
	/** Present only while the coordinator still owns an active work run. */
	readonly work: ThreadWorkItem | undefined;
}

/** A local interaction cannot be sent until the durable thread context is complete. */
export class ThreadInteractionError extends Data.TaggedError("ThreadInteractionError")<{
	readonly message: string;
}> {}

class AcceptedProjectionPending extends Data.TaggedError("AcceptedProjectionPending")<{
	readonly message: string;
}> {}

export type ThreadMessageCommandResult =
	| { readonly _tag: "ready"; readonly command: ArtisanCommandInput }
	| { readonly _tag: "invalid"; readonly error: ThreadInteractionError };

export interface ThreadMessageSubmissionOutcome {
	readonly expects_user_message: boolean;
	readonly user_message_reference?: string;
}

/** Serializes component submit fibers without coupling them to DOM timing. */
export interface SubmitGate {
	readonly Acquire: Effect.Effect<boolean>;
	readonly Release: Effect.Effect<void>;
}

export const MakeSubmitGate = Effect.gen(function* () {
	const state = yield* Ref.make(false);
	return {
		Acquire: Effect.gen(function* () {
			return yield* Ref.modify(state, (locked) => [!locked, true] as const);
		}),
		Release: Effect.gen(function* () {
			yield* Ref.set(state, false);
		}),
	} satisfies SubmitGate;
});

const AcceptedProjectionRetrySchedule = Schedule.spaced("50 millis").pipe(
	Schedule.upTo({ duration: "1 second", times: 8 }),
);

const AwaitAcceptedProjectionRetrySchedule = Schedule.exponential("50 millis").pipe(
	Schedule.modifyDelay(({ duration }) =>
		Effect.gen(function* () {
			return Duration.min(duration, Duration.seconds(1));
		}),
	),
);

/**
 * Polls the authoritative query after a durable receipt until it contains the
 * accepted projection. Query failures and projection lag share one short,
 * bounded retry budget because neither may turn an accepted command into a
 * second submission.
 */
export const ObserveAcceptedProjection = <Projection, QueryError, QueryRequirements>(
	query: Effect.Effect<Projection, QueryError, QueryRequirements>,
	is_observed: (projection: Projection) => boolean,
) =>
	Effect.gen(function* () {
		return yield* query.pipe(
			Effect.filterOrFail(
				is_observed,
				() =>
					new AcceptedProjectionPending({
						message: "The accepted command is not projected yet.",
					}),
			),
			Effect.retry({ schedule: AcceptedProjectionRetrySchedule }),
			Effect.option,
			Effect.timeoutOption("1 second"),
			Effect.map(Option.flatten),
		);
	});

/**
 * Holds an already accepted submission until its canonical user turn appears.
 * A steer is projected only after Forge delivers it to the live engine; bounded
 * capped backoff limits polling pressure while component scope owns interruption.
 */
export const AwaitAcceptedProjection = <Projection, QueryError, QueryRequirements>(
	query: Effect.Effect<Projection, QueryError, QueryRequirements>,
	is_observed: (projection: Projection) => boolean,
) =>
	Effect.gen(function* () {
		return yield* query.pipe(
			Effect.filterOrFail(
				is_observed,
				() =>
					new AcceptedProjectionPending({
						message: "The accepted command is not projected yet.",
					}),
			),
			Effect.retry({ schedule: AwaitAcceptedProjectionRetrySchedule }),
		);
	});

/**
 * A command receipt is the irreversible submit boundary. Refreshing projections
 * after that receipt improves immediacy, but failure to refresh must not invite
 * the user to send an already durable message again.
 */
export const SubmitDurableCommand = <
	Command,
	CommandError,
	CommandRequirements,
	RefreshRequirements,
>(
	command: Effect.Effect<Command, CommandError, CommandRequirements>,
	after_acceptance: (accepted: Command) => Effect.Effect<void, unknown, RefreshRequirements>,
) =>
	Effect.gen(function* () {
		const result = yield* command;

		yield* after_acceptance(result).pipe(Effect.ignore);

		return result;
	});

/**
 * Builds the sole public composer command. The backend decides whether this is
 * a newly queued message or a steering message for its active capable run.
 */
export const BuildThreadMessageCommand = (
	context: ThreadInteractionContext,
	submission: ComposerSubmission,
): ThreadMessageCommandResult => {
	const trimmed = submission.text.trim();
	if (trimmed.length === 0 && submission.attachments.length === 0) {
		return {
			_tag: "invalid",
			error: new ThreadInteractionError({
				message: "Write a message or attach an image before sending it.",
			}),
		};
	}

	const pending_question = context.session.pending_question;
	if (pending_question?.state === "pending") {
		if (submission.attachments.length > 0) {
			return {
				_tag: "invalid",
				error: new ThreadInteractionError({
					message: "Answer the pending question with text before attaching images.",
				}),
			};
		}
		return {
			_tag: "ready",
			command: {
				payload: {
					/** Dispatch resolves the answer by looking it up under the question's own id. */
					answers: { [pending_question.question_id]: [trimmed] },
					question_id: pending_question.question_id,
					type: "intake.respond_question",
				},
				thread_id: context.thread_id,
			},
		};
	}

	const project = context.thread?.primary_project;
	if (project === undefined) {
		return {
			_tag: "invalid",
			error: new ThreadInteractionError({
				message: "Assign a project to this thread before sending a message.",
			}),
		};
	}

	return {
		_tag: "ready",
		command: {
			...(context.work === undefined
				? {}
				: { agent_id: context.work.agent_id, run_id: context.work.run_id }),
			payload: {
				engine_id: context.work?.engine_id ?? context.session.policy.engine_id,
				attachments: submission.attachments.map((attachment) => ({
					bytes: Uint8Array.from(
						globalThis.atob(attachment.content_base64),
						(character) => character.codePointAt(0) ?? 0,
					),
					client_token: attachment.id,
					media_type: attachment.mime_type,
					name: attachment.name,
				})),
				content: MakeUserMessageContent(submission.text, submission.attachments),
				text: trimmed || "Attached image",
				type: "thread.send_message",
			} satisfies ThreadSendMessageCommand,
			thread_id: context.thread_id,
		},
	};
};
