import { Effect, Schema } from "effect";

import { PreviewRepositoryError, PreviewTargetProjection } from "./contracts";
import { is_local_preview_hostname } from "./network-policy";

export const ValidateLocalPreviewUrl = (value: string) =>
	Effect.try({
		try: () => new URL(value),
		catch: () =>
			new PreviewRepositoryError({ code: "invalid", message: "Preview URL is invalid" }),
	}).pipe(
		Effect.flatMap((url) =>
			(url.protocol === "http:" || url.protocol === "https:") &&
			!url.username &&
			!url.password &&
			is_local_preview_hostname(url.hostname)
				? Effect.succeed(url.href)
				: Effect.fail(
						new PreviewRepositoryError({
							code: "invalid",
							message: "Preview URL must be local HTTP(S) without credentials",
						}),
					),
		),
	);

export const ValidatePreviewRegistrationPort = (url: string, port: number) =>
	Schema.decodeUnknownEffect(PreviewTargetProjection.fields.port)(port).pipe(
		Effect.mapError(
			() =>
				new PreviewRepositoryError({
					code: "invalid",
					message: "Preview port is invalid",
				}),
		),
		Effect.flatMap((declared_port) =>
			ValidateLocalPreviewUrl(url).pipe(
				Effect.flatMap((canonical_url) => {
					const parsed = new URL(canonical_url);
					const canonical_port =
						parsed.port === ""
							? parsed.protocol === "https:"
								? 443
								: 80
							: Number(parsed.port);
					return declared_port === canonical_port
						? Effect.succeed(canonical_url)
						: Effect.fail(
								new PreviewRepositoryError({
									code: "invalid",
									message: "Preview port must match the canonical URL port",
								}),
							);
				}),
			),
		),
	);
