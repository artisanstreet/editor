import { Effect, type FileSystem } from "effect";

import {
	type Engine,
	type EngineCapabilities,
	EngineUnsupportedOperationError,
	ValidateEngineGlobalGuidance,
} from "../engine";
import type { CodexProcessFactory } from "./process";
import { OpenCodexExecRun } from "./internal/exec-run";
import { ProbeCodexExecVersion } from "./internal/exec-probe";
import { codex_exec_transport } from "./internal/exec-contract";

/** Declares the capabilities of the startup-selected one-shot exec transport. */
export const codex_exec_capabilities: EngineCapabilities = {
	approval: {
		state: "unsupported",
		reason: "Non-interactive exec cannot resolve interactive approval requests.",
	},
	auth: {
		state: "supported",
		reason: "Exec reuses saved Codex CLI authentication without credential injection.",
	},
	cancel: { state: "supported" },
	close: { state: "supported" },
	events: { state: "supported" },
	global_guidance: {
		state: "unsupported",
		reason: "V1 exec has no proven per-run native instruction channel; synced guidance files are managed outside this input.",
	},
	model_selection: { state: "supported" },
	native_continuation: {
		state: "unsupported",
		reason: "V1 exec intentionally cannot resume native sessions.",
	},
	native_tools: {
		state: "experimental",
		reason: "Known exec item families are normalized and unknown events remain native actions.",
	},
	probe: {
		state: "experimental",
		reason: "Version is non-billable; exec authentication is only confirmed by an actual run.",
	},
	question: {
		state: "unsupported",
		reason: "Non-interactive exec cannot answer provider questions.",
	},
	raw_frames: { state: "supported" },
	resume: {
		state: "unsupported",
		reason: "V1 fallback intentionally does not implement codex exec resume.",
	},
	start: { state: "supported" },
	steer: {
		state: "unsupported",
		reason: "A one-shot exec process cannot be steered after launch.",
	},
	subagents: {
		state: "unsupported",
		reason: "Exec subagent activity is retained as provider-native events only.",
	},
};

/** Identifies the startup-selected one-shot Codex exec transport. */
export const codex_exec_engine_descriptor = {
	capabilities: codex_exec_capabilities,
	display_name: "Codex CLI (one-shot fallback)",
	id: "codex",
	transport: codex_exec_transport,
} as const;

/** Configures one startup-selected Codex exec Engine. */
export interface CodexExecEngineOptions {
	readonly event_capacity: number;
	readonly executable_args: ReadonlyArray<string>;
	readonly executable: string;
	readonly fallback_reason: string;
	readonly file_system: FileSystem.FileSystem;
	readonly factory: typeof CodexProcessFactory.Service;
	readonly max_frame_bytes: number;
	readonly max_stderr_bytes: number;
	readonly max_stdout_bytes: number;
	readonly timeout_ms: number;
	readonly version_timeout_ms: number;
}

/** Builds an Engine whose selected transport and capabilities never change per run. */
export function make_codex_exec_engine(options: CodexExecEngineOptions): Engine {
	const Probe: Engine["Probe"] = () =>
		ProbeCodexExecVersion({
			executable: options.executable,
			executable_args: options.executable_args,
			factory: options.factory,
			max_stderr_bytes: 64 * 1_024,
			max_stdout_bytes: 64 * 1_024,
			timeout_ms: options.version_timeout_ms,
		}).pipe(
			Effect.map((version) => ({
				authentication: {
					reason: "Saved CLI authentication is checked only when exec starts",
					state: "unknown" as const,
				},
				capabilities: codex_exec_capabilities,
				descriptor: codex_exec_engine_descriptor,
				metadata: { fallback_reason: options.fallback_reason },
				ready: true,
				version,
			})),
		);
	const Open: Engine["Open"] = (input) =>
		ValidateEngineGlobalGuidance("codex", input.global_guidance).pipe(
			Effect.andThen(() =>
				input.global_guidance === undefined
					? Effect.void
					: Effect.fail(
							new EngineUnsupportedOperationError({
								engine_id: "codex",
								operation: "global_guidance",
							}),
						),
			),
			Effect.andThen(Probe({})),
			Effect.andThen(
				OpenCodexExecRun(
					{
						capabilities: codex_exec_capabilities,
						event_capacity: options.event_capacity,
						executable: options.executable,
						executable_args: options.executable_args,
						fallback_reason: options.fallback_reason,
						file_system: options.file_system,
						factory: options.factory,
						max_frame_bytes: options.max_frame_bytes,
						max_stderr_bytes: options.max_stderr_bytes,
						max_stdout_bytes: options.max_stdout_bytes,
						timeout_ms: options.timeout_ms,
					},
					input,
				),
			),
		);

	return { Descriptor: codex_exec_engine_descriptor, Open, Probe };
}
