import { Context, Data, Effect, Layer, Option } from "effect";

import { IntegrationError, type IntegrationKind, IntegrationPlatform } from "./integrations";

export class WindowsIntegrationAdapterError extends Data.TaggedError(
	"WindowsIntegrationAdapterError",
)<{
	readonly cause?: unknown;
	readonly operation: "read" | "remove" | "write";
}> {}

/** Injected current-user Windows registry, shell-link, PATH, and task edge. */
export class WindowsIntegrationAdapter extends Context.Service<
	WindowsIntegrationAdapter,
	{
		readonly ReadAutostart: (
			target_path: string,
		) => Effect.Effect<Option.Option<string>, WindowsIntegrationAdapterError>;
		readonly ReadPathEntry: (
			target_path: string,
		) => Effect.Effect<Option.Option<string>, WindowsIntegrationAdapterError>;
		readonly ReadProtocol: (
			target_path: string,
		) => Effect.Effect<Option.Option<string>, WindowsIntegrationAdapterError>;
		readonly ReadShortcut: (
			shortcut_path: string,
		) => Effect.Effect<Option.Option<string>, WindowsIntegrationAdapterError>;
		readonly RemoveAutostart: (
			target_path: string,
		) => Effect.Effect<void, WindowsIntegrationAdapterError>;
		readonly RemovePathEntry: (
			target_path: string,
		) => Effect.Effect<void, WindowsIntegrationAdapterError>;
		readonly RemoveProtocol: (
			target_path: string,
		) => Effect.Effect<void, WindowsIntegrationAdapterError>;
		readonly RemoveShortcut: (
			shortcut_path: string,
		) => Effect.Effect<void, WindowsIntegrationAdapterError>;
		readonly WriteAutostart: (
			target_path: string,
			content: string,
		) => Effect.Effect<void, WindowsIntegrationAdapterError>;
		readonly WritePathEntry: (
			target_path: string,
			content: string,
		) => Effect.Effect<void, WindowsIntegrationAdapterError>;
		readonly WriteProtocol: (
			target_path: string,
			content: string,
		) => Effect.Effect<void, WindowsIntegrationAdapterError>;
		readonly WriteShortcut: (
			shortcut_path: string,
			content: string,
		) => Effect.Effect<void, WindowsIntegrationAdapterError>;
	}
>()("Artisan/Distribution/WindowsIntegrationAdapter") {}

const IsShortcut = (kind: IntegrationKind) =>
	kind === "application_shortcut" ||
	kind === "desktop_shortcut" ||
	kind === "forge_logs_shortcut" ||
	kind === "forge_start_shortcut" ||
	kind === "uninstall_shortcut";

export const WindowsIntegrationPlatformLive = Layer.effect(
	IntegrationPlatform,
	Effect.gen(function* () {
		const adapter = yield* WindowsIntegrationAdapter;
		const MapError =
			(kind: IntegrationKind, path: string, code: IntegrationError["code"]) =>
			(cause: WindowsIntegrationAdapterError) =>
				new IntegrationError({ cause, code, kind, path });

		return IntegrationPlatform.of({
			Read: (kind, path) => {
				const operation = IsShortcut(kind)
					? adapter.ReadShortcut(path)
					: kind === "ae_path"
						? adapter.ReadPathEntry(path)
						: kind === "protocol"
							? adapter.ReadProtocol(path)
							: adapter.ReadAutostart(path);
				return operation.pipe(Effect.mapError(MapError(kind, path, "inspect_failed")));
			},
			Remove: (kind, path) => {
				const operation = IsShortcut(kind)
					? adapter.RemoveShortcut(path)
					: kind === "ae_path"
						? adapter.RemovePathEntry(path)
						: kind === "protocol"
							? adapter.RemoveProtocol(path)
							: adapter.RemoveAutostart(path);
				return operation.pipe(Effect.mapError(MapError(kind, path, "remove_failed")));
			},
			Write: (kind, path, content) => {
				const operation = IsShortcut(kind)
					? adapter.WriteShortcut(path, content)
					: kind === "ae_path"
						? adapter.WritePathEntry(path, content)
						: kind === "protocol"
							? adapter.WriteProtocol(path, content)
							: adapter.WriteAutostart(path, content);
				return operation.pipe(Effect.mapError(MapError(kind, path, "install_failed")));
			},
		});
	}),
);
