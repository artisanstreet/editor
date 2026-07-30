import { Effect } from "effect";

import type { EngineObservation } from "@artisan/engines";

import { OrchestrationRawObservations } from "../../persistence/tables";
import { AgentGraphInvalid } from "../agent-graph-model";
import type { GraphTransaction } from "./graph-context";

export interface RawObservationLedger {
	readonly append_raw_observation: (
		transaction: GraphTransaction,
		observation: EngineObservation,
	) => Effect.Effect<boolean, unknown>;
}

/** Appends exact provider provenance once before any projection decision is made. */
export function make_raw_observation_ledger(): RawObservationLedger {
	const append_raw_observation = (
		transaction: GraphTransaction,
		observation: EngineObservation,
	) =>
		Effect.gen(function* () {
			const frame_json = yield* Effect.try({
				try: () => JSON.stringify(observation.raw.frame) ?? "null",
				catch: () =>
					new AgentGraphInvalid({
						message: `Raw observation ${observation.observation_id} frame is not serializable`,
					}),
			});
			/** Preserve base64 only where JSON cannot recreate the exact native bytes. */
			const raw_frame_base64 =
				observation.raw.raw_frame_base64 !== undefined &&
				Buffer.from(frame_json, "utf8").toString("base64") ===
					observation.raw.raw_frame_base64
					? null
					: (observation.raw.raw_frame_base64 ?? null);
			const inserted = yield* transaction
				.insert(OrchestrationRawObservations)
				.values({
					engine_id: observation.raw.engine_id,
					frame_json,
					native_id:
						observation.raw.native_id === undefined
							? null
							: String(observation.raw.native_id),
					native_method: observation.raw.native_method ?? null,
					observation_id: observation.observation_id,
					protocol_version: observation.raw.protocol_version ?? null,
					raw_frame_base64,
					run_id: observation.artisan_run_id,
					sequence: observation.sequence,
					transport: observation.raw.transport,
				})
				.onConflictDoNothing()
				.returning({ observation_id: OrchestrationRawObservations.observation_id });

			return inserted.length === 1;
		});

	return { append_raw_observation };
}
