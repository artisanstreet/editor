import { Buffer } from "node:buffer";
import { createHash, timingSafeEqual } from "node:crypto";
import { gunzipSync } from "node:zlib";

const tar_block_bytes = 512;
const tar_name_bytes = 100;
const tar_prefix_offset = 345;
const tar_prefix_bytes = 155;
const tar_size_offset = 124;
const tar_size_bytes = 12;
const tar_checksum_offset = 148;
const tar_checksum_bytes = 8;

const text = (bytes: Uint8Array) => {
	const end = bytes.indexOf(0);
	return new TextDecoder("utf-8", { fatal: true }).decode(
		end === -1 ? bytes : bytes.subarray(0, end),
	);
};

const octal = (bytes: Uint8Array, field: string) => {
	const value = text(bytes).trim();
	if (value.length === 0) return 0;
	if (!/^[0-7]+$/.test(value)) throw new Error(`Invalid tar ${field}`);
	const parsed = Number.parseInt(value, 8);
	if (!Number.isSafeInteger(parsed) || parsed < 0) throw new Error(`Unsafe tar ${field}`);
	return parsed;
};

const safe_member_name = (name: string) => {
	if (name.length === 0 || name.includes("\\") || name.startsWith("/") || /^[A-Za-z]:/.test(name))
		throw new Error(`Unsafe tar member path: ${name}`);
	const parts = name.split("/");
	if (parts.some((part) => part.length === 0 || part === "." || part === ".."))
		throw new Error(`Unsafe tar member path: ${name}`);
};

const verify_header_checksum = (header: Uint8Array) => {
	const expected = octal(
		header.subarray(tar_checksum_offset, tar_checksum_offset + tar_checksum_bytes),
		"checksum",
	);
	let actual = 0;
	for (let index = 0; index < header.length; index += 1) {
		actual +=
			index >= tar_checksum_offset && index < tar_checksum_offset + tar_checksum_bytes
				? 32
				: (header[index] ?? 0);
	}
	if (actual !== expected) throw new Error("Invalid tar header checksum");
};

/** Verifies the exact SHA-512 payload from an NPM registry integrity record. */
export const VerifyNpmSha512Integrity = (archive: Uint8Array, expected_base64: string) => {
	const expected = Buffer.from(expected_base64, "base64");
	const actual = createHash("sha512").update(archive).digest();
	if (expected.length !== actual.length || !timingSafeEqual(expected, actual))
		throw new Error("NPM tarball SHA-512 integrity mismatch");
};

/**
 * Extracts exactly one regular file from a verified NPM tgz in memory.
 *
 * No archive path ever reaches the filesystem. Links, PAX/GNU extensions,
 * duplicate members, absolute paths, traversal, and oversized output are
 * rejected before the expected executable bytes are returned to the caller.
 */
export const ExtractNpmTarballExecutable = (
	archive: Uint8Array,
	expected_member: string,
	maximum_output_bytes: number,
) => {
	safe_member_name(expected_member);
	const tar = gunzipSync(archive, { maxOutputLength: maximum_output_bytes + 4 * 1024 * 1024 });
	const seen = new Set<string>();
	let executable: Uint8Array | undefined;
	let offset = 0;
	while (offset + tar_block_bytes <= tar.byteLength) {
		const header = tar.subarray(offset, offset + tar_block_bytes);
		if (header.every((byte) => byte === 0)) break;
		verify_header_checksum(header);
		const base = text(header.subarray(0, tar_name_bytes));
		const prefix = text(
			header.subarray(tar_prefix_offset, tar_prefix_offset + tar_prefix_bytes),
		);
		const name = prefix.length === 0 ? base : `${prefix}/${base}`;
		safe_member_name(name);
		if (seen.has(name)) throw new Error(`Duplicate tar member: ${name}`);
		seen.add(name);
		const size = octal(
			header.subarray(tar_size_offset, tar_size_offset + tar_size_bytes),
			"member size",
		);
		const type = header[156] ?? 0;
		const data_start = offset + tar_block_bytes;
		const data_end = data_start + size;
		if (data_end > tar.byteLength) throw new Error(`Truncated tar member: ${name}`);
		if (type !== 0 && type !== 48 && type !== 53)
			throw new Error(`Unsupported tar member type ${String(type)}: ${name}`);
		if (name === expected_member) {
			if (type !== 0 && type !== 48)
				throw new Error("Expected executable is not a regular file");
			if (size <= 0 || size > maximum_output_bytes)
				throw new Error("Expected executable exceeds its output bound");
			executable = Uint8Array.from(tar.subarray(data_start, data_end));
		}
		offset = data_start + Math.ceil(size / tar_block_bytes) * tar_block_bytes;
	}
	if (executable === undefined)
		throw new Error(`Expected tar member is absent: ${expected_member}`);
	return executable;
};
