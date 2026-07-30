import { Context, Data, Effect } from "effect";

import {
	SurfaceSnapshot,
	SurfaceUsage,
	SurfaceUsageAggregate,
	SurfaceUsageDailySnapshot,
} from "@artisan/protocol";

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
