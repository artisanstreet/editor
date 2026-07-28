import { Schema } from "effect";

/**
 * The host accepts only loopback listeners by default. Remote use is reached
 * through SSH forwarding until the authenticated TLS listener exists.
 */
export const ForgeConfigSchema = Schema.Struct({
	allowed_origins: Schema.Array(Schema.String),
	auth_token: Schema.optional(Schema.String.check(Schema.isMinLength(32))),
	database_path: Schema.String,
	instance_id: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(128)),
	listen_host: Schema.Union([Schema.Literal("127.0.0.1"), Schema.Literal("::1")]),
	listen_port: Schema.Int.check(Schema.isBetween({ maximum: 65_535, minimum: 0 })),
	migrations_path: Schema.String,
	/**
	 * Machine-global root the instance registry lives under. Absent in embedded
	 * compositions that have no announcement duty, such as tests.
	 */
	instance_registry_root: Schema.optional(Schema.String),
	static_frontend_root: Schema.optional(Schema.String),
	websocket_path: Schema.Literal("/api/ws"),
});

export type ForgeConfig = typeof ForgeConfigSchema.Type;

export type ForgeConfigInput = Readonly<{
	readonly database_path: string;
	readonly instance_id: string;
	readonly allowed_origins?: ReadonlyArray<string>;
	readonly auth_token?: string;
	readonly listen_host?: "127.0.0.1" | "::1";
	readonly listen_port?: number;
	readonly instance_registry_root?: string;
	readonly migrations_path: string;
	readonly static_frontend_root?: string;
}>;

/** Schema-decodes a concrete host configuration before any native resource is opened. */
export function decode_forge_config(input: ForgeConfigInput): ForgeConfig {
	return Schema.decodeUnknownSync(ForgeConfigSchema)({
		...input,
		allowed_origins: input.allowed_origins ?? ["artisan://app"],
		listen_host: input.listen_host ?? "127.0.0.1",
		listen_port: input.listen_port ?? 0,
		websocket_path: "/api/ws",
	});
}
