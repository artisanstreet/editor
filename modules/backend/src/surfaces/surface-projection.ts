import { createHash } from "node:crypto";

import { Effect, Option, Schema } from "effect";
import { eq } from "drizzle-orm";

import type { EngineObservation } from "@artisan/engines";
import { SurfaceItem } from "@artisan/protocol";

import type { DatabaseClient } from "../persistence/database";
import { SurfaceItems, SurfaceUsageTotals } from "../persistence/tables";
import { SurfaceInvariantFailed } from "./contracts";
import { SurfaceFromEngineObservation } from "./engine-observation";

const IsSafeRawIdentifier = (value: string) =>
	value.length > 0 && value.length <= 4_096 && /^\S+$/u.test(value) && !/[\p{Cc}]/u.test(value);

/** Validates the public projection and replaces unsafe provider provenance with an opaque item. */
const DecodeSurfaceProjection = (
	observation: EngineObservation,
	attribution: SurfaceItem["attribution"],
	occurred_at: string,
) => {
	const projected = SurfaceFromEngineObservation(observation, attribution, occurred_at);
	const native_reference = String(observation.raw.native_id ?? observation.observation_id);
	const raw_identifiers_are_safe =
		IsSafeRawIdentifier(observation.raw.engine_id) && IsSafeRawIdentifier(native_reference);

	if (raw_identifiers_are_safe) {
		return Schema.decodeUnknownEffect(SurfaceItem)(projected).pipe(Effect.option);
	}

	return Effect.succeed(Option.none());
};

const DecodeOpaqueSurfaceProjection = (
	observation: EngineObservation,
	attribution: SurfaceItem["attribution"],
	occurred_at: string,
) => {
	const identity = createHash("sha256")
		.update(observation.observation_id)
		.update("\0")
		.update(observation.raw.engine_id)
		.update("\0")
		.update(String(observation.raw.native_id ?? ""))
		.digest("hex");

	return Schema.decodeUnknownEffect(SurfaceItem)({
		attribution,
		category: "native_action",
		kind: "opaque_engine_work",
		occurred_at,
		summary: { label: "Opaque engine work" },
		surface_id: `surface:opaque:${identity}`,
	});
};

/** Persists one safe surface and optional usage total in the raw-observation transaction. */
export const PersistSurfaceProjection = (
	transaction: DatabaseClient,
	observation: EngineObservation,
	input: {
		readonly thread_id: string;
		readonly run_id: string;
		readonly agent_id: string;
		readonly occurred_at: string;
		readonly group_id?: string;
		readonly assignment_id?: string;
	},
) =>
	Effect.gen(function* () {
		const attribution = {
			agent_id: input.agent_id,
			run_id: input.run_id,
			thread_id: input.thread_id,
			...(input.group_id ? { group_id: input.group_id } : {}),
			...(input.assignment_id ? { assignment_id: input.assignment_id } : {}),
		};
		const decoded = yield* DecodeSurfaceProjection(observation, attribution, input.occurred_at);
		const item =
			decoded._tag === "Some"
				? decoded.value
				: yield* DecodeOpaqueSurfaceProjection(observation, attribution, input.occurred_at);
		const inserted = yield* transaction
			.insert(SurfaceItems)
			.values({
				surface_id: item.surface_id,
				observation_id: observation.observation_id,
				thread_id: input.thread_id,
				run_id: input.run_id,
				group_id: input.group_id ?? null,
				assignment_id: input.assignment_id ?? null,
				sequence: observation.sequence,
				category: item.category,
				kind: item.kind,
				summary_json: JSON.stringify(item.summary),
				raw_origin_json:
					item.raw_origin === undefined ? null : JSON.stringify(item.raw_origin),
				occurred_at: input.occurred_at,
			})
			.onConflictDoNothing()
			.returning({ surface_id: SurfaceItems.surface_id });
		if (inserted.length === 0) return;
		if (observation._tag !== "usage") return;
		const [prior] = yield* transaction
			.select()
			.from(SurfaceUsageTotals)
			.where(eq(SurfaceUsageTotals.run_id, input.run_id))
			.limit(1);
		const add = (prior_value: number | null | undefined, next: number | undefined) =>
			next === undefined ? (prior_value ?? null) : (prior_value ?? 0) + next;
		const next_total = (prior_value: number | null | undefined, next: number | undefined) => {
			if (observation.basis === "cumulative") return next ?? prior_value ?? null;
			if (observation.basis === "delta") return add(prior_value, next);
			return next === undefined ? (prior_value ?? null) : null;
		};
		/** Context gauges are point-in-time snapshots: the newest report wins regardless of basis. */
		const next_gauge = (prior_value: number | null | undefined, next: number | undefined) =>
			next ?? prior_value ?? null;
		const input_tokens = next_total(prior?.input_tokens, observation.input_tokens);
		const output_tokens = next_total(prior?.output_tokens, observation.output_tokens);
		const cached_input_tokens = next_total(
			prior?.cached_input_tokens,
			observation.cached_input_tokens,
		);
		const context_tokens = next_gauge(prior?.context_tokens, observation.context_tokens);
		const context_window_tokens = next_gauge(
			prior?.context_window_tokens,
			observation.context_window_tokens,
		);
		yield* transaction
			.insert(SurfaceUsageTotals)
			.values({
				run_id: input.run_id,
				group_id: input.group_id ?? null,
				assignment_id: input.assignment_id ?? null,
				input_tokens,
				output_tokens,
				cached_input_tokens,
				context_tokens,
				context_window_tokens,
				updated_at: input.occurred_at,
			})
			.onConflictDoUpdate({
				target: SurfaceUsageTotals.run_id,
				set: {
					input_tokens,
					output_tokens,
					cached_input_tokens,
					context_tokens,
					context_window_tokens,
					updated_at: input.occurred_at,
				},
			});
	}).pipe(
		Effect.mapError(
			() =>
				new SurfaceInvariantFailed({
					message: `Surface projection ${observation.observation_id} could not be persisted`,
				}),
		),
	);
