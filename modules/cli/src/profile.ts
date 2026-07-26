import { randomBytes } from "node:crypto";

import { Context, Data, Effect, Layer, Schema } from "effect";

export const ForgeMode = Schema.Literals(["local", "headless"]);
export type ForgeMode = typeof ForgeMode.Type;

/** A profile is a single path component, never a path or a Windows device name. */
export const ForgeProfileName = Schema.String.check(
	Schema.isPattern(
		/^(?!CON$|PRN$|AUX$|NUL$|COM[1-9]$|LPT[1-9]$)[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/i,
	),
);

const IsoDateTime = Schema.String.check(
	Schema.isPattern(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/),
	Schema.makeFilter<string>((input) => {
		const parsed = new Date(input);
		const normalized = input.includes(".") ? input : input.replace("Z", ".000Z");
		return Number.isNaN(parsed.getTime()) || parsed.toISOString() !== normalized
			? "Expected a real ISO 8601 UTC timestamp"
			: undefined;
	}),
);

const LoopbackHttpEndpoint = Schema.String.check(
	Schema.isMaxLength(256),
	Schema.makeFilter<string>((input) => {
		try {
			const url = new URL(input);
			return url.protocol === "http:" &&
				(url.hostname === "127.0.0.1" || url.hostname === "::1") &&
				url.username === "" &&
				url.password === "" &&
				(url.pathname === "/" || url.pathname === "") &&
				url.search === "" &&
				url.hash === "" &&
				url.port !== ""
				? undefined
				: "Expected a loopback HTTP endpoint with an explicit port";
		} catch {
			return "Expected a loopback HTTP endpoint";
		}
	}),
);

export const ForgeProfileConfig = Schema.Struct({
	data_root: Schema.String.check(Schema.isMaxLength(4_096)),
	listen_host: Schema.Literals(["127.0.0.1", "::1"]),
	listen_port: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 65_535 })),
	mode: ForgeMode,
	project_roots: Schema.NonEmptyArray(
		Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(4_096)),
	),
	version: Schema.Literal(1),
});
export type ForgeProfileConfig = typeof ForgeProfileConfig.Type;

export const ForgeRuntimeState = Schema.Struct({
	endpoint: LoopbackHttpEndpoint,
	instance_id: Schema.String.check(
		Schema.isPattern(
			/^(?:forge_[0-9]+|[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$/i,
		),
	),
	pid: Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: Number.MAX_SAFE_INTEGER })),
	profile: ForgeProfileName,
	started_at: IsoDateTime,
	version: Schema.Literal(1),
});
export type ForgeRuntimeState = typeof ForgeRuntimeState.Type;

export const ForgeSecrets = Schema.Struct({
	auth_token: Schema.String.check(Schema.isPattern(/^[A-Za-z0-9_-]{43}$/)),
	version: Schema.Literal(1),
});
export type ForgeSecrets = typeof ForgeSecrets.Type;

export class ForgeProfileError extends Data.TaggedError("ForgeProfileError")<{
	readonly cause?: unknown;
	readonly code: "invalid" | "missing" | "unsafe_listener";
}> {}

export interface ForgeProfilePaths {
	readonly config_path: string;
	readonly log_path: string;
	readonly readiness_path: string;
	readonly secrets_path: string;
	readonly state_path: string;
}

export class ForgeProfileStore extends Context.Service<
	ForgeProfileStore,
	{
		readonly Ensure: (
			profile: string,
			config: ForgeProfileConfig,
		) => Effect.Effect<void, ForgeProfileError>;
		readonly Load: (profile: string) => Effect.Effect<ForgeProfileConfig, ForgeProfileError>;
		readonly LoadSecrets: (profile: string) => Effect.Effect<ForgeSecrets, ForgeProfileError>;
		readonly ReadState: (
			profile: string,
		) => Effect.Effect<ForgeRuntimeState | undefined, ForgeProfileError>;
		/** Deletes only the exact state record observed by the caller. */
		readonly RemoveStateIfOwned: (
			profile_name: string,
			instance_id: string,
		) => Effect.Effect<boolean, ForgeProfileError>;
		readonly Paths: (profile: string) => Effect.Effect<ForgeProfilePaths, ForgeProfileError>;
	}
>()("Artisan/ForgeProfileStore") {}

export const GenerateForgeToken = () => randomBytes(32).toString("base64url");

/** Validates the public config boundary; direct TLS and public binds are V1-invalid. */
export const DecodeForgeProfileConfig = (input: unknown) =>
	Schema.decodeUnknownSync(ForgeProfileConfig)(input);

/** A deterministic in-memory store is intentionally exported for lifecycle tests. */
export const make_memory_profile_store = () => {
	const configs = new Map<string, ForgeProfileConfig>();
	const secrets = new Map<string, ForgeSecrets>();
	const states = new Map<string, ForgeRuntimeState>();
	const paths = (profile: string): ForgeProfilePaths => ({
		config_path: `${profile}/config.json`,
		log_path: `${profile}/forge.log`,
		readiness_path: `${profile}/ready.json`,
		secrets_path: `${profile}/secrets.json`,
		state_path: `${profile}/state.json`,
	});
	return Layer.succeed(
		ForgeProfileStore,
		ForgeProfileStore.of({
			Ensure: (profile, config) =>
				Effect.sync(() => {
					configs.set(profile, DecodeForgeProfileConfig(config));
					if (!secrets.has(profile))
						secrets.set(profile, { auth_token: GenerateForgeToken(), version: 1 });
				}),
			Load: (profile) => {
				const config = configs.get(profile);
				return config === undefined
					? Effect.fail(new ForgeProfileError({ code: "missing" }))
					: Effect.succeed(config);
			},
			LoadSecrets: (profile) => {
				const secret = secrets.get(profile);
				return secret === undefined
					? Effect.fail(new ForgeProfileError({ code: "missing" }))
					: Effect.succeed(secret);
			},
			ReadState: (profile) => Effect.succeed(states.get(profile)),
			RemoveStateIfOwned: (profile, expected_instance_id) =>
				Effect.sync(() => {
					const state = states.get(profile);
					if (state?.instance_id !== expected_instance_id) return false;
					states.delete(profile);
					return true;
				}),
			Paths: (profile) => Effect.succeed(paths(profile)),
		}),
	);
};
