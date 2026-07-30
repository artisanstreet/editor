import { NodeChildProcessSpawner, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Layer } from "effect";

import { make_preview_external_browser_layer } from "./runtime";

/**
 * Provides the Node production browser launcher. Effect owns process lifetime;
 * reading Node's platform discriminator is the minimal custom boundary needed to
 * select the operating system's configured URL handler.
 */
export const NodePreviewExternalBrowserLive = make_preview_external_browser_layer(
	process.platform,
).pipe(
	Layer.provide(
		NodeChildProcessSpawner.layer.pipe(
			Layer.provideMerge(NodeFileSystem.layer),
			Layer.provideMerge(NodePath.layer),
		),
	),
);
