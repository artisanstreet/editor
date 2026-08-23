import { createWriteStream } from "node:fs";
import { mkdir, stat } from "node:fs/promises";
import { dirname, resolve, sep } from "node:path";
import { Transform } from "node:stream";
import { pipeline } from "node:stream/promises";

import { open, type Entry, type ZipFile } from "yauzl";

const unix_file_type_mask = 0o170000;
const unix_regular_file = 0o100000;
const unix_directory = 0o040000;
const unix_symbolic_link = 0o120000;

const safe_archive_path = (value: string) => {
	if (
		value.length === 0 ||
		value.includes("\\") ||
		value.startsWith("/") ||
		/^[A-Za-z]:/.test(value)
	)
		throw new Error(`Unsafe ZIP member path: ${value}`);
	const parts = value.replace(/\/$/, "").split("/");
	if (parts.some((part) => part.length === 0 || part === "." || part === ".."))
		throw new Error(`Unsafe ZIP member path: ${value}`);
	return parts;
};

const open_zip = (archive_path: string) =>
	new Promise<ZipFile>((resolve_zip, reject) => {
		open(
			archive_path,
			{ autoClose: false, lazyEntries: true, validateEntrySizes: true },
			(cause, zip_file) => {
				if (cause !== null) reject(cause);
				else if (zip_file === undefined) reject(new Error("ZIP archive did not open"));
				else resolve_zip(zip_file);
			},
		);
	});

const open_entry = (zip_file: ZipFile, entry: Entry) =>
	new Promise<NodeJS.ReadableStream>((resolve_stream, reject) => {
		zip_file.openReadStream(entry, (cause, stream) => {
			if (cause !== null) reject(cause);
			else if (stream === undefined)
				reject(new Error(`ZIP member did not open: ${entry.fileName}`));
			else resolve_stream(stream);
		});
	});

const unix_file_type = (entry: Entry) =>
	(entry.externalFileAttributes >>> 16) & unix_file_type_mask;

export interface ExtractZipBundleOptions {
	readonly archive_path: string;
	/** Every member must live below this one vendor-owned directory. */
	readonly archive_root: string;
	readonly destination: string;
	readonly maximum_entries?: number;
	readonly maximum_output_bytes: number;
}

/**
 * Extracts one bounded vendor bundle without allowing archive paths or links to
 * influence the destination tree. The archive root is stripped so its launcher
 * can be stored as the generation's ordinary executable basename.
 */
export const ExtractZipBundle = async (options: ExtractZipBundleOptions): Promise<void> => {
	const root_parts = safe_archive_path(options.archive_root);
	const root = `${root_parts.join("/")}/`;
	const destination = resolve(options.destination);
	const maximum_entries = options.maximum_entries ?? 4_096;
	await mkdir(destination, { recursive: true });

	const zip_file = await open_zip(options.archive_path);
	let settled = false;
	let entries = 0;
	let output_bytes = 0;
	const seen = new Set<string>();

	try {
		await new Promise<void>((resolve_done, reject) => {
			const finish = (cause?: unknown) => {
				if (settled) return;
				settled = true;
				if (cause === undefined) resolve_done();
				else reject(cause);
			};

			zip_file.on("error", finish);
			zip_file.on("end", () => finish());
			zip_file.on("entry", (entry: Entry) => {
				void (async () => {
					entries += 1;
					if (entries > maximum_entries)
						throw new Error("ZIP archive contains too many members");
					if ((entry.generalPurposeBitFlag & 1) !== 0)
						throw new Error(`Encrypted ZIP member is unsupported: ${entry.fileName}`);
					const member_parts = safe_archive_path(entry.fileName);
					const member = member_parts.join("/");
					if (member !== options.archive_root && !member.startsWith(root))
						throw new Error(
							`ZIP member is outside ${options.archive_root}: ${entry.fileName}`,
						);
					if (member === options.archive_root) {
						zip_file.readEntry();
						return;
					}

					const relative_parts = member.slice(root.length).split("/");
					const relative = relative_parts.join("/");
					if (seen.has(relative)) throw new Error(`Duplicate ZIP member: ${relative}`);
					seen.add(relative);
					const target = resolve(destination, ...relative_parts);
					if (target !== destination && !target.startsWith(`${destination}${sep}`))
						throw new Error(`ZIP member escapes its destination: ${entry.fileName}`);

					const directory = entry.fileName.endsWith("/");
					const file_type = unix_file_type(entry);
					if (file_type === unix_symbolic_link)
						throw new Error(`ZIP symbolic links are unsupported: ${entry.fileName}`);
					if (
						file_type !== 0 &&
						file_type !== unix_regular_file &&
						file_type !== unix_directory
					)
						throw new Error(`Unsupported ZIP member type: ${entry.fileName}`);
					if (directory) {
						if (file_type === unix_regular_file)
							throw new Error(`ZIP directory is marked as a file: ${entry.fileName}`);
						await mkdir(target, { recursive: true });
						zip_file.readEntry();
						return;
					}

					output_bytes += entry.uncompressedSize;
					if (output_bytes > options.maximum_output_bytes)
						throw new Error("ZIP archive exceeds its expanded-size bound");
					await mkdir(dirname(target), { recursive: true });
					let streamed = 0;
					const source = await open_entry(zip_file, entry);
					const counter = new Transform({
						transform(chunk: Buffer, _encoding, callback) {
							streamed += chunk.byteLength;
							if (streamed > entry.uncompressedSize)
								callback(
									new Error(
										`ZIP member exceeded its declared size: ${entry.fileName}`,
									),
								);
							else callback(null, chunk);
						},
					});
					await pipeline(source, counter, createWriteStream(target, { flags: "wx" }));
					if (streamed !== entry.uncompressedSize)
						throw new Error(`ZIP member size mismatch: ${entry.fileName}`);
					const metadata = await stat(target);
					if (!metadata.isFile())
						throw new Error(`ZIP member is not a regular file: ${entry.fileName}`);
					zip_file.readEntry();
				})().catch(finish);
			});
			zip_file.readEntry();
		});
	} finally {
		zip_file.close();
	}
};
