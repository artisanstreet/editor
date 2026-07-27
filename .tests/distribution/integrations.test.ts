import { describe, expect, it } from "vitest";
import { Effect, Layer, Option, Ref } from "effect";

import type { InstalledIntegrations } from "../../modules/distribution/src/installation-manifest";
import {
	IntegrationFingerprint,
	IntegrationLifecycle,
	IntegrationLifecycleLive,
	IntegrationPlatform,
	type IntegrationSpecification,
} from "../../modules/distribution/src/integrations";

const path = "C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.cmd";
const protocol_path = "C:\\Users\\test\\AppData\\Local\\Artisan\\Artisan Editor.exe";

const specifications = [
	{ content: "PATH:C:\\Artisan\\bin", kind: "ae_path", path },
	{ content: '"Artisan Editor.exe" "%1"', kind: "protocol", path: protocol_path },
] satisfies ReadonlyArray<IntegrationSpecification>;

const MakeHarness = Effect.gen(function* () {
	const state = yield* Ref.make(new Map<string, string>());
	const removals = yield* Ref.make<ReadonlyArray<string>>([]);
	const Key = (kind: string, target_path: string) => `${kind}:${target_path}`;
	const platform = IntegrationPlatform.of({
		Read: (kind, target_path) =>
			Ref.get(state).pipe(
				Effect.map((values) => Option.fromUndefinedOr(values.get(Key(kind, target_path)))),
			),
		Remove: (kind, target_path) =>
			Effect.all(
				[
					Ref.update(state, (values) => {
						const next = new Map(values);
						next.delete(Key(kind, target_path));
						return next;
					}),
					Ref.update(removals, (values) => [...values, Key(kind, target_path)]),
				],
				{ discard: true },
			),
		Write: (kind, target_path, content) =>
			Ref.update(state, (values) => new Map(values).set(Key(kind, target_path), content)),
	});
	const lifecycle = yield* IntegrationLifecycle.pipe(
		Effect.provide(IntegrationLifecycleLive),
		Effect.provide(Layer.succeed(IntegrationPlatform, platform)),
	);
	return { lifecycle, removals, state };
});

describe("IntegrationLifecycle", () => {
	it("installs idempotently and records content ownership fingerprints", async () => {
		const { lifecycle } = await Effect.runPromise(MakeHarness);
		const first = await Effect.runPromise(lifecycle.Install(specifications));
		const second = await Effect.runPromise(lifecycle.Install(specifications));
		const path_specification = specifications[0];

		expect(second).toEqual(first);
		expect(path_specification).toBeDefined();
		expect(first.ae_path).toEqual({
			path,
			fingerprint: IntegrationFingerprint(path_specification?.content ?? ""),
		});
		expect(await Effect.runPromise(lifecycle.Inspect(first))).toEqual([
			{ _tag: "Owned", kind: "ae_path", path },
			{ _tag: "Owned", kind: "protocol", path: protocol_path },
		]);
	});

	it("repairs missing owned entries without overwriting drifted entries", async () => {
		const { lifecycle, state } = await Effect.runPromise(MakeHarness);
		const records = await Effect.runPromise(lifecycle.Install(specifications));
		await Effect.runPromise(
			Ref.update(state, (values) => {
				const next = new Map(values);
				next.delete(`ae_path:${path}`);
				next.set(`protocol:${protocol_path}`, "another application");
				return next;
			}),
		);

		expect(await Effect.runPromise(lifecycle.Repair(specifications, records))).toEqual(records);
		expect(await Effect.runPromise(lifecycle.Inspect(records))).toMatchObject([
			{ _tag: "Owned", kind: "ae_path" },
			{ _tag: "Drifted", kind: "protocol" },
		]);
	});

	it("refuses to overwrite a pre-existing integration it cannot prove it owns", async () => {
		const { lifecycle, state } = await Effect.runPromise(MakeHarness);
		await Effect.runPromise(
			Ref.update(state, (values) =>
				new Map(values).set(`protocol:${protocol_path}`, "another application"),
			),
		);

		await expect(Effect.runPromise(lifecycle.Install(specifications))).rejects.toMatchObject({
			_tag: "IntegrationError",
			code: "ownership_conflict",
			kind: "protocol",
		});
	});

	it("uninstalls only entries whose current content still proves ownership", async () => {
		const { lifecycle, removals, state } = await Effect.runPromise(MakeHarness);
		const records = await Effect.runPromise(lifecycle.Install(specifications));
		await Effect.runPromise(
			Ref.update(state, (values) =>
				new Map(values).set(`protocol:${protocol_path}`, "user replacement"),
			),
		);

		expect(await Effect.runPromise(lifecycle.Uninstall(records))).toEqual([
			{ _tag: "Removed", kind: "ae_path", path },
			{
				_tag: "Preserved",
				kind: "protocol",
				path: protocol_path,
				reason: "ownership_mismatch",
			},
		]);
		expect(await Effect.runPromise(Ref.get(removals))).toEqual([`ae_path:${path}`]);
	});

	it("treats absent records as intentionally disabled integrations", async () => {
		const { lifecycle, removals } = await Effect.runPromise(MakeHarness);
		const records = {} satisfies InstalledIntegrations;
		expect(await Effect.runPromise(lifecycle.Inspect(records))).toEqual([]);
		expect(await Effect.runPromise(lifecycle.Uninstall(records))).toEqual([]);
		expect(await Effect.runPromise(Ref.get(removals))).toEqual([]);
	});
});
