import { Effect } from "effect";

import type {
	EngineUsageQueryEnvelope,
	EngineUsageQueryResultEnvelope,
	EngineUsageReport,
	EngineUsageWindow,
	ProtocolErrorDetail,
} from "@artisan/protocol";
import { EngineRegistry, type Engine, type EngineQuotaWindow } from "@artisan/engines";

import { RuntimeMetadata } from "../../../runtime/runtime-metadata";

const max_windows_per_report = 64;
const max_engines_per_snapshot = 16;
const usage_timeout = "15 seconds";

function to_usage_window(window: EngineQuotaWindow): EngineUsageWindow {
	return {
		id: window.id,
		kind: window.kind,
		...(window.label === undefined ? {} : { label: window.label }),
		percent_used: window.percent_used,
		...(window.resets_at === undefined ? {} : { resets_at: window.resets_at }),
		...(window.window_minutes === undefined ? {} : { window_minutes: window.window_minutes }),
	};
}

/** Renders a short, secret-free human message for a failed or timed-out usage fetch. */
function describe_usage_failure(cause: unknown): string {
	if (cause === null || typeof cause !== "object" || !("_tag" in cause)) {
		return "Usage lookup failed.";
	}

	const tag = String((cause as { _tag: unknown })._tag);

	if (tag === "TimeoutException" || tag === "TimeoutError") {
		return "Usage lookup timed out.";
	}

	const message = (cause as { message?: unknown }).message;

	return typeof message === "string" && message.length > 0
		? message
		: `Usage lookup failed (${tag}).`;
}

/** Reports one engine's provider-account usage, absorbing any failure into the report shape. */
function ReportFor(engine: Engine): Effect.Effect<EngineUsageReport> {
	const usage = engine.Usage;

	if (usage === undefined) {
		return Effect.succeed({
			authentication: "unknown",
			display_name: engine.Descriptor.display_name,
			engine_id: engine.Descriptor.id,
			windows: [],
		});
	}

	return usage.pipe(
		Effect.timeout(usage_timeout),
		Effect.match({
			onFailure: (cause): EngineUsageReport => ({
				authentication: "unknown",
				display_name: engine.Descriptor.display_name,
				engine_id: engine.Descriptor.id,
				failure: describe_usage_failure(cause),
				windows: [],
			}),
			onSuccess: (account_usage): EngineUsageReport => ({
				authentication: account_usage.authentication.state,
				display_name: engine.Descriptor.display_name,
				engine_id: engine.Descriptor.id,
				...(account_usage.authentication.reason === undefined
					? {}
					: { failure: account_usage.authentication.reason }),
				windows: account_usage.windows
					.slice(0, max_windows_per_report)
					.map(to_usage_window),
			}),
		}),
	);
}

export const MakeEngineUsageQueryHandler = Effect.gen(function* () {
	const registry = yield* EngineRegistry;
	const metadata = yield* RuntimeMetadata;

	const Envelope = <Kind extends EngineUsageQueryResultEnvelope["kind"], Payload>(
		query: EngineUsageQueryEnvelope,
		kind: Kind,
		payload: Payload,
	) =>
		Effect.gen(function* () {
			const message_id = yield* metadata.MakeId("message");
			const sent_at = yield* metadata.Now;

			return {
				correlation_id: query.message_id,
				kind,
				message_id,
				origin: "backend" as const,
				payload,
				protocol_version: 1 as const,
				schema_version: 1 as const,
				sent_at,
			};
		});

	const handlers = {
		"engine.usage.query": (query: EngineUsageQueryEnvelope) =>
			registry.List.pipe(
				Effect.map((engines) => engines.filter((engine) => engine.Usage !== undefined)),
				Effect.flatMap((engines) =>
					Effect.all(engines.map(ReportFor), { concurrency: "unbounded" }),
				),
				Effect.flatMap((reports) =>
					Effect.gen(function* () {
						const fetched_at = yield* metadata.Now;

						return yield* Envelope(query, "engine.usage.query.result", {
							engines: reports.slice(0, max_engines_per_snapshot),
							fetched_at,
						});
					}),
				),
			),
	};

	return (
		query: EngineUsageQueryEnvelope,
	): Effect.Effect<EngineUsageQueryResultEnvelope, ProtocolErrorDetail> => {
		switch (query.kind) {
			case "engine.usage.query":
				return handlers["engine.usage.query"](query);
		}
	};
});
