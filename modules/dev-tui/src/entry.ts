import { createReadStream, createWriteStream } from "node:fs";
import { createInterface } from "node:readline";

import { Effect, Schema } from "effect";

import { create_dev_tui, is_dev_tui_event, type DevTui } from "./index";

const event_input = createReadStream("dev-tui-events", {
	autoClose: true,
	fd: 3,
});
const command_output = createWriteStream("dev-tui-commands", {
	autoClose: true,
	fd: 4,
});
const DecodeEventJson = Schema.decodeUnknownSync(Schema.UnknownFromJsonString);

const send_shutdown_request = () => {
	command_output.write(`${JSON.stringify({ type: "shutdown" })}\n`);
};

const ConsumeEvents = (tui: DevTui) =>
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

				if (!is_dev_tui_event(event)) {
					tui.dispatch({
						lane_id: "runner",
						line: "Ignored an invalid dashboard event.",
						type: "log",
					});
					return;
				}

				if (event.type === "shutdown") {
					finish(Effect.void);
					return;
				}

				tui.dispatch(event);
			} catch {
				tui.dispatch({
					lane_id: "runner",
					line: "Ignored malformed dashboard event.",
					type: "log",
				});
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
				try: () => create_dev_tui({ on_quit: send_shutdown_request }),
			}),
			(tui) => Effect.sync(tui.destroy),
		);

		yield* ConsumeEvents(tui);
	}),
);

Effect.runPromise(Main)
	.catch((cause: unknown) => {
		console.error(`[dev-tui] ${cause instanceof Error ? cause.message : String(cause)}`);
		process.exitCode = 1;
	})
	.finally(() => {
		event_input.destroy();
		command_output.end();
	});
