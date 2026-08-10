import { Option, Schema } from "effect";

import type { EngineRawProvenance, EngineSubagentObservation } from "../engine";

/** The subset of a Claude normalization input needed by native task lifecycle events. */
export interface ClaudeTaskNormalizationInput {
	readonly artisan_run_id: string;
	readonly frame_sequence: number;
	readonly native_thread_id: string;
	readonly native_subagent_task?: boolean;
	readonly subagent_parent_native_thread_id?: string;
	readonly payload: unknown;
	readonly protocol_version?: string;
	readonly raw_frame_base64: string;
	readonly transport?: string;
}

/** CLI task events carry a stable task id and the Task/Agent tool invocation that spawned it. */
const TaskStartedSchema = Schema.Struct({
	type: Schema.Literal("system"),
	subtype: Schema.Literal("task_started"),
	task_id: Schema.String,
	tool_use_id: Schema.optional(Schema.String),
	description: Schema.String,
	subagent_type: Schema.optional(Schema.String),
	task_type: Schema.optional(Schema.String),
});
/** A task start's tool invocation links it to a root or nested parent task. @since 0.8.0 */
export type ClaudeTaskStarted = Schema.Schema.Type<typeof TaskStartedSchema>;

const TaskProgressSchema = Schema.Struct({
	type: Schema.Literal("system"),
	subtype: Schema.Literal("task_progress"),
	task_id: Schema.String,
	tool_use_id: Schema.optional(Schema.String),
	description: Schema.String,
	subagent_type: Schema.optional(Schema.String),
});
const TaskUpdatedSchema = Schema.Struct({
	type: Schema.Literal("system"),
	subtype: Schema.Literal("task_updated"),
	task_id: Schema.String,
	patch: Schema.Struct({
		status: Schema.optional(
			Schema.Union([
				Schema.Literal("pending"),
				Schema.Literal("running"),
				Schema.Literal("completed"),
				Schema.Literal("failed"),
				Schema.Literal("killed"),
				Schema.Literal("paused"),
			]),
		),
		description: Schema.optional(Schema.String),
	}),
});
const TaskNotificationSchema = Schema.Struct({
	type: Schema.Literal("system"),
	subtype: Schema.Literal("task_notification"),
	task_id: Schema.String,
	status: Schema.Union([
		Schema.Literal("completed"),
		Schema.Literal("failed"),
		Schema.Literal("stopped"),
	]),
	summary: Schema.String,
});

type DecodedSchema<S extends Schema.ConstraintDecoder<unknown>> = S["Type"] | undefined;

function decode<S extends Schema.ConstraintDecoder<unknown>>(
	schema: S,
	value: unknown,
): DecodedSchema<S> {
	const result = Schema.decodeUnknownOption(schema, { onExcessProperty: "preserve" })(value);
	return Option.isSome(result) ? result.value : undefined;
}

/** Reads the native task lifecycle identity before root/child transcript routing. @since 0.8.0 */
export function read_claude_task_started(payload: unknown): ClaudeTaskStarted | undefined {
	return decode(TaskStartedSchema, payload);
}

/** Reads a task identity from any CLI lifecycle event. @since 0.8.0 */
export function read_claude_task_id(payload: unknown): string | undefined {
	return (
		decode(TaskStartedSchema, payload)?.task_id ??
		decode(TaskProgressSchema, payload)?.task_id ??
		decode(TaskUpdatedSchema, payload)?.task_id ??
		decode(TaskNotificationSchema, payload)?.task_id
	);
}

/**
 * Identifies provider-authored metadata that proves a generic CLI background
 * task is an agent. Tool correlation remains the stronger signal for default
 * Agent/Task invocations, whose `subagent_type` is optional.
 *
 * @since 0.8.0
 */
export function has_claude_subagent_lifecycle_hint(payload: unknown) {
	const started = decode(TaskStartedSchema, payload);
	if (started !== undefined)
		return started.subagent_type !== undefined || started.task_type === "remote_agent";

	return decode(TaskProgressSchema, payload)?.subagent_type !== undefined;
}

type ClaudeTaskLifecycle =
	| {
			readonly _tag: "observation";
			readonly activity?: string;
			readonly agent_path?: string;
			readonly native_method: string;
			readonly state: EngineSubagentObservation["state"];
			readonly task_id: string;
	  }
	| { readonly _tag: "silent" };

function read_task_lifecycle(payload: unknown): ClaudeTaskLifecycle | undefined {
	const started = decode(TaskStartedSchema, payload);
	if (started !== undefined)
		return {
			_tag: "observation",
			activity: started.description,
			...(started.subagent_type === undefined ? {} : { agent_path: started.subagent_type }),
			native_method: "system.task_started",
			state: "running",
			task_id: started.task_id,
		};

	const progress = decode(TaskProgressSchema, payload);
	if (progress !== undefined)
		return {
			_tag: "observation",
			activity: progress.description,
			...(progress.subagent_type === undefined ? {} : { agent_path: progress.subagent_type }),
			native_method: "system.task_progress",
			state: "running",
			task_id: progress.task_id,
		};

	const updated = decode(TaskUpdatedSchema, payload);
	if (updated !== undefined) {
		if (updated.patch.status === undefined) return { _tag: "silent" };
		const state =
			updated.patch.status === "pending"
				? "discovered"
				: updated.patch.status === "running"
					? "running"
					: updated.patch.status === "completed"
						? "completed"
						: updated.patch.status === "paused"
							? "waiting"
							: updated.patch.status === "failed"
								? "failed"
								: "interrupted";
		return {
			_tag: "observation",
			...(updated.patch.description === undefined
				? {}
				: { activity: updated.patch.description }),
			native_method: "system.task_updated",
			state,
			task_id: updated.task_id,
		};
	}

	const notification = decode(TaskNotificationSchema, payload);
	if (notification === undefined) return undefined;
	return {
		_tag: "observation",
		activity: notification.summary,
		native_method: "system.task_notification",
		state:
			notification.status === "completed"
				? "completed"
				: notification.status === "failed"
					? "failed"
					: "interrupted",
		task_id: notification.task_id,
	};
}

/**
 * Canonicalizes only lifecycle frames; `undefined` means the payload belongs
 * to another Claude normalizer family, while an empty array is a recognized
 * generic task that must stay outside the agent roster.
 *
 * @since 0.8.0
 */
export function normalize_claude_task_lifecycle(
	input: ClaudeTaskNormalizationInput,
): ReadonlyArray<EngineSubagentObservation> | undefined {
	const lifecycle = read_task_lifecycle(input.payload);
	if (
		lifecycle === undefined ||
		input.native_subagent_task !== true ||
		lifecycle._tag === "silent"
	)
		return lifecycle === undefined ? undefined : [];

	const raw: EngineRawProvenance = {
		engine_id: "claude",
		frame: input.payload,
		frame_sequence: input.frame_sequence,
		native_method: lifecycle.native_method,
		protocol_version: input.protocol_version ?? "claude-stream-json-v1",
		raw_frame_base64: input.raw_frame_base64,
		transport: input.transport ?? "claude-cli-stream-json",
	};
	return [
		{
			_tag: "subagent",
			agent_native_thread_id: lifecycle.task_id,
			artisan_run_id: input.artisan_run_id,
			native_thread_id: input.native_thread_id,
			observation_id: [
				input.artisan_run_id,
				"claude",
				String(input.frame_sequence),
				lifecycle.task_id,
			].join(":"),
			parent_native_thread_id:
				input.subagent_parent_native_thread_id ?? input.native_thread_id,
			raw,
			sequence: 0,
			...(lifecycle.activity === undefined ? {} : { activity: lifecycle.activity }),
			...(lifecycle.agent_path === undefined ? {} : { agent_path: lifecycle.agent_path }),
			state: lifecycle.state,
		},
	];
}
