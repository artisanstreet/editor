import { Effect } from "effect";

import { type EngineObservation, type EngineRunTerminalState } from "../../engine";
import { CodexTransportMetadata } from "../protocol";
import { MakeCodexEventBuffer } from "./event-buffer";

/** Configures app-server event ordering and terminal cleanup for one run. */
export interface CodexAppServerEventBufferOptions {
	readonly artisan_run_id: string;
	readonly BeforeEnqueue?: (observation: EngineObservation) => Effect.Effect<void>;
	readonly BeforeFinish?: Effect.Effect<void>;
	readonly capacity: number;
	readonly CloseSession: Effect.Effect<void>;
}

/** Adapts the shared ordered Codex event buffer to app-server provenance. */
export function MakeCodexAppServerEventBuffer(options: CodexAppServerEventBufferOptions) {
	return MakeCodexEventBuffer({
		artisan_run_id: options.artisan_run_id,
		...(options.BeforeEnqueue === undefined ? {} : { BeforeEnqueue: options.BeforeEnqueue }),
		...(options.BeforeFinish === undefined ? {} : { BeforeFinish: options.BeforeFinish }),
		capacity: options.capacity,
		CloseResource: options.CloseSession,
		make_terminal_observation: (terminal_state: EngineRunTerminalState, sequence: number) => ({
			_tag: "run_terminal",
			artisan_run_id: options.artisan_run_id,
			observation_id: `${options.artisan_run_id}:terminal:${sequence}`,
			raw: {
				engine_id: "codex",
				frame: { source: "run-lifecycle", terminal_state },
				protocol_version: CodexTransportMetadata.protocol_version,
				transport: CodexTransportMetadata.transport,
			},
			sequence,
			state: terminal_state,
		}),
	});
}
