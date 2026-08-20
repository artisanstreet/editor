import { Context, Effect, Layer } from "effect";

import {
	type HostMachineConnectFailureReason,
	type HostMachineConnectOutcome,
} from "@artisan/protocol";

import {
	type HostIdentityCommandRunnerShape,
	NodeHostIdentityCommandRunner,
} from "./host-identity";

/**
 * A cold connect boots the distribution's VM and starts a fresh Forge before
 * `ae` can print its handoff, so the budget is minutes, not seconds. The
 * client's request deadline for `host.machines.connect.request` must stay
 * above this so the bounded failure produced here is what surfaces.
 */
const connect_timeout = "2 minutes";

/** Matches WSL distribution names safe to place in a spawned argv. */
const SafeDistributionName = /^[A-Za-z0-9._-]+$/;

/**
 * Builds the argv that runs `ae open --handoff` inside the distribution.
 * `ARTISAN_WSL_AE_COMMAND` overrides how `ae` is invoked in-distro (for
 * example `sh /root/artisan/ae.sh` for a staged development payload); the
 * default assumes a managed install put `ae` on the distribution's PATH.
 */
export function build_wsl_handoff_command(
	distribution: string,
	ae_command_override: string | undefined,
): { readonly args: ReadonlyArray<string>; readonly command: string } {
	const ae_tokens =
		ae_command_override === undefined
			? ["ae"]
			: ae_command_override.split(" ").filter((token) => token.length > 0);

	return {
		args: ["-d", distribution, "--", ...ae_tokens, "open", "--handoff"],
		command: "wsl.exe",
	};
}

/**
 * Extracts the handoff from `ae open --handoff` stdout: the first line that
 * parses as JSON carrying non-empty `endpoint` and `pair_code` strings.
 * Surrounding runtime chatter and interleaved NULs from `wsl.exe`'s own
 * UTF-16 diagnostics are tolerated rather than assumed absent.
 */
export function parse_handoff_output(
	raw_output: string,
): { readonly endpoint: string; readonly pair_code: string } | undefined {
	for (const raw_line of raw_output.replaceAll("\u0000", "").split("\n")) {
		const line = raw_line.trim();
		if (!line.startsWith("{")) continue;
		let parsed: unknown;
		try {
			parsed = JSON.parse(line);
		} catch {
			continue;
		}
		if (typeof parsed !== "object" || parsed === null) continue;
		const endpoint = (parsed as Record<string, unknown>)["endpoint"];
		const pair_code = (parsed as Record<string, unknown>)["pair_code"];
		if (typeof endpoint !== "string" || endpoint.length === 0) continue;
		if (typeof pair_code !== "string" || pair_code.length === 0) continue;
		return { endpoint, pair_code };
	}

	return undefined;
}

/**
 * The peer's endpoint must stay on this machine. A distribution is trusted to
 * run work, not to redirect the Editor to an arbitrary origin, so anything
 * other than a loopback HTTP endpoint is rejected as a failed start.
 */
export function is_loopback_endpoint(endpoint: string): boolean {
	let parsed: URL;
	try {
		parsed = new URL(endpoint);
	} catch {
		return false;
	}

	return (
		parsed.protocol === "http:" &&
		(parsed.hostname === "127.0.0.1" ||
			parsed.hostname === "localhost" ||
			parsed.hostname === "[::1]" ||
			parsed.hostname === "::1")
	);
}

/** Starts and pairs a selectable machine's Forge on behalf of the Editor. */
export class HostMachineBrokerService extends Context.Service<
	HostMachineBrokerService,
	{
		readonly Connect: (machine_id: string) => Effect.Effect<HostMachineConnectOutcome>;
	}
>()("Artisan/HostMachineBroker") {}

function failed(
	reason: HostMachineConnectFailureReason,
	message: string,
): HostMachineConnectOutcome {
	return { message, reason, status: "failed" };
}

/**
 * Builds the broker from an injectable platform command runner, so tests can
 * fake the `wsl.exe` boundary. Only `wsl:<distribution>` machines are
 * connectable today; the local machine needs no connect, and remote fleet
 * machines arrive with the registry transport.
 */
export function make_host_machine_broker_layer(
	command_runner: HostIdentityCommandRunnerShape = NodeHostIdentityCommandRunner,
): Layer.Layer<HostMachineBrokerService> {
	return Layer.sync(HostMachineBrokerService, () => {
		const Connect = (machine_id: string): Effect.Effect<HostMachineConnectOutcome> =>
			Effect.gen(function* () {
				if (!machine_id.startsWith("wsl:")) {
					return failed(
						"unknown_machine",
						`Machine "${machine_id}" is not a connectable WSL distribution.`,
					);
				}
				const distribution = machine_id.slice("wsl:".length);
				if (!SafeDistributionName.test(distribution)) {
					return failed(
						"unknown_machine",
						`"${distribution}" is not a valid WSL distribution name.`,
					);
				}
				if (process.platform !== "win32") {
					return failed(
						"start_failed",
						"WSL distributions are only reachable from a Windows-hosted Forge.",
					);
				}

				const descriptor = build_wsl_handoff_command(
					distribution,
					process.env["ARTISAN_WSL_AE_COMMAND"],
				);

				return yield* command_runner.Run(descriptor.command, descriptor.args).pipe(
					Effect.timeout(connect_timeout),
					Effect.match({
						onFailure: (cause) =>
							failed(
								"start_failed",
								`Starting Forge in "${distribution}" failed: ${
									cause instanceof Error ? cause.message : String(cause)
								}`,
							),
						onSuccess: (raw_output): HostMachineConnectOutcome => {
							const handoff = parse_handoff_output(raw_output);
							if (handoff === undefined) {
								return failed(
									"start_failed",
									`Forge in "${distribution}" started but printed no pairable handoff.`,
								);
							}
							if (!is_loopback_endpoint(handoff.endpoint)) {
								return failed(
									"start_failed",
									`Forge in "${distribution}" offered a non-loopback endpoint; refusing to pair.`,
								);
							}
							return {
								endpoint: handoff.endpoint,
								pair_code: handoff.pair_code,
								status: "connected",
							};
						},
					}),
				);
			});

		return HostMachineBrokerService.of({ Connect });
	});
}

/** Provides the broker using the real platform command runner. */
export const HostMachineBrokerLive = make_host_machine_broker_layer();
