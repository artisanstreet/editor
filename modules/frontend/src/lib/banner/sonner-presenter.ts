import { Effect, Layer } from "effect";
import { toast } from "svelte-sonner";

import BannerView from "./view.sv";
import { BannerPresenter } from "./service";

export const SonnerBannerPresenterLive = Layer.succeed(
	BannerPresenter,
	BannerPresenter.of({
		Dismiss: (id) => Effect.sync(() => toast.dismiss(id)),
		Show: (event, on_action) =>
			Effect.sync(() => {
				toast.custom(BannerView, {
					componentProps: { event, onaction: on_action },
					...(event.duration_ms === undefined ? {} : { duration: event.duration_ms }),
					...(event.id === undefined ? {} : { id: event.id }),
					important: event.severity === "error",
				});
			}),
	}),
);
