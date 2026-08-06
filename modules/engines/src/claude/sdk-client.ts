import { query } from "@anthropic-ai/claude-agent-sdk";
import type { Options, Query, SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import { Context, Layer } from "effect";

/** Names one streaming-input Claude Agent SDK invocation. @since 0.8.0 */
export interface ClaudeQueryInput {
	readonly options?: Options;
	readonly prompt: AsyncIterable<SDKUserMessage>;
}

/**
 * Starts Claude Agent SDK queries. The service exists as the injectable seam
 * between the engine and the SDK: the live layer spawns the SDK's managed
 * Claude Code runtime, while tests supply scripted message generators.
 *
 * @since 0.8.0
 */
export class ClaudeQueryClient extends Context.Service<
	ClaudeQueryClient,
	{ readonly Start: (input: ClaudeQueryInput) => Query }
>()("Artisan/ClaudeQueryClient") {}

/** Runs real queries against the SDK's bundled Claude Code runtime. @since 0.8.0 */
export const ClaudeQueryClientLive = Layer.succeed(ClaudeQueryClient, {
	Start: (input: ClaudeQueryInput) =>
		query({
			prompt: input.prompt,
			...(input.options === undefined ? {} : { options: input.options }),
		}),
});
