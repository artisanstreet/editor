import { fstat } from "node:fs";

import { Effect } from "effect";

/** Identifies one opened file across path replacement races. */
export interface FileIdentity {
	readonly device: bigint;
	readonly inode: bigint;
}

const uint64_modulus = 1n << 64n;

function normalize_uint64(value: bigint) {
	return value < 0n ? value + uint64_modulus : value;
}

/** Reads the exact bigint identity for an Effect-managed file descriptor.
 *
 * @example
 * ```ts
 * const identity = yield* ReadFileIdentity(yield* FileDescriptorOf(file, "read"));
 * ```
 *
 * @since 0.1.0
 * @param descriptor - The native file descriptor whose identity should be read.
 * @returns An Effect that resolves to the descriptor's normalized device and inode identity.
 */
export function ReadFileIdentity(descriptor: number) {
	return Effect.callback<FileIdentity, NodeJS.ErrnoException>((resume) => {
		fstat(descriptor, { bigint: true }, (cause, info) => {
			if (cause !== null) {
				resume(Effect.fail(cause));

				return;
			}

			resume(
				Effect.succeed({
					device: normalize_uint64(info.dev),
					inode: normalize_uint64(info.ino),
				}),
			);
		});
	});
}

/** Compares two file identities by device and inode.
 *
 * @example
 * ```ts
 * if (!same_file_identity(current_identity, expected_identity)) {
 * 	return false;
 * }
 * ```
 *
 * @since 0.1.0
 * @param left - The first file identity to compare.
 * @param right - The second file identity to compare.
 * @returns Whether both identities refer to the same device and inode.
 */
export function same_file_identity(left: FileIdentity, right: FileIdentity) {
	return left.device === right.device && left.inode === right.inode;
}
