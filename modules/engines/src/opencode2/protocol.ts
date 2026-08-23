import { Buffer } from "node:buffer";

import { Data, Effect, Schema, Stream } from "effect";

import type { EngineCatalogScope, EngineModelSelection } from "../engine";

const maximum_json_bytes = 8 * 1024 * 1024;
const maximum_sse_event_bytes = 8 * 1024 * 1024;

export class OpenCode2ApiError extends Data.TaggedError("OpenCode2ApiError")<{
	readonly code: "decode" | "http" | "network" | "protocol";
	readonly operation: string;
	readonly status?: number;
}> {}

const HealthResponse = Schema.Struct({
	healthy: Schema.Literal(true),
	pid: Schema.Int,
	version: Schema.NonEmptyString,
});

const OpenCode2Model = Schema.Struct({
	capabilities: Schema.Struct({
		input: Schema.Array(Schema.String),
		output: Schema.Array(Schema.String),
		tools: Schema.Boolean,
	}),
	cost: Schema.Array(
		Schema.Struct({
			input: Schema.Number,
			output: Schema.Number,
		}),
	),
	enabled: Schema.Boolean,
	id: Schema.NonEmptyString,
	limit: Schema.Struct({
		context: Schema.Int,
		output: Schema.Int,
	}),
	modelID: Schema.NonEmptyString,
	name: Schema.NonEmptyString,
	providerID: Schema.NonEmptyString,
	status: Schema.Literals(["active", "alpha", "beta", "deprecated"]),
	variants: Schema.Array(Schema.Struct({ id: Schema.NonEmptyString })),
});
export type OpenCode2Model = typeof OpenCode2Model.Type;

const ModelsResponse = Schema.Struct({ data: Schema.Array(OpenCode2Model) });
const SessionResponse = Schema.Struct({ data: Schema.Struct({ id: Schema.NonEmptyString }) });
const SessionDetailResponse = Schema.Struct({
	data: Schema.Struct({
		id: Schema.NonEmptyString,
		location: Schema.Struct({ directory: Schema.NonEmptyString }),
		model: Schema.optional(
			Schema.Struct({
				id: Schema.NonEmptyString,
				providerID: Schema.NonEmptyString,
				variant: Schema.optional(Schema.NonEmptyString),
			}),
		),
	}),
});
const ProjectedMessage = Schema.Struct({
	content: Schema.optional(Schema.Array(Schema.Unknown)),
	cost: Schema.optional(Schema.Number),
	error: Schema.optional(Schema.Unknown),
	finish: Schema.optional(Schema.String),
	id: Schema.NonEmptyString,
	time: Schema.Struct({
		completed: Schema.optional(Schema.Number),
		created: Schema.Number,
	}),
	tokens: Schema.optional(
		Schema.Struct({
			cache: Schema.Struct({ read: Schema.Number, write: Schema.Number }),
			input: Schema.Number,
			output: Schema.Number,
			reasoning: Schema.Number,
		}),
	),
	type: Schema.NonEmptyString,
});
export type OpenCode2ProjectedMessage = typeof ProjectedMessage.Type;
const MessagesResponse = Schema.Struct({
	cursor: Schema.Struct({
		next: Schema.optional(Schema.String),
		previous: Schema.optional(Schema.String),
	}),
	data: Schema.Array(ProjectedMessage),
});
const IntegrationInfo = Schema.Struct({
	connections: Schema.Array(Schema.Unknown),
	id: Schema.NonEmptyString,
	methods: Schema.Array(Schema.Unknown),
	name: Schema.NonEmptyString,
});
const IntegrationsResponse = Schema.Struct({ data: Schema.Array(IntegrationInfo) });
export type OpenCode2IntegrationInfo = typeof IntegrationInfo.Type;
const OAuthAttemptResponse = Schema.Struct({
	data: Schema.Struct({
		attemptID: Schema.NonEmptyString,
		instructions: Schema.String,
		mode: Schema.Literals(["auto", "code"]),
		time: Schema.Struct({ created: Schema.Number, expires: Schema.Number }),
		url: Schema.NonEmptyString,
	}),
});
const OAuthStatusResponse = Schema.Struct({
	data: Schema.Union([
		Schema.Struct({ status: Schema.Literals(["complete", "expired", "pending"]) }),
		Schema.Struct({ message: Schema.String, status: Schema.Literal("failed") }),
	]),
});

export interface OpenCode2Fetch {
	(input: string | URL, init?: RequestInit): Promise<Response>;
}

export interface OpenCode2ApiClientOptions {
	readonly endpoint: URL;
	readonly Fetch?: OpenCode2Fetch;
	readonly password: string;
}

const is_json_record = (value: unknown): value is Readonly<Record<string, unknown>> =>
	typeof value === "object" && value !== null && !Array.isArray(value);

const location_query = (scope: EngineCatalogScope) => {
	const query = new URLSearchParams();
	query.set("location[directory]", scope.working_directory);
	return query;
};

async function bounded_json(response: Response, operation: string) {
	const declared = Number(response.headers.get("content-length"));
	if (Number.isSafeInteger(declared) && declared > maximum_json_bytes)
		throw new OpenCode2ApiError({ code: "protocol", operation });
	const bytes = new Uint8Array(await response.arrayBuffer());
	if (bytes.byteLength > maximum_json_bytes)
		throw new OpenCode2ApiError({ code: "protocol", operation });
	try {
		return Schema.decodeUnknownSync(Schema.UnknownFromJsonString)(
			new TextDecoder("utf-8", { fatal: true }).decode(bytes),
		);
	} catch {
		throw new OpenCode2ApiError({ code: "decode", operation });
	}
}

async function* sse_events(
	url: URL,
	headers: Readonly<Record<string, string>>,
	fetcher: OpenCode2Fetch,
	operation: string,
) {
	const abort = new AbortController();
	try {
		const response = await fetcher(url, {
			headers: { ...headers, Accept: "text/event-stream" },
			signal: abort.signal,
		});
		if (!response.ok || response.body === null)
			throw new OpenCode2ApiError({ code: "http", operation, status: response.status });
		const reader = response.body.getReader();
		const decoder = new TextDecoder("utf-8", { fatal: true });
		let pending = "";
		let data: Array<string> = [];
		for (;;) {
			const chunk = await reader.read();
			pending += decoder.decode(chunk.value ?? new Uint8Array(), { stream: !chunk.done });
			if (pending.length > maximum_sse_event_bytes)
				throw new OpenCode2ApiError({ code: "protocol", operation });
			let newline = pending.indexOf("\n");
			while (newline !== -1) {
				const line = pending.slice(0, newline).replace(/\r$/, "");
				pending = pending.slice(newline + 1);
				if (line.length === 0) {
					if (data.length > 0) {
						const joined = data.join("\n");
						data = [];
						try {
							yield Schema.decodeUnknownSync(Schema.UnknownFromJsonString)(joined);
						} catch {
							throw new OpenCode2ApiError({ code: "decode", operation });
						}
					}
				} else if (line.startsWith("data:")) {
					data.push(line.slice(5).replace(/^ /, ""));
				}
				newline = pending.indexOf("\n");
			}
			if (chunk.done) break;
		}
	} catch (cause) {
		if (cause instanceof OpenCode2ApiError) throw cause;
		throw new OpenCode2ApiError({ code: "network", operation });
	} finally {
		abort.abort();
	}
}

/** Narrow source-pinned client; unstable OpenCode types do not escape this file. */
export const MakeOpenCode2ApiClient = (options: OpenCode2ApiClientOptions) => {
	const fetcher = options.Fetch ?? fetch;
	const headers = {
		Authorization: `Basic ${Buffer.from(`opencode:${options.password}`).toString("base64")}`,
		"Content-Type": "application/json",
	};
	const Url = (pathname: string, query?: URLSearchParams) => {
		const url = new URL(pathname, options.endpoint);
		if (query !== undefined) url.search = query.toString();
		return url;
	};
	const Request = (
		operation: string,
		method: string,
		pathname: string,
		body?: unknown,
		query?: URLSearchParams,
	) =>
		Effect.tryPromise({
			try: async () => {
				const response = await fetcher(Url(pathname, query), {
					...(body === undefined ? {} : { body: JSON.stringify(body) }),
					headers,
					method,
				});
				if (!response.ok)
					throw new OpenCode2ApiError({
						code: "http",
						operation,
						status: response.status,
					});
				return response.status === 204 ? undefined : bounded_json(response, operation);
			},
			catch: (cause) =>
				cause instanceof OpenCode2ApiError
					? cause
					: new OpenCode2ApiError({ code: "network", operation }),
		});
	const Decode = <A, I>(operation: string, schema: Schema.Codec<A, I>, value: unknown) =>
		Schema.decodeUnknownEffect(schema)(value).pipe(
			Effect.mapError(() => new OpenCode2ApiError({ code: "decode", operation })),
		);
	const SessionPath = (session_id: string, suffix = "") =>
		`/api/session/${encodeURIComponent(session_id)}${suffix}`;

	return {
		CreateSession: (input: {
			readonly agent: string;
			readonly location: { readonly directory: string };
			readonly model: {
				readonly id: string;
				readonly providerID: string;
				readonly variant?: string;
			};
		}) =>
			Request("session.create", "POST", "/api/session", input).pipe(
				Effect.flatMap((value) => Decode("session.create", SessionResponse, value)),
				Effect.map((value) => value.data.id),
			),
		GlobalEvents: () =>
			Stream.fromAsyncIterable(
				sse_events(Url("/api/event"), headers, fetcher, "event.subscribe"),
				(cause) =>
					cause instanceof OpenCode2ApiError
						? cause
						: new OpenCode2ApiError({ code: "network", operation: "event.subscribe" }),
			),
		Health: Request("health.get", "GET", "/api/health").pipe(
			Effect.flatMap((value) => Decode("health.get", HealthResponse, value)),
		),
		Interrupt: (session_id: string) =>
			Request("session.interrupt", "POST", SessionPath(session_id, "/interrupt")).pipe(
				Effect.asVoid,
			),
		Integrations: (scope: EngineCatalogScope) =>
			Request(
				"integration.list",
				"GET",
				"/api/integration",
				undefined,
				location_query(scope),
			).pipe(
				Effect.flatMap((value) => Decode("integration.list", IntegrationsResponse, value)),
				Effect.map((value) => value.data),
			),
		BeginOAuth: (scope: EngineCatalogScope, integration_id: string, method_id: string) =>
			Request(
				"integration.oauth.connect",
				"POST",
				`/api/integration/${encodeURIComponent(integration_id)}/connect/oauth`,
				{ methodID: method_id },
				location_query(scope),
			).pipe(
				Effect.flatMap((value) =>
					Decode("integration.oauth.connect", OAuthAttemptResponse, value),
				),
				Effect.map((value) => value.data),
			),
		CancelOAuth: (scope: EngineCatalogScope, integration_id: string, attempt_id: string) =>
			Request(
				"integration.oauth.cancel",
				"DELETE",
				`/api/integration/${encodeURIComponent(integration_id)}/connect/oauth/${encodeURIComponent(attempt_id)}`,
				undefined,
				location_query(scope),
			).pipe(Effect.asVoid),
		CompleteOAuth: (
			scope: EngineCatalogScope,
			integration_id: string,
			attempt_id: string,
			code?: string,
		) =>
			Request(
				"integration.oauth.complete",
				"POST",
				`/api/integration/${encodeURIComponent(integration_id)}/connect/oauth/${encodeURIComponent(attempt_id)}/complete`,
				code === undefined ? {} : { code },
				location_query(scope),
			).pipe(Effect.asVoid),
		ConnectKey: (scope: EngineCatalogScope, integration_id: string, key: string) =>
			Request(
				"integration.connect.key",
				"POST",
				`/api/integration/${encodeURIComponent(integration_id)}/connect/key`,
				{ key },
				location_query(scope),
			).pipe(Effect.asVoid),
		OAuthStatus: (scope: EngineCatalogScope, integration_id: string, attempt_id: string) =>
			Request(
				"integration.oauth.status",
				"GET",
				`/api/integration/${encodeURIComponent(integration_id)}/connect/oauth/${encodeURIComponent(attempt_id)}`,
				undefined,
				location_query(scope),
			).pipe(
				Effect.flatMap((value) =>
					Decode("integration.oauth.status", OAuthStatusResponse, value),
				),
				Effect.map((value) => value.data),
			),
		GetSession: (session_id: string) =>
			Request("session.get", "GET", SessionPath(session_id)).pipe(
				Effect.flatMap((value) => Decode("session.get", SessionDetailResponse, value)),
				Effect.map((value) => value.data),
			),
		ListForms: (session_id: string) =>
			Request("session.form.list", "GET", SessionPath(session_id, "/form")).pipe(
				Effect.map((value) =>
					is_json_record(value) && Array.isArray(value.data) ? value.data : [],
				),
			),
		ListPermissions: (session_id: string) =>
			Request("session.permission.list", "GET", SessionPath(session_id, "/permission")).pipe(
				Effect.map((value) =>
					is_json_record(value) && Array.isArray(value.data) ? value.data : [],
				),
			),
		ListMessages: (session_id: string, limit = 200) => {
			const query = new URLSearchParams({ limit: String(limit), order: "desc" });
			return Request(
				"session.message.list",
				"GET",
				SessionPath(session_id, "/message"),
				undefined,
				query,
			).pipe(
				Effect.flatMap((value) => Decode("session.message.list", MessagesResponse, value)),
				Effect.map((value) => value.data),
			);
		},
		Models: (scope: EngineCatalogScope) =>
			Request("model.list", "GET", "/api/model", undefined, location_query(scope)).pipe(
				Effect.flatMap((value) => Decode("model.list", ModelsResponse, value)),
				Effect.map((value) => value.data),
			),
		Prompt: (
			session_id: string,
			input: {
				readonly delivery?: "queue" | "steer";
				readonly files?: ReadonlyArray<{ readonly name?: string; readonly uri: string }>;
				readonly id: string;
				readonly resume?: boolean;
				readonly text: string;
			},
		) => Request("session.prompt", "POST", SessionPath(session_id, "/prompt"), input),
		PutInstruction: (session_id: string, key: string, value: unknown) =>
			Request(
				"session.instructions.entry.put",
				"PUT",
				SessionPath(session_id, `/instructions/entries/${encodeURIComponent(key)}`),
				{ value },
			).pipe(Effect.asVoid),
		ReplyForm: (
			session_id: string,
			form_id: string,
			answer: Readonly<Record<string, unknown>>,
		) =>
			Request(
				"session.form.reply",
				"POST",
				SessionPath(session_id, `/form/${encodeURIComponent(form_id)}/reply`),
				{ answer },
			).pipe(Effect.asVoid),
		ReplyPermission: (session_id: string, permission_id: string, approved: boolean) =>
			Request(
				"session.permission.reply",
				"POST",
				SessionPath(session_id, `/permission/${encodeURIComponent(permission_id)}/reply`),
				{ reply: approved ? "once" : "reject" },
			).pipe(Effect.asVoid),
		SessionLog: (session_id: string, after?: number, follow = true) => {
			const query = new URLSearchParams({ follow: String(follow) });
			if (after !== undefined) query.set("after", String(after));
			return Stream.fromAsyncIterable(
				sse_events(
					Url(`/api/experimental/session/${encodeURIComponent(session_id)}/log`, query),
					headers,
					fetcher,
					"session.log",
				),
				(cause) =>
					cause instanceof OpenCode2ApiError
						? cause
						: new OpenCode2ApiError({ code: "network", operation: "session.log" }),
			);
		},
		SwitchAgent: (session_id: string, agent: string) =>
			Request("session.switchAgent", "POST", SessionPath(session_id, "/agent"), {
				agent,
			}).pipe(Effect.asVoid),
		SwitchModel: (session_id: string, model: EngineModelSelection) =>
			Request("session.switchModel", "POST", SessionPath(session_id, "/model"), {
				model: {
					id: model.model_id,
					providerID: model.provider_route_id,
					...(model.variant_id === undefined ? {} : { variant: model.variant_id }),
				},
			}).pipe(Effect.asVoid),
		Wait: (session_id: string) =>
			Request("session.wait", "POST", SessionPath(session_id, "/wait")).pipe(Effect.asVoid),
	};
};

export type OpenCode2ApiClient = ReturnType<typeof MakeOpenCode2ApiClient>;
