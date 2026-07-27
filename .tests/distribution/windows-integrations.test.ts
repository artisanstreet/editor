import { describe, expect, it } from "vitest";
import { Effect, Layer, Option, Ref } from "effect";

import { IntegrationPlatform } from "../../modules/distribution/src/integrations";
import {
	WindowsIntegrationAdapter,
	WindowsIntegrationPlatformLive,
} from "../../modules/distribution/src/windows-integrations";

describe("WindowsIntegrationPlatform", () => {
	it("routes each integration to its current-user Windows capability", async () => {
		const calls = await Effect.runPromise(Ref.make<ReadonlyArray<string>>([]));
		const Record = (call: string) => Ref.update(calls, (values) => [...values, call]);
		const adapter = WindowsIntegrationAdapter.of({
			ReadAutostart: (path) =>
				Record(`read-task:${path}`).pipe(Effect.as(Option.some("task"))),
			ReadPathEntry: (path) =>
				Record(`read-path:${path}`).pipe(Effect.as(Option.some("path"))),
			ReadProtocol: (path) =>
				Record(`read-protocol:${path}`).pipe(Effect.as(Option.some("protocol"))),
			ReadShortcut: (path) =>
				Record(`read-shortcut:${path}`).pipe(Effect.as(Option.some("shortcut"))),
			RemoveAutostart: (path) => Record(`remove-task:${path}`),
			RemovePathEntry: (path) => Record(`remove-path:${path}`),
			RemoveProtocol: (path) => Record(`remove-protocol:${path}`),
			RemoveShortcut: (path) => Record(`remove-shortcut:${path}`),
			WriteAutostart: (path, content) => Record(`write-task:${path}:${content}`),
			WritePathEntry: (path, content) => Record(`write-path:${path}:${content}`),
			WriteProtocol: (path, content) => Record(`write-protocol:${path}:${content}`),
			WriteShortcut: (path, content) => Record(`write-shortcut:${path}:${content}`),
		});
		const platform = await Effect.runPromise(
			IntegrationPlatform.pipe(
				Effect.provide(WindowsIntegrationPlatformLive),
				Effect.provide(Layer.succeed(WindowsIntegrationAdapter, adapter)),
			),
		);

		expect(Option.getOrThrow(await Effect.runPromise(platform.Read("ae_path", "C:\\ae")))).toBe(
			"path",
		);
		expect(
			Option.getOrThrow(await Effect.runPromise(platform.Read("protocol", "C:\\editor"))),
		).toBe("protocol");
		expect(
			Option.getOrThrow(
				await Effect.runPromise(platform.Read("application_shortcut", "C:\\start.lnk")),
			),
		).toBe("shortcut");
		expect(
			Option.getOrThrow(await Effect.runPromise(platform.Read("autostart", "C:\\ae"))),
		).toBe("task");
		await Effect.runPromise(platform.Write("desktop_shortcut", "C:\\desktop.lnk", "link"));
		await Effect.runPromise(platform.Remove("protocol", "C:\\editor"));

		expect(await Effect.runPromise(Ref.get(calls))).toEqual([
			"read-path:C:\\ae",
			"read-protocol:C:\\editor",
			"read-shortcut:C:\\start.lnk",
			"read-task:C:\\ae",
			"write-shortcut:C:\\desktop.lnk:link",
			"remove-protocol:C:\\editor",
		]);
	});
});
