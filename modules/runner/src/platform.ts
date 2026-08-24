import { Context, type Effect, type Scope } from "effect";
import type { ChildProcessSpawner } from "effect/unstable/process";

import type { DashboardError } from "./error.ts";
import type { Configuration, LaneStatus } from "./model.ts";

export interface Dashboard {
	readonly AwaitQuit: Effect.Effect<never>;
	readonly Log: (lane_id: string, line: string) => Effect.Effect<void>;
	readonly SetStatus: (lane_id: string, status: LaneStatus) => Effect.Effect<void>;
}

export class DashboardFactory extends Context.Service<
	DashboardFactory,
	{
		readonly Make: (
			configuration: Configuration,
		) => Effect.Effect<
			Dashboard,
			DashboardError,
			ChildProcessSpawner.ChildProcessSpawner | Scope.Scope
		>;
	}
>()("@artisanstreet/runner/DashboardFactory") {}
