import { Clock, Effect, Layer, Schema } from "effect";
import { and, asc, desc, eq, gt, gte, inArray, isNotNull, lte } from "drizzle-orm";

import {
	SurfaceItem,
	SurfaceUsage,
	SurfaceUsageAggregate,
	SurfaceUsageDailySnapshot,
	ThreadUsageSeries,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import {
	AgentRuns,
	Assignments,
	JournalEvents,
	OrchestrationGroups,
	OrchestrationRuns,
	SurfaceItems,
	SurfaceUsageTotals,
} from "../persistence/tables";

import { SurfaceInvariantFailed, SurfaceService } from "./contracts";
import {
	DecodeSurfaceJson,
	PersistedSurfaceRawOrigin,
	PersistedSurfaceSummary,
} from "./storage-codec";

export { SurfaceInvariantFailed, SurfaceService } from "./contracts";

const NormalizeSurfaceFailure = (error: unknown, operation: string) =>
	error instanceof SurfaceInvariantFailed
		? error
		: new SurfaceInvariantFailed({ message: `${operation} failed` });

/**
 * Folds decoded usage rows into one aggregate payload. Token counts sum
 * across the scope's runs and stay unknown when any run's count is unknown;
 * the context gauges carry over from the most recently updated run because a
 * context window belongs to one live session and summing several is
 * meaningless.
 */
const aggregate_usage_fields = (
	scope: "run" | "assignment" | "group",
	scope_id: string,
	usage: ReadonlyArray<SurfaceUsage>,
) => {
	const sum = (values: ReadonlyArray<number | undefined>) =>
		values.length === 0 || values.some((value) => value === undefined)
			? undefined
			: (values as ReadonlyArray<number>).reduce((left, right) => left + right, 0);
	const latest = usage.reduce<SurfaceUsage | undefined>(
		(newest, row) =>
			newest === undefined || newest.updated_at <= row.updated_at ? row : newest,
		undefined,
	);
	const input_tokens = sum(usage.map((row) => row.input_tokens));
	const output_tokens = sum(usage.map((row) => row.output_tokens));
	const cached_input_tokens = sum(usage.map((row) => row.cached_input_tokens));
	return {
		scope,
		scope_id,
		...(input_tokens === undefined ? {} : { input_tokens }),
		...(output_tokens === undefined ? {} : { output_tokens }),
		...(cached_input_tokens === undefined ? {} : { cached_input_tokens }),
		...(latest?.context_tokens === undefined ? {} : { context_tokens: latest.context_tokens }),
		...(latest?.context_window_tokens === undefined
			? {}
			: { context_window_tokens: latest.context_window_tokens }),
		...(latest?.context_tokens === undefined && latest?.context_window_tokens === undefined
			? {}
			: { context_run_id: latest?.run_id }),
	};
};

export const SurfaceServiceLive = Layer.effect(
	SurfaceService,
	Effect.gen(function* () {
		const database = yield* Database;
		const DecodeAggregateUsage = (
			input: { readonly scope: "run" | "assignment" | "group"; readonly scope_id: string },
			usage: ReadonlyArray<SurfaceUsage>,
			LookupRun: (
				run_id: string,
			) => Effect.Effect<
				{ readonly engine_id: string; readonly model_id: string | null } | undefined,
				unknown
			>,
		) =>
			Effect.gen(function* () {
				const fields = aggregate_usage_fields(input.scope, input.scope_id, usage);
				const { context_run_id, ...aggregate } = fields;
				const origin =
					context_run_id === undefined ? undefined : yield* LookupRun(context_run_id);
				return yield* Schema.decodeUnknownEffect(SurfaceUsageAggregate)({
					...aggregate,
					...(context_run_id === undefined
						? {}
						: {
								context_origin: {
									run_id: context_run_id,
									...(origin === undefined
										? {}
										: {
												engine_id: origin.engine_id,
												...(origin.model_id === null
													? {}
													: { model_id: origin.model_id }),
											}),
								},
							}),
				});
			});
		const DecodeSurfaceItem = (row: typeof SurfaceItems.$inferSelect) =>
			Effect.gen(function* () {
				const raw_origin =
					row.raw_origin_json === null
						? undefined
						: yield* DecodeSurfaceJson(
								PersistedSurfaceRawOrigin,
								row.raw_origin_json,
								row.surface_id,
							);
				const summary = yield* DecodeSurfaceJson(
					PersistedSurfaceSummary,
					row.summary_json,
					row.surface_id,
				);
				return yield* Schema.decodeUnknownEffect(SurfaceItem)({
					surface_id: row.surface_id,
					category: row.category,
					kind: row.kind,
					summary,
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
				});
			}).pipe(
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
				...(row.cached_input_tokens === null
					? {}
					: { cached_input_tokens: row.cached_input_tokens }),
				...(row.context_tokens === null ? {} : { context_tokens: row.context_tokens }),
				...(row.context_window_tokens === null
					? {}
					: { context_window_tokens: row.context_window_tokens }),
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
		/**
		 * The per-turn token series for one thread's current context window.
		 *
		 * Cut at the most recent compaction: a compaction replaces the history
		 * with a summary, so turns before it no longer occupy the window and
		 * charting across the boundary would draw one fill over two unrelated
		 * windows. Runs are ordered by when the thread first observed them, which
		 * is the order they consumed the window in.
		 */
		const UsageSeries = (input: { readonly thread_id: string }) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [boundary] = yield* transaction
							.select({ projection_order: SurfaceItems.projection_order })
							.from(SurfaceItems)
							.where(
								and(
									eq(SurfaceItems.thread_id, input.thread_id),
									eq(SurfaceItems.kind, "compaction"),
								),
							)
							.orderBy(desc(SurfaceItems.projection_order))
							.limit(1);
						const rows = yield* transaction
							.select({
								projection_order: SurfaceItems.projection_order,
								run_id: SurfaceItems.run_id,
							})
							.from(SurfaceItems)
							.where(
								and(
									eq(SurfaceItems.thread_id, input.thread_id),
									...(boundary === undefined
										? []
										: [
												gt(
													SurfaceItems.projection_order,
													boundary.projection_order,
												),
											]),
								),
							)
							.orderBy(asc(SurfaceItems.projection_order));

						/** First sighting wins, so a run keeps the position it started at. */
						const ordered_run_ids: Array<string> = [];
						const seen = new Set<string>();
						for (const row of rows) {
							if (seen.has(row.run_id)) continue;
							seen.add(row.run_id);
							ordered_run_ids.push(row.run_id);
						}
						if (ordered_run_ids.length === 0) {
							return {
								compacted: boundary !== undefined,
								points: [],
								thread_id: input.thread_id,
							};
						}

						/** Keep the newest chart runs when a long thread exceeds the UI cap. */
						const selected_run_ids = ordered_run_ids.slice(-512);
						const totals = yield* transaction
							.select()
							.from(SurfaceUsageTotals)
							.where(inArray(SurfaceUsageTotals.run_id, selected_run_ids));
						const by_run = new Map(totals.map((row) => [row.run_id, row] as const));

						const points = selected_run_ids
							.map((run_id) => by_run.get(run_id))
							.filter((row) => row !== undefined)
							.map((row, index) => ({
								...(row.cached_input_tokens === null
									? {}
									: { cached_input_tokens: row.cached_input_tokens }),
								...(row.context_tokens === null
									? {}
									: { context_tokens: row.context_tokens }),
								...(row.input_tokens === null
									? {}
									: { input_tokens: row.input_tokens }),
								ordinal: index + 1,
								...(row.output_tokens === null
									? {}
									: { output_tokens: row.output_tokens }),
								run_id: row.run_id,
								updated_at: row.updated_at,
							}));

						/**
						 * Gauge freshness is independent of chart truncation and first-sighting
						 * order. Joining back to the current compaction segment lets the newest
						 * report win even when its run is outside the 512 rendered points.
						 */
						const [latest_report] = yield* transaction
							.select({
								context_window_tokens: SurfaceUsageTotals.context_window_tokens,
							})
							.from(SurfaceUsageTotals)
							.innerJoin(
								SurfaceItems,
								eq(SurfaceItems.run_id, SurfaceUsageTotals.run_id),
							)
							.where(
								and(
									eq(SurfaceItems.thread_id, input.thread_id),
									isNotNull(SurfaceUsageTotals.context_window_tokens),
									...(boundary === undefined
										? []
										: [
												gt(
													SurfaceItems.projection_order,
													boundary.projection_order,
												),
											]),
								),
							)
							.orderBy(
								desc(SurfaceUsageTotals.updated_at),
								desc(SurfaceUsageTotals.run_id),
							)
							.limit(1);
						const window_tokens = latest_report?.context_window_tokens ?? undefined;

						return {
							compacted: boundary !== undefined,
							...(window_tokens === undefined
								? {}
								: { context_window_tokens: window_tokens }),
							points,
							thread_id: input.thread_id,
						};
					}),
				)
				.pipe(
					Effect.flatMap(Schema.decodeUnknownEffect(ThreadUsageSeries)),
					Effect.mapError(
						() =>
							new SurfaceInvariantFailed({
								message: `Usage series for ${input.thread_id} does not match its schema`,
							}),
					),
				);
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
					Effect.flatMap((usage) =>
						DecodeAggregateUsage(input, usage, (run_id) =>
							Effect.gen(function* () {
								const [thread_run] = yield* database.client
									.select({
										engine_id: OrchestrationRuns.engine_id,
										model_id: OrchestrationRuns.model_id,
									})
									.from(OrchestrationRuns)
									.where(eq(OrchestrationRuns.run_id, run_id))
									.limit(1);
								if (thread_run !== undefined) return thread_run;
								const [agent_run] = yield* database.client
									.select({
										engine_id: AgentRuns.engine_id,
										model_id: AgentRuns.model_id,
									})
									.from(AgentRuns)
									.where(eq(AgentRuns.run_id, run_id))
									.limit(1);
								return agent_run;
							}),
						),
					),
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
					const aggregate = yield* DecodeAggregateUsage(input, usage, (run_id) =>
						Effect.gen(function* () {
							const [thread_run] = yield* transaction
								.select({
									engine_id: OrchestrationRuns.engine_id,
									model_id: OrchestrationRuns.model_id,
								})
								.from(OrchestrationRuns)
								.where(eq(OrchestrationRuns.run_id, run_id))
								.limit(1);
							if (thread_run !== undefined) return thread_run;
							const [agent_run] = yield* transaction
								.select({
									engine_id: AgentRuns.engine_id,
									model_id: AgentRuns.model_id,
								})
								.from(AgentRuns)
								.where(eq(AgentRuns.run_id, run_id))
								.limit(1);
							return agent_run;
						}),
					).pipe(
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
					const run_ids = usage.map((row) => row.run_id);
					const run_origins = new Map<
						string,
						{ engine_id: string; model_id: string | null }
					>();
					if (run_ids.length > 0) {
						const thread_runs = yield* transaction
							.select({
								engine_id: OrchestrationRuns.engine_id,
								model_id: OrchestrationRuns.model_id,
								run_id: OrchestrationRuns.run_id,
							})
							.from(OrchestrationRuns)
							.where(inArray(OrchestrationRuns.run_id, run_ids));
						const agent_runs = yield* transaction
							.select({
								engine_id: AgentRuns.engine_id,
								model_id: AgentRuns.model_id,
								run_id: AgentRuns.run_id,
							})
							.from(AgentRuns)
							.where(inArray(AgentRuns.run_id, run_ids));
						for (const run of [...thread_runs, ...agent_runs])
							run_origins.set(run.run_id, {
								engine_id: run.engine_id,
								model_id: run.model_id,
							});
					}
					/**
					 * Slices key on engine and model together; empty components mark
					 * usage whose run or model is no longer known. Identifiers never
					 * contain whitespace, so a space joins them unambiguously.
					 */
					const slice_key = (engine_id: string, model_id: string) =>
						`${engine_id} ${model_id}`;
					const totals = new Map<
						string,
						Map<string, { input_tokens: number; output_tokens: number }>
					>();

					for (const row of usage) {
						const date = row.updated_at.slice(0, 10);
						const origin = run_origins.get(row.run_id);
						const key = slice_key(origin?.engine_id ?? "", origin?.model_id ?? "");
						const day_slices = totals.get(date) ?? new Map();
						const running = day_slices.get(key) ?? {
							input_tokens: 0,
							output_tokens: 0,
						};
						day_slices.set(key, {
							input_tokens: running.input_tokens + (row.input_tokens ?? 0),
							output_tokens: running.output_tokens + (row.output_tokens ?? 0),
						});
						totals.set(date, day_slices);
					}

					const [last] = yield* transaction
						.select({ journal_sequence: JournalEvents.sequence })
						.from(JournalEvents)
						.orderBy(desc(JournalEvents.sequence))
						.limit(1);
					return yield* Schema.decodeUnknownEffect(SurfaceUsageDailySnapshot)({
						buckets: [...totals.entries()]
							.sort(([left], [right]) => left.localeCompare(right))
							.map(([date, day_slices]) => {
								const slices = [...day_slices.entries()]
									.map(([key, slice]) => {
										const [engine_id = "", model_id = ""] = key.split(" ");
										return {
											...(engine_id === "" ? {} : { engine_id }),
											...(model_id === "" ? {} : { model_id }),
											...slice,
										};
									})
									.sort(
										(left, right) =>
											right.input_tokens +
											right.output_tokens -
											(left.input_tokens + left.output_tokens),
									);
								return {
									date,
									engines: slices,
									input_tokens: slices.reduce(
										(sum, slice) => sum + slice.input_tokens,
										0,
									),
									output_tokens: slices.reduce(
										(sum, slice) => sum + slice.output_tokens,
										0,
									),
								};
							}),
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
		return SurfaceService.of({
			List: (input) =>
				List(input).pipe(
					Effect.mapError((error) => NormalizeSurfaceFailure(error, "Surface list")),
				),
			ListSnapshot: (input) =>
				ListSnapshot(input).pipe(
					Effect.mapError((error) => NormalizeSurfaceFailure(error, "Surface snapshot")),
				),
			Usage: (input) =>
				Usage(input).pipe(
					Effect.mapError((error) => NormalizeSurfaceFailure(error, "Surface usage")),
				),
			UsageSeries: (input) =>
				UsageSeries(input).pipe(
					Effect.mapError((error) =>
						NormalizeSurfaceFailure(error, "Surface usage series"),
					),
				),
			AggregateUsage: (input) =>
				AggregateUsage(input).pipe(
					Effect.mapError((error) =>
						NormalizeSurfaceFailure(error, "Surface usage aggregate"),
					),
				),
			AggregateUsageSnapshot: (input) =>
				AggregateUsageSnapshot(input).pipe(
					Effect.mapError((error) =>
						NormalizeSurfaceFailure(error, "Surface aggregate snapshot"),
					),
				),
			DailyUsageSnapshot: (input) =>
				DailyUsageSnapshot(input).pipe(
					Effect.mapError((error) =>
						NormalizeSurfaceFailure(error, "Surface daily usage"),
					),
				),
			UsageEventAffects: (input, run_id) =>
				UsageEventAffects(input, run_id).pipe(
					Effect.mapError((error) =>
						NormalizeSurfaceFailure(error, "Surface usage event lookup"),
					),
				),
			UsageScopeThread: (input) =>
				UsageScopeThread(input).pipe(
					Effect.mapError((error) =>
						NormalizeSurfaceFailure(error, "Surface usage scope lookup"),
					),
				),
		});
	}),
);
