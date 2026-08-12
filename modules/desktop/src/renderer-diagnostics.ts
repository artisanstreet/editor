import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Semaphore } from "effect";

const maximum_log_bytes = 8 * 1024 * 1024;
const maximum_retained_logs = 8;

export interface RendererDiagnosticLog {
	readonly path: string;
	readonly Write: (record: Readonly<Record<string, unknown>>) => Effect.Effect<void, unknown>;
}

/** Creates one bounded JSONL session log and retires the oldest prior sessions. */
export const CreateRendererDiagnosticLog = (directory: string, session_name: string) =>
	Effect.gen(function* () {
		const file_system = yield* FileSystem.FileSystem;
		const write_lock = yield* Semaphore.make(1);
		yield* file_system.makeDirectory(directory, { recursive: true });

		const retained = yield* file_system.readDirectory(directory).pipe(
			Effect.map((entries) => entries.filter((entry) => entry.endsWith(".jsonl")).sort()),
			Effect.map((entries) =>
				entries.slice(0, Math.max(0, entries.length - maximum_retained_logs + 1)),
			),
		);
		for (const entry of retained) {
			const session_name = entry.replace(/\.jsonl$/u, "");
			yield* file_system.remove(`${directory}/${entry}`).pipe(Effect.ignore);
			const session_artifacts = yield* file_system
				.readDirectory(directory)
				.pipe(
					Effect.map((entries) =>
						entries.filter(
							(candidate) =>
								candidate.startsWith(`${session_name}.trace`) &&
								candidate.endsWith(".json"),
						),
					),
				);
			for (const artifact of session_artifacts) {
				yield* file_system.remove(`${directory}/${artifact}`).pipe(Effect.ignore);
			}
		}

		const path = `${directory}/${session_name}.jsonl`;
		let bytes_written = 0;
		const Write = (record: Readonly<Record<string, unknown>>) =>
			write_lock.withPermits(1)(
				Effect.gen(function* () {
					const encoded = `${JSON.stringify(record)}\n`;
					const encoded_bytes = Buffer.byteLength(encoded);
					if (bytes_written + encoded_bytes > maximum_log_bytes) return;
					yield* file_system.writeFileString(path, encoded, { flag: "a" });
					bytes_written += encoded_bytes;
				}),
			);

		return { path, Write } satisfies RendererDiagnosticLog;
	}).pipe(Effect.provide(NodeFileSystem.layer));
