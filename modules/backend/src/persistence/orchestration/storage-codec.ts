import { Effect, Schema } from "effect";

import { EventPayload, ProjectRef, RawOrigin } from "@artisan/protocol";

import { OrchestrationFailure } from "./contracts";

const JsonValue = Schema.UnknownFromJsonString;

export const PersistedAssumptions = Schema.Array(Schema.String);
export const PersistedEventPayload = EventPayload;
export const PersistedJsonValue = Schema.Unknown;
export const PersistedMentionedProjects = Schema.Array(ProjectRef);
export const PersistedRawOrigin = RawOrigin;
export const PersistedResumeToken = Schema.Struct({
	native_thread_id: Schema.String,
	opaque_checkpoint: Schema.optional(Schema.String),
});

export type PersistedState<A> =
	| { readonly _tag: "Absent" }
	| { readonly _tag: "Decoded"; readonly value: A }
	| { readonly _tag: "Corrupt"; readonly cause: unknown };

export const DecodePersistedJson = <S extends Schema.Constraint>(
	schema: S,
	json: string,
): Effect.Effect<S["Type"], OrchestrationFailure, S["DecodingServices"]> =>
	Schema.decodeUnknownEffect(JsonValue)(json).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })),
		Effect.mapError((cause) => new OrchestrationFailure({ cause })),
	);

export const DecodeOptionalPersistedJson = <S extends Schema.Constraint>(
	schema: S,
	json: string | null | undefined,
): Effect.Effect<PersistedState<S["Type"]>, never, S["DecodingServices"]> =>
	json === null || json === undefined
		? Effect.succeed({ _tag: "Absent" as const })
		: DecodePersistedJson(schema, json).pipe(
				Effect.match({
					onFailure: (cause) => ({ _tag: "Corrupt" as const, cause }),
					onSuccess: (value) => ({ _tag: "Decoded" as const, value }),
				}),
			);
