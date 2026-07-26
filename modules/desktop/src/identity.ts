import { hostname, userInfo } from "node:os";
import { Effect } from "effect";

import type { DesktopIdentity } from "./contracts";

const normalize_identity_text = (value: string | undefined, fallback: string) => {
	const normalized = [...(value ?? "")]
		.filter((character) => {
			const code_point = character.codePointAt(0) ?? 0;
			return code_point > 0x1f && code_point !== 0x7f;
		})
		.join("")
		.trim();
	return normalized && normalized.length > 0 ? normalized.slice(0, 120) : fallback;
};

/** Produces a renderer-safe, non-authoritative local identity without exposing OS APIs to web code. */
export const resolve_desktop_identity = (input: {
	readonly avatar_data_url?: string;
	readonly machine_name: string | undefined;
	readonly username: string | undefined;
}): DesktopIdentity => {
	const machine_name = normalize_identity_text(input.machine_name, "This device");
	const display_name = normalize_identity_text(input.username, machine_name);

	return {
		...(input.avatar_data_url === undefined ? {} : { avatar_data_url: input.avatar_data_url }),
		avatar_seed: `${display_name}:${machine_name}`,
		display_name,
		machine_name,
	};
};

/** Reads only the stable local account and hostname; avatar acquisition remains deliberately optional. */
export const read_desktop_identity = Effect.try({
	try: () =>
		resolve_desktop_identity({
			machine_name: hostname(),
			username: userInfo().username,
		}),
	catch: () => resolve_desktop_identity({ machine_name: undefined, username: undefined }),
}).pipe(Effect.catch((identity) => Effect.succeed(identity)));
