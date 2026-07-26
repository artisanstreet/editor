import type { ChildProcessWithoutNullStreams } from "node:child_process";

import cross_spawn from "cross-spawn";

import type { EngineProcessSpawnInput } from "./process";

interface ClaimMessage {
	readonly claim_token: string;
	readonly type: "claim";
}

interface StartMessage {
	readonly input: EngineProcessSpawnInput;
	readonly type: "start";
}

let engine_process: ChildProcessWithoutNullStreams | undefined;
let started = false;
let ending = false;
let claim_token: string | undefined;

const Send = (message: unknown, callback: (cause?: Error | null) => void = () => undefined) => {
	const send = process.send;
	if (process.connected && send) {
		send.call(process, message, callback);
	} else {
		callback();
	}
};

const exit_code = (code: number | null, signal: NodeJS.Signals | null) => {
	if (code !== null) return code;
	if (signal === "SIGINT") return 130;
	if (signal === "SIGTERM") return 143;
	if (signal === "SIGKILL") return 137;
	return 1;
};

const FlushAndExit = (code: number) => {
	let pending = 2;
	const flushed = () => {
		pending -= 1;
		if (pending > 0) return;
		const disconnect = process.disconnect;
		if (process.connected && disconnect) disconnect.call(process);
		process.exit(code);
	};

	process.stdout.write("", flushed);
	process.stderr.write("", flushed);
};

const Fail = (message: string) => {
	if (ending) return;
	ending = true;
	Send({ message, type: "spawn_error" }, () => FlushAndExit(127));
};

const is_claim_message = (message: unknown): message is ClaimMessage =>
	message !== null &&
	typeof message === "object" &&
	"type" in message &&
	message.type === "claim" &&
	"claim_token" in message &&
	typeof message.claim_token === "string" &&
	message.claim_token.length > 0;

const is_start_message = (message: unknown): message is StartMessage =>
	message !== null &&
	typeof message === "object" &&
	"type" in message &&
	message.type === "start" &&
	"input" in message &&
	message.input !== null &&
	typeof message.input === "object";

process.on("message", (message: unknown) => {
	if (claim_token === undefined && is_claim_message(message)) {
		claim_token = message.claim_token;
		Send({ claim_token, type: "claim_ack" });
		return;
	}

	if (started || claim_token === undefined || !is_start_message(message)) {
		Fail("The Windows process host received an invalid start message");
		return;
	}

	started = true;
	const input = message.input;

	let spawned_engine;
	try {
		spawned_engine = cross_spawn(input.command, input.args, {
			cwd: input.cwd,
			env: input.env,
			shell: false,
			stdio: "pipe",
			windowsHide: true,
		});
	} catch (cause) {
		Fail(
			`${input.command} ${input.args.join(" ")} failed to spawn: ${
				cause instanceof Error ? cause.message : "unknown process error"
			}`,
		);
		return;
	}

	if (!spawned_engine.stdin || !spawned_engine.stdout || !spawned_engine.stderr) {
		spawned_engine.kill("SIGKILL");
		Fail("The engine process did not provide piped stdio");
		return;
	}

	const child = spawned_engine as ChildProcessWithoutNullStreams;
	engine_process = child;
	child.stdin.once("error", () => undefined);
	process.stdin.pipe(child.stdin);
	child.stdout.pipe(process.stdout, { end: false });
	child.stderr.pipe(process.stderr, { end: false });

	child.once("spawn", () => {
		Send({ process_id: child.pid, type: "ready" });
	});
	child.once("error", (cause) => {
		Fail(`${input.command} ${input.args.join(" ")} failed to spawn: ${cause.message}`);
	});
	child.once("close", (code, signal) => {
		if (ending) return;
		ending = true;
		FlushAndExit(exit_code(code, signal));
	});
});

process.once("disconnect", () => {
	if (engine_process?.exitCode === null && engine_process.signalCode === null) {
		engine_process.kill("SIGKILL");
	}
});
