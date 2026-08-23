import { Cache, Clock, Effect, Ref } from "effect";

import type {
	EngineUsageQueryEnvelope,
	EngineUsageQueryResultEnvelope,
	EngineUsageReport,
	EngineUsageWindow,
	ProtocolErrorDetail,
} from "@artisan/protocol";
import { EngineRegistry, type Engine, type EngineQuotaWindow } from "@artisan/engines";

import { RuntimeMetadata } from "../../../runtime/metadata";

const max_windows_per_report = 64;
const max_engines_per_snapshot = 16;
const max_concurrent_usage_probes = 64;
const usage_timeout = "15 seconds";

/**
 * A cached report is served as-is, with no engine call, while it is younger than this window.
 * Once it ages past the window it is still kept as the "last-good" fallback (see
 * `FetchUsageOutcome` below) for as long as no newer success replaces it.
 */
const usage_fresh_window_ms = 180_000;

/** The last successful report for one engine, plus when it was fetched. */
interface UsageCacheEntry {
	readonly fetched_at_ms: number;
	readonly report: EngineUsageReport;
}

/** Last-good usage reports keyed by `engine_id`. Empty entries mean "never succeeded yet". */
type UsageCache = ReadonlyMap<string, UsageCacheEntry>;

/** The result of attempting one engine's `Usage` effect, tagged so success is never inferred from report shape. */
interface UsageOutcome {
	readonly ok: boolean;
	readonly report: EngineUsageReport;
}

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

/** Runs one engine's `Usage` effect and tags whether it truly succeeded, absorbing failure into the report shape. */
function FetchUsageOutcome(engine: Engine): Effect.Effect<UsageOutcome> {
	const usage = engine.Usage;

	if (usage === undefined) {
		return Effect.succeed({
			ok: false,
			report: {
				authentication: "unknown",
				display_name: engine.Descriptor.display_name,
				engine_id: engine.Descriptor.id,
				windows: [],
			},
		});
	}

	return usage.pipe(
		Effect.timeout(usage_timeout),
		/**
		 * The report below carries a short line for the reader; this carries the
		 * whole cause for whoever has to diagnose it. Absorbing the failure into
		 * the report shape without recording it first left no trace of a repeated
		 * usage failure anywhere in the Forge log.
		 */
		Effect.tapError((cause) =>
			Effect.logWarning(
				`Engine usage lookup failed for ${engine.Descriptor.id}: ${describe_usage_failure(cause)}`,
				cause,
			),
		),
		Effect.match({
			onFailure: (cause): UsageOutcome => ({
				ok: false,
				report: {
					authentication: "unknown",
					display_name: engine.Descriptor.display_name,
					engine_id: engine.Descriptor.id,
					failure: describe_usage_failure(cause),
					windows: [],
				},
			}),
			onSuccess: (account_usage): UsageOutcome => ({
				ok: true,
				report: {
					...(account_usage.account_email === undefined
						? {}
						: { account_email: account_usage.account_email }),
					authentication: account_usage.authentication.state,
					display_name: engine.Descriptor.display_name,
					engine_id: engine.Descriptor.id,
					...(account_usage.authentication.reason === undefined
						? {}
						: { failure: account_usage.authentication.reason }),
					...(account_usage.quota_surface === undefined
						? {}
						: { quota_surface: account_usage.quota_surface }),
					windows: account_usage.windows
						.slice(0, max_windows_per_report)
						.map(to_usage_window),
				},
			}),
		}),
	);
}

/**
 * Reports one engine's usage, serving the cache instead of the engine whenever possible.
 *
 * - A cached report younger than `usage_fresh_window_ms` is returned with no engine call.
 * - Otherwise the engine's `Usage` effect runs. A success refreshes the cache and is returned.
 *   A failure (including timeout) falls back to the last-good cached report, of any age, if one
 *   exists; only an engine that has never once succeeded produces the failure-shaped report.
 *
 * This is decided per engine, so one engine being stale or failing never blocks another's fresh
 * fetch, and a snapshot where every engine's cache is fresh makes zero engine calls.
 */
function ReportWithCache(
	engine: Engine,
	cache: Ref.Ref<UsageCache>,
	usage_probes: Cache.Cache<string, UsageOutcome>,
	now_ms: number,
	force: boolean,
): Effect.Effect<EngineUsageReport> {
	return Effect.gen(function* () {
		const engine_id = engine.Descriptor.id;
		const cached = (yield* Ref.get(cache)).get(engine_id);

		if (
			!force &&
			cached !== undefined &&
			now_ms - cached.fetched_at_ms < usage_fresh_window_ms
		) {
			return cached.report;
		}

		const outcome = yield* Cache.get(usage_probes, engine_id);

		if (outcome.ok) {
			yield* Ref.update(cache, (entries) =>
				new Map(entries).set(engine_id, { fetched_at_ms: now_ms, report: outcome.report }),
			);

			return outcome.report;
		}

		return cached === undefined ? outcome.report : cached.report;
	});
}

export const MakeEngineUsageQueryHandler = Effect.gen(function* () {
	const registry = yield* EngineRegistry;
	const metadata = yield* RuntimeMetadata;
	const cache = yield* Ref.make<UsageCache>(new Map());
	const usage_probes = yield* Cache.makeWith<string, UsageOutcome>(
		(engine_id) =>
			registry.List.pipe(
				Effect.flatMap((engines) => {
					const engine = engines.find(
						(candidate) => candidate.Descriptor.id === engine_id,
					);

					return engine === undefined
						? Effect.die(
								`Engine usage probe requested for unknown engine ${engine_id}.`,
							)
						: FetchUsageOutcome(engine);
				}),
			),
		{
			capacity: max_concurrent_usage_probes,
			/** Retain only an in-flight probe; last-good freshness is owned by `cache`. */
			timeToLive: () => "0 millis",
		},
	);

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
			Effect.gen(function* () {
				const now_ms = yield* Clock.currentTimeMillis;
				const engines = (yield* registry.List).filter(
					(engine) =>
						engine.Usage !== undefined &&
						(query.payload.engine_id === undefined ||
							engine.Descriptor.id === query.payload.engine_id),
				);
				const reports = yield* Effect.all(
					engines.map((engine) =>
						ReportWithCache(
							engine,
							cache,
							usage_probes,
							now_ms,
							query.payload.force === true,
						),
					),
					{ concurrency: "unbounded" },
				);
				const fetched_at = yield* metadata.Now;

				return yield* Envelope(query, "engine.usage.query.result", {
					engines: reports.slice(0, max_engines_per_snapshot),
					fetched_at,
				});
			}),
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
