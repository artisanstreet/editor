import { Schema } from "effect";

const PreviewIdentifier = Schema.String.check(
	Schema.isPattern(/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/, {
		message: "Expected a bounded preview identifier",
	}),
);

const PreviewTimestampMs = Schema.Int.check(
	Schema.isGreaterThanOrEqualTo(0),
	Schema.isLessThanOrEqualTo(4102444800000),
);

const PreviewMessage = Schema.String.check(
	Schema.makeFilter<string>((message) =>
		message.length <= 512 &&
		![...message].some((character) => {
			const code = character.codePointAt(0)!;

			return code <= 31 || (code >= 127 && code <= 159);
		})
			? undefined
			: "Expected a bounded health message without control characters",
	),
);

const PreviewUrl = Schema.String.check(
	Schema.makeFilter<string>((value) => {
		if (value.length > 2048) {
			return "Expected a bounded preview URL";
		}

		if (!URL.canParse(value)) {
			return "Expected a valid preview URL";
		}

		const url = new URL(value);
		const hostname = url.hostname.toLowerCase().replace(/\.$/, "");
		const is_localhost = hostname === "localhost";
		const ipv4_parts = hostname.split(".");
		const is_ipv4_loopback =
			ipv4_parts.length === 4 &&
			ipv4_parts[0] === "127" &&
			ipv4_parts
				.slice(1)
				.every((part) => /^(?:0|[1-9]\d{0,2})$/.test(part) && Number(part) <= 255);
		const is_ipv6_loopback = hostname === "[::1]" || hostname === "::1";

		return url.protocol !== "http:" && url.protocol !== "https:"
			? "Expected an HTTP(S) preview URL"
			: url.username || url.password
				? "Preview URLs must not contain credentials"
				: is_localhost || is_ipv4_loopback || is_ipv6_loopback
					? undefined
					: "Preview URLs must target localhost or loopback";
	}),
);

/** Identifies the local process-like owner of a preview target. */
export const PreviewTargetSource = Schema.Union([
	Schema.Struct({ kind: Schema.Literal("process"), process_id: PreviewIdentifier }),
	Schema.Struct({ kind: Schema.Literal("terminal"), terminal_id: PreviewIdentifier }),
]);

export type PreviewTargetSource = typeof PreviewTargetSource.Type;

/** Describes one bounded health observation attached to a preview target. */
export const PreviewTargetHealth = Schema.Struct({
	checked_at_ms: PreviewTimestampMs,
	latency_ms: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(600_000),
	),
	message: Schema.optional(PreviewMessage),
	status: Schema.Literals(["healthy", "unhealthy"]),
	status_code: Schema.optional(
		Schema.Int.check(Schema.isGreaterThanOrEqualTo(100), Schema.isLessThanOrEqualTo(599)),
	),
});

export type PreviewTargetHealth = typeof PreviewTargetHealth.Type;

const PreviewTargetRecordFields = {
	created_at_ms: PreviewTimestampMs,
	health: Schema.optional(PreviewTargetHealth),
	project_id: PreviewIdentifier,
	source: Schema.optional(PreviewTargetSource),
	target_id: PreviewIdentifier,
	updated_at_ms: PreviewTimestampMs,
	url: PreviewUrl,
	workspace_id: PreviewIdentifier,
};

/** Attributes an external-browser action without exposing provider-private metadata. */
export const PreviewBrowserInitiator = Schema.Union([
	Schema.Struct({ kind: Schema.Literal("user") }),
	Schema.Struct({ agent_id: PreviewIdentifier, kind: Schema.Literal("agent") }),
]);

export type PreviewBrowserInitiator = typeof PreviewBrowserInitiator.Type;

/** Projects one source-safe current local preview target. */
export const PreviewTargetRecord = Schema.Struct({
	...PreviewTargetRecordFields,
	state: Schema.Literals(["healthy", "registered", "stopped", "unhealthy"]),
});

export type PreviewTargetRecord = typeof PreviewTargetRecord.Type;

/** Retains the final source-safe target only in its removal event. */
export const PreviewTargetRemovedRecord = Schema.Struct({
	...PreviewTargetRecordFields,
	generation_id: PreviewIdentifier,
	state: Schema.Literal("removed"),
});

export type PreviewTargetRemovedRecord = typeof PreviewTargetRemovedRecord.Type;

/** Decodes removal events persisted before exact target generations were published. */
const LegacyPreviewTargetRemovedRecord = Schema.Struct({
	...PreviewTargetRecordFields,
	state: Schema.Literal("removed"),
});

/** Registers one explicitly identified local preview target. */
export const PreviewTargetRegisterCommand = Schema.Struct({
	project_id: PreviewIdentifier,
	source: Schema.optional(PreviewTargetSource),
	type: Schema.Literal("preview.target.register"),
	target_id: PreviewIdentifier,
	url: PreviewUrl,
	workspace_id: PreviewIdentifier,
});

export type PreviewTargetRegisterCommand = typeof PreviewTargetRegisterCommand.Type;

/** Requests a bounded health probe for one identified preview target. */
export const PreviewTargetProbeCommand = Schema.Struct({
	project_id: PreviewIdentifier,
	target_id: PreviewIdentifier,
	type: Schema.Literal("preview.target.probe"),
	workspace_id: PreviewIdentifier,
});

export type PreviewTargetProbeCommand = typeof PreviewTargetProbeCommand.Type;

/** Removes one identified preview target from the current projection. */
export const PreviewTargetRemoveCommand = Schema.Struct({
	project_id: PreviewIdentifier,
	target_id: PreviewIdentifier,
	type: Schema.Literal("preview.target.remove"),
	workspace_id: PreviewIdentifier,
});

export type PreviewTargetRemoveCommand = typeof PreviewTargetRemoveCommand.Type;

/** Requests one explicit handoff of a registered target to the user's external browser. */
export const PreviewBrowserLaunchCommand = Schema.Struct({
	project_id: PreviewIdentifier,
	target_id: PreviewIdentifier,
	type: Schema.Literal("preview.browser.open"),
	workspace_id: PreviewIdentifier,
});

export type PreviewBrowserLaunchCommand = typeof PreviewBrowserLaunchCommand.Type;

/** Attaches an explicit configured connector to one registered external-browser target. */
export const PreviewInspectionAttachCommand = Schema.Struct({
	connector_id: PreviewIdentifier,
	inspection_id: PreviewIdentifier,
	project_id: PreviewIdentifier,
	target_id: PreviewIdentifier,
	type: Schema.Literal("preview.inspection.attach"),
	workspace_id: PreviewIdentifier,
});

export type PreviewInspectionAttachCommand = typeof PreviewInspectionAttachCommand.Type;

/** Detaches one explicitly identified external-browser inspection session. */
export const PreviewInspectionDetachCommand = Schema.Struct({
	inspection_id: PreviewIdentifier,
	project_id: PreviewIdentifier,
	type: Schema.Literal("preview.inspection.detach"),
	workspace_id: PreviewIdentifier,
});

export type PreviewInspectionDetachCommand = typeof PreviewInspectionDetachCommand.Type;

/** Queries preview targets within one explicit workspace and project scope. */
export const PreviewTargetsQuery = Schema.Struct({
	project_id: PreviewIdentifier,
	workspace_id: PreviewIdentifier,
});

export type PreviewTargetsQuery = typeof PreviewTargetsQuery.Type;

const PreviewTargetRecords = Schema.Array(PreviewTargetRecord).check(Schema.isMaxLength(256));

/** Returns one bounded, uniquely keyed preview target projection for an exact scope. */
export const PreviewTargetsQueryResult = Schema.Struct({
	project_id: PreviewIdentifier,
	targets: PreviewTargetRecords,
	workspace_id: PreviewIdentifier,
}).check(
	Schema.makeFilter((result) => {
		const target_ids = new Set(result.targets.map((target) => target.target_id));
		const has_foreign_scope = result.targets.some(
			(target) =>
				target.project_id !== result.project_id ||
				target.workspace_id !== result.workspace_id,
		);

		return target_ids.size === result.targets.length && !has_foreign_scope
			? undefined
			: "Expected unique preview targets from the result scope";
	}),
);

export type PreviewTargetsQueryResult = typeof PreviewTargetsQueryResult.Type;

/** Projects one durable external-browser launch attempt without claiming page navigation. */
export const PreviewBrowserLaunchRecord = Schema.Struct({
	initiator: PreviewBrowserInitiator,
	launch_id: PreviewIdentifier,
	project_id: PreviewIdentifier,
	reason: Schema.optional(
		Schema.Literals([
			"interrupted",
			"launcher_failed",
			"launcher_rejected",
			"launcher_unavailable",
			"target_changed",
		]),
	),
	requested_at_ms: PreviewTimestampMs,
	state: Schema.Literals([
		"accepted",
		"dispatching",
		"dispatched",
		"outcome_unknown",
		"rejected",
	]),
	target_generation_id: PreviewIdentifier,
	target_id: PreviewIdentifier,
	updated_at_ms: PreviewTimestampMs,
	url: PreviewUrl,
	workspace_id: PreviewIdentifier,
}).check(
	Schema.makeFilter((record) => {
		const has_reason = record.reason !== undefined;
		const reason_matches =
			(record.state === "rejected" &&
				(record.reason === "launcher_rejected" ||
					record.reason === "launcher_unavailable" ||
					record.reason === "target_changed")) ||
			(record.state === "outcome_unknown" &&
				(record.reason === "interrupted" || record.reason === "launcher_failed")) ||
			((record.state === "accepted" ||
				record.state === "dispatching" ||
				record.state === "dispatched") &&
				!has_reason);

		return reason_matches && record.updated_at_ms >= record.requested_at_ms
			? undefined
			: "Expected a coherent external-browser launch lifecycle";
	}),
);

export type PreviewBrowserLaunchRecord = typeof PreviewBrowserLaunchRecord.Type;

/** Projects one attributable, explicit external-browser inspection attachment. */
export const PreviewInspectionSessionRecord = Schema.Struct({
	connector_id: PreviewIdentifier,
	initiator: PreviewBrowserInitiator,
	inspection_id: PreviewIdentifier,
	project_id: PreviewIdentifier,
	reason: Schema.optional(
		Schema.Literals([
			"connection_lost",
			"connector_rejected",
			"connector_unavailable",
			"detached",
			"interrupted",
			"target_changed",
			"thread_erased",
		]),
	),
	requested_at_ms: PreviewTimestampMs,
	state: Schema.Literals(["attached", "attaching", "disconnected", "failed"]),
	target_generation_id: PreviewIdentifier,
	target_id: PreviewIdentifier,
	updated_at_ms: PreviewTimestampMs,
	url: PreviewUrl,
	workspace_id: PreviewIdentifier,
}).check(
	Schema.makeFilter((record) => {
		const has_reason = record.reason !== undefined;
		const reason_matches =
			((record.state === "attached" || record.state === "attaching") && !has_reason) ||
			(record.state === "failed" &&
				(record.reason === "connector_rejected" ||
					record.reason === "connector_unavailable" ||
					record.reason === "target_changed")) ||
			(record.state === "disconnected" &&
				(record.reason === "connection_lost" ||
					record.reason === "detached" ||
					record.reason === "interrupted" ||
					record.reason === "target_changed" ||
					record.reason === "thread_erased"));

		return reason_matches && record.updated_at_ms >= record.requested_at_ms
			? undefined
			: "Expected a coherent external-browser inspection lifecycle";
	}),
);

export type PreviewInspectionSessionRecord = typeof PreviewInspectionSessionRecord.Type;

/** Queries external-browser launches and inspection sessions in one exact scope. */
export const PreviewBrowserLifecycleQuery = Schema.Struct({
	project_id: PreviewIdentifier,
	workspace_id: PreviewIdentifier,
});

export type PreviewBrowserLifecycleQuery = typeof PreviewBrowserLifecycleQuery.Type;

const PreviewBrowserLaunchRecords = Schema.Array(PreviewBrowserLaunchRecord).check(
	Schema.isMaxLength(256),
);
const PreviewInspectionSessionRecords = Schema.Array(PreviewInspectionSessionRecord).check(
	Schema.isMaxLength(256),
);

/** Returns bounded external-browser lifecycle projections from one exact scope. */
export const PreviewBrowserLifecycleQueryResult = Schema.Struct({
	inspections: PreviewInspectionSessionRecords,
	launches: PreviewBrowserLaunchRecords,
	project_id: PreviewIdentifier,
	workspace_id: PreviewIdentifier,
}).check(
	Schema.makeFilter((result) => {
		const launch_ids = new Set(result.launches.map((launch) => launch.launch_id));
		const inspection_ids = new Set(
			result.inspections.map((inspection) => inspection.inspection_id),
		);
		const has_foreign_scope =
			result.launches.some(
				(launch) =>
					launch.project_id !== result.project_id ||
					launch.workspace_id !== result.workspace_id,
			) ||
			result.inspections.some(
				(inspection) =>
					inspection.project_id !== result.project_id ||
					inspection.workspace_id !== result.workspace_id,
			);

		return launch_ids.size === result.launches.length &&
			inspection_ids.size === result.inspections.length &&
			!has_foreign_scope
			? undefined
			: "Expected unique external-browser lifecycle records from the result scope";
	}),
);

export type PreviewBrowserLifecycleQueryResult = typeof PreviewBrowserLifecycleQueryResult.Type;

/** Records a projection update while keeping removed targets out of current queries. */
export const PreviewTargetUpdatedEvent = Schema.Union([
	Schema.Struct({
		action: Schema.Literals(["registered", "probed", "state"]),
		target: PreviewTargetRecord,
		type: Schema.Literal("preview.target.updated"),
	}),
	Schema.Struct({
		action: Schema.Literal("removed"),
		target: PreviewTargetRemovedRecord,
		type: Schema.Literal("preview.target.updated"),
	}),
	Schema.Struct({
		action: Schema.Literal("removed"),
		target: LegacyPreviewTargetRemovedRecord,
		type: Schema.Literal("preview.target.updated"),
	}),
]);

export type PreviewTargetUpdatedEvent = typeof PreviewTargetUpdatedEvent.Type;

/** Records a source-safe external-browser launch or inspection lifecycle transition. */
export const PreviewBrowserLifecycleEvent = Schema.Union([
	Schema.Struct({
		action: Schema.Literals(["dispatched", "outcome_unknown", "rejected"]),
		launch: PreviewBrowserLaunchRecord,
		type: Schema.Literal("preview.browser.launch.updated"),
	}).check(
		Schema.makeFilter((event) =>
			event.action === event.launch.state
				? undefined
				: "Expected the browser launch action to match its lifecycle state",
		),
	),
	Schema.Struct({
		action: Schema.Literals(["attached", "disconnected", "failed"]),
		inspection: PreviewInspectionSessionRecord,
		type: Schema.Literal("preview.inspection.updated"),
	}).check(
		Schema.makeFilter((event) =>
			event.action === event.inspection.state
				? undefined
				: "Expected the inspection action to match its lifecycle state",
		),
	),
]);

export type PreviewBrowserLifecycleEvent = typeof PreviewBrowserLifecycleEvent.Type;

/** Unions the preview command payloads accepted by the control router. */
export const PreviewTargetCommand = Schema.Union([
	PreviewTargetRegisterCommand,
	PreviewTargetProbeCommand,
	PreviewTargetRemoveCommand,
]);

export type PreviewTargetCommand = typeof PreviewTargetCommand.Type;

/** Unions explicit external-browser and inspection commands accepted by the control router. */
export const PreviewBrowserCommand = Schema.Union([
	PreviewBrowserLaunchCommand,
	PreviewInspectionAttachCommand,
	PreviewInspectionDetachCommand,
]);

export type PreviewBrowserCommand = typeof PreviewBrowserCommand.Type;
