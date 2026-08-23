import type { EngineObservation, EngineObservationBase, EngineUsageObservation } from "../engine";
import { CountWrittenLines } from "../patch/unified-diff";
import type { OpenCode2ProjectedMessage } from "./protocol";

type RecordValue = Readonly<Record<string, unknown>>;

const record = (value: unknown): RecordValue | undefined =>
	typeof value === "object" && value !== null && !Array.isArray(value)
		? (value as RecordValue)
		: undefined;
const string = (value: unknown) => (typeof value === "string" ? value : undefined);
const number = (value: unknown) =>
	typeof value === "number" && Number.isFinite(value) ? value : undefined;

const tool_path = (input: RecordValue | undefined) =>
	string(input?.filePath) ?? string(input?.path);

/** One typed argument that names the call without exposing the provider envelope. */
const tool_detail = (input: RecordValue | undefined) =>
	string(input?.command) ??
	tool_path(input) ??
	string(input?.pattern) ??
	string(input?.query) ??
	string(input?.url) ??
	string(input?.action) ??
	string(input?.name);

type OpenCode2FileChange = Readonly<{
	action: "created" | "deleted" | "modified";
	lines_added: number;
	lines_deleted: number;
	path: string;
}>;

/** V2 success metadata is the canonical, bounded file-change projection. */
const metadata_file_changes = (data: RecordValue): ReadonlyArray<OpenCode2FileChange> => {
	const files = record(data.metadata)?.files;
	if (!Array.isArray(files)) return [];
	return files.flatMap((value) => {
		const file = record(value);
		const path = string(file?.file);
		const status = string(file?.status);
		const additions = number(file?.additions);
		const deletions = number(file?.deletions);
		if (
			path === undefined ||
			(status !== "added" && status !== "deleted" && status !== "modified") ||
			additions === undefined ||
			deletions === undefined ||
			additions < 0 ||
			deletions < 0
		)
			return [];
		return [
			{
				action:
					status === "added" ? "created" : status === "deleted" ? "deleted" : "modified",
				lines_added: additions,
				lines_deleted: deletions,
				path,
			},
		];
	});
};
export const OpenCode2EventSessionId = (event: unknown) => {
	const envelope = record(event);
	const data = record(envelope?.data);
	return string(data?.sessionID) ?? string(record(data?.form)?.sessionID);
};

export const IsOpenCode2DurableEvent = (event: unknown) =>
	record(record(event)?.durable) !== undefined;

export const OpenCode2DurableSequence = (event: unknown) =>
	number(record(record(event)?.durable)?.seq);

/** Stable across the replay, live, and projected-message recovery channels. */
export const OpenCode2EventDeduplicationKey = (event: unknown) => {
	const envelope = record(event);
	const type = string(envelope?.type);
	if (type === undefined) return undefined;
	const data = record(envelope?.data) ?? {};
	const assistant_id = string(data.assistantMessageID);
	const ordinal = number(data.ordinal);
	if (
		assistant_id !== undefined &&
		new Set([
			"session.reasoning.ended",
			"session.step.ended",
			"session.step.failed",
			"session.step.started",
			"session.text.ended",
		]).has(type)
	)
		return `${type}:${assistant_id}:${ordinal ?? 0}`;
	const tool_id = string(data.id);
	if (tool_id !== undefined && type.startsWith("session.tool."))
		return `${type}:${assistant_id ?? "unknown"}:${tool_id}`;
	if (type.startsWith("session.execution.")) return type;
	return (
		string(envelope?.id) ??
		`${type}:${OpenCode2DurableSequence(event) ?? "live"}:${assistant_id ?? "session"}:${ordinal ?? 0}`
	);
};

const projected_event = (
	session_id: string,
	message_id: string,
	type: string,
	ordinal: number,
	data: RecordValue,
) => ({
	data: { ...data, sessionID: session_id },
	id: `projection:${message_id}:${type}:${ordinal}`,
	type,
});

export interface OpenCode2ProjectionRecovery {
	readonly events: ReadonlyArray<unknown>;
	readonly terminal?: "completed" | "failed";
}

/** Rebuilds the current turn from the authoritative projected transcript after the agent is idle. */
export const RecoverOpenCode2Projection = (
	messages_descending: ReadonlyArray<OpenCode2ProjectedMessage>,
	prompt_id: string,
	session_id: string,
): OpenCode2ProjectionRecovery | undefined => {
	const prompt_index = messages_descending.findIndex((message) => message.id === prompt_id);
	if (prompt_index === -1) return undefined;
	const assistants = messages_descending
		.slice(0, prompt_index)
		.filter((message) => message.type === "assistant")
		.reverse();
	if (assistants.length === 0) return undefined;

	const events: Array<unknown> = [];
	for (const message of assistants) {
		const content = message.content ?? [];
		for (const [ordinal, value] of content.entries()) {
			const item = record(value);
			const type = string(item?.type);
			if (item === undefined || type === undefined) continue;
			const base = { assistantMessageID: message.id, ordinal };
			if (type === "text") {
				events.push(
					projected_event(session_id, message.id, "session.text.ended", ordinal, {
						...base,
						text: string(item.text) ?? "",
					}),
				);
				continue;
			}
			if (type === "reasoning") {
				events.push(
					projected_event(session_id, message.id, "session.reasoning.ended", ordinal, {
						...base,
						text: string(item.text) ?? "",
					}),
				);
				continue;
			}
			if (type !== "tool") continue;
			const tool_id = string(item.id);
			if (tool_id === undefined) continue;
			const state = record(item.state);
			const status = string(state?.status);
			const input = record(state?.input);
			const metadata = record(state?.metadata);
			const tool = {
				...base,
				id: tool_id,
				name: string(item.name) ?? "tool",
			};
			events.push(
				projected_event(
					session_id,
					message.id,
					"session.tool.input.started",
					ordinal,
					tool,
				),
			);
			if (input !== undefined)
				events.push(
					projected_event(session_id, message.id, "session.tool.called", ordinal, {
						...tool,
						input,
					}),
				);
			events.push(
				projected_event(
					session_id,
					message.id,
					status === "error"
						? "session.tool.failed"
						: status === "completed"
							? "session.tool.success"
							: "session.tool.progress",
					ordinal,
					{ ...tool, ...(metadata === undefined ? {} : { metadata }) },
				),
			);
		}
		if (message.tokens !== undefined || message.cost !== undefined)
			events.push(
				projected_event(session_id, message.id, "session.step.ended", 0, {
					assistantMessageID: message.id,
					...(message.cost === undefined ? {} : { cost: message.cost }),
					...(message.tokens === undefined ? {} : { tokens: message.tokens }),
				}),
			);
	}

	const latest = assistants.at(-1);
	const terminal =
		latest?.time.completed === undefined
			? undefined
			: latest.error !== undefined
				? ("failed" as const)
				: latest.finish !== undefined
					? ("completed" as const)
					: undefined;
	if (terminal !== undefined)
		events.push(
			projected_event(
				session_id,
				latest?.id ?? prompt_id,
				terminal === "failed" ? "session.execution.failed" : "session.execution.succeeded",
				0,
				{},
			),
		);
	return { events, ...(terminal === undefined ? {} : { terminal }) };
};

const error_ref = {
	artisan_code: "AE-RUN-301",
	detail: "OpenCode reported that the run failed.",
} as const;

const usage = (
	base: EngineObservationBase,
	data: RecordValue,
	provider_route_id: string | undefined,
	variant_id: string | undefined,
): EngineUsageObservation | undefined => {
	const tokens = record(data.tokens);
	if (tokens === undefined && number(data.cost) === undefined) return undefined;
	const cache = record(tokens?.cache);
	const cached_input_tokens = number(cache?.read);
	const cost_usd = number(data.cost);
	const input_tokens = number(tokens?.input);
	const output_tokens = number(tokens?.output);
	const turn_id = string(data.assistantMessageID);
	return {
		...base,
		_tag: "usage",
		basis: "delta",
		...(cached_input_tokens === undefined ? {} : { cached_input_tokens }),
		...(cost_usd === undefined ? {} : { cost_usd }),
		...(input_tokens === undefined ? {} : { input_tokens }),
		...(output_tokens === undefined ? {} : { output_tokens }),
		...(provider_route_id === undefined ? {} : { provider_route_id }),
		...(turn_id === undefined ? {} : { turn_id }),
		...(variant_id === undefined ? {} : { variant_id }),
	};
};

/** Stateful event mapper; tool names begin before their later terminal events. */
export class OpenCode2EventNormalizer {
	#active_compaction_id: string | undefined;
	readonly #artisan_run_id: string;
	readonly #native_thread_id: string;
	readonly #provider_route_id: string | undefined;
	readonly #variant_id: string | undefined;
	readonly #tool_inputs = new Map<string, RecordValue>();
	readonly #tool_names = new Map<string, string>();
	constructor(input: {
		readonly artisan_run_id: string;
		readonly native_thread_id: string;
		readonly provider_route_id?: string;
		readonly variant_id?: string;
	}) {
		this.#artisan_run_id = input.artisan_run_id;
		this.#native_thread_id = input.native_thread_id;
		this.#provider_route_id = input.provider_route_id;
		this.#variant_id = input.variant_id;
	}

	Normalize(event: unknown): ReadonlyArray<EngineObservation> {
		const envelope = record(event);
		const type = string(envelope?.type);
		if (envelope === undefined || type === undefined || type === "log.synced") return [];
		const data = record(envelope.data) ?? {};
		const durable_sequence = OpenCode2DurableSequence(envelope);
		const native_event_id = string(envelope.id) ?? `${type}:${durable_sequence ?? "live"}`;
		const base: EngineObservationBase = {
			artisan_run_id: this.#artisan_run_id,
			native_thread_id: this.#native_thread_id,
			observation_id: `${this.#artisan_run_id}:opencode2:${native_event_id}`,
			raw: {
				engine_id: "opencode2",
				frame: {
					...(durable_sequence === undefined ? {} : { durable_sequence }),
					event_id: native_event_id,
					type,
				},
				...(durable_sequence === undefined ? {} : { frame_sequence: durable_sequence }),
				native_id: native_event_id,
				native_method: type,
				protocol_version: "opencode-v2-0.0.1",
				transport: "opencode2-http-sse",
			},
			sequence: durable_sequence ?? 0,
		};
		const assistant_id = string(data.assistantMessageID) ?? native_event_id;
		const ordinal = number(data.ordinal) ?? 0;
		const text_item_id = `${assistant_id}:part:${ordinal}:text`;
		const reasoning_item_id = `${assistant_id}:part:${ordinal}:reasoning`;

		switch (type) {
			case "session.execution.started":
				return [{ ...base, _tag: "run_state", state: "running" }];
			case "session.execution.succeeded":
				return [{ ...base, _tag: "run_terminal", state: "completed" }];
			case "session.execution.failed":
				return [{ ...base, _tag: "run_terminal", error_ref, state: "failed" }];
			case "session.execution.interrupted":
				return [
					{
						...base,
						_tag: "run_terminal",
						state: data.reason === "user" ? "cancelled" : "interrupted",
					},
				];
			case "session.step.started":
				return [{ ...base, _tag: "turn_state", state: "started", turn_id: assistant_id }];
			case "session.step.ended": {
				const measured = usage(base, data, this.#provider_route_id, this.#variant_id);
				return [
					...(measured === undefined ? [] : [measured]),
					{ ...base, _tag: "turn_state", state: "completed", turn_id: assistant_id },
				];
			}
			case "session.step.failed": {
				const measured = usage(base, data, this.#provider_route_id, this.#variant_id);
				return [
					...(measured === undefined ? [] : [measured]),
					{ ...base, _tag: "turn_state", state: "failed", turn_id: assistant_id },
				];
			}
			case "session.text.delta":
				return [
					{
						...base,
						_tag: "agent_message_delta",
						delta: string(data.delta) ?? "",
						item_id: text_item_id,
						phase: "unspecified",
						turn_id: assistant_id,
					},
				];
			case "session.text.ended":
				return [
					{
						...base,
						_tag: "agent_message_completed",
						item_id: text_item_id,
						message: string(data.text) ?? "",
						phase: "unspecified",
						turn_id: assistant_id,
					},
				];
			case "session.reasoning.delta":
				return [
					{
						...base,
						_tag: "reasoning_summary_delta",
						delta: string(data.delta) ?? "",
						item_id: reasoning_item_id,
						summary_index: ordinal,
						turn_id: assistant_id,
					},
				];
			case "session.reasoning.ended":
				return [
					{
						...base,
						_tag: "reasoning_summary_completed",
						item_id: reasoning_item_id,
						text: string(data.text) ?? "",
						turn_id: assistant_id,
					},
				];
			case "session.tool.input.started": {
				const tool_id = string(data.id) ?? native_event_id;
				const tool_name = string(data.name) ?? "tool";
				const input = record(data.input);
				const detail = tool_detail(input);
				if (input !== undefined) this.#tool_inputs.set(tool_id, input);
				this.#tool_names.set(tool_id, tool_name);
				return [
					{
						...base,
						_tag: "tool",
						action: "started",
						...(detail === undefined ? {} : { detail }),
						tool_id,
						tool_name,
					},
				];
			}
			case "session.tool.called":
			case "session.tool.progress":
			case "session.tool.success":
			case "session.tool.failed": {
				const tool_id = string(data.id) ?? native_event_id;
				const tool_name = this.#tool_names.get(tool_id) ?? string(data.name) ?? "tool";
				const input = record(data.input) ?? this.#tool_inputs.get(tool_id);
				if (type === "session.tool.called" && input !== undefined)
					this.#tool_inputs.set(tool_id, input);
				const file_changes =
					type === "session.tool.success" ? metadata_file_changes(data) : [];
				const detail =
					tool_detail(input) ??
					(file_changes.length === 1
						? file_changes[0]?.path
						: file_changes.length > 1
							? `${file_changes.length} files`
							: undefined);
				const action =
					type === "session.tool.success"
						? "completed"
						: type === "session.tool.failed"
							? "failed"
							: type === "session.tool.called"
								? "started"
								: "progress";
				const observations: Array<EngineObservation> = [
					{
						...base,
						_tag: "tool",
						action,
						...(detail === undefined ? {} : { detail }),
						tool_id,
						tool_name,
					},
				];
				if (action === "completed" && file_changes.length > 0) {
					for (const change of file_changes)
						observations.push({ ...base, _tag: "file", ...change });
				} else if (action === "completed" && input !== undefined) {
					const path = tool_path(input);
					if (path !== undefined && tool_name === "read")
						observations.push({ ...base, _tag: "file", action: "read", path });
					if (path !== undefined && (tool_name === "edit" || tool_name === "write")) {
						const old_string = string(input.oldString);
						const new_string = string(input.newString) ?? string(input.content);
						observations.push({
							...base,
							_tag: "file",
							action: "modified",
							...(new_string === undefined
								? {}
								: CountWrittenLines(new_string, old_string)),
							path,
						});
					}
				}
				if (action === "completed" || action === "failed") {
					this.#tool_inputs.delete(tool_id);
					this.#tool_names.delete(tool_id);
				}
				return observations;
			}
			case "session.retry.scheduled":
				return [
					{
						...base,
						_tag: "retry",
						attempt_state: "retrying",
						message: "OpenCode scheduled another provider attempt.",
						turn_id: assistant_id,
						will_retry: true,
					},
				];
			case "session.compaction.started": {
				const compaction_id = this.#active_compaction_id ?? native_event_id;
				this.#active_compaction_id = compaction_id;
				return [
					{
						...base,
						_tag: "compaction",
						compaction_id,
						state: "started",
					},
				];
			}
			case "session.compaction.ended": {
				const summary = string(data.text);
				const compaction_id = this.#active_compaction_id ?? native_event_id;
				this.#active_compaction_id = undefined;
				return [
					{
						...base,
						_tag: "compaction",
						compaction_id,
						state: "completed",
						...(summary === undefined ? {} : { summary }),
					},
				];
			}
			case "session.shell.started":
			case "session.shell.ended": {
				const shell = record(data.shell);
				const command = string(shell?.command);
				const exit_code = number(shell?.exit);
				const output = string(record(data.output)?.output);
				const shell_name = string(shell?.shell);
				return [
					{
						...base,
						_tag: "terminal_activity",
						activity_id: string(shell?.id) ?? native_event_id,
						...(command === undefined ? {} : { command }),
						...(exit_code === undefined ? {} : { exit_code }),
						...(output === undefined ? {} : { output }),
						...(shell_name === undefined ? {} : { shell: shell_name }),
						state:
							type === "session.shell.started"
								? "started"
								: number(shell?.exit) === 0
									? "completed"
									: "failed",
					},
				];
			}
			default:
				return [];
		}
	}
}

export interface OpenCode2PendingPermission {
	readonly action: string;
	readonly id: string;
	readonly metadata?: RecordValue;
	readonly resources: ReadonlyArray<string>;
}

export const DecodeOpenCode2PendingPermission = (
	value: unknown,
): OpenCode2PendingPermission | undefined => {
	const item = record(value);
	const id = string(item?.id);
	const action = string(item?.action);
	if (id === undefined || action === undefined || !Array.isArray(item?.resources))
		return undefined;
	const metadata = record(item.metadata);
	return {
		action,
		id,
		...(metadata === undefined ? {} : { metadata }),
		resources: item.resources.filter(
			(resource): resource is string => typeof resource === "string",
		),
	};
};

export interface OpenCode2PendingForm {
	readonly fields: ReadonlyArray<RecordValue>;
	readonly id: string;
	readonly title: string;
}

export const DecodeOpenCode2PendingForm = (value: unknown): OpenCode2PendingForm | undefined => {
	const item = record(value);
	const id = string(item?.id);
	if (id === undefined || !Array.isArray(item?.fields)) return undefined;
	return {
		fields: item.fields.flatMap((field) =>
			record(field) === undefined ? [] : [record(field)!],
		),
		id,
		title: string(item.title) ?? "OpenCode question",
	};
};

export const OpenCode2FormAnswer = (
	form: OpenCode2PendingForm,
	answers: Readonly<Record<string, ReadonlyArray<string>>>,
) => {
	const result: Record<string, unknown> = {};
	for (const field of form.fields) {
		const key = string(field.key);
		if (key === undefined) continue;
		const values = answers[`${form.id}:${key}`] ?? answers[key];
		if (values === undefined) continue;
		switch (field.type) {
			case "multiselect":
				result[key] = [...values];
				break;
			case "boolean":
				result[key] = values[0]?.toLowerCase() === "true";
				break;
			case "number":
			case "integer":
				result[key] = Number(values[0]);
				break;
			default:
				result[key] = values[0] ?? "";
		}
	}
	return result;
};
