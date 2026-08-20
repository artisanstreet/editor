import { Schema } from "effect";

/** Names the host operating system family that produced an identity snapshot. */
export const HostPlatform = Schema.Literals(["win32", "darwin", "linux", "unknown"]);

export type HostPlatform = typeof HostPlatform.Type;

/**
 * Projects the signed-in OS account for the machine hosting Forge.
 *
 * `display_name` carries the human profile name when the platform provides one
 * (Windows account full name, macOS RealName, Linux GECOS). `username` is the
 * raw OS account name. `hostname` always carries the machine identifier (for
 * example `DESKTOP-123123`) so clients can fall back when the profile carries
 * no human name.
 */
export const HostIdentitySnapshot = Schema.Struct({
	display_name: Schema.optional(Schema.String.check(Schema.isMinLength(1))),
	hostname: Schema.String.check(Schema.isMinLength(1)),
	platform: HostPlatform,
	username: Schema.optional(Schema.String.check(Schema.isMinLength(1))),
});

export type HostIdentitySnapshot = typeof HostIdentitySnapshot.Type;

/** Distinguishes the machine kinds a thread can execute on. */
export const HostMachineKind = Schema.Literals(["local", "wsl"]);

export type HostMachineKind = typeof HostMachineKind.Type;

/**
 * One selectable execution machine. `local` is the machine hosting Forge
 * itself; `wsl` entries are WSL2 distributions discovered on a Windows host.
 * `label` is the primary display text ("This computer"); `detail` carries the
 * secondary line (the hostname for `local`, the distribution name for `wsl`).
 */
export const HostMachineSnapshot = Schema.Struct({
	detail: Schema.optional(Schema.String.check(Schema.isMinLength(1))),
	id: Schema.String.check(Schema.isMinLength(1)),
	kind: HostMachineKind,
	label: Schema.String.check(Schema.isMinLength(1)),
});

export type HostMachineSnapshot = typeof HostMachineSnapshot.Type;

/** Machines available to execute threads, local machine first. */
export const HostMachinesSnapshot = Schema.Struct({
	machines: Schema.Array(HostMachineSnapshot),
});

export type HostMachinesSnapshot = typeof HostMachinesSnapshot.Type;

/** Asks Forge to start and pair the named machine's own Forge instance. */
export const HostMachineConnectRequest = Schema.Struct({
	machine_id: Schema.String.check(Schema.isMinLength(1)),
});

export type HostMachineConnectRequest = typeof HostMachineConnectRequest.Type;

/** Why a machine connect request could not produce a pairable endpoint. */
export const HostMachineConnectFailureReason = Schema.Literals(["unknown_machine", "start_failed"]);

export type HostMachineConnectFailureReason = typeof HostMachineConnectFailureReason.Type;

/**
 * Terminal outcome of one connect request. A connected outcome carries the
 * peer Forge's loopback endpoint plus a fresh single-use pair code; the client
 * completes pairing against that endpoint itself, exactly as it would for its
 * own Forge — the broker never proxies the peer's traffic.
 */
export const HostMachineConnectOutcome = Schema.Union([
	Schema.Struct({
		endpoint: Schema.String.check(Schema.isMinLength(1)),
		pair_code: Schema.String.check(Schema.isMinLength(1)),
		status: Schema.Literal("connected"),
	}),
	Schema.Struct({
		message: Schema.String.check(Schema.isMinLength(1)),
		reason: HostMachineConnectFailureReason,
		status: Schema.Literal("failed"),
	}),
]);

export type HostMachineConnectOutcome = typeof HostMachineConnectOutcome.Type;
