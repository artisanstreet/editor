import type { SessionUpdate, ToolCallContent } from "@agentclientprotocol/sdk";

import type { EngineObservation } from "../engine";

export interface AcpNormalizationState {
	readonly messages: Map<string, string>;
	readonly thoughts: Map<string, string>;
	readonly tools: Map<
		string,
		{
			kind?: string | null;
			name: string;
			title: string;
		}
	>;
}

export const EmptyAcpNormalizationState = (): AcpNormalizationState => ({
	messages: new Map(),
	thoughts: new Map(),
	tools: new Map(),
});

export interface AcpNormalizerInput {
	readonly artisan_run_id: string;
	readonly engine_id: string;
	readonly frame_sequence: number;
	readonly native_thread_id: string;
	readonly protocol_version: string;
	readonly state: AcpNormalizationState;
	readonly transport: string;
	readonly turn_id: string;
	readonly update: SessionUpdate;
}

const text_content = (content: { readonly type: string; readonly text?: string }) =>
	content.type === "text" ? (content.text ?? "") : "";

const record = (value: unknown): Readonly<Record<string, unknown>> | undefined =>
	typeof value === "object" && value !== null && !Array.isArray(value)
		? (value as Readonly<Record<string, unknown>>)
		: undefined;

const string = (value: unknown) => (typeof value === "string" ? value : undefined);

const tool_action = (status: string | null | undefined) => {
	switch (status) {
		case "completed":
			return "completed" as const;
		case "failed":
			return "failed" as const;
		case "in_progress":
			return "progress" as const;
		default:
			return "started" as const;
	}
};

const terminal_state = (status: string | null | undefined) => {
	switch (status) {
		case "completed":
			return "completed" as const;
		case "failed":
			return "failed" as const;
		case "in_progress":
			return "output" as const;
		default:
			return "started" as const;
	}
};

const file_action = (kind: string | null | undefined, has_old_text?: boolean) => {
	if (kind === "read") return "read" as const;
	if (kind === "delete") return "deleted" as const;
	return has_old_text === false ? ("created" as const) : ("modified" as const);
};

export const NormalizeAcpUpdate = (input: AcpNormalizerInput): ReadonlyArray<EngineObservation> => {
	const { update } = input;
	const base = (suffix: string) => ({
		artisan_run_id: input.artisan_run_id,
		native_thread_id: input.native_thread_id,
		observation_id: `${input.artisan_run_id}:${input.engine_id}:acp:${input.frame_sequence}:${suffix}`,
		raw: {
			engine_id: input.engine_id,
			frame: update,
			frame_sequence: input.frame_sequence,
			native_method: "session/update",
			protocol_version: input.protocol_version,
			transport: input.transport,
		},
		sequence: 0,
	});

	switch (update.sessionUpdate) {
		case "agent_message_chunk": {
			const delta = text_content(update.content);
			if (delta.length === 0) return [];
			const item_id = update.messageId ?? "agent-message";
			input.state.messages.set(item_id, `${input.state.messages.get(item_id) ?? ""}${delta}`);
			return [
				{
					...base(`message:${item_id}:delta`),
					_tag: "agent_message_delta",
					delta,
					item_id,
					phase: "unspecified",
					turn_id: input.turn_id,
				},
			];
		}
		case "agent_thought_chunk": {
			const delta = text_content(update.content);
			const item_id = update.messageId ?? "agent-thought";
			input.state.thoughts.set(item_id, `${input.state.thoughts.get(item_id) ?? ""}${delta}`);
			return [
				{
					...base(`thought:${item_id}:delta`),
					_tag: "reasoning_summary_delta",
					delta,
					item_id,
					summary_index: 0,
					turn_id: input.turn_id,
				},
			];
		}
		case "plan":
			return [
				{
					...base("plan"),
					_tag: "plan",
					entries: update.entries.map((entry, index) => ({
						id: `${input.turn_id}:plan:${index}`,
						status: entry.status,
						text: entry.content,
					})),
					turn_id: input.turn_id,
				},
			];
		case "plan_update":
			return update.plan.type === "items"
				? [
						{
							...base(`plan:${update.plan.planId}`),
							_tag: "plan",
							entries: update.plan.entries.map((entry, index) => ({
								id: `${update.plan.planId}:${index}`,
								status: entry.status,
								text: entry.content,
							})),
							turn_id: input.turn_id,
						},
					]
				: [];
		case "usage_update":
			return [
				{
					...base("usage"),
					_tag: "usage",
					basis: "cumulative",
					context_tokens: update.used,
					context_window_tokens: update.size,
					...(update.cost?.currency === "USD" ? { cost_usd: update.cost.amount } : {}),
					turn_id: input.turn_id,
				},
			];
		case "compaction_update": {
			const summary = update.summary
				?.map((content) => (content.type === "text" ? content.text : ""))
				.join("")
				.trim();
			return [
				{
					...base(`compaction:${update.compactionId}`),
					_tag: "compaction",
					compaction_id: update.compactionId,
					state: update.status === "in_progress" ? "started" : "completed",
					...(summary === undefined || summary.length === 0 ? {} : { summary }),
				},
			];
		}
		case "tool_call":
		case "tool_call_update": {
			const prior = input.state.tools.get(update.toolCallId);
			const title = update.title ?? prior?.title ?? "Tool call";
			const name = update.name ?? prior?.name ?? update.kind ?? prior?.kind ?? "tool";
			const kind = update.kind ?? prior?.kind;
			input.state.tools.set(update.toolCallId, {
				...(kind === undefined ? {} : { kind }),
				name,
				title,
			});
			const observations: Array<EngineObservation> = [
				{
					...base(`tool:${update.toolCallId}`),
					_tag: "tool",
					action: tool_action(update.status),
					detail: title,
					tool_id: update.toolCallId,
					tool_name: name,
				},
			];
			const locations = update.locations ?? [];
			if (kind === "read" || kind === "edit" || kind === "delete" || kind === "move") {
				const diffs = (update.content ?? []).filter(
					(content): content is Extract<ToolCallContent, { type: "diff" }> =>
						content.type === "diff",
				);
				const paths = new Map<string, boolean | undefined>();
				for (const location of locations) paths.set(location.path, undefined);
				for (const diff of diffs)
					paths.set(diff.path, diff.oldText !== null && diff.oldText !== undefined);
				for (const [path, has_old_text] of paths)
					observations.push({
						...base(`file:${update.toolCallId}:${path}`),
						_tag: "file",
						action: file_action(kind, has_old_text),
						path,
					});
			}
			if (kind === "execute") {
				const raw_input = record(update.rawInput);
				const raw_output = record(update.rawOutput);
				const exit_code = raw_output?.exitCode;
				const command = string(raw_input?.command ?? raw_input?.cmd);
				const output = string(raw_output?.output ?? raw_output?.stdout);
				observations.push({
					...base(`terminal:${update.toolCallId}`),
					_tag: "terminal_activity",
					activity_id: update.toolCallId,
					...(command === undefined ? {} : { command }),
					...(typeof exit_code === "number" && Number.isInteger(exit_code)
						? { exit_code }
						: {}),
					...(output === undefined ? {} : { output }),
					state: terminal_state(update.status),
				});
			}
			if (kind === "search") {
				const raw_input = record(update.rawInput);
				observations.push({
					...base(`search:${update.toolCallId}`),
					_tag: "search",
					query: string(raw_input?.query ?? raw_input?.pattern) ?? title,
					scope: /web|fetch/i.test(name) ? "web" : "workspace",
					search_id: update.toolCallId,
					state:
						update.status === "completed" || update.status === "failed"
							? "completed"
							: "started",
				});
			}
			return observations;
		}
		default:
			return [];
	}
};

export const CompleteAcpMessages = (input: Omit<AcpNormalizerInput, "update">) => {
	const observations: Array<EngineObservation> = [];
	let index = 0;
	for (const [item_id, message] of input.state.messages) {
		index += 1;
		observations.push({
			_tag: "agent_message_completed",
			artisan_run_id: input.artisan_run_id,
			item_id,
			message,
			native_thread_id: input.native_thread_id,
			observation_id: `${input.artisan_run_id}:${input.engine_id}:acp:message:${index}:completed`,
			phase: "unspecified",
			raw: {
				engine_id: input.engine_id,
				frame: { item_id, message, type: "prompt-complete" },
				protocol_version: input.protocol_version,
				transport: input.transport,
			},
			sequence: 0,
			turn_id: input.turn_id,
		});
	}
	for (const [item_id, text] of input.state.thoughts) {
		index += 1;
		observations.push({
			_tag: "reasoning_summary_completed",
			artisan_run_id: input.artisan_run_id,
			item_id,
			native_thread_id: input.native_thread_id,
			observation_id: `${input.artisan_run_id}:${input.engine_id}:acp:thought:${index}:completed`,
			raw: {
				engine_id: input.engine_id,
				frame: { item_id, type: "prompt-complete" },
				protocol_version: input.protocol_version,
				transport: input.transport,
			},
			sequence: 0,
			text,
			turn_id: input.turn_id,
		});
	}
	return observations;
};
