import { Buffer } from "node:buffer";
import { Encoding, Option, Schema } from "effect";

import { type EngineObservation, MakeEngineSubagentTranscriptObservation } from "../engine";
import {
	normalize_claude_event,
	read_claude_stream_message_id,
	read_claude_tool_uses,
	type ClaudeNormalizationInput,
	type ClaudeToolUse,
} from "./normalizer";
import { claude_protocol_version, claude_transport } from "./descriptor";
import { ResolveClaudeChildTranscriptOwner, type ClaudeTaskLineageState } from "./task-lineage";

const max_deferred_child_frames = 128;

interface DeferredChildFrame {
	readonly frame_sequence: number;
	readonly message: unknown;
	readonly parent_tool_use_id: string;
}

interface ChildStream {
	readonly stream_message_id?: string;
	readonly tool_uses: ReadonlyMap<string, ClaudeToolUse>;
}

/** Run-owned bounded correlation state for multiplexed Claude child streams. */
export interface ClaudeChildTranscriptState {
	readonly deferred: ReadonlyArray<DeferredChildFrame>;
	readonly streams: ReadonlyMap<string, ChildStream>;
}

/** Creates empty child transcript correlation state for one Claude query. */
export const EmptyClaudeChildTranscripts = (): ClaudeChildTranscriptState => ({
	deferred: [],
	streams: new Map(),
});

/** True only for provider-marked multiplexed child transcript frames. */
const ChildTranscriptFrame = Schema.Struct({ parent_tool_use_id: Schema.String });

/** Recognizes byte-decoded direct-CLI child frames through one schema boundary. */
export function is_claude_child_transcript_frame(
	message: unknown,
): message is { readonly parent_tool_use_id: string } {
	return Option.isSome(Schema.decodeUnknownOption(ChildTranscriptFrame)(message));
}

export interface ClaudeChildTranscriptInput {
	readonly base: Omit<
		ClaudeNormalizationInput,
		| "frame_sequence"
		| "native_subagent_task"
		| "native_thread_id"
		| "payload"
		| "stream_message_id"
		| "subagent_parent_native_thread_id"
		| "tool_uses"
	>;
	readonly frame_sequence: number;
	readonly lineage: ClaudeTaskLineageState;
	readonly message?: unknown;
	readonly parent_tool_use_id?: string;
	readonly state: ClaudeChildTranscriptState;
}

/** Builds shared normalizer provenance for one multiplexed CLI frame. */
export const MakeClaudeChildTranscriptBase = (
	artisan_run_id: string,
	message: unknown,
	raw_frame_base64?: string,
): ClaudeChildTranscriptInput["base"] => ({
	artisan_run_id,
	protocol_version: claude_protocol_version,
	raw_frame_base64:
		raw_frame_base64 ?? Encoding.encodeBase64(Buffer.from(JSON.stringify(message), "utf8")),
	transport: claude_transport,
	turn_id: `claude:${artisan_run_id}:turn`,
});

/**
 * Defers out-of-order child frames until their Agent/Task topology is proven,
 * then emits only canonical child content. The returned observations never
 * include ordinary root transcript entries.
 */
export function AdvanceClaudeChildTranscripts(input: ClaudeChildTranscriptInput): {
	readonly observations: ReadonlyArray<EngineObservation>;
	readonly state: ClaudeChildTranscriptState;
} {
	const deferred = [
		...input.state.deferred,
		...(input.message === undefined || input.parent_tool_use_id === undefined
			? []
			: [
					{
						frame_sequence: input.frame_sequence,
						message: input.message,
						parent_tool_use_id: input.parent_tool_use_id,
					},
				]),
	].slice(-max_deferred_child_frames);
	const streams = new Map(input.state.streams);
	const pending: Array<DeferredChildFrame> = [];
	const observations: Array<EngineObservation> = [];

	for (const frame of deferred) {
		const owner = ResolveClaudeChildTranscriptOwner(input.lineage, frame.parent_tool_use_id);
		if (owner === undefined) {
			pending.push(frame);
			continue;
		}
		const prior: ChildStream = streams.get(frame.parent_tool_use_id) ?? {
			tool_uses: new Map(),
		};
		const announced_message_id = read_claude_stream_message_id(frame.message);
		const announced_tools = read_claude_tool_uses(frame.message);
		const tool_uses = new Map([
			...prior.tool_uses,
			...announced_tools.map((tool): readonly [string, ClaudeToolUse] => [tool.id, tool]),
		]);
		streams.set(frame.parent_tool_use_id, {
			...(announced_message_id === undefined
				? prior.stream_message_id === undefined
					? {}
					: { stream_message_id: prior.stream_message_id }
				: { stream_message_id: announced_message_id }),
			tool_uses,
		});
		for (const observation of normalize_claude_event({
			...input.base,
			frame_sequence: frame.frame_sequence,
			native_subagent_task: true,
			native_thread_id: owner.agent_native_thread_id,
			payload: frame.message,
			...(announced_message_id === undefined && prior.stream_message_id === undefined
				? {}
				: { stream_message_id: announced_message_id ?? prior.stream_message_id }),
			subagent_parent_native_thread_id: owner.parent_native_thread_id,
			tool_uses,
		})) {
			if (observation._tag === "subagent") observations.push(observation);
			else {
				const transcript = MakeEngineSubagentTranscriptObservation({
					agent_native_thread_id: owner.agent_native_thread_id,
					observation,
					parent_native_thread_id: owner.parent_native_thread_id,
				});
				if (transcript !== undefined) observations.push(transcript);
			}
		}
	}

	return { observations, state: { deferred: pending, streams } };
}
