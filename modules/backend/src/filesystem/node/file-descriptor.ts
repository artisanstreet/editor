import { Data, Effect, type FileSystem } from "effect";

/**
 * Raised when an opened file cannot produce the descriptor a caller needs.
 *
 * Only reachable if the file system in use is not the Node one — a browser or
 * in-memory implementation has no descriptor to give. Failing is the point:
 * every caller here uses the descriptor to prove a file's identity before
 * writing to it, and silently proceeding without that proof would turn a
 * defended replacement race into an undefended one.
 */
export class FileDescriptorUnavailable extends Data.TaggedError("FileDescriptorUnavailable")<{
	readonly operation: string;
}> {
	override get message() {
		return `The opened file exposes no descriptor for ${this.operation}`;
	}
}

/**
 * The native descriptor behind an Effect-opened file.
 *
 * Effect declared `fd` on the portable `File` interface up to 4.0.0-beta.100
 * and removed it in beta.103, because a numeric descriptor is a Node concept
 * and `FileSystem` moved into core where it must also serve browser and
 * in-memory backends. The Node implementation still carries the field — it is
 * simply no longer in the published type — so this reads it through one
 * narrowing in one place rather than scattering casts across the call sites.
 *
 * It is deliberately not replaced by `file.stat`. That returns `dev` and `ino`
 * as JavaScript numbers, and file identity is 64-bit: a Windows NTFS file id
 * above 2^53 loses precision as a double, which is exactly how two different
 * files come to compare equal. `fstat` with `bigint: true` on this descriptor
 * is what keeps the comparison exact, so the descriptor is the requirement and
 * the portable stat is not a substitute for it.
 */
export const FileDescriptorOf = (
	file: FileSystem.File,
	operation: string,
): Effect.Effect<number, FileDescriptorUnavailable> => {
	const descriptor = (file as { readonly fd?: unknown }).fd;

	return typeof descriptor === "number"
		? Effect.succeed(descriptor)
		: Effect.fail(new FileDescriptorUnavailable({ operation }));
};
