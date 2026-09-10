import { Data } from "effect";

export class ConfigurationError extends Data.TaggedError("ConfigurationError")<{
	readonly message: string;
}> {}

export class DashboardError extends Data.TaggedError("DashboardError")<{
	readonly cause: unknown;
	readonly operation: "create" | "render";
}> {}

export class ProcessError extends Data.TaggedError("ProcessError")<{
	readonly cause: unknown;
	readonly operation: "output" | "spawn";
	readonly process_id: string;
}> {}

export class ProcessExitedError extends Data.TaggedError("ProcessExitedError")<{
	readonly exit_code: number;
	readonly process_id: string;
}> {}

export class ReadinessError extends Data.TaggedError("ReadinessError")<{
	readonly cause: unknown;
	readonly process_id: string;
	readonly readiness: "http" | "output";
}> {}

export type RunnerError = ConfigurationError | ProcessError | ProcessExitedError | ReadinessError;
