import { Schema } from "effect";

import {
	CapabilityEffect,
	CapabilityIdentifier,
	CapabilitySafeSummary,
	CapabilityVisibleLabel,
} from "./capability";
import { Identifier, IsoDateTime, PositiveInt } from "./common";

const text_encoder = new TextEncoder();

/** Defines the maximum UTF-8 byte count accepted for one private tool JSON value. */
export const tool_json_maximum_bytes = 64 * 1024;

/** Defines the maximum nesting depth accepted for one private tool JSON value. */
export const tool_json_maximum_depth = 32;

/** Defines the maximum number of values accepted for one private tool JSON value. */
export const tool_json_maximum_nodes = 10_000;

/** Defines the maximum UTF-8 byte count for one stable eligibility reason code. */
export const tool_reason_code_maximum_bytes = 128;

const unsafe_json_property_names = new Set(["__proto__", "constructor", "prototype"]);

function is_bounded_tool_json(value: unknown): value is Schema.Json {
	let node_count = 0;
	const active = new WeakSet<object>();

	function visit(current: unknown, depth: number): number | undefined {
		node_count += 1;

		if (node_count > tool_json_maximum_nodes || depth > tool_json_maximum_depth) {
			return undefined;
		}

		if (current === null) {
			return 4;
		}

		if (typeof current === "string") {
			if (current.length > tool_json_maximum_bytes) {
				return undefined;
			}

			return text_encoder.encode(JSON.stringify(current)).byteLength;
		}

		if (typeof current === "number") {
			return Number.isFinite(current) ? JSON.stringify(current).length : undefined;
		}

		if (typeof current === "boolean") {
			return current ? 4 : 5;
		}

		if (typeof current !== "object" || active.has(current)) {
			return undefined;
		}

		active.add(current);

		if (Array.isArray(current)) {
			if (
				Object.getPrototypeOf(current) !== Array.prototype ||
				Object.getOwnPropertySymbols(current).length > 0 ||
				current.length > tool_json_maximum_nodes
			) {
				return undefined;
			}

			const property_names = Object.getOwnPropertyNames(current);

			if (
				property_names.length !== current.length + 1 ||
				!property_names.includes("length")
			) {
				return undefined;
			}

			let byte_count = 2 + Math.max(0, current.length - 1);

			for (let index = 0; index < current.length; index += 1) {
				const descriptor = Object.getOwnPropertyDescriptor(current, String(index));

				if (
					descriptor === undefined ||
					!descriptor.enumerable ||
					!("value" in descriptor)
				) {
					return undefined;
				}

				const entry_bytes = visit(descriptor.value, depth + 1);

				if (entry_bytes === undefined) {
					return undefined;
				}

				byte_count += entry_bytes;

				if (byte_count > tool_json_maximum_bytes) {
					return undefined;
				}
			}

			active.delete(current);

			return byte_count;
		}

		const prototype = Object.getPrototypeOf(current);
		const property_names = Object.getOwnPropertyNames(current);
		const property_symbols = Object.getOwnPropertySymbols(current);

		if (
			(prototype !== Object.prototype && prototype !== null) ||
			property_symbols.length > 0 ||
			property_names.length > tool_json_maximum_nodes
		) {
			return undefined;
		}

		let byte_count = 2 + Math.max(0, property_names.length - 1);

		for (const property_name of property_names) {
			const descriptor = Object.getOwnPropertyDescriptor(current, property_name);

			if (
				unsafe_json_property_names.has(property_name) ||
				descriptor === undefined ||
				!descriptor.enumerable ||
				!("value" in descriptor)
			) {
				return undefined;
			}

			const property_bytes = text_encoder.encode(JSON.stringify(property_name)).byteLength;
			const value_bytes = visit(descriptor.value, depth + 1);

			if (value_bytes === undefined) {
				return undefined;
			}

			byte_count += property_bytes + 1 + value_bytes;

			if (byte_count > tool_json_maximum_bytes) {
				return undefined;
			}
		}

		active.delete(current);

		return byte_count;
	}

	try {
		const byte_count = visit(value, 0);

		return byte_count !== undefined && byte_count <= tool_json_maximum_bytes;
	} catch {
		return false;
	}
}

/** Validates a bounded stable reason code without provider diagnostics. */
export const ToolReasonCode = CapabilityIdentifier.check(
	Schema.makeFilter<string>((value) =>
		text_encoder.encode(value).byteLength > tool_reason_code_maximum_bytes ||
		!/^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*$/u.test(value)
			? `Expected a tool reason code within ${tool_reason_code_maximum_bytes} UTF-8 bytes`
			: undefined,
	),
);

/** Validates one bounded strict JSON value retained only within the tool control plane. */
export const ToolJsonValue = Schema.declare<Schema.Json>(is_bounded_tool_json, {
	expected: `strict JSON within ${tool_json_maximum_bytes} UTF-8 bytes, ${tool_json_maximum_depth} levels, and ${tool_json_maximum_nodes} values`,
});

/** Validates one bounded strict JSON object supplied as private tool arguments. */
export const ToolArguments = ToolJsonValue.check(
	Schema.makeFilter<Schema.Json>((value) =>
		typeof value !== "object" || value === null || Array.isArray(value)
			? "Expected tool arguments to be a JSON object"
			: undefined,
	),
);

function tool_input_schema_error(value: Schema.Json): string | undefined {
	if (typeof value !== "object" || value === null || Array.isArray(value)) {
		return "Expected a JSON Schema object for tool input";
	}

	const type = Object.getOwnPropertyDescriptor(value, "type")?.value;
	const properties = Object.getOwnPropertyDescriptor(value, "properties")?.value;
	const required = Object.getOwnPropertyDescriptor(value, "required")?.value;

	if (type !== "object") {
		return 'Expected a tool input schema with root type "object"';
	}

	if (
		properties !== undefined &&
		(typeof properties !== "object" || properties === null || Array.isArray(properties))
	) {
		return "Expected tool input schema properties to be an object";
	}

	if (
		required !== undefined &&
		(!Array.isArray(required) ||
			required.some((entry) => typeof entry !== "string") ||
			new Set(required).size !== required.length)
	) {
		return "Expected unique string names in tool input schema required fields";
	}

	return undefined;
}

/** Carries a bounded revision-owned JSON Schema for constructing tool arguments. */
export const ToolInputSchema = ToolArguments.check(
	Schema.makeFilter<Schema.Json>(tool_input_schema_error),
);

export type ToolInputSchema = typeof ToolInputSchema.Type;

/** Validates one bounded strict JSON value retained as a private completed tool result. */
export const ToolResult = ToolJsonValue;

const ToolPublicDescriptorFields = {
	approval_policy: Schema.Literals(["automatic", "required"]),
	effect: CapabilityEffect,
	label: CapabilityVisibleLabel,
	revision: PositiveInt,
	source: Schema.Literals(["artisan", "marketplace"]),
	summary: CapabilitySafeSummary,
	tool_id: CapabilityIdentifier,
};

/** Identifies source-safe curated metadata for one canonical tool revision. */
export const ToolPublicDescriptor = Schema.Struct(ToolPublicDescriptorFields);

export type ToolPublicDescriptor = typeof ToolPublicDescriptor.Type;

/** Identifies a callable canonical tool supplied by Artisan or the marketplace. */
export const ToolDescriptor = Schema.Struct({
	...ToolPublicDescriptorFields,
	input_schema: ToolInputSchema,
});

export type ToolDescriptor = typeof ToolDescriptor.Type;

/** Binds a request or projection to one immutable tool descriptor revision. */
export const ToolDescriptorReference = Schema.Struct({
	revision: PositiveInt,
	tool_id: CapabilityIdentifier,
});

export type ToolDescriptorReference = typeof ToolDescriptorReference.Type;

/** Attributes a tool operation to the engine run that requested it. */
export const ToolInvocationContext = Schema.Struct({
	agent_id: Identifier,
	run_id: Identifier,
	thread_id: Identifier,
	workspace_id: Schema.optional(Identifier),
});

export type ToolInvocationContext = typeof ToolInvocationContext.Type;

/** Requests the canonical tools eligible for one engine-run context. */
export const ListEligibleRequest = Schema.Struct({
	context: ToolInvocationContext,
});

export type ListEligibleRequest = typeof ListEligibleRequest.Type;

/** Describes an eligible canonical tool. */
export const EligibleTool = Schema.Struct({
	descriptor: ToolDescriptor,
	state: Schema.Literal("eligible"),
});

/** Describes an unavailable canonical tool using a stable source-safe reason code. */
export const UnavailableTool = Schema.Struct({
	descriptor: ToolDescriptor,
	reason_code: ToolReasonCode,
	state: Schema.Literal("unavailable"),
});

/** Describes eligibility for one canonical tool in an engine-run context. */
export const ToolEligibility = Schema.Union([EligibleTool, UnavailableTool]);

export type ToolEligibility = typeof ToolEligibility.Type;

/** Returns the bounded canonical eligibility set for one engine-run context. */
export const ListEligibleResult = Schema.Struct({
	tools: Schema.Array(ToolEligibility).check(
		Schema.makeFilter<ReadonlyArray<ToolEligibility>>((tools) =>
			tools.length > 512
				? "Expected at most 512 eligible tools"
				: new Set(tools.map(({ descriptor }) => descriptor.tool_id)).size !== tools.length
					? "Expected eligible tool identifiers to be unique"
					: undefined,
		),
	),
});

export type ListEligibleResult = typeof ListEligibleResult.Type;

/** Requests an exact-replay invocation of one immutable canonical tool revision. */
export const InvokeRequest = Schema.Struct({
	arguments: ToolArguments,
	context: ToolInvocationContext,
	request_id: Identifier,
	tool: ToolDescriptorReference,
});

export type InvokeRequest = typeof InvokeRequest.Type;

const ToolInvocationProjectionBase = {
	context: ToolInvocationContext,
	created_at: IsoDateTime,
	invocation_id: Identifier,
	request_id: Identifier,
	tool: ToolPublicDescriptor,
	updated_at: IsoDateTime,
};

/** Binds an execution state to the immutable approval decision that authorized it. */
export const ToolApprovalDecisionReference = Schema.Struct({
	approval_id: Identifier,
	context: ToolInvocationContext,
	decided_at: IsoDateTime,
	decision: Schema.Literals(["approved", "denied"]),
	decision_id: Identifier,
	invocation_id: Identifier,
	request_id: Identifier,
	tool: ToolDescriptorReference,
});

export type ToolApprovalDecisionReference = typeof ToolApprovalDecisionReference.Type;

interface ToolLifecycleTimes {
	readonly approval?: ToolApprovalDecisionReference | undefined;
	readonly created_at: string;
	readonly decided_at?: string;
	readonly settled_at?: string;
	readonly started_at?: string;
	readonly suspended_at?: string;
	readonly updated_at: string;
}

function lifecycle_time_error(value: ToolLifecycleTimes): string | undefined {
	const created_at = Date.parse(value.created_at);
	const updated_at = Date.parse(value.updated_at);
	const phase_times = [
		value.approval?.decided_at ?? value.decided_at,
		value.started_at,
		value.suspended_at,
		value.settled_at,
	].filter((timestamp): timestamp is string => timestamp !== undefined);
	let previous = created_at;

	for (const timestamp of phase_times) {
		const current = Date.parse(timestamp);

		if (current < previous || current > updated_at) {
			return "Expected tool lifecycle timestamps in chronological order";
		}

		previous = current;
	}

	return created_at > updated_at
		? "Expected tool lifecycle timestamps in chronological order"
		: undefined;
}

function approval_binding_error(
	value: {
		readonly approval?: ToolApprovalDecisionReference | undefined;
		readonly context: ToolInvocationContext;
		readonly invocation_id: string;
		readonly request_id: string;
		readonly tool: ToolPublicDescriptor;
	},
	expected_decision: "approved" | "denied",
): string | undefined {
	if (value.tool.approval_policy === "automatic") {
		return value.approval === undefined
			? undefined
			: "Expected no approval decision for an automatic tool";
	}

	const approval = value.approval;

	if (approval === undefined) {
		return "Expected an approval decision for a required-approval tool";
	}

	const context_matches =
		approval.context.agent_id === value.context.agent_id &&
		approval.context.run_id === value.context.run_id &&
		approval.context.thread_id === value.context.thread_id &&
		approval.context.workspace_id === value.context.workspace_id;

	return approval.decision !== expected_decision ||
		approval.invocation_id !== value.invocation_id ||
		approval.request_id !== value.request_id ||
		approval.tool.revision !== value.tool.revision ||
		approval.tool.tool_id !== value.tool.tool_id ||
		!context_matches
		? "Expected the invocation to match its immutable approval decision"
		: undefined;
}

const lifecycle_check = Schema.makeFilter<ToolLifecycleTimes>(lifecycle_time_error);

/** Projects an invocation waiting for an explicit approval without private arguments or outcomes. */
export const ToolInvocationApprovalRequired = Schema.Struct({
	...ToolInvocationProjectionBase,
	approval_id: Identifier,
	state: Schema.Literal("approval_required"),
}).check(
	lifecycle_check,
	Schema.makeFilter((value) =>
		value.tool.approval_policy === "required"
			? undefined
			: "Expected approval-required state only for a required-approval tool",
	),
);

/** Projects an invocation currently executing without private arguments or outcomes. */
export const ToolInvocationRunning = Schema.Struct({
	...ToolInvocationProjectionBase,
	approval: Schema.optional(ToolApprovalDecisionReference),
	started_at: IsoDateTime,
	state: Schema.Literal("running"),
}).check(
	lifecycle_check,
	Schema.makeFilter((value) => approval_binding_error(value, "approved")),
);

/** Projects a completed invocation without exposing its private result. */
export const ToolInvocationCompleted = Schema.Struct({
	...ToolInvocationProjectionBase,
	approval: Schema.optional(ToolApprovalDecisionReference),
	settled_at: IsoDateTime,
	started_at: IsoDateTime,
	state: Schema.Literal("completed"),
}).check(
	lifecycle_check,
	Schema.makeFilter((value) => approval_binding_error(value, "approved")),
);

/** Projects a failed invocation without exposing provider diagnostics. */
export const ToolInvocationFailed = Schema.Struct({
	...ToolInvocationProjectionBase,
	approval: Schema.optional(ToolApprovalDecisionReference),
	settled_at: IsoDateTime,
	started_at: IsoDateTime,
	state: Schema.Literal("failed"),
}).check(
	lifecycle_check,
	Schema.makeFilter((value) => approval_binding_error(value, "approved")),
);

/** Projects an invocation denied by its required approval without private arguments or outcomes. */
export const ToolInvocationDenied = Schema.Struct({
	...ToolInvocationProjectionBase,
	approval: ToolApprovalDecisionReference,
	settled_at: IsoDateTime,
	state: Schema.Literal("denied"),
}).check(
	lifecycle_check,
	Schema.makeFilter((value) => approval_binding_error(value, "denied")),
);

/** Projects an invocation whose external outcome cannot be safely determined. */
export const ToolInvocationOutcomeUnknown = Schema.Struct({
	...ToolInvocationProjectionBase,
	approval: Schema.optional(ToolApprovalDecisionReference),
	settled_at: IsoDateTime,
	started_at: IsoDateTime,
	state: Schema.Literal("outcome_unknown"),
}).check(
	lifecycle_check,
	Schema.makeFilter((value) => approval_binding_error(value, "approved")),
);

/** Projects an invocation paused for a durable external condition. */
export const ToolInvocationSuspended = Schema.Struct({
	...ToolInvocationProjectionBase,
	approval: Schema.optional(ToolApprovalDecisionReference),
	started_at: IsoDateTime,
	suspended_at: IsoDateTime,
	state: Schema.Literal("suspended"),
}).check(
	lifecycle_check,
	Schema.makeFilter((value) => approval_binding_error(value, "approved")),
);

/** Projects the source-safe lifecycle state of one tool invocation. */
export const ToolInvocationProjection = Schema.Union([
	ToolInvocationApprovalRequired,
	ToolInvocationRunning,
	ToolInvocationCompleted,
	ToolInvocationFailed,
	ToolInvocationDenied,
	ToolInvocationOutcomeUnknown,
	ToolInvocationSuspended,
]);

export type ToolInvocationProjection = typeof ToolInvocationProjection.Type;

/** Returns an invocation paused for an explicit approval. */
export const InvokeApprovalRequiredResult = Schema.Struct({
	invocation: ToolInvocationApprovalRequired,
	outcome: Schema.Literal("approval_required"),
});

/** Returns completed invocation metadata with its bounded private JSON result. */
export const InvokeCompletedResult = Schema.Struct({
	invocation: ToolInvocationCompleted,
	outcome: Schema.Literal("completed"),
	result: ToolResult,
});

/** Returns terminal failed invocation metadata without provider diagnostics. */
export const InvokeFailedResult = Schema.Struct({
	invocation: ToolInvocationFailed,
	outcome: Schema.Literal("failed"),
});

/** Returns terminal denied invocation metadata without private arguments or outcomes. */
export const InvokeDeniedResult = Schema.Struct({
	invocation: ToolInvocationDenied,
	outcome: Schema.Literal("denied"),
});

/** Returns terminal unknown-outcome invocation metadata without provider diagnostics. */
export const InvokeOutcomeUnknownResult = Schema.Struct({
	invocation: ToolInvocationOutcomeUnknown,
	outcome: Schema.Literal("outcome_unknown"),
});

/** Returns a source-safe invocation outcome; only completed outcomes carry a private result. */
export const InvokeResult = Schema.Union([
	InvokeApprovalRequiredResult,
	InvokeCompletedResult,
	InvokeFailedResult,
	InvokeDeniedResult,
	InvokeOutcomeUnknownResult,
]);

export type InvokeResult = typeof InvokeResult.Type;

const ToolApprovalProjectionBase = {
	approval_id: Identifier,
	context: ToolInvocationContext,
	created_at: IsoDateTime,
	invocation_id: Identifier,
	request_id: Identifier,
	tool: ToolPublicDescriptor,
	updated_at: IsoDateTime,
};

const required_approval_check = Schema.makeFilter<{
	readonly tool: ToolPublicDescriptor;
}>((value) =>
	value.tool.approval_policy === "required"
		? undefined
		: "Expected a tool approval only for a required-approval tool",
);

/** Projects a required tool approval before any decision is recorded. */
export const ToolApprovalRequested = Schema.Struct({
	...ToolApprovalProjectionBase,
	state: Schema.Literal("requested"),
}).check(lifecycle_check, required_approval_check);

/** Projects an approved tool invocation before execution begins. */
export const ToolApprovalApproved = Schema.Struct({
	...ToolApprovalProjectionBase,
	decided_at: IsoDateTime,
	decision_id: Identifier,
	state: Schema.Literal("approved"),
}).check(lifecycle_check, required_approval_check);

/** Projects a denied tool invocation without exposing a private denial rationale. */
export const ToolApprovalDenied = Schema.Struct({
	...ToolApprovalProjectionBase,
	decided_at: IsoDateTime,
	decision_id: Identifier,
	state: Schema.Literal("denied"),
}).check(lifecycle_check, required_approval_check);

/** Projects an approved tool invocation currently executing. */
export const ToolApprovalExecuting = Schema.Struct({
	...ToolApprovalProjectionBase,
	decided_at: IsoDateTime,
	decision_id: Identifier,
	started_at: IsoDateTime,
	state: Schema.Literal("executing"),
}).check(lifecycle_check, required_approval_check);

/** Projects an approved tool invocation after execution settles. */
export const ToolApprovalSettled = Schema.Struct({
	...ToolApprovalProjectionBase,
	decided_at: IsoDateTime,
	decision_id: Identifier,
	settled_at: IsoDateTime,
	started_at: IsoDateTime,
	state: Schema.Literal("settled"),
}).check(lifecycle_check, required_approval_check);

/** Projects the source-safe lifecycle of one required tool approval. */
export const ToolApprovalProjection = Schema.Union([
	ToolApprovalRequested,
	ToolApprovalApproved,
	ToolApprovalDenied,
	ToolApprovalExecuting,
	ToolApprovalSettled,
]);

export type ToolApprovalProjection = typeof ToolApprovalProjection.Type;

/** Records an exact-replay approval decision for one pending tool invocation. */
export const DecideApprovalRequest = Schema.Struct({
	approval_id: Identifier,
	decision: Schema.Literals(["approved", "denied"]),
	decision_id: Identifier,
});

export type DecideApprovalRequest = typeof DecideApprovalRequest.Type;

/** Returns the source-safe approval projection after an exact-replay decision. */
export const DecideApprovalResult = Schema.Struct({
	approval: ToolApprovalProjection,
});

export type DecideApprovalResult = typeof DecideApprovalResult.Type;
