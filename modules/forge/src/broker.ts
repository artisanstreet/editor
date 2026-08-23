import { execFile } from "node:child_process";
import { isAbsolute } from "node:path";

import { Data, Effect } from "effect";

const broker_timeout_ms = 5_000;
const broker_max_output_bytes = 1_024;

export class ArtisanBrokerFailure extends Data.TaggedError("ArtisanBrokerFailure")<{
	readonly cause?: unknown;
	readonly message: string;
	readonly reason: "blocked" | "invalid_output" | "unavailable";
}> {}

export type BrokerExecutor = (
	executable: string,
	environment: NodeJS.ProcessEnv,
) => Promise<{
	readonly exit_code: number | null;
	readonly stderr: string;
	readonly stdout: string;
}>;

const ExecuteBroker: BrokerExecutor = (executable, environment) =>
	new Promise((resolve, reject) => {
		execFile(
			executable,
			[],
			{
				encoding: "utf8",
				env: environment,
				maxBuffer: broker_max_output_bytes,
				timeout: broker_timeout_ms,
				windowsHide: true,
			},
			(error, stdout, stderr) => {
				if (error === null) {
					resolve({ exit_code: 0, stderr, stdout });
					return;
				}
				if (typeof error.code !== "number") {
					reject(error);
					return;
				}
				resolve({
					exit_code: error.code,
					stderr,
					stdout,
				});
			},
		);
	});

const BrokerEnvironment = (environment: NodeJS.ProcessEnv): NodeJS.ProcessEnv =>
	Object.fromEntries(
		Object.entries(environment).filter(([name]) => {
			const normalized = name.toUpperCase();
			return (
				normalized.startsWith("ARTISAN_BROKER_") ||
				["LANG", "LC_ALL", "LC_MESSAGES", "SYSTEMROOT", "WINDIR"].includes(normalized)
			);
		}),
	);

/**
 * Runs the native startup heuristic before Forge acquires any durable or
 * network resource. Development compositions may omit the Broker; installed
 * launchers set both the path and the required marker.
 */
export const EvaluateArtisanBroker = (
	environment: NodeJS.ProcessEnv = process.env,
	execute: BrokerExecutor = ExecuteBroker,
) =>
	Effect.gen(function* () {
		const executable = environment.ARTISAN_BROKER_PATH;
		const required = environment.ARTISAN_BROKER_REQUIRED === "1";
		if (executable === undefined || executable.length === 0) {
			if (required)
				return yield* new ArtisanBrokerFailure({
					message: "Artisan Broker is required but unavailable",
					reason: "unavailable",
				});
			return;
		}
		if (!isAbsolute(executable))
			return yield* new ArtisanBrokerFailure({
				message: "Artisan Broker path must be absolute",
				reason: "unavailable",
			});
		const result = yield* Effect.tryPromise({
			try: () => execute(executable, BrokerEnvironment(environment)),
			catch: (cause) =>
				new ArtisanBrokerFailure({
					cause,
					message: "Artisan Broker could not be executed",
					reason: "unavailable",
				}),
		});
		if (result.exit_code !== 0) {
			return yield* new ArtisanBrokerFailure({
				cause: new Error(
					result.stderr.trim() || `Artisan Broker exited ${String(result.exit_code)}`,
				),
				message: "Artisan Broker could not evaluate startup",
				reason: "unavailable",
			});
		}
		const decision = result.stdout.trim();
		if (decision === "true")
			return yield* new ArtisanBrokerFailure({
				message: "Startup was blocked by Artisan Broker",
				reason: "blocked",
			});
		if (decision !== "false")
			return yield* new ArtisanBrokerFailure({
				cause: new Error("Artisan Broker returned an invalid decision"),
				message: "Artisan Broker returned an invalid decision",
				reason: "invalid_output",
			});
	});
