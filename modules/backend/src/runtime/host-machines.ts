import { hostname as os_hostname } from "node:os";

import { Context, Effect, Layer } from "effect";

import {
	type HostMachineSnapshot,
	type HostMachinesSnapshot,
	type HostPlatform,
} from "@artisan/protocol";

import {
	type HostIdentityCommandRunnerShape,
	NodeHostIdentityCommandRunner,
} from "./host-identity";

const enumeration_timeout = "5 seconds";

/**
 * Distributions WSL registers for container tooling rather than user work;
 * they are not meaningful execution targets for a thread.
 */
const UtilityDistributions = new Set([
	"docker-desktop",
	"docker-desktop-data",
	"rancher-desktop",
	"rancher-desktop-data",
]);

/** Maps Node's `process.platform` to the protocol's closed platform literal. */
function map_host_platform(value: string): HostPlatform {
	return value === "win32" || value === "darwin" || value === "linux" ? value : "unknown";
}

/**
 * Extracts distribution names from `wsl.exe -l -q` stdout. The command emits
 * UTF-16LE, which a UTF-8 capture renders as interleaved NUL bytes, so the
 * parser strips NULs and BOMs before splitting rather than assuming a clean
 * encoding.
 */
export function parse_wsl_distributions(raw_output: string): ReadonlyArray<string> {
	return raw_output
		.replaceAll("\u0000", "")
		.replaceAll("\uFEFF", "")
		.split("\n")
		.map((line) => line.replaceAll("\r", "").trim())
		.filter((line) => line.length > 0)
		.filter((line) => !UtilityDistributions.has(line.toLowerCase()));
}

/**
 * Builds the machine list for one Forge host. The machine hosting Forge is
 * always first: labelled "This computer" normally, or "This computer on WSL2"
 * when Forge itself runs inside a distribution (`wsl_distribution` carries the
 * `WSL_DISTRO_NAME` value in that case). Windows hosts additionally surface
 * each discovered distribution as a selectable `wsl` machine.
 */
export function build_machines_snapshot(
	platform: HostPlatform,
	hostname: string,
	wsl_distribution: string | undefined,
	distributions: ReadonlyArray<string>,
): HostMachinesSnapshot {
	const local: HostMachineSnapshot =
		platform === "linux" && wsl_distribution !== undefined
			? {
					detail: wsl_distribution,
					id: "local",
					kind: "local",
					label: "This computer on WSL2",
				}
			: { detail: hostname, id: "local", kind: "local", label: "This computer" };

	const wsl_machines: ReadonlyArray<HostMachineSnapshot> =
		platform === "win32"
			? distributions.map((distribution) => ({
					detail: distribution,
					id: `wsl:${distribution}`,
					kind: "wsl" as const,
					label: "This computer on WSL2",
				}))
			: [];

	return { machines: [local, ...wsl_machines] };
}

/** Exposes the execution machines reachable from the machine hosting Forge. */
export class HostMachinesService extends Context.Service<
	HostMachinesService,
	{
		readonly List: Effect.Effect<HostMachinesSnapshot>;
	}
>()("Artisan/HostMachines") {}

/**
 * Builds the machines service from an injectable platform command runner, so
 * tests can fake `wsl.exe` output without spawning processes. Enumeration runs
 * per query rather than being cached: distributions can be installed or
 * removed while Forge runs, and the listing is one short-lived process.
 */
export function make_host_machines_layer(
	command_runner: HostIdentityCommandRunnerShape = NodeHostIdentityCommandRunner,
): Layer.Layer<HostMachinesService> {
	return Layer.sync(HostMachinesService, () => {
		const List = Effect.gen(function* () {
			const platform = map_host_platform(process.platform);
			const wsl_distribution = process.env["WSL_DISTRO_NAME"];
			const distributions =
				platform === "win32"
					? yield* command_runner.Run("wsl.exe", ["-l", "-q"]).pipe(
							Effect.timeout(enumeration_timeout),
							Effect.map(parse_wsl_distributions),
							Effect.match({
								onFailure: (): ReadonlyArray<string> => [],
								onSuccess: (value) => value,
							}),
						)
					: [];

			return build_machines_snapshot(
				platform,
				os_hostname(),
				wsl_distribution,
				distributions,
			);
		});

		return HostMachinesService.of({ List });
	});
}

/** Provides the machines service using the real platform command runner. */
export const HostMachinesLive = make_host_machines_layer();
