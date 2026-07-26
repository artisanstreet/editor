import { spawn, type ChildProcess } from "node:child_process";
import { randomUUID } from "node:crypto";
import { existsSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";

import { afterEach, describe, expect, it } from "vitest";
import { Effect, Fiber, Layer, ManagedRuntime, Stream } from "effect";
import { WebSocket } from "ws";

import { resolve_codex_executable } from "@artisan/engines";
import type { ConversationSnapshot } from "@artisan/protocol";
import {
	ArtisanClient,
	type ConversationUpdate,
	make_artisan_client_layer,
	TransportRuntimeLive,
} from "@artisan/transport/client";
import {
	type BrowserWebSocket,
	make_websocket_connector_layer,
} from "@artisan/transport/websocket/client";

interface ForgeReady {
	readonly endpoint: string;
	readonly kind: "artisan:forge-ready";
	readonly pid: number;
}

interface RunningForge {
	readonly child: ChildProcess;
	readonly ready: ForgeReady;
	readonly stderr: () => string;
}

const temporary_roots: Array<string> = [];
const children: Array<ChildProcess> = [];

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
	await Promise.all(
		temporary_roots.splice(0).map((root) => rm(root, { force: true, recursive: true })),
	);
});

const StartForge = async (database_path: string, codex_home: string): Promise<RunningForge> => {
	const executable = resolve(".dist/forge/Artisan Forge.exe");
	const entry = resolve(".dist/forge/host.js");
	const codex_executable = resolve_codex_executable();
	if (!existsSync(codex_executable)) {
		throw new Error(`Real Codex CLI was not found at ${codex_executable}`);
	}
	if (codex_executable.toLowerCase().includes("fake-codex")) {
		throw new Error("The real Codex acceptance test cannot use a fixture executable");
	}

	let stderr = "";
	const child = spawn(executable, [entry], {
		env: {
			...process.env,
			ARTISAN_ALLOWED_ORIGINS: "http://127.0.0.1",
			ARTISAN_AUTH_TOKEN: "artisan-real-codex-acceptance-token",
			ARTISAN_CODEX_EXECUTABLE: codex_executable,
			ARTISAN_DATABASE_PATH: database_path,
			ARTISAN_LISTEN_HOST: "127.0.0.1",
			ARTISAN_LISTEN_PORT: "0",
			ARTISAN_MIGRATIONS_PATH: resolve(".dist/forge/migrations"),
			ARTISAN_NATIVE_RUNTIME: resolve(".dist/forge/native-runtime"),
			ARTISAN_NODE_EXECUTABLE: resolve(".dist/forge/node.exe"),
			ARTISAN_PROJECT_ROOTS: JSON.stringify([process.cwd()]),
			ARTISAN_STATIC_FRONTEND_ROOT: resolve(".dist/forge/frontend"),
			ARTISAN_WINDOWS_PROCESS_HOST: resolve(".dist/forge/windows-process-host.js"),
			CODEX_HOME: codex_home,
			NODE_PATH: resolve(".dist/forge/native-runtime"),
		},
		stdio: ["ignore", "pipe", "pipe", "ipc"],
		windowsHide: true,
	});
	children.push(child);
	if (child.stdout === null || child.stderr === null) {
		throw new Error("Artisan Forge did not expose diagnostic pipes");
	}
	child.stderr.setEncoding("utf8");
	child.stderr.on("data", (data: string) => {
		stderr += data;
	});

	const ready = await new Promise<ForgeReady>((accept, reject) => {
		const timeout = setTimeout(
			() => reject(new Error(`Forge readiness timed out:\n${stderr}`)),
			30_000,
		);
		child.once("exit", (code) => {
			clearTimeout(timeout);
			reject(new Error(`Forge exited before readiness (${String(code)}):\n${stderr}`));
		});
		createInterface({ input: child.stdout! }).on("line", (line) => {
			try {
				const decoded = JSON.parse(line) as ForgeReady;
				if (decoded.kind !== "artisan:forge-ready") return;
				clearTimeout(timeout);
				accept(decoded);
			} catch {
				/** Ordinary diagnostics are not readiness records. */
			}
		});
	});

	return { child, ready, stderr: () => stderr };
};

const MakeClient = async (endpoint: string) => {
	const url = new URL("/api/ws", endpoint);
	url.protocol = "ws:";
	const requested = await fetch(new URL("/api/pair/request", endpoint), {
		headers: { authorization: "Bearer artisan-real-codex-acceptance-token" },
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

	return ManagedRuntime.make(
		make_artisan_client_layer().pipe(
			Layer.provideMerge(
				make_websocket_connector_layer({
					create_socket: (url) =>
						new WebSocket(url, { headers: { cookie } }) as unknown as BrowserWebSocket,
					url: url.toString(),
				}),
			),
			Layer.provide(TransportRuntimeLive),
		),
	);
};

const CreateThread = (client: typeof ArtisanClient.Service, thread_id: string, title: string) =>
	Effect.gen(function* () {
		yield* client.Command({
			command_id: `create:${thread_id}`,
			payload: { title, type: "thread.create" },
			thread_id,
		});
		yield* client.Command({
			command_id: `project:${thread_id}`,
			payload: {
				project: {
					display_name: "Artisan Editor",
					project_id: "project_artisan_real_acceptance",
					root_path: process.cwd(),
				},
				type: "thread.project.assign",
			},
			thread_id,
		});
	});

const WaitForFinal = (client: typeof ArtisanClient.Service, thread_id: string, marker: string) =>
	Effect.gen(function* () {
		let latest: ConversationSnapshot | undefined;
		for (let attempt = 0; attempt < 360; attempt += 1) {
			latest = yield* client.GetConversation({ thread_id });
			const answer = latest.items.find(
				(item) =>
					item.type === "assistant_message" &&
					item.lifecycle === "completed" &&
					item.text.includes(marker),
			);
			if (answer !== undefined) return latest;

			const failure = latest.items.find(
				(item) =>
					item.type === "error" ||
					(item.type === "work_session" && item.status === "failed"),
			);
			if (failure !== undefined) {
				return yield* Effect.fail(
					new Error(
						`Artisan exposed a failed Codex run: ${JSON.stringify(failure)}\nConversation: ${JSON.stringify(latest)}`,
					),
				);
			}
			yield* Effect.sleep("250 millis");
		}

		return yield* Effect.fail(
			new Error(`Codex did not settle ${thread_id}: ${JSON.stringify(latest)}`),
		);
	});

const RunPrompt = (
	client: typeof ArtisanClient.Service,
	thread_id: string,
	marker: string,
	updates: Array<ConversationUpdate>,
) =>
	Effect.scoped(
		Effect.gen(function* () {
			const stream = yield* client.SubscribeConversation(thread_id);
			const subscription = yield* stream.pipe(
				Stream.runForEach((update) =>
					Effect.sync(() => {
						updates.push(update);
					}),
				),
				Effect.forkScoped,
			);
			yield* client.Command({
				command_id: `prompt:${thread_id}`,
				payload: {
					engine_id: "codex",
					mentioned_projects: [],
					text: `Reply with exactly ${marker} and no other text. Do not call tools.`,
					type: "thread.send_message",
					working_directory: process.cwd(),
				},
				thread_id,
			});
			const settled = yield* WaitForFinal(client, thread_id, marker);
			yield* Fiber.interrupt(subscription);
			return settled;
		}),
	);

const PatchItems = (updates: ReadonlyArray<ConversationUpdate>) =>
	updates.flatMap((update) =>
		update.type === "patch"
			? update.batch.patches.flatMap((patch) =>
					patch.type === "item_upsert" ? [patch.item] : [],
				)
			: update.snapshot.items,
	);

describe.runIf(process.env.ARTISAN_RUN_REAL_CODEX_E2E === "1")(
	"real Codex standalone acceptance",
	() => {
		it("streams isolated durable answers for two threads and restores both after restart", async () => {
			const root = await mkdtemp(join(tmpdir(), "artisan-real-codex-"));
			temporary_roots.push(root);
			const database_path = join(root, "artisan.sqlite");
			const codex_home = join(root, "codex-home");
			const source_codex_home =
				process.env.CODEX_HOME ?? join(process.env.USERPROFILE ?? "", ".codex");
			await mkdir(codex_home, { recursive: true });
			await copyFile(join(source_codex_home, "auth.json"), join(codex_home, "auth.json"));
			if (existsSync(join(source_codex_home, "config.toml"))) {
				await copyFile(
					join(source_codex_home, "config.toml"),
					join(codex_home, "config.toml"),
				);
			}
			const alpha_thread = `thread_alpha_${randomUUID()}`;
			const beta_thread = `thread_beta_${randomUUID()}`;
			const alpha_marker = `ARTISAN_ALPHA_${randomUUID().replaceAll("-", "")}`;
			const beta_marker = `ARTISAN_BETA_${randomUUID().replaceAll("-", "")}`;
			const first = await StartForge(database_path, codex_home);
			const runtime = await MakeClient(first.ready.endpoint);
			const client = await runtime.runPromise(ArtisanClient);
			const alpha_updates: Array<ConversationUpdate> = [];
			const beta_updates: Array<ConversationUpdate> = [];
			let alpha: ConversationSnapshot;
			let beta: ConversationSnapshot;

			try {
				await runtime.runPromise(CreateThread(client, alpha_thread, "Alpha real Codex"));
				await runtime.runPromise(CreateThread(client, beta_thread, "Beta real Codex"));
				alpha = await runtime.runPromise(
					RunPrompt(client, alpha_thread, alpha_marker, alpha_updates),
				);
				beta = await runtime.runPromise(
					RunPrompt(client, beta_thread, beta_marker, beta_updates),
				);

				expect(alpha.conversation_id).not.toBe(beta.conversation_id);
				expect(JSON.stringify(alpha)).toContain(alpha_marker);
				expect(JSON.stringify(alpha)).not.toContain(beta_marker);
				expect(JSON.stringify(beta)).toContain(beta_marker);
				expect(JSON.stringify(beta)).not.toContain(alpha_marker);

				for (const updates of [alpha_updates, beta_updates]) {
					const items = PatchItems(updates);
					expect(
						items.some(
							(item) =>
								item.type === "work_session" &&
								(item.lifecycle === "pending" ||
									item.lifecycle === "active" ||
									item.lifecycle === "streaming"),
						),
					).toBe(true);
					expect(
						items.some(
							(item) =>
								item.type === "assistant_message" &&
								(item.lifecycle === "streaming" || item.lifecycle === "completed"),
						),
					).toBe(true);
				}
			} catch (cause) {
				throw new Error(
					`${cause instanceof Error ? cause.message : String(cause)}\nForge stderr:\n${first.stderr()}`,
				);
			} finally {
				await runtime.runPromise(client.Dispose).catch(() => undefined);
				await runtime.dispose().catch(() => undefined);
				await StopChild(first.child);
			}

			const restarted = await StartForge(database_path, codex_home);
			const restarted_runtime = await MakeClient(restarted.ready.endpoint);
			const restarted_client = await restarted_runtime.runPromise(ArtisanClient);
			try {
				const restored_alpha = await restarted_runtime.runPromise(
					restarted_client.GetConversation({ thread_id: alpha_thread }),
				);
				const restored_beta = await restarted_runtime.runPromise(
					restarted_client.GetConversation({ thread_id: beta_thread }),
				);
				expect(JSON.stringify(restored_alpha)).toContain(alpha_marker);
				expect(JSON.stringify(restored_alpha)).not.toContain(beta_marker);
				expect(JSON.stringify(restored_beta)).toContain(beta_marker);
				expect(JSON.stringify(restored_beta)).not.toContain(alpha_marker);
			} catch (cause) {
				throw new Error(
					`${cause instanceof Error ? cause.message : String(cause)}\nRestarted Forge stderr:\n${restarted.stderr()}`,
				);
			} finally {
				await restarted_runtime.runPromise(restarted_client.Dispose).catch(() => undefined);
				await restarted_runtime.dispose().catch(() => undefined);
				await StopChild(restarted.child);
			}
		}, 240_000);
	},
);
