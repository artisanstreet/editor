import { Option, Schema } from "effect";

import type { EngineResumeToken } from "@artisan/engines";

import { EngineResumeTokenSchema } from "./contracts";

const PersistedUnknown = Schema.fromJsonString(Schema.Unknown);

export const DecodePersistedJson = (value: string | null | undefined) =>
	value === null || value === undefined
		? Option.none<unknown>()
		: Schema.decodeUnknownOption(PersistedUnknown)(value);

export const DecodeResumeToken = (value: unknown): Option.Option<EngineResumeToken> =>
	Schema.decodeUnknownOption(EngineResumeTokenSchema)(value).pipe(
		Option.map((token) => ({
			native_thread_id: token.native_thread_id,
			...(token.opaque_checkpoint === undefined
				? {}
				: { opaque_checkpoint: token.opaque_checkpoint }),
		})),
	);
