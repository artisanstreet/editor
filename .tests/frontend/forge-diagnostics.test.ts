import { describe, expect, it } from "vitest";

import type { TransportDiagnosticEvent } from "@artisan/transport/client";

import {
	FormatTransportDiagnostic,
	FormatTransportDiagnosticsDump,
	FormatTransportDuration,
	FormatTransportEventTime,
	TransportDiagnosticIsFailure,
} from "../../modules/frontend/src/lib/forge/diagnostics";

const ended: TransportDiagnosticEvent = {
	at: "2026-08-06T09:17:44.902Z",
	code: "stream_gap",
	detail: "journal skipped 384 to 390",
	kind: "session.ended",
	lifetime_ms: 312_000,
	message: "The event journal has a gap.",
	protocol_code: "journal_gap",
	reached_ready: true,
};

describe("forge transport diagnostics presentation", () => {
	it("scales durations from milliseconds to minutes", () => {
		expect(FormatTransportDuration(213)).toBe("213ms");
		expect(FormatTransportDuration(5_200)).toBe("5.2s");
		expect(FormatTransportDuration(312_000)).toBe("5m 12s");
	});

	it("renders each journal entry as one line carrying its real codes", () => {
		expect(FormatTransportDiagnostic(ended)).toBe(
			"session ended (stream_gap/journal_gap) after 5m 12s, was ready: The event journal has a gap. — journal skipped 384 to 390",
		);
		expect(
			FormatTransportDiagnostic({
				at: "2026-08-06T09:12:01.180Z",
				connection_id: "connection_9",
				event_cursor_count: 6,
				journal_sequence: 384,
				kind: "session.negotiating",
				resume_mode: "resume",
			}),
		).toBe("negotiating connection_9 (resume, 6 cursors, journal 384)");
		expect(
			FormatTransportDiagnostic({
				at: "2026-08-06T09:17:45.630Z",
				attempts: 3,
				code: "connection",
				kind: "supervisor.exhausted",
				message: "The transport disconnected and will reconnect.",
				protocol_code: "",
			}),
		).toBe(
			"reconnect budget exhausted after 3 attempts (connection): The transport disconnected and will reconnect.",
		);
		expect(
			FormatTransportDiagnostic({
				at: "2026-08-06T09:17:45.100Z",
				code: "stream_gap",
				kind: "session.resume_dropped",
			}),
		).toBe("resume state dropped after stream_gap; next attempt bootstraps fresh");
	});

	it("classifies failures apart from lifecycle chatter", () => {
		expect(TransportDiagnosticIsFailure(ended)).toBe(true);
		expect(
			TransportDiagnosticIsFailure({
				at: "2026-08-06T09:12:01.000Z",
				kind: "session.attempt",
				ordinal: 4,
			}),
		).toBe(false);
	});

	it("carries the UTC time of day and a full-timestamp dump", () => {
		expect(FormatTransportEventTime(ended)).toBe("09:17:44.902");

		const dump = FormatTransportDiagnosticsDump({ dropped: 3, events: [ended] });
		expect(dump).toContain("Artisan transport journal (3 older events evicted)");
		expect(dump).toContain("2026-08-06T09:17:44.902Z session ended (stream_gap/journal_gap)");
	});
});
