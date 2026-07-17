import { Context, Effect, Layer, Schema } from "effect";

import {
	EventEnvelope,
	SurfaceItem,
	type EventEnvelope as EventEnvelopeValue,
} from "@artisan/protocol";

type SurfaceItemValue = typeof SurfaceItem.Type;
type EventPayloadType = EventEnvelopeValue["payload"]["type"];

interface SurfaceItemInput {
	readonly agent_id?: string;
	readonly group: SurfaceItemValue["group"];
	readonly kind: SurfaceItemValue["kind"];
	readonly label: string;
	readonly project_id?: string;
	readonly run_id?: string;
	readonly source?: SurfaceItemValue["source"];
	readonly state: string;
	readonly summary: string;
	readonly surface_id: string;
	readonly usage?: NonNullable<SurfaceItemValue["usage"]>;
	readonly workspace_id?: string;
}

type OrchestrationLifecyclePayload = Extract<
	EventEnvelopeValue["payload"],
	{ readonly type: "orchestration.graph.lifecycle" }
>;

const DecodeEventEnvelope = Schema.decodeUnknownEffect(EventEnvelope, {
	onExcessProperty: "error",
});

const DecodeEventEnvelopes = Schema.decodeUnknownEffect(Schema.Array(EventEnvelope), {
	onExcessProperty: "error",
});

const DecodeSurfaceItems = Schema.decodeUnknownEffect(Schema.Array(SurfaceItem), {
	onExcessProperty: "error",
});

const orchestration_surface_namespaces = {
	agent_run: "agent-run",
	assignment: "assignment",
	join: "agent-join",
	orchestration_group: "agent-group",
} as const satisfies Record<OrchestrationLifecyclePayload["node_type"], string>;

function make_item(envelope: EventEnvelopeValue, input: SurfaceItemInput): SurfaceItemValue {
	const agent_id = input.agent_id ?? envelope.agent_id;
	const run_id = input.run_id ?? envelope.run_id;

	return {
		...(agent_id === undefined ? {} : { agent_id }),
		group: input.group,
		kind: input.kind,
		label: input.label,
		...(input.project_id === undefined ? {} : { project_id: input.project_id }),
		...(envelope.raw_origin === undefined ? {} : { raw_origin: envelope.raw_origin }),
		...(run_id === undefined ? {} : { run_id }),
		source: input.source ?? "artisan",
		state: input.state,
		summary: input.summary,
		surface_id: input.surface_id,
		thread_id: envelope.thread_id,
		timestamp: envelope.sent_at,
		...(input.usage === undefined ? {} : { usage: input.usage }),
		...(input.workspace_id === undefined ? {} : { workspace_id: input.workspace_id }),
	};
}

function orchestration_surface_id(payload: OrchestrationLifecyclePayload): string {
	return `surface:${orchestration_surface_namespaces[payload.node_type]}:${payload.node_id}`;
}

function preserve_tool_metadata(
	tool_item: SurfaceItemValue,
	activity_item: SurfaceItemValue,
): SurfaceItemValue {
	return {
		...activity_item,
		...(tool_item.agent_id === undefined ? {} : { agent_id: tool_item.agent_id }),
		label: tool_item.label,
		...(tool_item.run_id === undefined ? {} : { run_id: tool_item.run_id }),
		source: tool_item.source,
		summary: tool_item.summary,
		...(tool_item.workspace_id === undefined ? {} : { workspace_id: tool_item.workspace_id }),
	};
}

function merge_run_usage(
	lifecycle_item: SurfaceItemValue,
	usage_item: SurfaceItemValue,
): SurfaceItemValue {
	return {
		...lifecycle_item,
		...(usage_item.usage === undefined ? {} : { usage: usage_item.usage }),
	};
}

function project_event(envelope: EventEnvelopeValue): ReadonlyArray<SurfaceItemValue> {
	const payload = envelope.payload;
	const item = (input: SurfaceItemInput) => [make_item(envelope, input)];

	switch (payload.type) {
		case "thread.created":
			return item({
				group: "Work",
				kind: "thread",
				label: "Thread",
				state: "created",
				summary: "Thread created.",
				surface_id: `surface:thread:${envelope.thread_id}`,
			});
		case "thread.metadata.updated":
		case "thread.project_affinity.updated":
			return item({
				group: "Work",
				kind: "thread",
				label: "Thread",
				...(payload.thread.primary_project === undefined
					? {}
					: { project_id: payload.thread.primary_project.project_id }),
				state: payload.change,
				summary: "Thread updated.",
				surface_id: `surface:thread:${payload.thread.thread_id}`,
			});
		case "thread.content_erased":
		case "thread.erased":
		case "thread.refinement.ignored":
		case "thread.project_affinity.ignored":
			return [];
		case "thread.retention.updated":
			return item({
				group: "Settings",
				kind: "setting",
				label: "Setting",
				state: payload.policy.enabled ? "enabled" : "disabled",
				summary: "Setting updated.",
				surface_id: "surface:setting:thread-retention",
			});
		case "guidance.canonical.updated":
			return item({
				group: "Guidance",
				kind: "guidance",
				label: "Guidance",
				state: "updated",
				summary: "Guidance updated.",
				surface_id: "surface:guidance:global",
			});
		case "guidance.selection.required":
			return item({
				group: "Guidance",
				kind: "guidance",
				label: "Guidance",
				state: "selection_required",
				summary: "Guidance selection required.",
				surface_id: "surface:guidance:global",
			});
		case "guidance.provider.reconciled":
			return item({
				group: "Guidance",
				kind: "guidance",
				label: "Guidance",
				state: payload.status,
				summary: "Guidance reconciled.",
				surface_id: "surface:guidance:global",
			});
		case "model_behaviour.setting.updated":
			return item({
				group: "Settings",
				kind: "setting",
				label: "Setting",
				state: "updated",
				summary: "Setting updated.",
				surface_id: `surface:setting:model-behaviour:${payload.setting_id}`,
			});
		case "model_behaviour.provider.reconciled":
			return item({
				group: "Settings",
				kind: "setting",
				label: "Setting",
				state: payload.status,
				summary: "Setting reconciled.",
				surface_id: `surface:setting:model-behaviour:${payload.setting_id}`,
			});
		case "workspace.change.updated":
			return item({
				agent_id: payload.change.agent_id,
				group: "Changes",
				kind: "change",
				label: "Change",
				run_id: payload.change.run_id,
				state: payload.change.review_state,
				summary: "Change updated.",
				surface_id: `surface:change:${payload.change.change_id}`,
				workspace_id: payload.change.workspace_id,
			});
		case "workspace.replace.approval.updated":
			return item({
				agent_id: payload.approval.agent_id,
				group: "Permissions",
				kind: "approval",
				label: "Approval",
				run_id: payload.approval.run_id,
				state: payload.approval.state,
				summary: "Approval updated.",
				surface_id: `surface:approval:${payload.approval.approval_id}`,
				workspace_id: payload.approval.workspace_id,
			});
		case "workspace.git.session.updated":
			return item({
				group: "Workspace",
				kind: "workspace",
				label: "Workspace",
				state: payload.session.state,
				summary: "Workspace updated.",
				surface_id: `surface:workspace:${payload.session.workspace_id}`,
				workspace_id: payload.session.workspace_id,
			});
		case "workspace.git.checkout.approval.updated":
		case "workspace.git.mutation.approval.updated":
		case "hosted.git.mutation.approval.updated":
			return item({
				group: "Permissions",
				kind: "approval",
				label: "Approval",
				state: payload.approval.state,
				summary: "Approval updated.",
				surface_id: `surface:approval:${payload.approval.approval_id}`,
				workspace_id: payload.approval.workspace_id,
			});
		case "hosted.project.clone.approval.updated":
			return item({
				group: "Permissions",
				kind: "approval",
				label: "Approval",
				...("project" in payload.approval
					? { project_id: payload.approval.project.project_id }
					: {}),
				state: payload.approval.state,
				summary: "Approval updated.",
				surface_id: `surface:approval:${payload.approval.approval_id}`,
			});
		case "workspace.git.fetch.policy.updated":
			return item({
				group: "Settings",
				kind: "setting",
				label: "Setting",
				state: payload.enabled ? "enabled" : "disabled",
				summary: "Setting updated.",
				surface_id: "surface:setting:workspace-git-fetch",
			});
		case "workspace.git.fetch.requested":
			return item({
				group: "Workspace",
				kind: "workspace",
				label: "Workspace",
				state: "fetch_requested",
				summary: "Workspace updated.",
				surface_id: `surface:workspace:${payload.workspace_id}`,
				workspace_id: payload.workspace_id,
			});
		case "workspace.git.fetch.completed":
			return item({
				group: "Workspace",
				kind: "workspace",
				label: "Workspace",
				state: payload.attempt.result,
				summary: "Workspace updated.",
				surface_id: `surface:workspace:${payload.workspace_id}`,
				workspace_id: payload.workspace_id,
			});
		case "hosted.git.snapshot.updated":
			return item({
				group: "Workspace",
				kind: "workspace",
				label: "Workspace",
				project_id: payload.snapshot.project_id,
				state: payload.snapshot.workspace_freshness,
				summary: "Workspace updated.",
				surface_id: `surface:workspace:${payload.snapshot.workspace_id}`,
				workspace_id: payload.snapshot.workspace_id,
			});
		case "external_wait.updated":
			return item({
				agent_id: payload.snapshot.owner.agent_id,
				group: "Time",
				kind: "timer",
				label: "Timer",
				project_id: payload.snapshot.project_id,
				run_id: payload.snapshot.owner.run_id,
				state: payload.snapshot.state._tag,
				summary: "Timer updated.",
				surface_id: `surface:timer:${payload.snapshot.wait_id}`,
				workspace_id: payload.snapshot.workspace_id,
			});
		case "thread.message_queued":
			return item({
				group: "Work",
				kind: "message",
				label: "Message",
				state: "queued",
				summary: "Message queued.",
				surface_id: `surface:message:${payload.message_id}`,
			});
		case "thread.message_steering":
			return item({
				group: "Work",
				kind: "message",
				label: "Message",
				state: "steering",
				summary: "Message sent.",
				surface_id: `surface:message:${payload.message_id}`,
			});
		case "assistant.message_completed":
			return item({
				group: "Work",
				kind: "message",
				label: "Message",
				state: "completed",
				summary: "Message completed.",
				surface_id: `surface:message:${payload.message_id}`,
			});
		case "run.lifecycle":
			if (envelope.run_id === undefined) {
				return [];
			}

			return item({
				group: "Work",
				kind: "run",
				label: "Run",
				state: payload.state,
				summary: "Run updated.",
				surface_id: `surface:run:${envelope.run_id}`,
			});
		case "run.usage.updated":
			if (envelope.run_id === undefined) {
				return [];
			}

			return item({
				group: "Work",
				kind: "run",
				label: "Run",
				state: "updated",
				summary: "Run usage updated.",
				surface_id: `surface:run:${envelope.run_id}`,
				usage: payload.usage,
			});
		case "interaction.approval":
			return item({
				group: "Permissions",
				kind: "approval",
				label: "Approval",
				state: payload.state,
				summary: "Approval updated.",
				surface_id: `surface:approval:${payload.approval_id}`,
			});
		case "interaction.question":
			return item({
				group: "Work",
				kind: "question",
				label: "Question",
				state: payload.state,
				summary: "Question updated.",
				surface_id: `surface:question:${payload.question_id}`,
			});
		case "filesystem.mutation":
			return item({
				group: "Changes",
				kind: "change",
				label: "Change",
				state: payload.operation,
				summary: "Change updated.",
				surface_id: `surface:change:filesystem:${envelope.causation_id}`,
			});
		case "process.ownership":
			return item({
				group: "Processes",
				kind: "process",
				label: "Process",
				source: payload.source === "engine" ? "engine" : "artisan",
				state: "observed",
				summary: "Process observed.",
				surface_id: `surface:process:ownership:${envelope.causation_id}`,
			});
		case "git.workspace.observed":
			return item({
				group: "Workspace",
				kind: "workspace",
				label: "Workspace",
				state: payload.has_diff ? "changed" : "clean",
				summary: "Workspace observed.",
				surface_id: `surface:workspace:observation:${envelope.causation_id}`,
			});
		case "terminal.lifecycle":
			return item({
				group: "Processes",
				kind: "process",
				label: "Process",
				state: payload.action,
				summary: "Process updated.",
				surface_id: `surface:process:terminal:${payload.terminal.terminal_id}`,
				workspace_id: payload.terminal.workspace_id,
			});
		case "orchestration.graph.lifecycle":
			return item({
				group: "Agents",
				kind: "agent",
				label: "Agent",
				...(payload.node_type === "agent_run" ? { run_id: payload.node_id } : {}),
				state: payload.state,
				summary: "Agent updated.",
				surface_id: orchestration_surface_id(payload),
			});
		case "assignment.heartbeat":
			return item({
				group: "Agents",
				kind: "agent",
				label: "Agent",
				state: "updated",
				summary: "Agent updated.",
				surface_id: `surface:assignment:${payload.assignment_id}`,
			});
		case "agent_instance.renamed":
			return item({
				agent_id: payload.agent_id,
				group: "Agents",
				kind: "agent",
				label: "Agent",
				state: "renamed",
				summary: "Agent renamed.",
				surface_id: `surface:agent:${payload.agent_id}`,
			});
		case "assignment.control":
			return item({
				group: "Agents",
				kind: "agent",
				label: "Agent",
				state: payload.outcome,
				summary: "Agent updated.",
				surface_id: `surface:assignment:${payload.assignment_id}`,
			});
		case "artifact.recorded":
			return item({
				group: "Knowledge",
				kind: "knowledge",
				label: "Knowledge",
				run_id: payload.artifact.run_id,
				state: "recorded",
				summary: "Knowledge captured.",
				surface_id: `surface:knowledge:${payload.artifact.artifact_id}`,
			});
		case "preview.target.updated":
			return item({
				group: "Workspace",
				kind: "preview",
				label: "Preview",
				project_id: payload.target.project_id,
				state: payload.target.state,
				summary: "Preview updated.",
				surface_id: `surface:preview:${payload.target.target_id}`,
				workspace_id: payload.target.workspace_id,
			});
		case "preview.browser.launch.updated":
			return item({
				...(payload.launch.initiator.kind === "agent"
					? { agent_id: payload.launch.initiator.agent_id }
					: {}),
				group: "Workspace",
				kind: "preview",
				label: "Preview",
				project_id: payload.launch.project_id,
				state: payload.launch.state,
				summary: "Preview updated.",
				surface_id: `surface:preview-launch:${payload.launch.launch_id}`,
				workspace_id: payload.launch.workspace_id,
			});
		case "preview.inspection.updated":
			return item({
				...(payload.inspection.initiator.kind === "agent"
					? { agent_id: payload.inspection.initiator.agent_id }
					: {}),
				group: "Workspace",
				kind: "preview",
				label: "Preview",
				project_id: payload.inspection.project_id,
				state: payload.inspection.state,
				summary: "Preview updated.",
				surface_id: `surface:preview-inspection:${payload.inspection.inspection_id}`,
				workspace_id: payload.inspection.workspace_id,
			});
		case "capability.invocation.updated":
			return item({
				group: "Capabilities",
				kind: "capability",
				label: payload.label,
				source: payload.source,
				state: payload.state,
				summary: payload.summary ?? "Capability updated.",
				surface_id: `surface:capability:${payload.invocation_id}`,
			});
		case "engine.native_action.observed":
			return item({
				group: "Capabilities",
				kind: "capability",
				label: payload.label,
				source: payload.source,
				state: payload.state,
				summary: payload.summary ?? "Capability observed.",
				surface_id: `surface:native-action:${payload.action_id}`,
			});
		case "tool.invocation.updated":
			return item({
				agent_id: payload.invocation.context.agent_id,
				group: "Capabilities",
				kind: "capability",
				label: payload.invocation.tool.label,
				run_id: payload.invocation.context.run_id,
				source: payload.invocation.tool.source,
				state: payload.invocation.state,
				summary: payload.invocation.tool.summary,
				surface_id: `surface:capability:${payload.invocation.invocation_id}`,
				...(payload.invocation.context.workspace_id === undefined
					? {}
					: { workspace_id: payload.invocation.context.workspace_id }),
			});
		case "tool.approval.updated":
			return item({
				agent_id: payload.approval.context.agent_id,
				group: "Permissions",
				kind: "approval",
				label: "Approval",
				run_id: payload.approval.context.run_id,
				source: payload.approval.tool.source,
				state: payload.approval.state,
				summary: payload.approval.tool.summary,
				surface_id: `surface:approval:${payload.approval.approval_id}`,
				...(payload.approval.context.workspace_id === undefined
					? {}
					: { workspace_id: payload.approval.context.workspace_id }),
			});
		case "marketplace.lifecycle.updated":
			return item({
				group: payload.entry_kind === "routine" ? "Routines" : "Capabilities",
				kind: payload.entry_kind === "routine" ? "routine" : "capability",
				label: payload.entry_kind === "routine" ? "Routine" : "MCP capability",
				source: "marketplace",
				state: payload.lifecycle,
				summary:
					payload.summary ??
					(payload.entry_kind === "routine"
						? "Routine updated."
						: "MCP capability updated."),
				surface_id:
					payload.entry_kind === "routine"
						? `surface:routine:${payload.entry_id}`
						: `surface:marketplace-capability:${payload.entry_id}`,
			});
		case "marketplace.invocation.updated":
			return item({
				group: payload.entry_kind === "routine" ? "Routines" : "Capabilities",
				kind: payload.entry_kind === "routine" ? "routine" : "capability",
				label: payload.entry_kind === "routine" ? "Routine invocation" : "MCP invocation",
				source: "marketplace",
				state: payload.state,
				summary:
					payload.summary ??
					(payload.entry_kind === "routine"
						? "Routine invocation updated."
						: "MCP invocation updated."),
				surface_id: `surface:marketplace-invocation:${payload.invocation_id}`,
			});
		default: {
			const exhaustive: never = payload;

			return exhaustive;
		}
	}
}

function reduce_events(
	envelopes: ReadonlyArray<EventEnvelopeValue>,
): ReadonlyArray<SurfaceItemValue> {
	const items: SurfaceItemValue[] = [];
	const item_indices = new Map<string, number>();
	const full_tool_metadata = new Set<string>();
	const run_lifecycle_items = new Set<string>();

	for (const envelope of envelopes) {
		const payload_type: EventPayloadType = envelope.payload.type;
		const projected = project_event(envelope);

		for (const next_item of projected) {
			const existing_index = item_indices.get(next_item.surface_id);

			if (existing_index === undefined) {
				item_indices.set(next_item.surface_id, items.length);
				items.push(next_item);
			} else {
				const existing_item = items[existing_index]!;
				let reconciled = next_item;

				if (
					payload_type === "run.usage.updated" &&
					run_lifecycle_items.has(next_item.surface_id)
				) {
					reconciled = merge_run_usage(existing_item, next_item);
				} else if (payload_type === "run.lifecycle" && existing_item.usage !== undefined) {
					reconciled = merge_run_usage(next_item, existing_item);
				} else if (
					payload_type === "capability.invocation.updated" &&
					full_tool_metadata.has(next_item.surface_id)
				) {
					reconciled = preserve_tool_metadata(existing_item, next_item);
				}

				items[existing_index] = reconciled;
			}

			if (payload_type === "run.lifecycle") {
				run_lifecycle_items.add(next_item.surface_id);
			}

			if (payload_type === "tool.invocation.updated") {
				full_tool_metadata.add(next_item.surface_id);
			}
		}
	}

	return items;
}

/** Projects strict event envelopes into canonical source-safe Artisan surface items. */
export class SurfaceProjector extends Context.Service<
	SurfaceProjector,
	{
		readonly Project: (
			input: unknown,
		) => Effect.Effect<ReadonlyArray<SurfaceItemValue>, Schema.SchemaError>;
		readonly ProjectMany: (
			inputs: ReadonlyArray<unknown>,
		) => Effect.Effect<ReadonlyArray<SurfaceItemValue>, Schema.SchemaError>;
	}
>()("Artisan/SurfaceProjector") {}

/** Provides the total canonical event-to-surface projection. */
export const SurfaceProjectorLive = Layer.succeed(SurfaceProjector, {
	Project: (input) =>
		DecodeEventEnvelope(input).pipe(
			Effect.flatMap((envelope) => DecodeSurfaceItems(project_event(envelope))),
		),
	ProjectMany: (inputs) =>
		DecodeEventEnvelopes(inputs).pipe(
			Effect.flatMap((envelopes) => DecodeSurfaceItems(reduce_events(envelopes))),
		),
});
