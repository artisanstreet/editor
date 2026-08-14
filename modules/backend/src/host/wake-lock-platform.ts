import { spawn } from "node:child_process";

import { Effect, Layer } from "effect";
import koffi from "koffi";

import {
	SystemWakeLockBackend,
	SystemWakeLockUnavailable,
	type SystemWakeLockHandle,
} from "./wake-lock";

const power_request_system_required = 1;
const power_request_context_version = 0;
const power_request_context_simple_string = 0x0000_0001;

function create_windows_power_api() {
	if (process.platform !== "win32") {
		throw new Error("Windows power requests are unavailable on this platform");
	}

	const kernel32 = koffi.load("kernel32.dll");

	koffi.pointer("ArtisanPowerRequestHandle", koffi.opaque());

	koffi.struct("ArtisanPowerReasonContext", {
		version: "uint32",
		flags: "uint32",
		simple_reason_string: "const char16_t *",
	});
	const power_create_request = kernel32.func(
		"ArtisanPowerRequestHandle __stdcall PowerCreateRequest(const ArtisanPowerReasonContext *context)",
	);
	const power_set_request = kernel32.func(
		"int __stdcall PowerSetRequest(ArtisanPowerRequestHandle request, int request_type)",
	);
	const power_clear_request = kernel32.func(
		"int __stdcall PowerClearRequest(ArtisanPowerRequestHandle request, int request_type)",
	);
	const close_handle = kernel32.func(
		"int __stdcall CloseHandle(ArtisanPowerRequestHandle handle)",
	);
	const get_last_error = kernel32.func("uint32 __stdcall GetLastError()");

	return {
		close_handle,
		get_last_error,
		power_clear_request,
		power_create_request,
		power_set_request,
	};
}

let windows_power_api: ReturnType<typeof create_windows_power_api> | undefined;

/**
 * Windows backend over kernel32 power requests. `PowerSetRequest` is preferred
 * over `SetThreadExecutionState` because the request is process-associated
 * rather than thread-affine, and its reason string shows up in
 * `powercfg /requests`, so a feature that silently changes OS power behavior
 * can explain itself.
 *
 * OS-imposed honesty: on Modern Standby hardware on DC power the request is
 * terminated five minutes after the sleep timeout, and every system terminates
 * requests on user-initiated sleep. Suspend detection and resume reconciliation
 * cover those paths.
 *
 * @since 0.7.0
 */
export const WindowsSystemWakeLockBackendLive = Layer.succeed(SystemWakeLockBackend, {
	Acquire: (reason) =>
		Effect.suspend(() => {
			try {
				windows_power_api ??= create_windows_power_api();

				const api = windows_power_api;
				const handle = api.power_create_request({
					version: power_request_context_version,
					flags: power_request_context_simple_string,
					simple_reason_string: reason,
				});

				if (handle === null) {
					return Effect.fail(
						new SystemWakeLockUnavailable({
							message: `PowerCreateRequest failed with Windows error ${api.get_last_error()}`,
						}),
					);
				}

				if (!api.power_set_request(handle, power_request_system_required)) {
					const error_code = api.get_last_error();

					api.close_handle(handle);

					return Effect.fail(
						new SystemWakeLockUnavailable({
							message: `PowerSetRequest failed with Windows error ${error_code}`,
						}),
					);
				}

				let released = false;

				return Effect.succeed<SystemWakeLockHandle>({
					Release: Effect.suspend(() => {
						if (released) return Effect.void;

						released = true;

						try {
							api.power_clear_request(handle, power_request_system_required);
							api.close_handle(handle);

							return Effect.void;
						} catch (cause) {
							return Effect.logWarning("Could not clear the Windows power request", {
								cause,
							});
						}
					}),
				});
			} catch (cause) {
				return Effect.fail(
					new SystemWakeLockUnavailable({
						cause,
						message: "The Windows power request API could not be loaded",
					}),
				);
			}
		}),
});

/**
 * Holds the assertion through a child process whose exit — commanded or not —
 * releases it. Both commands also self-release if Forge dies without cleanup:
 * `caffeinate -w` watches the Forge pid, and `cat` exits when its stdin pipe
 * closes with the parent.
 */
const make_child_process_backend = (
	command: string,
	make_arguments: (reason: string) => ReadonlyArray<string>,
) =>
	Layer.succeed(SystemWakeLockBackend, {
		Acquire: (reason) =>
			Effect.callback<SystemWakeLockHandle, SystemWakeLockUnavailable>((resume) => {
				const child = spawn(command, [...make_arguments(reason)], {
					stdio: ["pipe", "ignore", "ignore"],
				});

				child.once("spawn", () => {
					let released = false;

					resume(
						Effect.succeed<SystemWakeLockHandle>({
							Release: Effect.suspend(() => {
								if (released) return Effect.void;

								released = true;

								try {
									child.stdin?.end();
									child.kill();

									return Effect.void;
								} catch (cause) {
									return Effect.logWarning(
										"Could not stop the wake-lock helper process",
										{ cause, command },
									);
								}
							}),
						}),
					);
				});
				child.once("error", (cause) => {
					resume(
						Effect.fail(
							new SystemWakeLockUnavailable({
								cause,
								message: `${command} could not be started`,
							}),
						),
					);
				});
			}),
	});

/**
 * macOS backend over `/usr/bin/caffeinate`: shipped since 10.8, no
 * entitlement, and a thin wrapper over the same IOKit assertion an app would
 * take directly. `-i` prevents idle system sleep only; `-w` ties the
 * assertion's outer bound to Forge's own lifetime.
 *
 * @since 0.7.0
 */
export const DarwinSystemWakeLockBackendLive = make_child_process_backend(
	"/usr/bin/caffeinate",
	() => ["-i", "-w", `${process.pid}`],
);

/**
 * Linux backend over a logind inhibitor. `--what=idle` blocks the idle state
 * that triggers automatic suspend — the polkit action ordinary desktop
 * applications are expected to take, unlike blocking sleep outright. Hosts
 * without logind fail acquisition, which is correct: they do not idle-suspend.
 *
 * @since 0.7.0
 */
export const LinuxSystemWakeLockBackendLive = make_child_process_backend(
	"systemd-inhibit",
	(reason) => ["--what=idle", "--mode=block", "--who=Artisan", `--why=${reason}`, "cat"],
);

/** Fails every acquisition; the service logs once and the feature degrades to a no-op. @since 0.7.0 */
export const SystemWakeLockBackendUnsupported = Layer.succeed(SystemWakeLockBackend, {
	Acquire: () =>
		Effect.fail(
			new SystemWakeLockUnavailable({
				message: "System wake locks are not supported on this host",
			}),
		),
});

/** @since 0.7.0 */
export const make_platform_wake_lock_backend_layer = (platform: NodeJS.Platform) => {
	switch (platform) {
		case "win32":
			return WindowsSystemWakeLockBackendLive;
		case "darwin":
			return DarwinSystemWakeLockBackendLive;
		case "linux":
			return LinuxSystemWakeLockBackendLive;
		default:
			return SystemWakeLockBackendUnsupported;
	}
};
