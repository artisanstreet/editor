import { Effect } from "effect";

import { type EngineObservation, type EngineRunTerminalState } from "../../engine";
import { codex_exec_protocol_version, codex_exec_transport } from "./codex-exec-contract";
import { MakeCodexEventBuffer } from "./codex-event-buffer";

/** Configures bounded observation delivery and terminal cleanup for one exec run. */
export interface CodexExecEventBufferOptions {
	readonly artisan_run_id: string;
	readonly BeforeEnqueue?: (observation: EngineObservation) => Effect.Effect<void>;
	readonly BeforeFinish?: Effect.Effect<void>;
	readonly capacity: number;
	readonly CloseProcess: Effect.Effect<void>;
}

/** Adapts the shared ordered Codex event buffer to exec JSONL provenance. */
export function MakeCodexExecEventBuffer(options: CodexExecEventBufferOptions) {
	return MakeCodexEventBuffer({
		artisan_run_id: options.artisan_run_id,
		...(options.BeforeEnqueue === undefined ? {} : { BeforeEnqueue: options.BeforeEnqueue }),
		...(options.BeforeFinish === undefined ? {} : { BeforeFinish: options.BeforeFinish }),
		capacity: options.capacity,
		CloseResource: options.CloseProcess,
		make_terminal_observation: (terminal_state: EngineRunTerminalState, sequence: number) => ({
			_tag: "run_terminal",
			artisan_run_id: options.artisan_run_id,
			observation_id: `${options.artisan_run_id}:exec:terminal:${sequence}`,
			raw: {
				engine_id: "codex",
				frame: { source: "exec-lifecycle", terminal_state },
				protocol_version: codex_exec_protocol_version,
				transport: codex_exec_transport,
			},
			sequence,
			state: terminal_state,
		}),
	});
}
