import type {
	Options,
	PermissionResult,
	Query,
	SDKMessage,
	SDKUserMessage,
} from "@anthropic-ai/claude-agent-sdk";
import { Layer } from "effect";

import { ClaudeQueryClient } from "@artisan/engines";

/** One scripted Agent SDK session as observed and driven by a test. */
export interface FakeClaudeSession {
	/** Whether the engine closed or aborted the session. */
	readonly closed: () => boolean;
	/** Ends the message stream, as the SDK does when the child exits. */
	readonly end: () => void;
	/** Emits one SDK message to the engine, as the child would. */
	readonly emit: (message: SDKMessage) => void;
	readonly interrupts: () => number;
	readonly options: Options | undefined;
	/** Messages the engine pushed through the stream-input prompt, in order. */
	readonly received: ReadonlyArray<SDKUserMessage>;
	/** Invokes the engine's canUseTool bridge exactly as the SDK would. */
	readonly request_permission: (
		tool_name: string,
		tool_input: Record<string, unknown>,
		context?: {
			readonly decisionReason?: string;
			readonly signal?: AbortSignal;
			readonly title?: string;
		},
	) => Promise<PermissionResult | null>;
	/** The session id the engine selected (start `sessionId` or resume id). */
	readonly session_id: () => string;
}

export interface FakeClaudeQuery {
	readonly layer: Layer.Layer<ClaudeQueryClient>;
	readonly sessions: ReadonlyArray<FakeClaudeSession>;
}

type SessionScript = (session: FakeClaudeSession) => Promise<void> | void;

/**
 * Builds a ClaudeQueryClient whose sessions run a per-open script instead of
 * a real Claude Code child. The script emits SDK messages, exercises the
 * permission bridge, and ends the stream; the engine consumes the resulting
 * generator exactly as it would the SDK's.
 */
export function make_fake_claude_query(script: SessionScript): FakeClaudeQuery {
	const sessions: Array<FakeClaudeSession> = [];
	const layer = Layer.succeed(ClaudeQueryClient, {
		Start: (input) => {
			const values: Array<SDKMessage> = [];
			const received: Array<SDKUserMessage> = [];
			let ended = false;
			let close_count = 0;
			let interrupt_count = 0;
			let notify: (() => void) | undefined;
			const wake = () => {
				const current = notify;
				notify = undefined;
				current?.();
			};
			const end = () => {
				ended = true;
				wake();
			};

			const session: FakeClaudeSession = {
				closed: () => close_count > 0,
				end,
				emit: (message) => {
					if (ended) return;
					values.push(message);
					wake();
				},
				interrupts: () => interrupt_count,
				options: input.options,
				received,
				request_permission: (tool_name, tool_input, context) => {
					const can_use_tool = input.options?.canUseTool;
					if (can_use_tool === undefined) {
						return Promise.resolve({ behavior: "allow" } satisfies PermissionResult);
					}

					return can_use_tool(tool_name, tool_input, {
						requestId: `fake-request-${received.length}`,
						signal: context?.signal ?? new AbortController().signal,
						toolUseID: `fake-tool-use-${received.length}`,
						...(context?.title === undefined ? {} : { title: context.title }),
						...(context?.decisionReason === undefined
							? {}
							: { decisionReason: context.decisionReason }),
					});
				},
				session_id: () =>
					input.options?.sessionId ?? input.options?.resume ?? "unknown-session",
			};
			sessions.push(session);

			void (async () => {
				for await (const message of input.prompt) {
					received.push(message);
				}
			})();
			input.options?.abortController?.signal.addEventListener("abort", () => {
				close_count += 1;
				end();
			});
			queueMicrotask(() => {
				void Promise.resolve(script(session)).catch((cause: unknown) => {
					/**
					 * A failing script must terminate the stream, or the engine
					 * waits on a session that will never speak again and the test
					 * times out instead of failing on its assertions.
					 */
					console.error("fake Claude session script failed", cause);
					end();
				});
			});

			async function* generate(): AsyncGenerator<SDKMessage, void> {
				while (true) {
					while (values.length > 0) yield values.shift() as SDKMessage;
					if (ended) return;
					await new Promise<void>((resolve) => {
						notify = resolve;
					});
				}
			}

			const handle = Object.assign(generate(), {
				close: () => {
					close_count += 1;
					end();
				},
				interrupt: () => {
					interrupt_count += 1;
					return Promise.resolve(undefined);
				},
				setModel: () => Promise.resolve(),
				setPermissionMode: () => Promise.resolve(),
			});

			/**
			 * The Query interface carries many control methods the engine never
			 * touches; the fake implements the consumed subset and asserts the
			 * boundary through the type below.
			 */
			return handle as unknown as Query;
		},
	});

	return { layer, sessions };
}

/** A client whose sessions must never start; probing-only tests provide it. */
export function make_unstartable_claude_query(): Layer.Layer<ClaudeQueryClient> {
	return Layer.succeed(ClaudeQueryClient, {
		Start: () => {
			throw new Error("This test must not start an SDK session");
		},
	});
}
