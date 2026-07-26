import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";

import { Effect, Exit, Fiber, Option } from "effect";
import { beforeEach, describe, expect, it, vi } from "vitest";

const spawn_mock = vi.hoisted(() => vi.fn());

vi.mock("node:child_process", async (import_original) => {
	const original = await import_original<typeof import("node:child_process")>();
	return { ...original, spawn: spawn_mock };
});

import { AcquireForgeProcessSupervisor } from "../../modules/desktop/src/forge-process-supervisor";
import type {
	ForgeBrowserSession,
	ForgeProcessSupervisor,
} from "../../modules/desktop/src/forge-process-supervisor";
import type { ResolvedDesktopPaths } from "../../modules/desktop/src/paths";

class FakeChild extends EventEmitter {
	readonly pid: number;
	readonly stderr = new PassThrough();
	readonly stdout = new PassThrough();
	exitCode: number | null = null;
	readonly kill = vi.fn(() => {
		this.exitCode = 1;
		this.emit("exit", 1);
		return true;
	});
	readonly send = vi.fn((_message: unknown, callback?: (cause: Error | null) => void) => {
		callback?.(null);
		this.exitCode = 0;
		this.emit("exit", 0);
		return true;
	});

	constructor(pid: number) {
		super();
		this.pid = pid;
	}

	Ready(endpoint: string): void {
		this.stdout.write(
			`${JSON.stringify({
				endpoint,
				kind: "artisan:forge-ready",
				pid: this.pid,
			})}\n`,
		);
	}

	Crash(): void {
		this.exitCode = 1;
		this.emit("exit", 1);
	}
}

const paths: ResolvedDesktopPaths = {
	database_path: "C:\\Artisan\\data\\artisan.sqlite",
	forge_entry_path: "C:\\Artisan\\forge\\host.js",
	forge_executable_path: "C:\\Artisan\\forge\\Artisan Forge.exe",
	forge_native_runtime_path: "C:\\Artisan\\forge\\native-runtime",
	forge_node_executable_path: "C:\\Artisan\\forge\\node.exe",
	preload_path: "C:\\Artisan\\desktop\\preload.cjs",
};

describe("Effect Forge process supervisor", () => {
	const children: Array<FakeChild> = [];
	const fetch_mock = vi.fn();
	const install_browser_session = vi.fn();
	const AcquireSupervisor = AcquireForgeProcessSupervisor(paths, {
		Fetch: fetch_mock as typeof fetch,
		InstallBrowserSession: (session: ForgeBrowserSession) =>
			Effect.tryPromise({
				try: () => install_browser_session(session),
				catch: (cause) => cause,
			}),
	});

	beforeEach(() => {
		children.length = 0;
		spawn_mock.mockReset();
		spawn_mock.mockImplementation(() => {
			const child = new FakeChild(1_000 + children.length);
			children.push(child);
			return child;
		});
		fetch_mock.mockReset();
		fetch_mock.mockImplementation(async (input: Parameters<typeof fetch>[0]) =>
			String(input).endsWith("/api/pair/request")
				? new Response(JSON.stringify({ code: "one-time-pair-code" }), { status: 200 })
				: new Response(JSON.stringify({ status: "paired" }), {
						headers: {
							"set-cookie":
								"artisan_forge_session=browser-session; HttpOnly; SameSite=Strict; Path=/",
						},
						status: 200,
					}),
		);
		install_browser_session.mockReset();
		install_browser_session.mockResolvedValue(undefined);
	});

	it("starts, pairs, and exposes Effect-owned process state", async () => {
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const supervisor: ForgeProcessSupervisor = yield* AcquireSupervisor;
					const connection_fiber = yield* Effect.forkScoped(supervisor.Start);
					yield* Effect.yieldNow;
					yield* Effect.yieldNow;
					expect(children).toHaveLength(1);
					children[0]?.Ready("http://127.0.0.1:43123/");
					const connection = yield* Fiber.join(connection_fiber);
					expect(connection).toEqual({
						http_endpoint: "http://127.0.0.1:43123/",
						websocket_endpoint: "ws://127.0.0.1:43123/api/ws",
					});
					expect(Option.getOrUndefined(yield* supervisor.GetForgePid)).toBe(1_000);
					expect(install_browser_session).toHaveBeenCalledWith({
						http_endpoint: "http://127.0.0.1:43123/",
						token: "browser-session",
					});
				}),
			),
		);
	});

	it("uses IPC shutdown and prevents later starts after disposal", async () => {
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const supervisor = yield* AcquireSupervisor;
					const connection_fiber = yield* Effect.forkScoped(supervisor.Start);
					yield* Effect.yieldNow;
					yield* Effect.yieldNow;
					children[0]?.Ready("http://127.0.0.1:43123/");
					yield* Fiber.join(connection_fiber);
					yield* supervisor.Dispose;
					expect(children[0]?.send).toHaveBeenCalledWith(
						{ kind: "artisan:forge-shutdown" },
						expect.any(Function),
					);
					const disposed = yield* Effect.exit(supervisor.Start);
					expect(Exit.isFailure(disposed)).toBe(true);
				}),
			),
		);
	});
});
