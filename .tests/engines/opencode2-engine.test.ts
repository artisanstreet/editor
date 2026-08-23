import { describe, expect, it } from "@effect/vitest";
import { Effect, Layer, Stream } from "effect";

import {
	EngineProcessFactory,
	MakeOpenCode2ApiClient,
	OpenCode2Engine,
	OpenCode2EventNormalizer,
	OpenCode2PermissionRules,
	RecoverOpenCode2Projection,
	make_opencode2_engine_layer,
	type OpenCode2PrivateService,
} from "@artisan/engines";

const json = (value: unknown, status = 200) =>
	new Response(JSON.stringify(value), {
		headers: { "content-type": "application/json" },
		status,
	});

const durable = (type: string, seq: number, data: Readonly<Record<string, unknown>>) => ({
	created: seq,
	data: { sessionID: "ses_test", ...data },
	durable: { aggregateID: "ses_test", seq, version: 1 },
	id: `evt_${seq}`,
	type,
});

const sse = (events: ReadonlyArray<unknown>) =>
	new Response(events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join(""), {
		headers: { "content-type": "text/event-stream" },
	});

const requests: Array<{ readonly body?: unknown; readonly method: string; readonly url: URL }> = [];
const Fetch = async (input: string | URL, init?: RequestInit) => {
	const url = new URL(input);
	const method = init?.method ?? "GET";
	requests.push({
		...(typeof init?.body === "string" ? { body: JSON.parse(init.body) as unknown } : {}),
		method,
		url,
	});
	if (url.pathname === "/api/model")
		return json({
			data: [
				{
					capabilities: { input: ["text", "image"], output: ["text"], tools: true },
					cost: [{ input: 3, output: 15 }],
					enabled: true,
					id: "claude-sonnet-4-5",
					limit: { context: 200_000, output: 32_000 },
					modelID: "claude-sonnet-4-5-20250929",
					name: "Claude Sonnet 4.5",
					providerID: "opencode-go",
					status: "active",
					variants: [{ id: "high" }],
				},
				{
					capabilities: { input: ["text"], output: ["text"], tools: true },
					cost: [],
					enabled: true,
					id: "ox-alpha-free",
					limit: { context: 1_000_000, output: 64_000 },
					modelID: "ox-alpha-free",
					name: "Ox Alpha Free",
					providerID: "opencode",
					status: "alpha",
					variants: [],
				},
				{
					capabilities: { input: ["text"], output: ["text"], tools: false },
					cost: [],
					enabled: true,
					id: "internal-coder",
					limit: { context: 128_000, output: 16_000 },
					modelID: "internal-coder-v2",
					name: "Internal Coder",
					providerID: "acme",
					status: "active",
					variants: [],
				},
			],
			location: {},
		});
	if (url.pathname === "/api/integration")
		return json({
			data: [
				{
					connections: [{ id: "cred_console", label: "OpenCode", type: "credential" }],
					id: "opencode",
					methods: [
						{ id: "device", label: "OpenCode Console", type: "oauth" },
						{ label: "Service API key", type: "key" },
					],
					name: "OpenCode Console",
				},
			],
			location: {},
		});
	if (url.pathname.endsWith("/connect/oauth") && method === "POST")
		return json({
			data: {
				attemptID: "con_test",
				instructions: "Enter the code in your browser.",
				mode: "auto",
				time: { created: 1, expires: 2 },
				url: "https://console.opencode.ai/device",
			},
			location: {},
		});
	if (url.pathname === "/api/session" && method === "POST")
		return json({ data: { id: "ses_test" } });
	if (url.pathname.endsWith("/prompt")) return json({ data: { id: "msg_test" } });
	if (url.pathname.endsWith("/permission") || url.pathname.endsWith("/form"))
		return json({ data: [] });
	if (url.pathname === "/api/event")
		return sse([{ data: {}, id: "evt_connected", type: "server.connected" }]);
	if (url.pathname === "/api/experimental/session/ses_test/log")
		return sse([
			durable("session.execution.started", 1, {}),
			durable("session.step.started", 2, { assistantMessageID: "msg_assistant" }),
			durable("session.text.ended", 3, {
				assistantMessageID: "msg_assistant",
				ordinal: 0,
				text: "Implemented through Go.",
			}),
			durable("session.step.ended", 4, {
				assistantMessageID: "msg_assistant",
				cost: 0.01,
				tokens: { cache: { read: 4, write: 0 }, input: 10, output: 5, reasoning: 0 },
			}),
			durable("session.execution.succeeded", 5, {}),
			{ aggregateID: "ses_test", seq: 5, type: "log.synced" },
		]);
	if (method === "PUT" || method === "POST") return new Response(undefined, { status: 204 });
	throw new Error(`Unhandled OpenCode test request: ${method} ${url.pathname}`);
};

const client = MakeOpenCode2ApiClient({
	endpoint: new URL("http://127.0.0.1:4096"),
	Fetch,
	password: "test-password",
});

const factory = EngineProcessFactory.of({ Spawn: () => Effect.die("process must not start") });
const LayerFor = (Client: typeof client) =>
	make_opencode2_engine_layer({
		StartService: () =>
			Effect.succeed({
				Client,
				Close: Effect.void,
				endpoint: new URL("http://127.0.0.1:4096"),
				version: "0.0.0-beta-17778",
			} satisfies OpenCode2PrivateService),
	}).pipe(Layer.provide(Layer.succeed(EngineProcessFactory, factory)));
const layer = LayerFor(client);

const OpenTestRun = (artisan_run_id: string) =>
	Effect.gen(function* () {
		const engine = yield* OpenCode2Engine;
		const catalog = yield* engine.Catalog!({
			profile_id: "work",
			working_directory: "C:\\workspace",
			workspace_trust: "safe",
		});
		return yield* engine.Open({
			_tag: "start",
			artisan_run_id,
			catalog_revision: catalog.revision,
			initial_text: "Implement it",
			model: "claude-sonnet-4-5",
			model_id: "claude-sonnet-4-5",
			profile_id: "work",
			provider_options: {
				"opencode2.agent": "artisan-v1-autonomous-network-web",
				"opencode2.project_config": false,
			},
			provider_route_id: "opencode-go",
			working_directory: "C:\\workspace",
		});
	});

describe("OpenCode engine", () => {
	it("keeps reasoning and response identities distinct when live events omit ordinals", () => {
		const normalizer = new OpenCode2EventNormalizer({
			artisan_run_id: "run_parts",
			native_thread_id: "ses_parts",
		});
		const reasoning = normalizer.Normalize({
			data: { assistantMessageID: "msg_parts", text: "Private reasoning" },
			id: "evt_reasoning",
			type: "session.reasoning.ended",
		});
		const response = normalizer.Normalize({
			data: { assistantMessageID: "msg_parts", text: "Visible response" },
			id: "evt_text",
			type: "session.text.ended",
		});

		expect(reasoning).toEqual([
			expect.objectContaining({
				_tag: "reasoning_summary_completed",
				item_id: "msg_parts:part:0:reasoning",
			}),
		]);
		expect(response).toEqual([
			expect.objectContaining({
				_tag: "agent_message_completed",
				item_id: "msg_parts:part:0:text",
				message: "Visible response",
			}),
		]);
	});

	it("names OpenCode tool activity from its structured input", () => {
		const normalizer = new OpenCode2EventNormalizer({
			artisan_run_id: "run_tool_detail",
			native_thread_id: "ses_tool_detail",
		});
		normalizer.Normalize({
			data: { id: "tool_read", name: "read" },
			id: "evt_tool_read_start",
			type: "session.tool.input.started",
		});

		expect(
			normalizer.Normalize({
				data: { id: "tool_read", input: { filePath: "src/app.ts" } },
				id: "evt_tool_read_called",
				type: "session.tool.called",
			}),
		).toEqual([
			expect.objectContaining({
				_tag: "tool",
				action: "started",
				detail: "src/app.ts",
				tool_id: "tool_read",
				tool_name: "read",
			}),
		]);
	});

	it("reports a successful OpenCode edit as a canonical file mutation", () => {
		const normalizer = new OpenCode2EventNormalizer({
			artisan_run_id: "run_file_change",
			native_thread_id: "ses_file_change",
		});
		normalizer.Normalize({
			data: { id: "tool_edit", name: "edit" },
			id: "evt_tool_edit_start",
			type: "session.tool.input.started",
		});
		normalizer.Normalize({
			data: {
				id: "tool_edit",
				input: { filePath: "src/app.ts", newString: "first\nsecond", oldString: "first" },
			},
			id: "evt_tool_edit_called",
			type: "session.tool.called",
		});

		expect(
			normalizer.Normalize({
				data: { id: "tool_edit", name: "edit" },
				id: "evt_tool_edit_success",
				type: "session.tool.success",
			}),
		).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					_tag: "file",
					action: "modified",
					lines_added: 2,
					lines_deleted: 1,
					path: "src/app.ts",
				}),
			]),
		);
	});

	it("reports a successful OpenCode write with line statistics", () => {
		const normalizer = new OpenCode2EventNormalizer({
			artisan_run_id: "run_file_write",
			native_thread_id: "ses_file_write",
		});
		normalizer.Normalize({
			data: { id: "tool_write", name: "write" },
			id: "evt_tool_write_start",
			type: "session.tool.input.started",
		});
		normalizer.Normalize({
			data: {
				id: "tool_write",
				input: { content: "first\nsecond", filePath: "src/new.ts" },
			},
			id: "evt_tool_write_called",
			type: "session.tool.called",
		});

		expect(
			normalizer.Normalize({
				data: { id: "tool_write" },
				id: "evt_tool_write_success",
				type: "session.tool.success",
			}),
		).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					_tag: "file",
					action: "modified",
					lines_added: 2,
					lines_deleted: 0,
					path: "src/new.ts",
				}),
			]),
		);
	});

	it("reports every OpenCode patch metadata entry as a canonical file mutation", () => {
		const normalizer = new OpenCode2EventNormalizer({
			artisan_run_id: "run_patch_change",
			native_thread_id: "ses_patch_change",
		});
		normalizer.Normalize({
			data: { id: "tool_patch", name: "patch" },
			id: "evt_tool_patch_start",
			type: "session.tool.input.started",
		});
		normalizer.Normalize({
			data: { id: "tool_patch", input: { patchText: "*** Begin Patch" } },
			id: "evt_tool_patch_called",
			type: "session.tool.called",
		});

		expect(
			normalizer.Normalize({
				data: {
					id: "tool_patch",
					metadata: {
						files: [
							{ additions: 2, deletions: 1, file: "src/app.ts", status: "modified" },
							{ additions: 1, deletions: 0, file: "src/new.ts", status: "added" },
							{ additions: 0, deletions: 4, file: "src/old.ts", status: "deleted" },
						],
					},
					name: "patch",
				},
				id: "evt_tool_patch_success",
				type: "session.tool.success",
			}),
		).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					_tag: "tool",
					action: "completed",
					detail: "3 files",
					tool_id: "tool_patch",
				}),
				expect.objectContaining({
					_tag: "file",
					action: "modified",
					lines_added: 2,
					lines_deleted: 1,
					path: "src/app.ts",
				}),
				expect.objectContaining({ _tag: "file", action: "created", path: "src/new.ts" }),
				expect.objectContaining({ _tag: "file", action: "deleted", path: "src/old.ts" }),
			]),
		);
	});

	it("does not report failed OpenCode patch metadata as file mutations", () => {
		const normalizer = new OpenCode2EventNormalizer({
			artisan_run_id: "run_failed_patch_change",
			native_thread_id: "ses_failed_patch_change",
		});
		normalizer.Normalize({
			data: { id: "tool_failed_patch", name: "patch" },
			id: "evt_failed_patch_start",
			type: "session.tool.input.started",
		});
		normalizer.Normalize({
			data: { id: "tool_failed_patch", input: { patchText: "*** Begin Patch" } },
			id: "evt_failed_patch_called",
			type: "session.tool.called",
		});

		const observations = normalizer.Normalize({
			data: {
				id: "tool_failed_patch",
				metadata: {
					files: [{ additions: 1, deletions: 1, file: "src/app.ts", status: "modified" }],
				},
				name: "patch",
			},
			id: "evt_failed_patch",
			type: "session.tool.failed",
		});

		expect(observations).toEqual([
			expect.objectContaining({
				_tag: "tool",
				action: "failed",
				tool_id: "tool_failed_patch",
			}),
		]);
		expect(observations.some((observation) => observation._tag === "file")).toBe(false);
	});

	it("preserves structured tool metadata while recovering OpenCode projection", () => {
		const recovery = RecoverOpenCode2Projection(
			[
				{
					content: [
						{
							id: "tool_edit",
							name: "patch",
							state: {
								input: { patchText: "*** Begin Patch" },
								metadata: {
									files: [
										{
											additions: 1,
											deletions: 1,
											file: "src/app.ts",
											status: "modified",
										},
									],
								},
								status: "completed",
							},
							type: "tool",
						},
					],
					finish: "stop",
					id: "assistant_projected",
					time: { completed: 3, created: 2 },
					type: "assistant",
				},
				{ id: "prompt_projected", time: { created: 1 }, type: "user" },
			],
			"prompt_projected",
			"ses_projected",
		);
		const normalizer = new OpenCode2EventNormalizer({
			artisan_run_id: "run_projected_tools",
			native_thread_id: "ses_projected",
		});
		const observations = recovery?.events.flatMap((event) => normalizer.Normalize(event)) ?? [];
		expect(recovery?.events.map((event) => (event as { type?: string }).type)).toEqual([
			"session.tool.input.started",
			"session.tool.called",
			"session.tool.success",
			"session.execution.succeeded",
		]);

		expect(observations).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					_tag: "tool",
					detail: "src/app.ts",
					tool_id: "tool_edit",
				}),
				expect.objectContaining({
					_tag: "file",
					action: "modified",
					path: "src/app.ts",
				}),
			]),
		);
	});

	it("uses one stable identity for an OpenCode compaction lifecycle", () => {
		const normalizer = new OpenCode2EventNormalizer({
			artisan_run_id: "run_compaction",
			native_thread_id: "ses_compaction",
		});
		const [started] = normalizer.Normalize({
			data: {},
			id: "evt_compaction_started",
			type: "session.compaction.started",
		});
		const [repeated_start] = normalizer.Normalize({
			data: {},
			id: "evt_compaction_started_again",
			type: "session.compaction.started",
		});
		const [completed] = normalizer.Normalize({
			data: {},
			id: "evt_compaction_ended",
			type: "session.compaction.ended",
		});

		expect(started).toMatchObject({
			_tag: "compaction",
			compaction_id: "evt_compaction_started",
			state: "started",
		});
		expect(repeated_start).toMatchObject({ compaction_id: "evt_compaction_started" });
		expect(completed).toMatchObject({
			_tag: "compaction",
			compaction_id: "evt_compaction_started",
			state: "completed",
		});
	});

	it.effect("exposes one Console connection whose OAuth can provision Zen and Go routes", () =>
		Effect.gen(function* () {
			const engine = yield* OpenCode2Engine;
			const scope = {
				profile_id: "work",
				working_directory: "C:\\workspace",
				workspace_trust: "safe" as const,
			};
			const connections = yield* engine.Connections!.List(scope);
			expect(connections).toEqual([
				expect.objectContaining({
					connected: true,
					id: "opencode",
					methods: expect.arrayContaining([
						expect.objectContaining({ id: "device", type: "oauth" }),
						expect.objectContaining({ type: "key" }),
					]),
				}),
			]);
			const attempt = yield* engine.Connections!.BeginOAuth(scope, "opencode", "device");
			expect(attempt).toMatchObject({
				attempt_id: "con_test",
				url: "https://console.opencode.ai/device",
			});
		}).pipe(Effect.provide(layer)),
	);

	it.effect("keeps Go route and variant identity distinct in the scoped live catalog", () =>
		Effect.gen(function* () {
			const engine = yield* OpenCode2Engine;
			const catalog = yield* engine.Catalog!({
				profile_id: "work",
				working_directory: "C:\\workspace",
				workspace_trust: "safe",
			});
			expect(catalog.models).toHaveLength(4);
			expect(catalog.routes).toEqual([
				expect.objectContaining({
					group: { id: "go", label: "Go", order: 0, show_route_labels: false },
					id: "opencode-go",
					label: "Go",
				}),
				expect.objectContaining({
					group: { id: "zen", label: "Zen", order: 1, show_route_labels: false },
					id: "opencode",
					label: "Zen",
				}),
				expect.objectContaining({
					group: {
						id: "custom",
						label: "Custom",
						order: 2,
						show_route_labels: true,
					},
					id: "acme",
					label: "acme",
				}),
			]);
			expect(catalog.models.map((model) => model.provider_route_id)).toEqual([
				"opencode-go",
				"opencode-go",
				"opencode",
				"acme",
			]);
			expect(catalog.models[1]).toMatchObject({
				model_id: "claude-sonnet-4-5",
				variant_id: "high",
			});
			expect(catalog.models[0]?.catalog_id).not.toBe(catalog.models[1]?.catalog_id);
		}).pipe(Effect.provide(layer)),
	);

	it.effect("starts a session and settles from the durable log with route-aware usage", () =>
		Effect.scoped(
			Effect.gen(function* () {
				requests.length = 0;
				const engine = yield* OpenCode2Engine;
				const catalog = yield* engine.Catalog!({
					profile_id: "work",
					working_directory: "C:\\workspace",
					workspace_trust: "safe",
				});
				const run = yield* engine.Open({
					_tag: "start",
					artisan_run_id: "run_opencode2",
					catalog_revision: catalog.revision,
					initial_text: "Implement it",
					model: "claude-sonnet-4-5",
					model_id: "claude-sonnet-4-5",
					profile_id: "work",
					provider_options: {
						"opencode2.agent": "artisan-v1-autonomous-network-web",
						"opencode2.project_config": false,
					},
					provider_route_id: "opencode-go",
					working_directory: "C:\\workspace",
				});
				const observations = Array.from(yield* run.Events.pipe(Stream.runCollect));
				expect(observations).toEqual(
					expect.arrayContaining([
						expect.objectContaining({
							_tag: "agent_message_completed",
							message: "Implemented through Go.",
						}),
						expect.objectContaining({
							_tag: "usage",
							cost_usd: 0.01,
							provider_route_id: "opencode-go",
						}),
						expect.objectContaining({ _tag: "run_terminal", state: "completed" }),
					]),
				);
				const create = requests.find(
					(request) =>
						request.method === "POST" && request.url.pathname === "/api/session",
				);
				expect(create?.body).toMatchObject({
					model: { id: "claude-sonnet-4-5", providerID: "opencode-go" },
				});
			}),
		).pipe(Effect.provide(layer)),
	);

	it.effect("settles from live durable events when the server does not persist its log", () => {
		let publish: (() => void) | undefined;
		const LiveFetch = async (input: string | URL, init?: RequestInit) => {
			const url = new URL(input);
			if (url.pathname === "/api/event") {
				const encoder = new TextEncoder();
				return new Response(
					new ReadableStream<Uint8Array>({
						start(controller) {
							controller.enqueue(
								encoder.encode(
									`data: ${JSON.stringify({ data: {}, id: "evt_connected", type: "server.connected" })}\n\n`,
								),
							);
							publish = () => {
								for (const event of [
									durable("session.execution.started", 6, {}),
									durable("session.text.ended", 7, {
										assistantMessageID: "msg_live",
										ordinal: 0,
										text: "Recovered from the live feed.",
									}),
									durable("session.execution.succeeded", 8, {}),
								])
									controller.enqueue(
										encoder.encode(`data: ${JSON.stringify(event)}\n\n`),
									);
								controller.close();
							};
						},
					}),
					{ headers: { "content-type": "text/event-stream" } },
				);
			}
			if (url.pathname === "/api/experimental/session/ses_test/log")
				return sse([{ aggregateID: "ses_test", seq: 5, type: "log.synced" }]);
			const response = await Fetch(input, init);
			if (url.pathname.endsWith("/prompt")) queueMicrotask(() => publish?.());
			return response;
		};
		const live_client = MakeOpenCode2ApiClient({
			endpoint: new URL("http://127.0.0.1:4096"),
			Fetch: LiveFetch,
			password: "test-password",
		});
		return Effect.scoped(
			Effect.gen(function* () {
				const run = yield* OpenTestRun("run_live_only");
				const observations = Array.from(yield* run.Events.pipe(Stream.runCollect));
				expect(observations).toEqual(
					expect.arrayContaining([
						expect.objectContaining({
							_tag: "agent_message_completed",
							message: "Recovered from the live feed.",
						}),
						expect.objectContaining({ _tag: "run_terminal", state: "completed" }),
					]),
				);
			}),
		).pipe(Effect.provide(LayerFor(live_client)));
	});

	it.effect(
		"recovers a completed turn from messages when live terminal events were missed",
		() => {
			let prompt_id = "";
			const ProjectionFetch = async (input: string | URL, init?: RequestInit) => {
				const url = new URL(input);
				if (url.pathname === "/api/event")
					return sse([{ data: {}, id: "evt_connected", type: "server.connected" }]);
				if (url.pathname === "/api/experimental/session/ses_test/log")
					return sse([{ aggregateID: "ses_test", seq: 13, type: "log.synced" }]);
				if (url.pathname.endsWith("/prompt")) {
					const body = JSON.parse(String(init?.body)) as { id: string };
					prompt_id = body.id;
					return json({ data: { id: prompt_id } });
				}
				if (url.pathname.endsWith("/wait")) return new Response(undefined, { status: 204 });
				if (url.pathname.endsWith("/message"))
					return json({
						cursor: {},
						data: [
							{
								content: [{ text: "Recovered from projection.", type: "text" }],
								cost: 0.02,
								finish: "stop",
								id: "msg_projected_assistant",
								time: { completed: 3, created: 2 },
								tokens: {
									cache: { read: 2, write: 0 },
									input: 20,
									output: 8,
									reasoning: 1,
								},
								type: "assistant",
							},
							{ id: prompt_id, time: { created: 1 }, type: "user" },
						],
					});
				return Fetch(input, init);
			};
			const projection_client = MakeOpenCode2ApiClient({
				endpoint: new URL("http://127.0.0.1:4096"),
				Fetch: ProjectionFetch,
				password: "test-password",
			});
			return Effect.scoped(
				Effect.gen(function* () {
					const run = yield* OpenTestRun("run_projection_recovery");
					const observations = Array.from(yield* run.Events.pipe(Stream.runCollect));
					expect(
						observations.filter(
							(observation) => observation._tag === "agent_message_completed",
						),
					).toHaveLength(1);
					expect(observations).toEqual(
						expect.arrayContaining([
							expect.objectContaining({
								_tag: "agent_message_completed",
								message: "Recovered from projection.",
							}),
							expect.objectContaining({
								_tag: "usage",
								cost_usd: 0.02,
								input_tokens: 20,
							}),
							expect.objectContaining({ _tag: "run_terminal", state: "completed" }),
						]),
					);
				}),
			).pipe(Effect.provide(LayerFor(projection_client)));
		},
	);

	it("allows enabled web search in Read only without opening shell or edits", () => {
		const rules = OpenCode2PermissionRules({
			network_access: false,
			permission: "restricted",
			web_search_enabled: true,
		});
		expect(rules.some((rule) => rule.action === "edit" && rule.effect === "allow")).toBe(false);
		expect(rules.some((rule) => rule.action === "shell" && rule.effect !== "deny")).toBe(false);
		expect(rules).toContainEqual({ action: "websearch", effect: "allow", resource: "*" });
	});
});
