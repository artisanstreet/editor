import { Data, Effect, Ref, Scope } from "effect";

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

export type ThreadMessageCommandResult =
	| { readonly _tag: "ready"; readonly command: ArtisanCommandInput }
	| { readonly _tag: "invalid"; readonly error: ThreadInteractionError };

export interface ThreadMessageSubmissionOutcome {
	readonly expects_user_message: boolean;
}

/** Serializes component submit fibers without coupling them to DOM timing. */
export interface SubmitGate {
	readonly Acquire: Effect.Effect<boolean>;
	readonly Release: Effect.Effect<void>;
}

export const MakeSubmitGate = Ref.make(false).pipe(
	Effect.map(
		(state) =>
			({
				Acquire: Ref.modify(state, (locked) => [!locked, true] as const),
				Release: Ref.set(state, false),
			}) satisfies SubmitGate,
	),
);

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
	after_acceptance: Effect.Effect<void, unknown, RefreshRequirements>,
	scope: Scope.Scope,
) =>
	Effect.gen(function* () {
		const result = yield* command;

		yield* Effect.forkIn(after_acceptance.pipe(Effect.ignore), scope);

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
					answers: { answer: [trimmed] },
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
