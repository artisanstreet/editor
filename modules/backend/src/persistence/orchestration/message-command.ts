import { Effect, Schema } from "effect";

import {
	type CommandEnvelope,
	Identifier,
	ImageAttachmentBytes,
	ImageMediaType,
	ProjectRef,
	UserMessageContentPart,
} from "@artisan/protocol";

const DurableImageAttachment = Schema.Struct({
	bytes: Schema.optional(ImageAttachmentBytes),
	id: Identifier,
	media_type: ImageMediaType,
	name: Schema.String,
});

/** Forge-enriched message payload stored only after authority and ID resolution. */
export const AuthoritativeThreadSendMessageCommand = Schema.Struct({
	attachments: Schema.optional(Schema.Array(DurableImageAttachment)),
	content: Schema.optional(Schema.Array(UserMessageContentPart)),
	engine_id: Identifier,
	mentioned_projects: Schema.Array(ProjectRef).pipe(
		Schema.optional,
		Schema.withDecodingDefault(Effect.succeed([])),
	),
	text: Schema.NonEmptyString,
	type: Schema.Literal("thread.send_message"),
	working_directory: Schema.NonEmptyString,
});

export type AuthoritativeThreadSendMessageCommand =
	typeof AuthoritativeThreadSendMessageCommand.Type;

type NonMessageCommandPayload = CommandEnvelope["payload"] & {
	readonly type: Exclude<CommandEnvelope["payload"]["type"], "thread.send_message">;
};

/** Internal command shape used after Forge has resolved message authority. */
export type AuthoritativeCommandEnvelope = Omit<CommandEnvelope, "payload"> & {
	readonly payload: NonMessageCommandPayload | AuthoritativeThreadSendMessageCommand;
};

export type InboundOrAuthoritativeCommandEnvelope = CommandEnvelope | AuthoritativeCommandEnvelope;
