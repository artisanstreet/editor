import { spawn, type ChildProcess } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, delimiter, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createInterface } from "node:readline";

import { afterEach, describe, expect, it } from "vitest";
import { Effect, Fiber, Layer, ManagedRuntime, Stream } from "effect";
import { WebSocket } from "ws";

import type { Project } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";
import { make_artisan_client_layer, TransportRuntimeLive } from "@artisan/transport/client";
import {
	type BrowserWebSocket,
	make_websocket_connector_layer,
} from "@artisan/transport/websocket/client";

interface ForgeReady {
	readonly endpoint: string;
	readonly kind: "artisan:forge-ready";
	readonly pid: number;
}

const roots: Array<string> = [];
const children: Array<ChildProcess> = [];
const fake_codex_path = fileURLToPath(
	new URL("../../engines/fixtures/fake-codex.cmd", import.meta.url),
);

const StopChild = async (child: ChildProcess) => {
	if (child.exitCode !== null) return;
	const exited = new Promise<void>((accept) => child.once("exit", () => accept()));
	child.send({ kind: "artisan:forge-shutdown" }, (cause) => {
		if (cause) child.kill();
	});
	await Promise.race([
		exited,
		new Promise<void>((accept) =>
			setTimeout(() => {
				child.kill();
				accept();
			}, 5_000),
		),
	]);
};

afterEach(async () => {
	await Promise.all(children.splice(0).map(StopChild));
	await Promise.all(roots.splice(0).map((root) => rm(root, { force: true, recursive: true })));
});

const AwaitReady = (child: ChildProcess) =>
	new Promise<ForgeReady>((accept, reject) => {
		if (child.stdout === null || child.stderr === null) {
			reject(new Error("Artisan Forge did not expose diagnostic pipes"));
			return;
		}

		let stderr = "";
		const timeout = setTimeout(
			() => reject(new Error(`Artisan Forge readiness timed out: ${stderr}`)),
			20_000,
		);
		child.stderr.setEncoding("utf8");
		child.stderr.on("data", (data: string) => {
			stderr += data;
		});
		child.once("exit", (code) => {
			clearTimeout(timeout);
			reject(new Error(`Artisan Forge exited before readiness (${String(code)}): ${stderr}`));
		});
		createInterface({ input: child.stdout }).on("line", (line) => {
			try {
				const decoded = JSON.parse(line) as ForgeReady;
				if (decoded.kind !== "artisan:forge-ready") return;
				clearTimeout(timeout);
				accept(decoded);
			} catch {
				/** Non-JSON stdout remains ordinary diagnostics. */
			}
		});
	});

const StartForge = async (input: {
	readonly database_path: string;
	readonly root: string;
	readonly token: string;
}) => {
	const executable = resolve(".dist/forge/Artisan Forge.exe");
	const entry = resolve(".dist/forge/host.js");
	const child = spawn(executable, [entry], {
		env: {
			...process.env,
			ARTISAN_ALLOWED_ORIGINS: "http://127.0.0.1",
			ARTISAN_AUTH_TOKEN: input.token,
			ARTISAN_CODEX_EXECUTABLE: fake_codex_path,
			ARTISAN_DATABASE_PATH: input.database_path,
			ARTISAN_LISTEN_HOST: "127.0.0.1",
			ARTISAN_LISTEN_PORT: "0",
			ARTISAN_MIGRATIONS_PATH: resolve(".dist/forge/migrations"),
			ARTISAN_NATIVE_RUNTIME: resolve(".dist/forge/native-runtime"),
			ARTISAN_NODE_EXECUTABLE: process.execPath,
			ARTISAN_STATIC_FRONTEND_ROOT: resolve(".dist/forge/frontend"),
			ARTISAN_WINDOWS_PROCESS_HOST: resolve(".dist/forge/windows-process-host.js"),
			/** Exercises the canonical long-lived Codex CLI stdio boundary. */
			FAKE_APP_SERVER_REQUEST_FILE: join(input.root, "codex-app-server-requests.jsonl"),
			FAKE_APP_SERVER_SCENARIO: "complete",
			NODE_PATH: [resolve(".dist/forge/native-runtime"), process.env.NODE_PATH]
				.filter(Boolean)
				.join(delimiter),
		},
		stdio: ["ignore", "pipe", "pipe", "ipc"],
		windowsHide: true,
	});
	children.push(child);

	return { child, ready: await AwaitReady(child) };
};

const PairBrowserSession = async (endpoint: string, token: string) => {
	const requested = await fetch(new URL("/api/pair/request", endpoint), {
		headers: { authorization: `Bearer ${token}` },
		method: "POST",
	});
	const body = (await requested.json()) as { readonly code: string };
	const exchanged = await fetch(new URL("/api/pair", endpoint), {
		body: JSON.stringify({ code: body.code }),
		headers: { "content-type": "application/json" },
		method: "POST",
	});
	const cookie = exchanged.headers.get("set-cookie")?.split(";", 1)[0];
	if (!requested.ok || !exchanged.ok || cookie === undefined) {
		throw new Error("Could not establish a Forge browser session");
	}
	return cookie;
};

const MakeClient = async (endpoint: string, token: string) => {
	const websocket_url = new URL("/api/ws", endpoint);
	websocket_url.protocol = "ws:";
	const cookie = await PairBrowserSession(endpoint, token);

	return ManagedRuntime.make(
		make_artisan_client_layer().pipe(
			Layer.provideMerge(
				make_websocket_connector_layer({
					create_socket: (url) =>
						new WebSocket(url, { headers: { cookie } }) as unknown as BrowserWebSocket,
					url: websocket_url.toString(),
				}),
			),
			Layer.provide(TransportRuntimeLive),
		),
	);
};

const WaitForSettledConversation = async (
	runtime: ManagedRuntime.ManagedRuntime<any, any>,
	client: typeof ArtisanClient.Service,
	thread_id: string,
) => {
	let latest_snapshot: unknown;
	let latest_transcript: unknown;
	let latest_session: unknown;
	for (let attempt = 0; attempt < 100; attempt += 1) {
		const snapshot = await runtime.runPromise(client.GetConversation({ thread_id }));
		latest_snapshot = snapshot;
		latest_transcript = await runtime
			.runPromise(client.GetThreadTranscript({ thread_id }))
			.catch(() => undefined);
		latest_session = await runtime
			.runPromise(client.GetThreadSession(thread_id))
			.catch(() => undefined);
		const final_message = snapshot.items.find(
			(item) => item.type === "assistant_message" && item.text === "hello",
		);
		const settled_turn = snapshot.turns.find((turn) => turn.lifecycle === "completed");

		if (final_message !== undefined && settled_turn !== undefined) return snapshot;

		await new Promise<void>((accept) => setTimeout(accept, 50));
	}

	throw new Error(
		`Codex fixture conversation did not settle through standalone Forge: ${JSON.stringify({ snapshot: latest_snapshot, transcript: latest_transcript, session: latest_session })}`,
	);
};

describe("standalone Artisan Forge process", () => {
	it("serves the immutable app and accepts typed protocol requests independently of Electron", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-forge-process-"));
		roots.push(root);
		const executable = resolve(".dist/forge/Artisan Forge.exe");
		const entry = resolve(".dist/forge/host.js");
		const token = "standalone-process-proof-token-with-32-chars";
		const child = spawn(executable, [entry], {
			env: {
				...process.env,
				ARTISAN_ALLOWED_ORIGINS: "http://127.0.0.1",
				ARTISAN_AUTH_TOKEN: token,
				ARTISAN_DATABASE_PATH: join(root, "artisan.db"),
				ARTISAN_FORGE_DEVELOPMENT: "1",
				ARTISAN_LISTEN_HOST: "127.0.0.1",
				ARTISAN_LISTEN_PORT: "0",
				ARTISAN_MIGRATIONS_PATH: resolve(".dist/forge/migrations"),
				ARTISAN_NATIVE_RUNTIME: resolve(".dist/forge/native-runtime"),
				ARTISAN_STATIC_FRONTEND_ROOT: resolve(".dist/forge/frontend"),
				ARTISAN_WINDOWS_PROCESS_HOST: resolve(".dist/forge/windows-process-host.js"),
				NODE_PATH: [resolve(".dist/forge/native-runtime"), process.env.NODE_PATH]
					.filter(Boolean)
					.join(delimiter),
			},
			stdio: ["ignore", "pipe", "pipe", "ipc"],
			windowsHide: true,
		});
		children.push(child);

		const ready = await AwaitReady(child);
		expect(ready.pid).toBe(child.pid);
		const health = await fetch(new URL("/health", ready.endpoint));
		/** This composition serves the SPA, so health marks it as development. */
		expect(await health.json()).toEqual({
			development: true,
			service: "artisan-forge",
			status: "ready",
			version: 1,
		});
		const application = await fetch(ready.endpoint);
		expect(application.headers.get("content-type")).toContain("text/html");
		expect(await application.text()).toContain("data-sveltekit-preload-data");

		const websocket_url = new URL("/api/ws", ready.endpoint);
		websocket_url.protocol = "ws:";
		const cookie = await PairBrowserSession(ready.endpoint, token);
		const runtime = ManagedRuntime.make(
			make_artisan_client_layer().pipe(
				Layer.provideMerge(
					make_websocket_connector_layer({
						create_socket: (url) =>
							new WebSocket(url, {
								headers: { cookie },
							}) as unknown as BrowserWebSocket,
						url: websocket_url.toString(),
					}),
				),
				Layer.provide(TransportRuntimeLive),
			),
		);
		const client = await runtime.runPromise(ArtisanClient);
		expect(await runtime.runPromise(client.ListThreads)).toEqual([]);
		await runtime.runPromise(client.Dispose);
		await runtime.dispose();

		await StopChild(child);
		const lock_path = `${join(root, "artisan.db")}.artisan-forge.lock`;
		await expect(readFile(lock_path, "utf8")).rejects.toMatchObject({ code: "ENOENT" });
	}, 30_000);

	it("runs a project-assigned thread through the Codex subprocess and retains its settled conversation after restart", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-standalone-codex-"));
		roots.push(root);
		const database_path = join(root, "artisan.db");
		const token = "standalone-codex-flow-token-with-32-chars";
		let thread_id = "";
		let project: Project;
		const first = await StartForge({ database_path, root, token });
		const first_runtime = await MakeClient(first.ready.endpoint, token);
		const first_client = await first_runtime.runPromise(ArtisanClient);

		try {
			expect(await first_runtime.runPromise(first_client.ListThreads)).toEqual([]);
			const repository_roots = await first_runtime.runPromise(
				first_client.ListProjectDirectories(),
			);
			const repository_root = repository_roots.directories.find(
				(directory) =>
					directory.kind === "root" && directory.display_name === basename(process.cwd()),
			);
			expect(repository_root).toBeDefined();
			project = await first_runtime.runPromise(
				first_client.SelectProjectDirectory({
					directory_id: repository_root!.directory_id,
				}),
			);
			thread_id = (
				await first_runtime.runPromise(
					first_client.CreateThread({
						project_id: project.project_id,
						title: "Standalone Codex flow",
					}),
				)
			).thread_id;

			const updates = await first_runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* first_client.SubscribeConversation(thread_id);
						const fiber = yield* stream.pipe(
							Stream.take(2),
							Stream.runCollect,
							Effect.forkScoped,
						);
						yield* first_client.Command({
							command_id: "standalone-send-message",
							payload: {
								engine_id: "codex",
								text: "Reply through the Codex fixture.",
								type: "thread.send_message",
							},
							thread_id,
						});

						return [...(yield* Fiber.join(fiber))];
					}),
				),
			);
			expect(updates).toMatchObject([
				{ type: "snapshot" },
				{
					type: "patch",
					batch: {
						patches: expect.arrayContaining([
							expect.objectContaining({ type: "turn_upsert" }),
							expect.objectContaining({
								item: expect.objectContaining({
									text: "Reply through the Codex fixture.",
									type: "user_message",
								}),
							}),
						]),
					},
				},
			]);

			const settled = await WaitForSettledConversation(
				first_runtime,
				first_client,
				thread_id,
			);
			expect(settled.items).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						text: "Reply through the Codex fixture.",
						type: "user_message",
					}),
					expect.objectContaining({
						text: "hello",
						type: "assistant_message",
					}),
				]),
			);
			expect(settled.turns).toEqual(
				expect.arrayContaining([expect.objectContaining({ lifecycle: "completed" })]),
			);
			const requests = (await readFile(join(root, "codex-app-server-requests.jsonl"), "utf8"))
				.trim()
				.split("\n")
				.map(
					(line) =>
						JSON.parse(line) as {
							readonly method: string;
							readonly params: Record<string, unknown>;
						},
				);
			expect(requests).toEqual(
				expect.arrayContaining([
					expect.objectContaining({ method: "thread/start" }),
					expect.objectContaining({ method: "turn/start" }),
				]),
			);
		} finally {
			await first_runtime.runPromise(first_client.Dispose).catch(() => undefined);
			await first_runtime.dispose().catch(() => undefined);
			await StopChild(first.child);
		}

		const restarted = await StartForge({ database_path, root, token });
		const restarted_runtime = await MakeClient(restarted.ready.endpoint, token);
		const restarted_client = await restarted_runtime.runPromise(ArtisanClient);
		try {
			const threads = await restarted_runtime.runPromise(restarted_client.ListThreads);
			expect(threads).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						primary_project: expect.objectContaining({
							project_id: project.project_id,
						}),
						thread_id,
					}),
				]),
			);
			const durable = await restarted_runtime.runPromise(
				restarted_client.GetConversation({ thread_id }),
			);
			expect(durable.items).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						text: "hello",
						type: "assistant_message",
					}),
				]),
			);
		} finally {
			await restarted_runtime.runPromise(restarted_client.Dispose).catch(() => undefined);
			await restarted_runtime.dispose().catch(() => undefined);
			await StopChild(restarted.child);
		}
	}, 45_000);

	it("lets a browser client select an opaque server directory and retains its assigned project", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-standalone-project-browser-"));
		roots.push(root);
		const database_path = join(root, "artisan.db");
		const token = "standalone-browser-project-token-with-32-chars";
		let thread_id = "";
		const repository_name = basename(process.cwd());
		const first = await StartForge({ database_path, root, token });
		const first_runtime = await MakeClient(first.ready.endpoint, token);
		const first_client = await first_runtime.runPromise(ArtisanClient);
		let project: {
			readonly display_name: string;
			readonly project_id: string;
			readonly root_path: string;
		};

		try {
			const roots = await first_runtime.runPromise(first_client.ListProjectDirectories());
			const repository_root = roots.directories.find(
				(directory) =>
					directory.kind === "root" && directory.display_name === repository_name,
			);

			expect(repository_root).toBeDefined();
			expect(repository_root).toMatchObject({
				directory_id: expect.stringMatching(/^directory_[a-f0-9-]+$/),
				display_name: repository_name,
				kind: "root",
			});
			expect(Object.keys(repository_root ?? {}).toSorted()).toEqual([
				"directory_id",
				"display_name",
				"has_children",
				"kind",
			]);

			/** A browser can browse the server root by opaque ID, never raw host path. */
			const repository_children = await first_runtime.runPromise(
				first_client.ListProjectDirectories({
					parent_directory_id: repository_root!.directory_id,
				}),
			);
			expect(repository_children.parent_directory_id).toBe(repository_root!.directory_id);

			project = await first_runtime.runPromise(
				first_client.SelectProjectDirectory({
					directory_id: repository_root!.directory_id,
				}),
			);
			expect(project).toMatchObject({
				display_name: repository_name,
				project_id: expect.stringMatching(/^[1-9]\d*$/),
			});
			expect(project.root_path).toBeTruthy();

			thread_id = (
				await first_runtime.runPromise(
					first_client.CreateThread({
						project_id: project.project_id,
						title: "Standalone browser project",
					}),
				)
			).thread_id;
		} finally {
			await first_runtime.runPromise(first_client.Dispose).catch(() => undefined);
			await first_runtime.dispose().catch(() => undefined);
			await StopChild(first.child);
		}

		const restarted = await StartForge({ database_path, root, token });
		const restarted_runtime = await MakeClient(restarted.ready.endpoint, token);
		const restarted_client = await restarted_runtime.runPromise(ArtisanClient);
		try {
			const threads = await restarted_runtime.runPromise(restarted_client.ListThreads);
			expect(threads).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						primary_project: {
							display_name: project!.display_name,
							project_id: project!.project_id,
							root_path: project!.root_path,
						},
						thread_id,
					}),
				]),
			);
		} finally {
			await restarted_runtime.runPromise(restarted_client.Dispose).catch(() => undefined);
			await restarted_runtime.dispose().catch(() => undefined);
			await StopChild(restarted.child);
		}
	}, 45_000);
});
