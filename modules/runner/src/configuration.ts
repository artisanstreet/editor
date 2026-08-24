import { Effect } from "effect";

import { ConfigurationError } from "./error.ts";
import type {
	Configuration,
	Lane,
	NormalizedProcess,
	Options,
	Process,
	Readiness,
} from "./model.ts";

const DefaultReadiness: Readiness = { _tag: "Immediate" };

const IsPositiveInteger = (value: unknown): value is number =>
	typeof value === "number" && Number.isInteger(value) && value > 0;

const IsNonEmptyString = (value: string): boolean => value.trim().length > 0;

const FailConfiguration = (message: string) => Effect.fail(new ConfigurationError({ message }));

const NormalizeProcess = (
	process: Process,
	index: number,
): Effect.Effect<NormalizedProcess, ConfigurationError> =>
	Effect.gen(function* () {
		if (!IsNonEmptyString(process.name))
			return yield* FailConfiguration(`Process ${index + 1} must have a non-empty name`);

		const id = process.id ?? process.name;
		if (!IsNonEmptyString(id))
			return yield* FailConfiguration(`Process ${index + 1} must have a non-empty id`);

		const lane_ids = process.lane_ids ?? [id];
		if (lane_ids.length === 0 || lane_ids.some((lane_id) => !IsNonEmptyString(lane_id)))
			return yield* FailConfiguration(
				`Process ${id} must target at least one non-empty lane id`,
			);

		if (
			process.readiness?._tag === "Output" &&
			typeof process.readiness.timeout === "number" &&
			!IsPositiveInteger(process.readiness.timeout)
		)
			return yield* FailConfiguration(
				`Output readiness for ${id} needs a positive numeric timeout`,
			);
		if (process.readiness?._tag === "Http") {
			if (
				typeof process.readiness.timeout === "number" &&
				!IsPositiveInteger(process.readiness.timeout)
			)
				return yield* FailConfiguration(
					`HTTP readiness for ${id} needs a positive numeric timeout`,
				);
			if (
				typeof process.readiness.interval === "number" &&
				!IsPositiveInteger(process.readiness.interval)
			)
				return yield* FailConfiguration(
					`HTTP readiness for ${id} needs a positive numeric interval`,
				);
			try {
				new URL(process.readiness.url);
			} catch {
				return yield* FailConfiguration(`HTTP readiness for ${id} needs an absolute URL`);
			}
		}

		return { ...process, id, lane_ids, readiness: process.readiness ?? DefaultReadiness };
	});

/** Decodes runner-owned invariants while preserving Effect child command objects intact. */
export const NormalizeConfiguration = (
	processes: ReadonlyArray<Process>,
	options: Options = {},
): Effect.Effect<Configuration, ConfigurationError> =>
	Effect.gen(function* () {
		if (processes.length === 0)
			return yield* FailConfiguration("Runner requires at least one process");
		if (options.max_log_lines !== undefined && !IsPositiveInteger(options.max_log_lines))
			return yield* FailConfiguration("max_log_lines must be a positive integer");

		const normalized_processes = yield* Effect.forEach(processes, NormalizeProcess);
		const ids = new Set<string>();
		for (const process of normalized_processes) {
			if (ids.has(process.id))
				return yield* FailConfiguration(`Duplicate process id: ${process.id}`);
			ids.add(process.id);
		}

		const lanes: ReadonlyArray<Lane> =
			options.lanes ??
			normalized_processes.map((process) => ({
				id: process.id,
				name: process.name,
				status: "waiting",
			}));
		const lane_ids = new Set(lanes.map((lane) => lane.id));
		if (lanes.some((lane) => !IsNonEmptyString(lane.id) || !IsNonEmptyString(lane.name)))
			return yield* FailConfiguration("Every lane must have a non-empty id and name");
		if (lane_ids.size !== lanes.length)
			return yield* FailConfiguration("Every lane must have a unique id");
		for (const process of normalized_processes)
			for (const lane_id of process.lane_ids)
				if (!lane_ids.has(lane_id))
					return yield* FailConfiguration(
						`Process ${process.id} targets unknown lane ${lane_id}`,
					);

		return {
			dashboard: options.dashboard ?? "auto",
			endpoints: options.endpoints ?? [],
			lanes,
			max_log_lines: options.max_log_lines ?? 1_000,
			processes: normalized_processes,
			title: options.title ?? "Runner",
		};
	});
