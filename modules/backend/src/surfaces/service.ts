import { Clock, Effect, Layer, Schema } from "effect";
import { and, asc, desc, eq, gte, inArray, lte } from "drizzle-orm";

import {
	SurfaceItem,
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

export const SurfaceServiceLive = Layer.effect(
	SurfaceService,
	Effect.gen(function* () {
		const database = yield* Database;
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
