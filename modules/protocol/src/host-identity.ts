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
