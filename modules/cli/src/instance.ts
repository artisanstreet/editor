import { randomBytes } from "node:crypto";

import { Context, Data, Effect, Layer, Schema } from "effect";

export const ForgeMode = Schema.Literals(["local", "headless"]);
export type ForgeMode = typeof ForgeMode.Type;

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

export const ForgeInstanceConfig = Schema.Struct({
	data_root: Schema.String.check(Schema.isMaxLength(4_096)),
	listen_host: Schema.Literals(["127.0.0.1", "::1"]),
	listen_port: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 65_535 })),
	mode: ForgeMode,
	/**
	 * Static web hosting is a development capability. Absent or false, the
	 * launched Forge exposes only health and control/WS surfaces; installed
	 * homes never enable it and render through the Electron editor.
	 */
	serve_frontend: Schema.optional(Schema.Boolean),
	version: Schema.Literal(1),
});
export type ForgeInstanceConfig = typeof ForgeInstanceConfig.Type;

export const ForgeRuntimeState = Schema.Struct({
	endpoint: LoopbackHttpEndpoint,
	instance_id: Schema.String.check(
		Schema.isPattern(
			/^(?:forge_[0-9]+|[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$/i,
		),
	),
	pid: Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: Number.MAX_SAFE_INTEGER })),
	started_at: IsoDateTime,
	version: Schema.Literal(1),
});
export type ForgeRuntimeState = typeof ForgeRuntimeState.Type;

export const ForgeSecrets = Schema.Struct({
	auth_token: Schema.String.check(Schema.isPattern(/^[A-Za-z0-9_-]{43}$/)),
	version: Schema.Literal(1),
});
export type ForgeSecrets = typeof ForgeSecrets.Type;

export class ForgeInstanceError extends Data.TaggedError("ForgeInstanceError")<{
	readonly cause?: unknown;
	readonly code: "invalid" | "legacy_profiles" | "missing" | "unsafe_listener";
}> {}

export interface ForgeInstancePaths {
	readonly config_path: string;
	readonly log_path: string;
	readonly readiness_path: string;
	readonly secrets_path: string;
	readonly state_path: string;
}

/** The home's single Forge instance: its files live at the Artisan home root. */
export class ForgeInstanceStore extends Context.Service<
	ForgeInstanceStore,
	{
		readonly Ensure: (config: ForgeInstanceConfig) => Effect.Effect<void, ForgeInstanceError>;
		readonly Load: () => Effect.Effect<ForgeInstanceConfig, ForgeInstanceError>;
		readonly LoadSecrets: () => Effect.Effect<ForgeSecrets, ForgeInstanceError>;
		readonly ReadState: () => Effect.Effect<ForgeRuntimeState | undefined, ForgeInstanceError>;
		/** Deletes only the exact state record observed by the caller. */
		readonly RemoveStateIfOwned: (
			instance_id: string,
		) => Effect.Effect<boolean, ForgeInstanceError>;
		readonly Paths: () => Effect.Effect<ForgeInstancePaths, ForgeInstanceError>;
	}
>()("Artisan/ForgeInstanceStore") {}

export const GenerateForgeToken = () => randomBytes(32).toString("base64url");

/** Validates the public config boundary; direct TLS and public binds are V1-invalid. */
export const DecodeForgeInstanceConfig = (input: unknown) =>
	Schema.decodeUnknownSync(ForgeInstanceConfig)(input);

/** A deterministic in-memory store is intentionally exported for lifecycle tests. */
export const make_memory_instance_store = () => {
	let config: ForgeInstanceConfig | undefined;
	let secrets: ForgeSecrets | undefined;
	let state: ForgeRuntimeState | undefined;
	const paths: ForgeInstancePaths = {
		config_path: "home/config.json",
		log_path: "home/forge.log",
		readiness_path: "home/ready.json",
		secrets_path: "home/secrets.json",
		state_path: "home/state.json",
	};
	return Layer.succeed(
		ForgeInstanceStore,
		ForgeInstanceStore.of({
			Ensure: (input) =>
				Effect.sync(() => {
					config = DecodeForgeInstanceConfig(input);
					secrets ??= { auth_token: GenerateForgeToken(), version: 1 };
				}),
			Load: () =>
				config === undefined
					? Effect.fail(new ForgeInstanceError({ code: "missing" }))
					: Effect.succeed(config),
			LoadSecrets: () =>
				secrets === undefined
					? Effect.fail(new ForgeInstanceError({ code: "missing" }))
					: Effect.succeed(secrets),
			ReadState: () => Effect.succeed(state),
			RemoveStateIfOwned: (expected_instance_id) =>
				Effect.sync(() => {
					if (state?.instance_id !== expected_instance_id) return false;
					state = undefined;
					return true;
				}),
			Paths: () => Effect.succeed(paths),
		}),
	);
};
