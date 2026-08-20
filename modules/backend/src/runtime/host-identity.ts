import { spawn } from "node:child_process";
import { hostname as os_hostname, userInfo } from "node:os";

import { Context, Effect, Layer } from "effect";

import { type HostIdentitySnapshot, type HostPlatform } from "@artisan/protocol";

/** Runs one short-lived platform command and returns its captured stdout. */
export interface HostIdentityCommandRunnerShape {
	readonly Run: (command: string, args: ReadonlyArray<string>) => Effect.Effect<string, unknown>;
}

/** Matches OS account names safe to interpolate into the Windows CIM filter. */
const SafeWindowsUsername = /^[A-Za-z0-9 ._-]+$/;

const display_name_timeout = "5 seconds";

/** Maps Node's `process.platform` to the protocol's closed platform literal. */
function map_host_platform(value: string): HostPlatform {
	return value === "win32" || value === "darwin" || value === "linux" ? value : "unknown";
}

/**
 * Builds the platform-specific command that resolves the human profile name for
 * `username`. Returns `undefined` when the platform has no known lookup, no
 * username is available, or (on Windows) the username fails the interpolation
 * safety check — `Get-CimInstance`'s `-Filter` string is otherwise the only
 * place user-controlled text reaches a spawned command.
 */
export function build_display_name_command(
	platform: HostPlatform,
	username: string | undefined,
): { readonly args: ReadonlyArray<string>; readonly command: string } | undefined {
	if (username === undefined) {
		return undefined;
	}

	if (platform === "win32") {
		if (!SafeWindowsUsername.test(username)) {
			return undefined;
		}

		const script = `(Get-CimInstance Win32_UserAccount -Filter "Name='${username}'" | Select-Object -First 1).FullName`;

		return {
			args: ["-NoProfile", "-NonInteractive", "-Command", script],
			command: "powershell.exe",
		};
	}

	if (platform === "darwin") {
		return { args: ["-F"], command: "id" };
	}

	if (platform === "linux") {
		return { args: ["passwd", username], command: "getent" };
	}

	return undefined;
}

/**
 * Extracts the human display name from a resolved command's stdout. `getent`
 * on Linux prints the full `passwd` row, so the name is the GECOS field (the
 * 5th `:`-separated column) up to its first `,`; every other platform's
 * command prints the name directly.
 */
export function parse_display_name(platform: HostPlatform, raw_output: string): string | undefined {
	const trimmed = raw_output.trim();

	if (trimmed.length === 0) {
		return undefined;
	}

	if (platform === "linux") {
		const gecos = trimmed.split(":")[4]?.split(",")[0]?.trim();

		return gecos !== undefined && gecos.length > 0 ? gecos : undefined;
	}

	return trimmed;
}

/** Resolves the OS profile display name; any failure or timeout yields `undefined`. */
function ResolveDisplayName(
	platform: HostPlatform,
	username: string | undefined,
	command_runner: HostIdentityCommandRunnerShape,
): Effect.Effect<string | undefined> {
	const descriptor = build_display_name_command(platform, username);

	if (descriptor === undefined) {
		return Effect.succeed(undefined);
	}

	return command_runner.Run(descriptor.command, descriptor.args).pipe(
		Effect.timeout(display_name_timeout),
		Effect.map((raw_output) => parse_display_name(platform, raw_output)),
		Effect.match({ onFailure: () => undefined, onSuccess: (value) => value }),
	);
}

/** Exposes the signed-in OS account identity of the machine hosting Forge. */
export class HostIdentityService extends Context.Service<
	HostIdentityService,
	{
		readonly Get: Effect.Effect<HostIdentitySnapshot>;
	}
>()("Artisan/HostIdentity") {}

/** Spawns a shell-free child process to capture one platform command's stdout. */
export const NodeHostIdentityCommandRunner: HostIdentityCommandRunnerShape = {
	Run: (command, args) =>
		Effect.callback<string, Error>((resume) => {
			let settled = false;
			let stdout = "";
			const child = spawn(command, [...args], { shell: false, windowsHide: true });

			const finish = (result: Effect.Effect<string, Error>) => {
				if (settled) {
					return;
				}

				settled = true;
				child.stdout?.removeAllListeners("data");
				child.removeAllListeners("error");
				child.removeAllListeners("close");
				resume(result);
			};

			child.stdout?.on("data", (chunk: Buffer | string) => {
				stdout += chunk.toString();
			});
			child.once("error", (cause) => finish(Effect.fail(cause)));
			child.once("close", (exit_code) =>
				finish(
					exit_code === 0
						? Effect.succeed(stdout)
						: Effect.fail(new Error(`${command} exited with code ${exit_code ?? -1}`)),
				),
			);

			return Effect.sync(() => {
				if (settled) {
					return;
				}

				settled = true;
				child.kill();
			});
		}),
};

/**
 * Builds the host identity service from an injectable platform command
 * runner, so tests can fake platform command output without spawning
 * processes. The resolved snapshot is cached for the life of the returned
 * layer's service instance, since the signed-in OS identity does not change
 * while Forge runs.
 */
export function make_host_identity_layer(
	command_runner: HostIdentityCommandRunnerShape = NodeHostIdentityCommandRunner,
): Layer.Layer<HostIdentityService> {
	return Layer.effect(
		HostIdentityService,
		Effect.gen(function* () {
			const ResolveSnapshot = Effect.gen(function* () {
				const platform = map_host_platform(process.platform);
				const username = yield* Effect.try(() => userInfo().username).pipe(
					Effect.match({ onFailure: () => undefined, onSuccess: (value) => value }),
				);
				const display_name = yield* ResolveDisplayName(platform, username, command_runner);

				return {
					...(display_name === undefined ? {} : { display_name }),
					hostname: os_hostname(),
					platform,
					...(username === undefined ? {} : { username }),
				} satisfies HostIdentitySnapshot;
			});

			const Get = yield* Effect.cached(ResolveSnapshot);

			return HostIdentityService.of({ Get });
		}),
	);
}

/** Provides the host identity service using the real platform command runner. */
export const HostIdentityLive = make_host_identity_layer();
