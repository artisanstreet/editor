import type { EngineEventBufferOptions } from "../../process/event-buffer";
import { MakeEngineEventBuffer } from "../../process/event-buffer";

/** Configures bounded ordered delivery for one Codex run. @since 0.2.0 */
export type CodexEventBufferOptions = EngineEventBufferOptions;

/** Adapts the provider-neutral event buffer to the historical Codex export. @since 0.2.0 */
export function MakeCodexEventBuffer(options: CodexEventBufferOptions) {
	return MakeEngineEventBuffer(options);
}
