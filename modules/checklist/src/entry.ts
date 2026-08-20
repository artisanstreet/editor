import { createReadStream, createWriteStream } from "node:fs";
import { createInterface } from "node:readline";

import { Effect, Schema } from "effect";

import { is_checklist_event } from "./model.ts";
import { create_checklist_tui, type ChecklistTui } from "./tui.ts";

/**
 * Bun-hosted renderer process. Events arrive on fd 3, the quit request goes
 * back on fd 4; the parent owns the run and every decision about it.
 */

const event_input = createReadStream("checklist-events", { autoClose: true, fd: 3 });
const command_output = createWriteStream("checklist-commands", { autoClose: true, fd: 4 });
const DecodeEventJson = Schema.decodeUnknownSync(Schema.UnknownFromJsonString);

const send_shutdown_request = () => {
	command_output.write(`${JSON.stringify({ type: "shutdown" })}\n`);
};

const ConsumeEvents = (tui: ChecklistTui) =>
	Effect.callback<void, Error>((resume) => {
		const lines = createInterface({ input: event_input });
		let settled = false;

		const finish = (effect: Effect.Effect<void, Error>) => {
			if (settled) return;
			settled = true;
			resume(effect);
		};

		lines.on("line", (line) => {
			try {
				const event = DecodeEventJson(line);

				if (!is_checklist_event(event)) return;

				if (event.type === "shutdown") {
					finish(Effect.void);
					return;
				}

				tui.dispatch(event);
			} catch {
				/** A malformed event is dropped; the parent still owns the run. */
			}
		});
		lines.once("close", () => finish(Effect.void));

		return Effect.sync(() => lines.close());
	});

const Main = Effect.scoped(
	Effect.gen(function* () {
		const tui = yield* Effect.acquireRelease(
			Effect.tryPromise({
				catch: (cause) => (cause instanceof Error ? cause : new Error(String(cause))),
				try: () => create_checklist_tui({ on_quit: send_shutdown_request }),
			}),
			(tui) => Effect.sync(tui.destroy),
		);

		yield* ConsumeEvents(tui);
	}),
);

Effect.runPromise(Main)
	.catch((cause: unknown) => {
		console.error(`[checklist] ${cause instanceof Error ? cause.message : String(cause)}`);
		process.exitCode = 1;
	})
	.finally(() => {
		event_input.destroy();
		command_output.end();
	});
