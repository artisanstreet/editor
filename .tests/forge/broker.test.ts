import { describe, expect, it } from "vitest";
import { Effect } from "effect";

import { EvaluateArtisanBroker } from "../../modules/forge/src/broker";

describe("Artisan Broker startup gate", () => {
	it("allows source-development compositions without a configured Broker", async () => {
		await expect(Effect.runPromise(EvaluateArtisanBroker({}))).resolves.toBeUndefined();
	});

	it("requires the Broker when the installed launcher marks it required", async () => {
		await expect(
			Effect.runPromise(EvaluateArtisanBroker({ ARTISAN_BROKER_REQUIRED: "1" })),
		).rejects.toMatchObject({ reason: "unavailable" });
	});

	it("blocks only an exact true decision", async () => {
		const broker = "C:/Artisan/Artisan Broker.exe";
		await expect(
			Effect.runPromise(
				EvaluateArtisanBroker({ ARTISAN_BROKER_PATH: broker }, async () => ({
					exit_code: 0,
					stderr: "",
					stdout: "true\n",
				})),
			),
		).rejects.toMatchObject({ reason: "blocked" });
		await expect(
			Effect.runPromise(
				EvaluateArtisanBroker({ ARTISAN_BROKER_PATH: broker }, async () => ({
					exit_code: 0,
					stderr: "",
					stdout: "false\n",
				})),
			),
		).resolves.toBeUndefined();
	});

	it("fails closed on invalid output or process failure", async () => {
		const broker = "C:/Artisan/Artisan Broker.exe";
		await expect(
			Effect.runPromise(
				EvaluateArtisanBroker({ ARTISAN_BROKER_PATH: broker }, async () => ({
					exit_code: 0,
					stderr: "",
					stdout: "yes",
				})),
			),
		).rejects.toMatchObject({ reason: "invalid_output" });
		await expect(
			Effect.runPromise(
				EvaluateArtisanBroker({ ARTISAN_BROKER_PATH: broker }, async () => ({
					exit_code: 2,
					stderr: "bad policy",
					stdout: "",
				})),
			),
		).rejects.toMatchObject({ reason: "unavailable" });
	});

	it("does not disclose Forge credentials to the Broker process", async () => {
		await Effect.runPromise(
			EvaluateArtisanBroker(
				{
					ARTISAN_AUTH_TOKEN: "forge-secret",
					ARTISAN_BROKER_NETWORK_COUNTRY: "NO",
					ARTISAN_BROKER_PATH: "C:/Artisan/Artisan Broker.exe",
					ARTISAN_DATABASE_PATH: "C:/private/artisan.sqlite",
				},
				async (_executable, environment) => {
					expect(environment.ARTISAN_AUTH_TOKEN).toBeUndefined();
					expect(environment.ARTISAN_DATABASE_PATH).toBeUndefined();
					expect(environment.ARTISAN_BROKER_NETWORK_COUNTRY).toBe("NO");
					return { exit_code: 0, stderr: "", stdout: "false" };
				},
			),
		);
	});
});
