import cross_spawn from "cross-spawn";

let engine_process;
let started = false;
let ending = false;
let claim_token;

function send(message, callback = () => undefined) {
	if (process.connected) {
		process.send(message, callback);
	} else {
		callback();
	}
}

function exit_code(code, signal) {
	if (code !== null) {
		return code;
	}

	if (signal === "SIGINT") {
		return 130;
	}

	if (signal === "SIGTERM") {
		return 143;
	}

	if (signal === "SIGKILL") {
		return 137;
	}

	return 1;
}

function flush_and_exit(code) {
	let pending = 2;
	const flushed = () => {
		pending -= 1;

		if (pending > 0) {
			return;
		}

		if (process.connected) {
			process.disconnect();
		}

		process.exit(code);
	};

	process.stdout.write("", flushed);
	process.stderr.write("", flushed);
}

function fail(message) {
	if (ending) {
		return;
	}

	ending = true;
	send({ type: "spawn_error", message }, () => flush_and_exit(127));
}

process.on("message", (message) => {
	if (
		claim_token === undefined &&
		message !== null &&
		typeof message === "object" &&
		message.type === "claim" &&
		typeof message.claim_token === "string" &&
		message.claim_token.length > 0
	) {
		claim_token = message.claim_token;
		send({ type: "claim_ack", claim_token });

		return;
	}

	if (
		started ||
		claim_token === undefined ||
		message === null ||
		typeof message !== "object" ||
		message.type !== "start" ||
		message.input === null ||
		typeof message.input !== "object"
	) {
		fail("The Windows process host received an invalid start message");

		return;
	}

	started = true;
	const input = message.input;

	try {
		engine_process = cross_spawn(input.command, input.args, {
			cwd: input.cwd,
			env: input.env,
			shell: false,
			stdio: "pipe",
			windowsHide: true,
		});
	} catch (cause) {
		fail(cause instanceof Error ? cause.message : "The engine process failed to spawn");

		return;
	}

	engine_process.stdin.once("error", () => undefined);
	process.stdin.pipe(engine_process.stdin);
	engine_process.stdout.pipe(process.stdout, { end: false });
	engine_process.stderr.pipe(process.stderr, { end: false });

	engine_process.once("spawn", () => {
		send({ type: "ready", process_id: engine_process.pid });
	});
	engine_process.once("error", (cause) => {
		fail(cause instanceof Error ? cause.message : "The engine process failed");
	});
	engine_process.once("close", (code, signal) => {
		if (ending) {
			return;
		}

		ending = true;
		flush_and_exit(exit_code(code, signal));
	});
});

process.once("disconnect", () => {
	if (engine_process?.exitCode === null && engine_process.signalCode === null) {
		engine_process.kill("SIGKILL");
	}
});
