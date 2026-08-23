import type { EngineObservation, EngineObservationBase, EngineUsageObservation } from "../engine";
import { CountWrittenLines } from "../patch/unified-diff";
import type { HermesGatewayEvent } from "./protocol";

type RecordValue = Readonly<Record<string, unknown>>;

const record = (value: unknown): RecordValue | undefined =>
	typeof value === "object" && value !== null && !Array.isArray(value)
		? (value as RecordValue)
		: undefined;
const string = (value: unknown) => (typeof value === "string" ? value : undefined);
const number = (value: unknown) =>
	typeof value === "number" && Number.isFinite(value) ? value : undefined;
const boolean = (value: unknown) => (typeof value === "boolean" ? value : undefined);

export const HermesEventSessionId = (event: HermesGatewayEvent) => event.session_id;

export interface HermesPendingApproval {
	readonly command?: string;
	readonly description: string;
	readonly id: string;
}

export interface HermesPendingQuestion {
	readonly id: string;
	readonly multi_select: boolean;
	readonly options?: ReadonlyArray<string>;
	readonly question_id?: string;
	readonly request_id: string;
	readonly text: string;
}

export const DecodeHermesApproval = (
	event: HermesGatewayEvent,
): HermesPendingApproval | undefined => {
	if (event.type !== "approval.request") return undefined;
	const payload = record(event.payload);
	const id = string(payload?.request_id);
	if (id === undefined) return undefined;
	return {
		...(string(payload?.command) === undefined ? {} : { command: string(payload?.command)! }),
		description: string(payload?.description) ?? "Hermes requests approval to continue.",
		id,
	};
};

export const DecodeHermesQuestions = (
	event: HermesGatewayEvent,
): ReadonlyArray<HermesPendingQuestion> => {
	if (event.type !== "clarify.request") return [];
	const payload = record(event.payload);
	const request_id = string(payload?.request_id);
	if (request_id === undefined) return [];
	if (Array.isArray(payload?.questions)) {
		return payload.questions.flatMap((value) => {
			const question = record(value);
			const question_id = string(question?.qid);
			const text = string(question?.question);
			if (question_id === undefined || text === undefined) return [];
			const options = Array.isArray(question?.choices)
				? question.choices.filter((choice): choice is string => typeof choice === "string")
				: undefined;
			return [
				{
					id: `${request_id}:${question_id}`,
					multi_select: boolean(question?.multi_select) ?? false,
					...(options === undefined || options.length === 0 ? {} : { options }),
					question_id,
					request_id,
					text,
				},
			];
		});
	}
	const text = string(payload?.question);
	if (text === undefined) return [];
	const options = Array.isArray(payload?.choices)
		? payload.choices.filter((choice): choice is string => typeof choice === "string")
		: undefined;
	return [
		{
			id: request_id,
			multi_select: boolean(payload?.multi_select) ?? false,
			...(options === undefined || options.length === 0 ? {} : { options }),
			request_id,
			text,
		},
	];
};

const tool_arguments = (payload: RecordValue) => {
	const structured = record(payload.args);
	if (structured !== undefined) return structured;
	const text = string(payload.args_text);
	if (text === undefined) return undefined;
	try {
		return record(JSON.parse(text));
	} catch {
		return undefined;
	}
};

const tool_failed = (payload: RecordValue): boolean => {
	if (payload.error !== undefined && payload.error !== null) return true;
	const result = payload.result;
	const structured = record(result);
	if (structured?.error !== undefined && structured.error !== null) return true;
	const exit_code = number(structured?.exit_code) ?? number(structured?.returncode);
	if (exit_code !== undefined && exit_code !== 0) return true;
	const status = string(structured?.status)?.toLowerCase();
	if (
		status === "error" ||
		status === "failed" ||
		structured?.success === false ||
		structured?.ok === false
	)
		return true;
	const text = string(result)?.trim();
	return text !== undefined && /^(?:error|failed|failure)\b/iu.test(text);
};

/** One reader-facing argument that names the work without dumping provider JSON. */
const tool_detail = (args: RecordValue | undefined) =>
	string(args?.command) ??
	string(args?.path) ??
	string(args?.file_path) ??
	string(args?.pattern) ??
	string(args?.query) ??
	string(args?.url) ??
	string(args?.action) ??
	string(args?.target) ??
	string(args?.name);

type HermesFileChange = Readonly<{
	action: "created" | "deleted" | "modified";
	lines_added?: number;
	lines_deleted?: number;
	path: string;
}>;

/** Parses only V4A file directives; hunk/source text never leaves the adapter. */
const v4a_file_changes = (patch: string): ReadonlyArray<HermesFileChange> => {
	const changes: Array<HermesFileChange> = [];
	let active:
		| { action: HermesFileChange["action"]; added: number; deleted: number; path: string }
		| undefined;
	const finish = () => {
		if (active === undefined) return;
		const completed = active;
		active = undefined;
		changes.push({
			action: completed.action,
			...(completed.action === "deleted"
				? {}
				: { lines_added: completed.added, lines_deleted: completed.deleted }),
			path: completed.path,
		});
	};
	for (const line of patch.split(/\r?\n/u)) {
		const move_to = /^\*\*\*\s*Move to:\s*(.+)\s*$/u.exec(line);
		if (move_to !== null) {
			const destination = move_to[1]?.trim();
			if (active !== undefined && destination !== undefined && destination.length > 0)
				active.path = destination;
			continue;
		}
		const move = /^\*\*\*\s*Move\s+File:\s*(.+?)\s*->\s*(.+)\s*$/u.exec(line);
		if (move !== null) {
			finish();
			const source = move[1]?.trim();
			const destination = move[2]?.trim();
			if (source !== undefined && source.length > 0)
				changes.push({ action: "deleted", path: source });
			if (destination !== undefined && destination.length > 0)
				changes.push({ action: "created", path: destination });
			continue;
		}
		const directive = /^\*\*\*\s*(Update|Add|Delete)\s+File:\s*(.+)\s*$/u.exec(line);
		if (directive !== null) {
			finish();
			const path = directive[2]?.trim();
			if (path === undefined || path.length === 0) {
				active = undefined;
				continue;
			}
			active = {
				action:
					directive[1] === "Add"
						? "created"
						: directive[1] === "Delete"
							? "deleted"
							: "modified",
				added: 0,
				deleted: 0,
				path,
			};
			continue;
		}
		if (active === undefined || line.startsWith("*** ")) continue;
		if (line.startsWith("+")) active.added += 1;
		if (line.startsWith("-")) active.deleted += 1;
	}
	finish();
	return changes;
};

const hermes_file_changes = (
	tool_name: string,
	args: RecordValue | undefined,
): ReadonlyArray<HermesFileChange> => {
	if (args === undefined) return [];
	if (tool_name === "patch") {
		const patch = string(args.patch);
		if (patch !== undefined) return v4a_file_changes(patch);
		const path = string(args.path) ?? string(args.file_path);
		if (path === undefined) return [];
		const old_string = string(args.old_string);
		const new_string = string(args.new_string);
		return [
			{
				action: "modified",
				...(new_string === undefined ? {} : CountWrittenLines(new_string, old_string)),
				path,
			},
		];
	}
	if (tool_name !== "write_file") return [];
	const path = string(args.path) ?? string(args.file_path);
	const content = string(args.content);
	return path === undefined
		? []
		: [
				{
					action: "modified",
					...(content === undefined ? {} : CountWrittenLines(content)),
					path,
				},
			];
};

const usage = (
	base: EngineObservationBase,
	payload: RecordValue | undefined,
	provider_route_id: string,
	turn_id: string,
): EngineUsageObservation | undefined => {
	const source = record(payload?.usage) ?? payload;
	if (source === undefined) return undefined;
	const input_tokens = number(source.input) ?? number(source.input_tokens);
	const output_tokens = number(source.output) ?? number(source.output_tokens);
	const context_tokens = number(source.context_used);
	const context_window_tokens = number(source.context_max);
	const cost_usd = number(source.cost_usd);
	if (
		input_tokens === undefined &&
		output_tokens === undefined &&
		context_tokens === undefined &&
		context_window_tokens === undefined &&
		cost_usd === undefined
	)
		return undefined;
	return {
		...base,
		_tag: "usage",
		basis: "cumulative",
		...(context_tokens === undefined ? {} : { context_tokens }),
		...(context_window_tokens === undefined ? {} : { context_window_tokens }),
		...(cost_usd === undefined ? {} : { cost_usd }),
		...(input_tokens === undefined ? {} : { input_tokens }),
		...(output_tokens === undefined ? {} : { output_tokens }),
		provider_route_id,
		turn_id,
	};
};

const run_error = {
	artisan_code: "AE-RUN-301",
	detail: "Hermes reported that the turn failed.",
} as const;

/** Events Hermes itself treats as proof that a compacted turn resumed. */
const compaction_resume_event_types = new Set([
	"message.start",
	"message.delta",
	"message.interim",
	"thinking.delta",
	"reasoning.delta",
	"reasoning.available",
	"moa.reference",
	"moa.aggregating",
	"moa.progress",
	"moa.phase",
	"tool.start",
	"tool.progress",
	"tool.generating",
	"tool.complete",
]);

/** Stateful projection from Hermes' desktop gateway into Artisan observations. */
export class HermesEventNormalizer {
	readonly #artisan_run_id: string;
	readonly #native_thread_id: string;
	readonly #provider_route_id: string;
	readonly #tool_arguments = new Map<string, RecordValue>();
	readonly #tool_names = new Map<string, string>();
	#anonymous_tool_id: string | undefined;
	#active_compaction_id: string | undefined;
	#compaction_index = 0;
	#frame_sequence = 0;
	#interim_index = 0;
	#turn_index = 0;
	#turn_id: string;

	constructor(input: {
		readonly artisan_run_id: string;
		readonly native_thread_id: string;
		readonly provider_route_id: string;
	}) {
		this.#artisan_run_id = input.artisan_run_id;
		this.#native_thread_id = input.native_thread_id;
		this.#provider_route_id = input.provider_route_id;
		this.#turn_id = `${input.artisan_run_id}:${input.native_thread_id}:turn:0`;
	}

	Normalize(event: HermesGatewayEvent): ReadonlyArray<EngineObservation> {
		this.#frame_sequence += 1;
		const payload = record(event.payload);
		const request_id = string(payload?.request_id);
		const tool_id = string(payload?.tool_id);
		const base: EngineObservationBase = {
			artisan_run_id: this.#artisan_run_id,
			native_thread_id: this.#native_thread_id,
			observation_id: `${this.#artisan_run_id}:hermes:${this.#frame_sequence}:${event.type}`,
			raw: {
				engine_id: "hermes",
				frame: {
					...(request_id === undefined ? {} : { request_id }),
					...(event.session_id === undefined ? {} : { session_id: event.session_id }),
					...(tool_id === undefined ? {} : { tool_id }),
					type: event.type,
				},
				frame_sequence: this.#frame_sequence,
				native_method: event.type,
				protocol_version: "hermes-desktop-contract-6",
				transport: "hermes-jsonrpc-websocket",
			},
			sequence: this.#frame_sequence,
		};
		const message_item_id = `${this.#turn_id}:message`;
		const resumed_compaction_id = this.#active_compaction_id;
		const compaction_resumed =
			resumed_compaction_id !== undefined &&
			(compaction_resume_event_types.has(event.type) ||
				(event.type === "status.update" && string(payload?.kind) === "compacted"));
		const compaction_completion: ReadonlyArray<EngineObservation> = compaction_resumed
			? [
					{
						...base,
						_tag: "compaction",
						compaction_id: resumed_compaction_id,
						state: "completed",
					},
				]
			: [];
		if (compaction_resumed) this.#active_compaction_id = undefined;

		const observations: ReadonlyArray<EngineObservation> = (() => {
			switch (event.type) {
				case "message.start":
					this.#turn_index += 1;
					this.#turn_id = `${this.#artisan_run_id}:${this.#native_thread_id}:turn:${this.#turn_index}`;
					return [
						{ ...base, _tag: "run_state", state: "running" },
						{ ...base, _tag: "turn_state", state: "started", turn_id: this.#turn_id },
					];
				case "message.delta": {
					const delta = string(payload?.text) ?? string(payload?.rendered) ?? "";
					return delta.length === 0
						? []
						: [
								{
									...base,
									_tag: "agent_message_delta",
									delta,
									item_id: `${this.#turn_id}:message`,
									phase: "unspecified",
									turn_id: this.#turn_id,
								},
							];
				}
				case "message.interim": {
					const text = string(payload?.text);
					if (text === undefined || text.length === 0) return [];
					this.#interim_index += 1;
					return [
						{
							...base,
							_tag: "agent_message_completed",
							item_id: `${this.#turn_id}:interim:${this.#interim_index}`,
							message: text,
							phase: "commentary",
							turn_id: this.#turn_id,
						},
					];
				}
				case "message.complete": {
					const text = string(payload?.text);
					const failure = string(payload?.failure_reason);
					const measured = usage(base, payload, this.#provider_route_id, this.#turn_id);
					const observations: Array<EngineObservation> = [];
					if (text !== undefined && text.length > 0)
						observations.push({
							...base,
							_tag: "agent_message_completed",
							item_id: message_item_id,
							message: text,
							phase: "final",
							turn_id: this.#turn_id,
						});
					if (measured !== undefined) observations.push(measured);
					if (this.#active_compaction_id !== undefined) {
						const compaction_id = this.#active_compaction_id;
						this.#active_compaction_id = undefined;
						observations.push({
							...base,
							_tag: "compaction",
							compaction_id,
							state: "completed",
						});
					}
					observations.push({
						...base,
						_tag: "turn_state",
						state: failure === undefined ? "completed" : "failed",
						turn_id: this.#turn_id,
					});
					observations.push({
						...base,
						_tag: "run_terminal",
						...(failure === undefined ? {} : { error_ref: run_error }),
						state: failure === undefined ? "completed" : "failed",
					});
					return observations;
				}
				/** Hermes exposes private model reasoning here, not a reader-authored summary. */
				case "thinking.delta":
				case "reasoning.delta":
				case "reasoning.available":
					return [];
				case "tool.start": {
					const id = tool_id ?? `${this.#turn_id}:tool:${this.#frame_sequence}`;
					if (tool_id === undefined) this.#anonymous_tool_id = id;
					const name = string(payload?.name) ?? "tool";
					const args = payload === undefined ? undefined : tool_arguments(payload);
					const detail = tool_detail(args);
					if (args !== undefined) this.#tool_arguments.set(id, args);
					this.#tool_names.set(id, name);
					const observations: Array<EngineObservation> = [
						{
							...base,
							_tag: "tool",
							action: "started",
							...(detail === undefined ? {} : { detail }),
							tool_id: id,
							tool_name: name,
						},
					];
					const command = string(args?.command);
					if (new Set(["terminal", "shell", "execute_code"]).has(name))
						observations.push({
							...base,
							_tag: "terminal_activity",
							activity_id: id,
							...(command === undefined ? {} : { command }),
							state: "started",
						});
					if (name.includes("search"))
						observations.push({
							...base,
							_tag: "search",
							query: string(args?.query) ?? string(args?.pattern) ?? "",
							scope: name.startsWith("web") ? "web" : "workspace",
							search_id: id,
							state: "started",
						});
					return observations;
				}
				case "tool.progress": {
					const id =
						tool_id ??
						this.#anonymous_tool_id ??
						`${this.#turn_id}:tool:${this.#frame_sequence}`;
					const args = this.#tool_arguments.get(id);
					const detail = tool_detail(args);
					return [
						{
							...base,
							_tag: "tool",
							action: "progress",
							...(detail === undefined ? {} : { detail }),
							tool_id: id,
							tool_name: this.#tool_names.get(id) ?? string(payload?.name) ?? "tool",
						},
					];
				}
				case "tool.complete": {
					const id =
						tool_id ??
						this.#anonymous_tool_id ??
						`${this.#turn_id}:tool:${this.#frame_sequence}`;
					const name = this.#tool_names.get(id) ?? string(payload?.name) ?? "tool";
					const args =
						(payload === undefined ? undefined : tool_arguments(payload)) ??
						this.#tool_arguments.get(id);
					const failed = payload === undefined ? false : tool_failed(payload);
					const detail = tool_detail(args);
					const output = string(payload?.result_text);
					this.#tool_arguments.delete(id);
					this.#tool_names.delete(id);
					if (id === this.#anonymous_tool_id) this.#anonymous_tool_id = undefined;
					const observations: Array<EngineObservation> = [
						{
							...base,
							_tag: "tool",
							action: failed ? "failed" : "completed",
							...(detail === undefined ? {} : { detail }),
							tool_id: id,
							tool_name: name,
						},
					];
					if (new Set(["terminal", "shell", "execute_code"]).has(name))
						observations.push({
							...base,
							_tag: "terminal_activity",
							activity_id: id,
							...(output === undefined ? {} : { output }),
							state: failed ? "failed" : "completed",
						});
					if (name.includes("search"))
						observations.push({
							...base,
							_tag: "search",
							query: "",
							scope: name.startsWith("web") ? "web" : "workspace",
							search_id: id,
							state: "completed",
						});
					if (!failed)
						for (const change of hermes_file_changes(name, args))
							observations.push({ ...base, _tag: "file", ...change });
					return observations;
				}
				case "approval.request": {
					const approval = DecodeHermesApproval(event);
					return approval === undefined
						? []
						: [
								{
									...base,
									_tag: "approval",
									approval_id: approval.id,
									description: approval.description,
									request: {
										...(approval.command === undefined
											? {}
											: { command: approval.command }),
										kind: approval.command === undefined ? "action" : "command",
									},
									state: "requested",
								},
							];
				}
				case "clarify.request":
					return DecodeHermesQuestions(event).map((question) => ({
						...base,
						_tag: "question" as const,
						header: "Hermes question",
						multi_select: question.multi_select,
						...(question.options === undefined
							? {}
							: { options: question.options.map((label) => ({ label })) }),
						question_id: question.id,
						state: "requested" as const,
						text: question.text,
					}));
				case "status.update": {
					if (string(payload?.kind) !== "compacting") return [];
					const compaction_id =
						this.#active_compaction_id ??
						`${this.#turn_id}:compaction:${(this.#compaction_index += 1)}`;
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
				case "session.usage": {
					const measured = usage(base, payload, this.#provider_route_id, this.#turn_id);
					return measured === undefined ? [] : [measured];
				}
				case "subagent.spawn_requested":
				case "subagent.start":
				case "subagent.progress":
				case "subagent.complete": {
					const agent_id = string(payload?.subagent_id);
					if (agent_id === undefined) return [];
					const status = string(payload?.status);
					const activity = string(payload?.goal) ?? string(payload?.summary);
					const state =
						event.type === "subagent.complete"
							? status === "failed"
								? ("failed" as const)
								: ("completed" as const)
							: event.type === "subagent.spawn_requested"
								? ("discovered" as const)
								: ("running" as const);
					return [
						{
							...base,
							_tag: "subagent",
							...(activity === undefined ? {} : { activity }),
							agent_native_thread_id: agent_id,
							parent_native_thread_id:
								string(payload?.parent_id) ?? this.#native_thread_id,
							state,
						},
					];
				}
				case "error":
					return [
						{
							...base,
							_tag: "retry",
							attempt_state: "terminal",
							message: string(payload?.message) ?? "Hermes reported an error.",
							turn_id: this.#turn_id,
							will_retry: false,
						},
						{ ...base, _tag: "run_terminal", error_ref: run_error, state: "failed" },
					];
				default:
					return [];
			}
		})();
		return compaction_completion.length === 0
			? observations
			: [...compaction_completion, ...observations];
	}
}
