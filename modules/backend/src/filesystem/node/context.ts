import { Context, type Crypto, type Effect, type FileSystem, type Option, type Path } from "effect";

import type { FileIdentity } from "../file-identity";

export interface RegularFileSnapshot {
	readonly bytes: Uint8Array;
	readonly identity: FileIdentity;
	readonly mode: number;
}

/** Provides deterministic interruption points for conditional replacement tests. */
export interface NodeBoundedRegularFileStoreHooks {
	readonly after_backup?: (path: string) => Promise<void>;
	readonly after_backup_cleanup?: (path: string) => Promise<void>;
	readonly after_publication?: (path: string) => Promise<void>;
	readonly after_stage?: (path: string) => Promise<void>;
	readonly after_stage_cleanup?: (path: string) => Promise<void>;
	readonly before_backup?: (path: string) => Promise<void>;
	readonly before_publication?: (path: string) => Promise<void>;
}

export interface NodeReplacementContextService {
	readonly crypto: Crypto.Crypto;
	readonly file_system: FileSystem.FileSystem;
	readonly hooks: NodeBoundedRegularFileStoreHooks | undefined;
	readonly path_service: Path.Path;
	readonly ReadOptionalRegularSnapshot: (
		resolved: string,
		path: string,
		maximum_bytes: number,
	) => Effect.Effect<Option.Option<RegularFileSnapshot>, unknown>;
	readonly ReadRegularSnapshot: (
		resolved: string,
		path: string,
		maximum_bytes: number,
	) => Effect.Effect<RegularFileSnapshot, unknown>;
	readonly resolve_mutable_path: (
		path: string,
		operation: "write",
	) => Effect.Effect<string, unknown>;
	readonly root: string;
}

export class NodeReplacementContext extends Context.Service<
	NodeReplacementContext,
	NodeReplacementContextService
>()("@artisan/backend/filesystem/node/NodeReplacementContext") {}
