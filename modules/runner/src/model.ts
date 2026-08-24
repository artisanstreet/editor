import type { Duration, Effect } from "effect";
import type { ChildProcess } from "effect/unstable/process";

export type DashboardMode = "always" | "auto" | "never";

export type LaneStatus = "failed" | "ready" | "running" | "starting" | "stopped" | "waiting";

export interface Endpoint {
	readonly label: string;
	readonly url: string;
}

export interface Lane {
	readonly id: string;
	readonly name: string;
	readonly status?: LaneStatus;
}

export interface ProcessOutput {
	readonly line: string;
	readonly process_id: string;
	readonly stream: "stderr" | "stdout";
}

export interface RoutedOutput {
	readonly lane_id: string;
	readonly line: string;
	readonly status?: LaneStatus;
}

export type OutputRouter = (
	output: ProcessOutput,
) => Effect.Effect<ReadonlyArray<RoutedOutput>, never>;

export type Readiness =
	| {
			/** Readiness is controlled entirely by route_output lane status events. */
			readonly _tag: "Manual";
	  }
	| {
			readonly _tag: "Immediate";
	  }
	| {
			readonly _tag: "Output";
			readonly pattern: RegExp;
			readonly stream: "either" | "stderr" | "stdout";
			readonly timeout: Duration.Input;
	  }
	| {
			readonly _tag: "Http";
			readonly interval: Duration.Input;
			readonly timeout: Duration.Input;
			readonly url: string;
	  };

export interface Process {
	readonly command: ChildProcess.Command;
	readonly id?: string;
	readonly lane_ids?: ReadonlyArray<string>;
	readonly name: string;
	readonly readiness?: Readiness;
	readonly route_output?: OutputRouter;
}

export interface Options {
	readonly dashboard?: DashboardMode;
	readonly endpoints?: ReadonlyArray<Endpoint>;
	readonly lanes?: ReadonlyArray<Lane>;
	readonly max_log_lines?: number;
	readonly title?: string;
}

export interface NormalizedProcess extends Process {
	readonly id: string;
	readonly lane_ids: ReadonlyArray<string>;
	readonly readiness: Readiness;
}

export interface Configuration {
	readonly dashboard: DashboardMode;
	readonly endpoints: ReadonlyArray<Endpoint>;
	readonly lanes: ReadonlyArray<Lane>;
	readonly max_log_lines: number;
	readonly processes: ReadonlyArray<NormalizedProcess>;
	readonly title: string;
}
