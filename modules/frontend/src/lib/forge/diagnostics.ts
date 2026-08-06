import type {
	TransportDiagnosticEvent,
	TransportDiagnosticsSnapshot,
} from "@artisan/transport/client";
import { Effect } from "effect";

/**
 * Renders a duration for humans scanning a journal: exact under a second,
 * then coarser as the magnitude grows.
 */
export const FormatTransportDuration = (duration_ms: number): string => {
	if (duration_ms < 1_000) return `${duration_ms}ms`;
	if (duration_ms < 60_000) return `${(duration_ms / 1_000).toFixed(1)}s`;
	const minutes = Math.floor(duration_ms / 60_000);
	const seconds = Math.round((duration_ms % 60_000) / 1_000);

	return `${minutes}m ${seconds}s`;
};

/** The UTC time-of-day carried by a journal entry's ISO timestamp. */
export const FormatTransportEventTime = (event: TransportDiagnosticEvent): string =>
	event.at.slice(11, 23);

/** One compact human-readable line per journal entry. */
export const FormatTransportDiagnostic = (event: TransportDiagnosticEvent): string => {
	switch (event.kind) {
		case "session.attempt":
			return `connection attempt #${event.ordinal}`;
		case "session.negotiating":
			return `negotiating ${event.connection_id} (${event.resume_mode}, ${event.event_cursor_count} cursors, journal ${event.journal_sequence})`;
		case "session.ready":
			return `session ready on ${event.connection_id} after ${FormatTransportDuration(event.negotiation_ms)}`;
		case "session.ended": {
			const readiness = event.reached_ready ? "was ready" : "never became ready";
			const detail = event.detail.length === 0 ? "" : ` — ${event.detail}`;
			const protocol = event.protocol_code.length === 0 ? "" : `/${event.protocol_code}`;

			return `session ended (${event.code}${protocol}) after ${FormatTransportDuration(event.lifetime_ms)}, ${readiness}: ${event.message}${detail}`;
		}
		case "session.resume_dropped":
			return `resume state dropped after ${event.code}; next attempt bootstraps fresh`;
		case "supervisor.exhausted": {
			const protocol = event.protocol_code.length === 0 ? "" : `/${event.protocol_code}`;

			return `reconnect budget exhausted after ${event.attempts} attempts (${event.code}${protocol}): ${event.message}`;
		}
		case "supervisor.retry_released":
			return "reconnect retry released";
		case "error.published": {
			const protocol = event.protocol_code.length === 0 ? "" : `/${event.protocol_code}`;

			return `error published (${event.code}${protocol}): ${event.message}`;
		}
		case "client.disposed":
			return "client disposed";
	}
};

/** Failures warn so they surface in a default console; chatter stays at info. */
export const TransportDiagnosticIsFailure = (event: TransportDiagnosticEvent): boolean =>
	event.kind === "session.ended" ||
	event.kind === "supervisor.exhausted" ||
	event.kind === "error.published";

/**
 * Mirrors every journal entry to the devtools console, prefixed and stamped,
 * so a machine that hits an intermittent transport failure carries its own
 * evidence — no reproduction session needed after the fact.
 */
export const LogTransportDiagnostic = (event: TransportDiagnosticEvent) =>
	Effect.gen(function* () {
		const line = `[forge:transport] ${FormatTransportEventTime(event)} ${FormatTransportDiagnostic(event)}`;

		if (TransportDiagnosticIsFailure(event)) {
			console.warn(line);
		} else {
			console.info(line);
		}
	});

/** The plaintext journal dump offered by the connection overlay's copy action. */
export const FormatTransportDiagnosticsDump = (snapshot: TransportDiagnosticsSnapshot): string => {
	const header =
		snapshot.dropped === 0
			? "Artisan transport journal"
			: `Artisan transport journal (${snapshot.dropped} older events evicted)`;
	const lines = snapshot.events.map((event) => `${event.at} ${FormatTransportDiagnostic(event)}`);

	return [header, ...lines].join("\n");
};
