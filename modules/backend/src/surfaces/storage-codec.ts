import { Effect, Schema } from "effect";

import { RawOrigin } from "@artisan/protocol";

import { SurfaceInvariantFailed } from "./contracts";

const JsonValue = Schema.UnknownFromJsonString;

export const PersistedSurfaceRawOrigin = RawOrigin;
export const PersistedSurfaceSummary = Schema.Unknown;

export const DecodeSurfaceJson = <S extends Schema.Constraint>(
	schema: S,
	json: string,
	surface_id: string,
): Effect.Effect<S["Type"], SurfaceInvariantFailed, S["DecodingServices"]> =>
	Schema.decodeUnknownEffect(JsonValue)(json).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })),
		Effect.mapError(
			() =>
				new SurfaceInvariantFailed({
					message: `Surface ${surface_id} contains malformed persisted JSON`,
				}),
		),
	);
