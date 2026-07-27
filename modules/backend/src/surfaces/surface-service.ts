import { Clock, Context, Data, Effect, Layer, Schema } from "effect";
import { and, asc, desc, eq, gte, lte } from "drizzle-orm";

import {
	SurfaceItem,
	SurfaceSnapshot,
	SurfaceUsage,
	SurfaceUsageAggregate,
	SurfaceUsageDailySnapshot,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import {
	AgentRuns,
	Assignments,
	JournalEvents,
	OrchestrationGroups,
	SurfaceItems,
	SurfaceUsageTotals,
} from "../persistence/schema";

/** Reports corrupt persisted surface state without converting a read into a defect. */
export class SurfaceInvariantFailed extends Data.TaggedError("SurfaceInvariantFailed")<{
	readonly message: string;
}> {}

/** Read-only provider-neutral surface and usage projection boundary. */
export class SurfaceService extends Context.Service<
	SurfaceService,
	{
		readonly List: (input: {
			readonly thread_id: string;
			readonly run_id?: string;
			readonly group_id?: string;
		}) => Effect.Effect<typeof SurfaceSnapshot.Type, SurfaceInvariantFailed>;
		readonly ListSnapshot: (input: {
			readonly thread_id: string;
			readonly run_id?: string;
			readonly group_id?: string;
		}) => Effect.Effect<typeof SurfaceSnapshot.Type, SurfaceInvariantFailed>;
		readonly Usage: (input: {
			readonly run_id?: string;
			readonly group_id?: string;
		}) => Effect.Effect<ReadonlyArray<typeof SurfaceUsage.Type>, SurfaceInvariantFailed>;
		readonly AggregateUsage: (input: {
			readonly scope: "run" | "assignment" | "group";
			readonly scope_id: string;
		}) => Effect.Effect<typeof SurfaceUsageAggregate.Type, SurfaceInvariantFailed>;
		readonly AggregateUsageSnapshot: (input: {
			readonly scope: "run" | "assignment" | "group";
			readonly scope_id: string;
		}) => Effect.Effect<
			{
				readonly aggregate: typeof SurfaceUsageAggregate.Type;
				readonly journal_sequence: number;
			},
			SurfaceInvariantFailed
		>;
		readonly DailyUsageSnapshot: (input: {
			readonly day_count: number;
		}) => Effect.Effect<typeof SurfaceUsageDailySnapshot.Type, SurfaceInvariantFailed>;
		readonly UsageEventAffects: (
			input: { readonly scope: "run" | "assignment" | "group"; readonly scope_id: string },
			run_id: string | undefined,
		) => Effect.Effect<boolean, SurfaceInvariantFailed>;
		readonly UsageScopeThread: (input: {
			readonly scope: "run" | "assignment" | "group";
			readonly scope_id: string;
		}) => Effect.Effect<string | undefined, SurfaceInvariantFailed>;
	}
>()("Artisan/SurfaceService") {}

export const SurfaceServiceLive = Layer.effect(
	SurfaceService,
	Effect.gen(function* () {
		const database = yield* Database;
		const DecodeSurfaceItem = (row: typeof SurfaceItems.$inferSelect) =>
			Effect.try({
				try: () => {
					const raw_origin =
						row.raw_origin_json === null
							? undefined
							: (JSON.parse(row.raw_origin_json) as { readonly provider?: unknown });

					return {
						surface_id: row.surface_id,
						category: row.category,
						kind: row.kind,
						summary: JSON.parse(row.summary_json),
						occurred_at: row.occurred_at,
						...(raw_origin === undefined
							? {}
							: {
									raw_origin,
									raw_observation: {
										engine_id: raw_origin.provider,
										observation_id: row.observation_id,
									},
								}),
						attribution: {
							thread_id: row.thread_id,
							run_id: row.run_id,
							...(row.group_id ? { group_id: row.group_id } : {}),
							...(row.assignment_id ? { assignment_id: row.assignment_id } : {}),
						},
					};
				},
				catch: () =>
					new SurfaceInvariantFailed({
						message: `Surface ${row.surface_id} contains malformed persisted JSON`,
					}),
			}).pipe(
				Effect.flatMap(Schema.decodeUnknownEffect(SurfaceItem)),
				Effect.mapError((error) =>
					error instanceof SurfaceInvariantFailed
						? error
						: new SurfaceInvariantFailed({
								message: `Surface ${row.surface_id} does not match its schema`,
							}),
				),
			);
		const DecodeSurfaceUsage = (row: typeof SurfaceUsageTotals.$inferSelect) =>
			Schema.decodeUnknownEffect(SurfaceUsage)({
				run_id: row.run_id,
				...(row.group_id ? { group_id: row.group_id } : {}),
				...(row.assignment_id ? { assignment_id: row.assignment_id } : {}),
				...(row.input_tokens === null ? {} : { input_tokens: row.input_tokens }),
				...(row.output_tokens === null ? {} : { output_tokens: row.output_tokens }),
				updated_at: row.updated_at,
			}).pipe(
				Effect.mapError(
					() =>
						new SurfaceInvariantFailed({
							message: `Usage ${row.run_id} does not match its schema`,
						}),
				),
			);
		const ListSnapshot = (input: {
			readonly thread_id: string;
			readonly run_id?: string;
			readonly group_id?: string;
		}) =>
			database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const rows = yield* transaction
						.select()
						.from(SurfaceItems)
						.where(
							and(
								eq(SurfaceItems.thread_id, input.thread_id),
								...(input.run_id ? [eq(SurfaceItems.run_id, input.run_id)] : []),
								...(input.group_id
									? [eq(SurfaceItems.group_id, input.group_id)]
									: []),
							),
						)
						.orderBy(asc(SurfaceItems.projection_order));
					const items = yield* Effect.forEach(rows, DecodeSurfaceItem);
					const [last] = yield* transaction
						.select({ journal_sequence: JournalEvents.sequence })
						.from(JournalEvents)
						.orderBy(desc(JournalEvents.sequence))
						.limit(1);
					return { items, journal_sequence: last?.journal_sequence ?? 0 };
				}),
			);
		const List = ListSnapshot;
		const Usage = (input: { readonly run_id?: string; readonly group_id?: string }) =>
			database.client
				.select()
				.from(SurfaceUsageTotals)
				.where(
					input.run_id
						? eq(SurfaceUsageTotals.run_id, input.run_id)
						: input.group_id
							? eq(SurfaceUsageTotals.group_id, input.group_id)
							: undefined,
				)
				.pipe(Effect.flatMap((rows) => Effect.forEach(rows, DecodeSurfaceUsage)));
		const AggregateUsage = (input: {
			readonly scope: "run" | "assignment" | "group";
			readonly scope_id: string;
		}) =>
			database.client
				.select()
				.from(SurfaceUsageTotals)
				.where(
					input.scope === "run"
						? eq(SurfaceUsageTotals.run_id, input.scope_id)
						: input.scope === "assignment"
							? eq(SurfaceUsageTotals.assignment_id, input.scope_id)
							: eq(SurfaceUsageTotals.group_id, input.scope_id),
				)
				.pipe(
					Effect.flatMap((rows) => Effect.forEach(rows, DecodeSurfaceUsage)),
					Effect.map((usage) => {
						const sum = (values: ReadonlyArray<number | undefined>) => {
							return values.length === 0 ||
								values.some((value) => value === undefined)
								? undefined
								: (values as ReadonlyArray<number>).reduce(
										(left, right) => left + right,
										0,
									);
						};
						const input_tokens = sum(usage.map((row) => row.input_tokens));
						const output_tokens = sum(usage.map((row) => row.output_tokens));
						return {
							scope: input.scope,
							scope_id: input.scope_id,
							...(input_tokens === undefined ? {} : { input_tokens }),
							...(output_tokens === undefined ? {} : { output_tokens }),
						};
					}),
					Effect.flatMap(Schema.decodeUnknownEffect(SurfaceUsageAggregate)),
					Effect.mapError((error) =>
						error instanceof SurfaceInvariantFailed
							? error
							: new SurfaceInvariantFailed({
									message: `Aggregate usage ${input.scope}:${input.scope_id} does not match its schema`,
								}),
					),
				);
		const AggregateUsageSnapshot = (input: {
			readonly scope: "run" | "assignment" | "group";
			readonly scope_id: string;
		}) =>
			database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const rows = yield* transaction
						.select()
						.from(SurfaceUsageTotals)
						.where(
							input.scope === "run"
								? eq(SurfaceUsageTotals.run_id, input.scope_id)
								: input.scope === "assignment"
									? eq(SurfaceUsageTotals.assignment_id, input.scope_id)
									: eq(SurfaceUsageTotals.group_id, input.scope_id),
						);
					const usage = yield* Effect.forEach(rows, DecodeSurfaceUsage);
					const sum = (values: ReadonlyArray<number | undefined>) =>
						values.length === 0 || values.some((value) => value === undefined)
							? undefined
							: (values as ReadonlyArray<number>).reduce(
									(left, right) => left + right,
									0,
								);
					const aggregate = yield* Schema.decodeUnknownEffect(SurfaceUsageAggregate)({
						scope: input.scope,
						scope_id: input.scope_id,
						...(sum(usage.map((row) => row.input_tokens)) === undefined
							? {}
							: { input_tokens: sum(usage.map((row) => row.input_tokens)) }),
						...(sum(usage.map((row) => row.output_tokens)) === undefined
							? {}
							: { output_tokens: sum(usage.map((row) => row.output_tokens)) }),
					}).pipe(
						Effect.mapError(
							() =>
								new SurfaceInvariantFailed({
									message: `Aggregate usage ${input.scope}:${input.scope_id} does not match its schema`,
								}),
						),
					);
					const [last] = yield* transaction
						.select({ journal_sequence: JournalEvents.sequence })
						.from(JournalEvents)
						.orderBy(desc(JournalEvents.sequence))
						.limit(1);
					return { aggregate, journal_sequence: last?.journal_sequence ?? 0 };
				}),
			);
		/**
		 * Rolls persisted run totals up into UTC calendar days. `updated_at` is
		 * the last time a run reported usage rather than the instant each token
		 * was spent, so a long-running run attributes its whole total to the day
		 * it finished reporting.
		 */
		const DailyUsageSnapshot = (input: { readonly day_count: number }) =>
			database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const now_ms = yield* Clock.currentTimeMillis;
					const day_ms = 86_400_000;
					const iso_date = (at_ms: number) => new Date(at_ms).toISOString().slice(0, 10);
					const first_date = iso_date(now_ms - (input.day_count - 1) * day_ms);
					const last_date = iso_date(now_ms);
					const rows = yield* transaction
						.select()
						.from(SurfaceUsageTotals)
						.where(
							and(
								gte(SurfaceUsageTotals.updated_at, `${first_date}T00:00:00.000Z`),
								lte(SurfaceUsageTotals.updated_at, `${last_date}T23:59:59.999Z`),
							),
						);
					const usage = yield* Effect.forEach(rows, DecodeSurfaceUsage);
					const totals = new Map<
						string,
						{ input_tokens: number; output_tokens: number }
					>();

					for (const row of usage) {
						const date = row.updated_at.slice(0, 10);
						const running = totals.get(date) ?? { input_tokens: 0, output_tokens: 0 };
						totals.set(date, {
							input_tokens: running.input_tokens + (row.input_tokens ?? 0),
							output_tokens: running.output_tokens + (row.output_tokens ?? 0),
						});
					}

					const [last] = yield* transaction
						.select({ journal_sequence: JournalEvents.sequence })
						.from(JournalEvents)
						.orderBy(desc(JournalEvents.sequence))
						.limit(1);
					return yield* Schema.decodeUnknownEffect(SurfaceUsageDailySnapshot)({
						buckets: [...totals.entries()]
							.sort(([left], [right]) => left.localeCompare(right))
							.map(([date, bucket]) => ({ date, ...bucket })),
						journal_sequence: last?.journal_sequence ?? 0,
					}).pipe(
						Effect.mapError(
							() =>
								new SurfaceInvariantFailed({
									message: "Daily usage does not match its schema",
								}),
						),
					);
				}),
			);
		const UsageEventAffects = (
			input: { readonly scope: "run" | "assignment" | "group"; readonly scope_id: string },
			run_id: string | undefined,
		) =>
			run_id === undefined
				? Effect.succeed(false)
				: database.client
						.select({ run_id: SurfaceUsageTotals.run_id })
						.from(SurfaceUsageTotals)
						.where(
							and(
								eq(SurfaceUsageTotals.run_id, run_id),
								input.scope === "run"
									? eq(SurfaceUsageTotals.run_id, input.scope_id)
									: input.scope === "assignment"
										? eq(SurfaceUsageTotals.assignment_id, input.scope_id)
										: eq(SurfaceUsageTotals.group_id, input.scope_id),
							),
						)
						.limit(1)
						.pipe(Effect.map((rows) => rows.length === 1));
		const UsageScopeThread = (input: {
			readonly scope: "run" | "assignment" | "group";
			readonly scope_id: string;
		}) => {
			const group_ids =
				input.scope === "group"
					? Effect.succeed([{ group_id: input.scope_id }])
					: input.scope === "assignment"
						? database.client
								.select({ group_id: Assignments.group_id })
								.from(Assignments)
								.where(eq(Assignments.assignment_id, input.scope_id))
								.limit(1)
						: database.client
								.select({ group_id: AgentRuns.group_id })
								.from(AgentRuns)
								.where(eq(AgentRuns.run_id, input.scope_id))
								.limit(1);
			return group_ids.pipe(
				Effect.flatMap(([group]) =>
					group === undefined
						? Effect.succeed(undefined)
						: database.client
								.select({ thread_id: OrchestrationGroups.thread_id })
								.from(OrchestrationGroups)
								.where(eq(OrchestrationGroups.group_id, group.group_id))
								.limit(1)
								.pipe(Effect.map(([row]) => row?.thread_id)),
				),
				Effect.mapError(
					() =>
						new SurfaceInvariantFailed({ message: "Usage scope thread lookup failed" }),
				),
			);
		};
		return {
			List,
			ListSnapshot,
			Usage,
			AggregateUsage,
			AggregateUsageSnapshot,
			DailyUsageSnapshot,
			UsageEventAffects,
			UsageScopeThread,
		} as unknown as typeof SurfaceService.Service;
	}),
);
