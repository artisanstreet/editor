import { contextBridge, ipcRenderer } from "electron";
import { Effect, Schema } from "effect";

import { ProjectRef } from "@artisan/protocol";

const identity_channel = "artisan:desktop-identity";
const activity_channel = "artisan:desktop-activity";
const project_picker_channel = "artisan:select-project-directory";

/** The contextBridge is the single browser-Promise adaptation boundary. */
const RunBridge = <A, E>(program: Effect.Effect<A, E>): Promise<A> => Effect.runPromise(program);

contextBridge.exposeInMainWorld("artisanDesktop", {
	forgeWebSocketEndpoint: process.argv
		.find((argument) => argument.startsWith("--artisan-forge-ws="))
		?.slice("--artisan-forge-ws=".length),
	identity: () =>
		RunBridge(
			Effect.tryPromise({
				try: () => ipcRenderer.invoke(identity_channel),
				catch: (cause) => cause,
			}),
		),
	selectProjectDirectory: () =>
		RunBridge(
			Effect.tryPromise({
				try: () => ipcRenderer.invoke(project_picker_channel),
				catch: (cause) => cause,
			}).pipe(
				Effect.flatMap((project) =>
					project === undefined
						? Effect.succeed(undefined)
						: Schema.decodeUnknownEffect(ProjectRef, {
								onExcessProperty: "error",
							})(project),
				),
			),
		),
	setWorking: (working: boolean) =>
		RunBridge(
			Effect.tryPromise({
				try: () => ipcRenderer.invoke(activity_channel, working),
				catch: (cause) => cause,
			}).pipe(Effect.asVoid),
		),
});
