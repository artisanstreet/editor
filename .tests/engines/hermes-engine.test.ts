import { describe, expect, it } from "@effect/vitest";
import { Effect, Layer, Stream } from "effect";

import {
	EngineProcessFactory,
	HermesCatalogFromOptions,
	HermesEngine,
	HermesEventNormalizer,
	make_hermes_engine_layer,
	type HermesGatewayClient,
	type HermesGatewayEvent,
	type HermesPrivateService,
} from "@artisan/engines";

const model_options = {
	model: "anthropic/claude-sonnet-4.6",
	provider: "nous",
	providers: [
		{
			authenticated: true,
			capabilities: {
				"anthropic/claude-sonnet-4.6": { fast: false, reasoning: true },
				"google/gemini-3-flash-preview": { fast: true, reasoning: true },
			},
			models: ["anthropic/claude-sonnet-4.6", "google/gemini-3-flash-preview"],
			name: "Nous Portal",
			pricing: {
				"anthropic/claude-sonnet-4.6": { input: "$3.00", output: "$15.00" },
			},
			slug: "nous",
		},
		{
			authenticated: true,
			capabilities: {
				"anthropic/claude-sonnet-4.6": { fast: false, reasoning: true },
			},
			models: ["anthropic/claude-sonnet-4.6"],
			name: "OpenRouter",
			slug: "openrouter",
		},
	],
};

class FakeHermesClient implements HermesGatewayClient {
	readonly requests: Array<{
		readonly method: string;
		readonly params: Readonly<Record<string, unknown>>;
	}> = [];
	readonly #listeners = new Set<(event: HermesGatewayEvent) => void>();
	readonly Close = Effect.void;
	readonly Closed = Effect.never;

	IsOpen = () => true;

	Request = (method: string, params: Readonly<Record<string, unknown>> = {}) =>
		Effect.sync(() => {
			this.requests.push({ method, params });
			switch (method) {
				case "model.options":
					return model_options;
				case "session.create":
					return {
						info: {
							desktop_contract: 6,
							model: "anthropic/claude-sonnet-4.6",
							provider: "nous",
						},
						session_id: "runtime_hermes",
						stored_session_id: "stored_hermes",
					};
				case "prompt.submit":
					queueMicrotask(() => {
						this.Publish({ session_id: "runtime_hermes", type: "message.start" });
						this.Publish({
							payload: { text: "Implemented through Hermes." },
							session_id: "runtime_hermes",
							type: "message.delta",
						});
						this.Publish({
							payload: {
								text: "Implemented through Hermes.",
								usage: {
									context_max: 200_000,
									context_used: 30,
									input: 20,
									output: 10,
								},
							},
							session_id: "runtime_hermes",
							type: "message.complete",
						});
					});
					return { status: "streaming" };
				default:
					return {};
			}
		});

	Subscribe = (listener: (event: HermesGatewayEvent) => void) => {
		this.#listeners.add(listener);
		return () => this.#listeners.delete(listener);
	};

	Publish(event: HermesGatewayEvent) {
		for (const listener of this.#listeners) listener(event);
	}
}

const client = new FakeHermesClient();
const service = {
	Client: client,
	Close: Effect.void,
	Closed: Effect.never,
	endpoint: new URL("ws://127.0.0.1:9119/api/ws"),
	version: "0.20.4",
} satisfies HermesPrivateService;
const factory = EngineProcessFactory.of({ Spawn: () => Effect.die("process must not start") });
const layer = make_hermes_engine_layer({ StartService: () => Effect.succeed(service) }).pipe(
	Layer.provide(Layer.succeed(EngineProcessFactory, factory)),
);

describe("Hermes engine", () => {
	it("scopes message identities to an Artisan run", () => {
		const normalize_message = (artisan_run_id: string) => {
			const normalizer = new HermesEventNormalizer({
				artisan_run_id,
				native_thread_id: "stored_shared",
				provider_route_id: "nous",
			});
			normalizer.Normalize({ session_id: "runtime_shared", type: "message.start" });
			return normalizer.Normalize({
				payload: { text: "Hello" },
				session_id: "runtime_shared",
				type: "message.complete",
			});
		};

		expect(normalize_message("run_first")).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					_tag: "agent_message_completed",
					item_id: "run_first:stored_shared:turn:1:message",
				}),
			]),
		);
		expect(normalize_message("run_second")).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					_tag: "agent_message_completed",
					item_id: "run_second:stored_shared:turn:1:message",
				}),
			]),
		);
	});

	it("does not publish Hermes private reasoning as the thinking label", () => {
		const normalizer = new HermesEventNormalizer({
			artisan_run_id: "run_private_reasoning",
			native_thread_id: "stored_private_reasoning",
			provider_route_id: "nous",
		});

		expect(
			normalizer.Normalize({
				payload: { text: "I should overwrite the thinking verb" },
				type: "thinking.delta",
			}),
		).toEqual([]);
		expect(
			normalizer.Normalize({
				payload: { text: "A private final reasoning trace" },
				type: "reasoning.available",
			}),
		).toEqual([]);
	});

	it("names Hermes tool activity from its structured input", () => {
		const normalizer = new HermesEventNormalizer({
			artisan_run_id: "run_tool_detail",
			native_thread_id: "stored_tool_detail",
			provider_route_id: "nous",
		});

		expect(
			normalizer.Normalize({
				payload: {
					args: { path: "src/app.ts" },
					name: "read_file",
					tool_id: "tool_read",
				},
				type: "tool.start",
			}),
		).toEqual([
			expect.objectContaining({
				_tag: "tool",
				action: "started",
				detail: "src/app.ts",
				tool_id: "tool_read",
				tool_name: "read_file",
			}),
		]);
		expect(
			normalizer.Normalize({
				payload: {
					args: { path: "src/app.ts" },
					name: "read_file",
					result: { content: "file contents" },
					summary: "Read completed",
					tool_id: "tool_read",
				},
				type: "tool.complete",
			}),
		).toEqual([
			expect.objectContaining({
				_tag: "tool",
				action: "completed",
				detail: "src/app.ts",
				tool_id: "tool_read",
			}),
		]);
	});

	it("retains typed detail and one fallback ID across an anonymous tool lifecycle", () => {
		const normalizer = new HermesEventNormalizer({
			artisan_run_id: "run_anonymous_tool",
			native_thread_id: "stored_anonymous_tool",
			provider_route_id: "nous",
		});
		const [started] = normalizer.Normalize({
			payload: {
				args_text: JSON.stringify({ mode: "replace", path: "src/app.ts" }),
				name: "patch",
			},
			type: "tool.start",
		});
		const [progress] = normalizer.Normalize({
			payload: { preview: "provider-authored preview" },
			type: "tool.progress",
		});
		const completed = normalizer.Normalize({
			payload: { summary: "provider-authored summary" },
			type: "tool.complete",
		});
		const anonymous_tool_id = (started as { tool_id: string }).tool_id;

		expect(progress).toMatchObject({
			_tag: "tool",
			detail: "src/app.ts",
			tool_id: anonymous_tool_id,
		});
		expect(completed).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					_tag: "tool",
					detail: "src/app.ts",
					tool_id: anonymous_tool_id,
				}),
				expect.objectContaining({ _tag: "file", path: "src/app.ts" }),
			]),
		);
	});

	it("reports a successful Hermes edit as a canonical file mutation", () => {
		const normalizer = new HermesEventNormalizer({
			artisan_run_id: "run_file_change",
			native_thread_id: "stored_file_change",
			provider_route_id: "nous",
		});
		normalizer.Normalize({
			payload: {
				args_text: JSON.stringify({
					mode: "replace",
					new_string: "first\nsecond",
					old_string: "first",
					path: "src/app.ts",
				}),
				name: "patch",
				tool_id: "tool_patch",
			},
			type: "tool.start",
		});

		expect(
			normalizer.Normalize({
				payload: { name: "patch", result_text: "applied", tool_id: "tool_patch" },
				type: "tool.complete",
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

	it("reports a successful Hermes write as a canonical file mutation", () => {
		const normalizer = new HermesEventNormalizer({
			artisan_run_id: "run_file_write",
			native_thread_id: "stored_file_write",
			provider_route_id: "nous",
		});
		normalizer.Normalize({
			payload: {
				args_text: JSON.stringify({
					content: "export const value = 1;",
					path: "src/new.ts",
				}),
				name: "write_file",
				tool_id: "tool_write",
			},
			type: "tool.start",
		});

		expect(
			normalizer.Normalize({
				payload: { result_text: "written", tool_id: "tool_write" },
				type: "tool.complete",
			}),
		).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					_tag: "file",
					action: "modified",
					lines_added: 1,
					lines_deleted: 0,
					path: "src/new.ts",
				}),
			]),
		);
	});

	it("does not report a failed Hermes edit as a file mutation", () => {
		const normalizer = new HermesEventNormalizer({
			artisan_run_id: "run_file_failure",
			native_thread_id: "stored_file_failure",
			provider_route_id: "nous",
		});
		normalizer.Normalize({
			payload: {
				args_text: JSON.stringify({ mode: "replace", path: "src/app.ts" }),
				name: "patch",
				tool_id: "tool_failed_patch",
			},
			type: "tool.start",
		});

		expect(
			normalizer
				.Normalize({
					payload: {
						args: { mode: "replace", path: "src/app.ts" },
						name: "patch",
						result: { message: "Patch rejected", ok: false },
						tool_id: "tool_failed_patch",
					},
					type: "tool.complete",
				})
				.some((observation) => observation._tag === "file"),
		).toBe(false);

		const direct_error = new HermesEventNormalizer({
			artisan_run_id: "run_direct_file_failure",
			native_thread_id: "stored_direct_file_failure",
			provider_route_id: "nous",
		});
		direct_error.Normalize({
			payload: {
				args_text: JSON.stringify({ mode: "replace", path: "src/direct.ts" }),
				name: "patch",
				tool_id: "tool_direct_failed_patch",
			},
			type: "tool.start",
		});
		expect(
			direct_error
				.Normalize({
					payload: {
						error: { message: "Patch rejected" },
						tool_id: "tool_direct_failed_patch",
					},
					type: "tool.complete",
				})
				.some((observation) => observation._tag === "file"),
		).toBe(false);
	});

	it("fails Hermes terminal activity when its structured exit code is non-zero", () => {
		const normalizer = new HermesEventNormalizer({
			artisan_run_id: "run_failed_terminal",
			native_thread_id: "stored_failed_terminal",
			provider_route_id: "nous",
		});
		normalizer.Normalize({
			payload: {
				args: { command: "pnpm test" },
				name: "terminal",
				tool_id: "tool_failed_terminal",
			},
			type: "tool.start",
		});

		expect(
			normalizer.Normalize({
				payload: {
					args: { command: "pnpm test" },
					name: "terminal",
					result: { exit_code: 2, output: "" },
					tool_id: "tool_failed_terminal",
				},
				type: "tool.complete",
			}),
		).toEqual(
			expect.arrayContaining([
				expect.objectContaining({ _tag: "tool", action: "failed" }),
				expect.objectContaining({ _tag: "terminal_activity", state: "failed" }),
			]),
		);
	});

	it("reports every Hermes V4A patch entry as a canonical file mutation", () => {
		const normalizer = new HermesEventNormalizer({
			artisan_run_id: "run_v4a_change",
			native_thread_id: "stored_v4a_change",
			provider_route_id: "nous",
		});
		normalizer.Normalize({
			payload: {
				args_text: JSON.stringify({
					mode: "patch",
					patch: [
						"*** Begin Patch",
						"***Update File: src/app.ts",
						"*** Move to: src/moved.ts",
						"@@",
						"-before",
						"+after",
						"*** Add File: src/new.ts",
						"+export const created = true;",
						"*** Delete File: src/old.ts",
						"*** Move File: src/from.ts -> src/to.ts",
						"*** End Patch",
					].join("\n"),
				}),
				name: "patch",
				tool_id: "tool_v4a",
			},
			type: "tool.start",
		});

		expect(
			normalizer.Normalize({
				payload: { result_text: "Done!", tool_id: "tool_v4a" },
				type: "tool.complete",
			}),
		).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					_tag: "file",
					action: "modified",
					lines_added: 1,
					lines_deleted: 1,
					path: "src/moved.ts",
				}),
				expect.objectContaining({
					_tag: "file",
					action: "created",
					lines_added: 1,
					lines_deleted: 0,
					path: "src/new.ts",
				}),
				expect.objectContaining({
					_tag: "file",
					action: "deleted",
					path: "src/old.ts",
				}),
				expect.objectContaining({ _tag: "file", action: "deleted", path: "src/from.ts" }),
				expect.objectContaining({ _tag: "file", action: "created", path: "src/to.ts" }),
			]),
		);
	});

	it("retires compaction when Hermes emits its explicit completion status", () => {
		const normalizer = new HermesEventNormalizer({
			artisan_run_id: "run_compaction_status",
			native_thread_id: "stored_compaction_status",
			provider_route_id: "nous",
		});

		const [started] = normalizer.Normalize({
			payload: { kind: "compacting" },
			type: "status.update",
		});
		const [repeated_start] = normalizer.Normalize({
			payload: { kind: "compacting" },
			type: "status.update",
		});
		const [completed] = normalizer.Normalize({
			payload: { kind: "compacted" },
			type: "status.update",
		});
		expect(started).toMatchObject({ _tag: "compaction", state: "started" });
		expect(completed).toMatchObject({
			_tag: "compaction",
			compaction_id: "run_compaction_status:stored_compaction_status:turn:0:compaction:1",
			state: "completed",
		});
		expect(repeated_start).toMatchObject({ compaction_id: started?.compaction_id });
		expect(completed).toMatchObject({ compaction_id: started?.compaction_id });
		const [next_started] = normalizer.Normalize({
			payload: { kind: "compacting" },
			type: "status.update",
		});
		expect(next_started).toMatchObject({
			compaction_id: "run_compaction_status:stored_compaction_status:turn:0:compaction:2",
		});
	});

	it.each([
		["message.start", {}],
		["message.delta", { text: "Resumed" }],
		["message.interim", { text: "Still working" }],
		["reasoning.delta", { text: "private" }],
		["tool.start", { name: "terminal", tool_id: "tool_1" }],
	] as const)("retires compaction when %s proves the turn resumed", (type, payload) => {
		const normalizer = new HermesEventNormalizer({
			artisan_run_id: `run_compaction_${type}`,
			native_thread_id: "stored_compaction_resume",
			provider_route_id: "nous",
		});
		const [started] = normalizer.Normalize({
			payload: { kind: "compacting" },
			type: "status.update",
		});

		const resumed = normalizer.Normalize({ payload, type });
		expect(resumed.filter((observation) => observation._tag === "compaction")).toEqual([
			expect.objectContaining({
				_tag: "compaction",
				compaction_id: started?.compaction_id,
				state: "completed",
			}),
		]);
		expect(
			normalizer
				.Normalize({
					payload: { preview: "continuing", tool_id: "tool_1" },
					type: "tool.progress",
				})
				.filter((observation) => observation._tag === "compaction"),
		).toEqual([]);
	});

	it("turns each provider into a collapsible route group", () => {
		const catalog = HermesCatalogFromOptions(model_options, {
			profile_id: "default",
			working_directory: "C:\\workspace",
			workspace_trust: "safe",
		});
		expect(catalog.routes).toEqual([
			expect.objectContaining({
				group: {
					id: "nous",
					label: "Nous Portal",
					order: 0,
					show_route_labels: false,
				},
				id: "nous",
			}),
			expect.objectContaining({
				group: {
					id: "openrouter",
					label: "OpenRouter",
					order: 1,
					show_route_labels: false,
				},
				id: "openrouter",
			}),
		]);
		expect(catalog.models).toHaveLength(3);
		expect(catalog.models[0]).toMatchObject({
			capabilities: { fast: false, reasoning: true },
			cost: { input_usd_per_million: 3, output_usd_per_million: 15 },
			model_id: "anthropic/claude-sonnet-4.6",
			provider_route_id: "nous",
		});
		expect(catalog.models[1]?.capabilities).toMatchObject({ fast: true, reasoning: true });
		expect(catalog.models[0]?.catalog_id).not.toBe(catalog.models[2]?.catalog_id);
	});

	it("preserves provider model casing and separators", () => {
		const catalog = HermesCatalogFromOptions(
			{
				providers: [
					{
						authenticated: true,
						models: ["openai/gpt-5.4-mini", "openai/gpt-5.6-sol-pro"],
						name: "OpenAI Codex",
						slug: "openai-codex",
					},
				],
			},
			{
				profile_id: "default",
				working_directory: "C:\\workspace",
				workspace_trust: "safe",
			},
		);

		expect(catalog.models.map((model) => model.name)).toEqual([
			"gpt-5.4-mini",
			"gpt-5.6-sol-pro",
		]);
	});

	it.effect("opens a selected route and settles from Hermes gateway events", () =>
		Effect.scoped(
			Effect.gen(function* () {
				client.requests.length = 0;
				const engine = yield* HermesEngine;
				const catalog = yield* engine.Catalog!({
					profile_id: "default",
					working_directory: "C:\\workspace",
					workspace_trust: "safe",
				});
				const run = yield* engine.Open({
					_tag: "start",
					artisan_run_id: "run_hermes",
					catalog_revision: catalog.revision,
					initial_text: "Implement it",
					model: "anthropic/claude-sonnet-4.6",
					model_id: "anthropic/claude-sonnet-4.6",
					profile_id: "default",
					provider_options: {
						"hermes.fast": false,
						"hermes.permission_mode": "profile",
						"hermes.reasoning_effort": "medium",
					},
					provider_route_id: "nous",
					working_directory: "C:\\workspace",
				});
				const observations = Array.from(yield* run.Events.pipe(Stream.runCollect));
				expect(observations).toEqual(
					expect.arrayContaining([
						expect.objectContaining({
							_tag: "agent_message_completed",
							message: "Implemented through Hermes.",
						}),
						expect.objectContaining({
							_tag: "usage",
							input_tokens: 20,
							provider_route_id: "nous",
						}),
						expect.objectContaining({ _tag: "run_terminal", state: "completed" }),
					]),
				);
				const create = client.requests.find(
					(request) => request.method === "session.create",
				);
				expect(create?.params).toMatchObject({
					model: "anthropic/claude-sonnet-4.6",
					provider: "nous",
					source: "artisan",
				});
			}),
		).pipe(Effect.provide(layer)),
	);

	it("maps approvals and batched clarification requests", () => {
		const normalizer = new HermesEventNormalizer({
			artisan_run_id: "run_events",
			native_thread_id: "stored_events",
			provider_route_id: "openrouter",
		});
		expect(
			normalizer.Normalize({
				payload: {
					command: "pnpm test",
					description: "Run the test suite",
					request_id: "approval_1",
				},
				session_id: "runtime_events",
				type: "approval.request",
			}),
		).toEqual([
			expect.objectContaining({
				_tag: "approval",
				approval_id: "approval_1",
				request: expect.objectContaining({ command: "pnpm test", kind: "command" }),
			}),
		]);
		expect(
			normalizer.Normalize({
				payload: {
					questions: [
						{ choices: ["A", "B"], qid: "first", question: "Choose one" },
						{ multi_select: true, qid: "second", question: "Choose several" },
					],
					request_id: "clarify_1",
				},
				session_id: "runtime_events",
				type: "clarify.request",
			}),
		).toEqual([
			expect.objectContaining({ _tag: "question", question_id: "clarify_1:first" }),
			expect.objectContaining({
				_tag: "question",
				multi_select: true,
				question_id: "clarify_1:second",
			}),
		]);
	});
});
