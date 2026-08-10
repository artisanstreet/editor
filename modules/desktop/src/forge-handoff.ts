import { spawn, type ChildProcess, type SpawnOptions } from "node:child_process";
import { extname } from "node:path";

import { Context, Deferred, Effect, Exit, Layer, Option, SynchronizedRef } from "effect";

import {
	DesktopLauncherError,
	DecodeHandoffOutput,
	ForgeHandoff,
	renderer_handoff_url,
	renderer_loader_url,
} from "./renderer-host";

const max_handoff_stdout_bytes = 64 * 1024;

/** Lets the CLI's final pipe write reach Node without waiting for inherited handles to close. */
export const handoff_exit_drain_delay_ms = 100;

/** Forge may use 15 seconds of readiness plus a five-second pair request before ownership returns. */
export const handoff_cleanup_timeout = "25 seconds";

/** Exact stop includes Forge's authenticated shutdown and process-exit wait. */
export const owned_stop_cleanup_timeout = "30 seconds";

/** Only legacy Windows command scripts require cmd.exe compatibility. */
export const IsWindowsCommandScript = (command_path: string) => {
	const extension = extname(command_path).toLowerCase();
	return extension === ".cmd" || extension === ".bat";
};

/** A desktop may stop only the Forge instance its handoff explicitly owns. */
export const OwnedForgeStopArguments = (instance_id: string) => [
	"stop",
	"--instance-id",
	instance_id,
];

export class ForgeHandoffProcess extends Context.Service<
	ForgeHandoffProcess,
	{
		readonly Request: (
			ae_command_path: string,
		) => Effect.Effect<ForgeHandoff, DesktopLauncherError>;
		readonly StopOwned: (
			ae_command_path: string,
			instance_id: string,
		) => Effect.Effect<void, DesktopLauncherError>;
	}
>()("Artisan/ForgeHandoffProcess") {}

export type DesktopProcessSpawner = (
	command: string,
	arguments_: ReadonlyArray<string>,
	options: SpawnOptions,
) => ChildProcess;

const SpawnDesktopProcess: DesktopProcessSpawner = (command, arguments_, options) =>
	spawn(command, [...arguments_], options);

/**
 * Effect's ChildProcess API models the streams and detached lifecycle we use
 * elsewhere, but beta.97 has no Node `windowsHide` equivalent. This narrow
 * adapter keeps that Windows-only no-terminal control at the OS boundary.
 */
export const make_node_forge_handoff_process_layer_with = (spawn_process: DesktopProcessSpawner) =>
	Layer.succeed(
		ForgeHandoffProcess,
		ForgeHandoffProcess.of({
			Request: (ae_command_path) =>
				Effect.callback<ForgeHandoff, DesktopLauncherError>((resume) => {
					const child = spawn_process(ae_command_path, ["open", "--handoff"], {
						shell: IsWindowsCommandScript(ae_command_path),
						stdio: ["ignore", "pipe", "ignore"],
						windowsHide: true,
					});
					let stdout = "";
					let stdout_overflowed = false;
					let settled = false;
					let exit_drain_timer: ReturnType<typeof setTimeout> | undefined;
					const complete = (
						result: Effect.Effect<ForgeHandoff, DesktopLauncherError>,
					) => {
						if (settled) return;
						settled = true;
						if (exit_drain_timer !== undefined) clearTimeout(exit_drain_timer);
						child.off("error", on_error);
						child.off("exit", on_exit);
						child.stdout?.off("data", on_stdout);
						resume(result);
					};
					const on_error = (cause: Error) =>
						complete(
							Effect.fail(
								new DesktopLauncherError({ cause, reason: "handoff_failed" }),
							),
						);
					const on_exit = (exit_code: number | null) => {
						if (exit_code !== 0 || stdout_overflowed) {
							complete(
								Effect.fail(
									new DesktopLauncherError({
										cause: new Error(
											stdout_overflowed
												? "ae open --handoff exceeded its stdout limit"
												: `ae open --handoff exited ${String(exit_code)}`,
										),
										reason: "handoff_failed",
									}),
								),
							);
							return;
						}
						/**
						 * Windows can leave Forge holding an inherited copy of this stdout pipe
						 * after `ae` exits. The `close` event would then wait for Forge itself,
						 * creating a lifecycle cycle before Electron can record ownership.
						 * The handoff is one bounded line written before CLI exit, so drain that
						 * final write briefly and deliberately do not wait for pipe EOF.
						 */
						exit_drain_timer = setTimeout(() => {
							if (stdout_overflowed) {
								complete(
									Effect.fail(
										new DesktopLauncherError({
											cause: new Error(
												"ae open --handoff exceeded its stdout limit",
											),
											reason: "handoff_failed",
										}),
									),
								);
								return;
							}
							complete(DecodeHandoffOutput(stdout));
						}, handoff_exit_drain_delay_ms);
					};
					const on_stdout = (chunk: string) => {
						if (stdout_overflowed) return;
						if (stdout.length + chunk.length > max_handoff_stdout_bytes) {
							stdout_overflowed = true;
							return;
						}
						stdout += chunk;
					};
					child.stdout?.setEncoding("utf8");
					child.stdout?.on("data", on_stdout);
					child.once("error", on_error);
					child.once("exit", on_exit);

					return Effect.sync(() => {
						settled = true;
						if (exit_drain_timer !== undefined) clearTimeout(exit_drain_timer);
						child.off("error", on_error);
						child.off("exit", on_exit);
						child.stdout?.off("data", on_stdout);
						child.kill();
					});
				}),
			StopOwned: (ae_command_path, instance_id) =>
				Effect.callback<void, DesktopLauncherError>((resume) => {
					const child = spawn_process(
						ae_command_path,
						OwnedForgeStopArguments(instance_id),
						{
							shell: IsWindowsCommandScript(ae_command_path),
							stdio: "ignore",
							windowsHide: true,
						},
					);
					let settled = false;
					const complete = (result: Effect.Effect<void, DesktopLauncherError>) => {
						if (settled) return;
						settled = true;
						child.off("error", on_error);
						child.off("close", on_close);
						resume(result);
					};
					const on_error = (cause: Error) =>
						complete(
							Effect.fail(
								new DesktopLauncherError({ cause, reason: "handoff_failed" }),
							),
						);
					const on_close = (exit_code: number | null) =>
						complete(
							exit_code === 0
								? Effect.void
								: Effect.fail(
										new DesktopLauncherError({
											cause: new Error(`ae stop exited ${String(exit_code)}`),
											reason: "handoff_failed",
										}),
									),
						);
					child.once("error", on_error);
					child.once("close", on_close);
					return Effect.sync(() => {
						settled = true;
						child.off("error", on_error);
						child.off("close", on_close);
						child.kill();
					});
				}),
		}),
	);

export const make_node_forge_handoff_process_layer =
	make_node_forge_handoff_process_layer_with(SpawnDesktopProcess);

export class DesktopRenderer extends Context.Service<
	DesktopRenderer,
	{
		readonly ClearCookies: () => Effect.Effect<void, unknown>;
		readonly LoadUrl: (url: string) => Effect.Effect<void, unknown>;
	}
>()("Artisan/DesktopRenderer") {}

interface HandoffState {
	readonly in_flight: Option.Option<Deferred.Deferred<ForgeHandoff, DesktopLauncherError>>;
	readonly next_navigation_id: number;
	readonly owned_instance_id: Option.Option<string>;
	readonly reconnect_in_flight: Option.Option<Deferred.Deferred<void>>;
}

export class DesktopForgeLifecycle extends Context.Service<
	DesktopForgeLifecycle,
	{
		readonly Cleanup: () => Effect.Effect<void>;
		readonly Start: () => Effect.Effect<void, unknown>;
		readonly Reconnect: () => Effect.Effect<void>;
	}
>()("Artisan/DesktopForgeLifecycle") {}

/** Serializes one-time handoffs and retains only an editor-created Forge identity. */
export const make_desktop_forge_lifecycle_layer = (ae_command_path: string) =>
	Layer.effect(
		DesktopForgeLifecycle,
		Effect.gen(function* () {
			const process = yield* ForgeHandoffProcess;
			const renderer = yield* DesktopRenderer;
			const state = yield* SynchronizedRef.make<HandoffState>({
				in_flight: Option.none(),
				next_navigation_id: 1,
				owned_instance_id: Option.none(),
				reconnect_in_flight: Option.none(),
			});

			const RequestHandoff = () =>
				Effect.uninterruptibleMask((restore) =>
					Effect.gen(function* () {
						const deferred = yield* Deferred.make<ForgeHandoff, DesktopLauncherError>();
						const selected = yield* SynchronizedRef.modify(state, (current) =>
							Option.isSome(current.in_flight)
								? [{ deferred: current.in_flight.value, start: false }, current]
								: [
										{ deferred, start: true },
										{ ...current, in_flight: Option.some(deferred) },
									],
						);
						if (!selected.start)
							return yield* restore(Deferred.await(selected.deferred));
						const exit = yield* restore(process.Request(ae_command_path)).pipe(
							Effect.exit,
						);
						if (Exit.isFailure(exit)) {
							yield* Deferred.complete(selected.deferred, exit);
							yield* SynchronizedRef.update(state, (current) => ({
								...current,
								in_flight: Option.none(),
							}));
							return yield* Effect.failCause(exit.cause);
						}
						const result = exit.value;
						const owned_instance_id = result.owned_instance_id;
						if (owned_instance_id !== undefined) {
							yield* SynchronizedRef.update(state, (current) => ({
								...current,
								owned_instance_id: Option.some(owned_instance_id),
							}));
						}
						yield* Deferred.succeed(selected.deferred, result);
						yield* SynchronizedRef.update(state, (current) => ({
							...current,
							in_flight: Option.none(),
						}));
						return result;
					}),
				);

			const RequestAndNavigate = () =>
				RequestHandoff().pipe(
					Effect.flatMap((handoff) =>
						SynchronizedRef.modify(state, (current) => [
							renderer_handoff_url(handoff, current.next_navigation_id),
							{
								...current,
								next_navigation_id: current.next_navigation_id + 1,
							},
						]).pipe(Effect.flatMap(renderer.LoadUrl)),
					),
					Effect.catch((cause) =>
						Effect.sync(() =>
							console.error(
								JSON.stringify({
									kind: "artisan:desktop-handoff",
									message: String(cause),
									ok: false,
								}),
							),
						),
					),
				);

			const Reconnect = () =>
				Effect.uninterruptibleMask((restore) =>
					Effect.gen(function* () {
						const deferred = yield* Deferred.make<void>();
						const selected = yield* SynchronizedRef.modify(state, (current) =>
							Option.isSome(current.reconnect_in_flight)
								? [
										{
											deferred: current.reconnect_in_flight.value,
											start: false,
										},
										current,
									]
								: [
										{ deferred, start: true },
										{ ...current, reconnect_in_flight: Option.some(deferred) },
									],
						);
						if (!selected.start)
							return yield* restore(Deferred.await(selected.deferred));
						const Complete = (exit: Exit.Exit<void>) =>
							(Exit.isSuccess(exit)
								? Deferred.succeed(selected.deferred, undefined)
								: Deferred.failCause(selected.deferred, exit.cause)
							).pipe(
								Effect.andThen(
									SynchronizedRef.update(state, (current) => ({
										...current,
										reconnect_in_flight: Option.none(),
									})),
								),
							);
						return yield* restore(RequestAndNavigate()).pipe(Effect.onExit(Complete));
					}),
				);

			return DesktopForgeLifecycle.of({
				Cleanup: () =>
					Effect.gen(function* () {
						const current = yield* SynchronizedRef.get(state);
						if (
							Option.isNone(current.owned_instance_id) &&
							Option.isSome(current.reconnect_in_flight)
						) {
							yield* Deferred.await(current.reconnect_in_flight.value).pipe(
								Effect.timeout(handoff_cleanup_timeout),
								Effect.catch(() => Effect.void),
							);
						}
						const owned_instance_id = (yield* SynchronizedRef.get(state))
							.owned_instance_id;
						if (Option.isNone(owned_instance_id)) return;
						yield* process.StopOwned(ae_command_path, owned_instance_id.value).pipe(
							Effect.timeout(owned_stop_cleanup_timeout),
							Effect.catch(() => Effect.void),
						);
					}),
				Reconnect,
				Start: () =>
					Effect.gen(function* () {
						yield* renderer.ClearCookies();
						yield* renderer.LoadUrl(renderer_loader_url);
					}),
			});
		}),
	);
