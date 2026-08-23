# Telemetry Operations Runbook

## Cloud setup checklist

Use the same approved data region for both vendors unless Legal records an exception.

### PostHog

Create `Artisan Product` and `Artisan Sandbox`. Product data from marketing, Editor, and Forge shares the production project and is segmented by `surface`. Disable autocapture, replay, error tracking, and broad URL capture. Configure the approved raw-event retention period and least-privilege team access.

Create dashboards for:

- weekly active installations and release adoption;
- activation: Editor ready → Forge connected → project opened → thread created → run started → successful run;
- time to first successful run and D1/W1/W4 installation retention;
- run completion/failure/cancellation and p50/p95 duration;
- built-in engine/model adoption and engine setup outcomes;
- model transition, approvals, interruption/recovery, and fixed feature adoption;
- Forge startup, reconnect, cold-start, clean-shutdown, and previous-unclean-exit health.

Do not call installation metrics “users.” Do not create product-metric alerts until at least two weeks of stable baseline data exists.

### Sentry

Create `artisan-editor` and `artisan-forge` projects plus a cross-project reliability dashboard. Disable default PII, replay, structured logs, request bodies, sensitive breadcrumbs, and initial tracing. Configure release retention and issue ownership.

Initial alerts:

- new production regression after two affected installations or three events in ten minutes;
- Editor fatal/native crash spike: five events or three installations in fifteen minutes;
- newly introduced renderer process-gone issue;
- crash-free Editor sessions below 99.5% after the agreed minimum sample;
- any new Forge database migration/invariant issue;
- Forge unclean-exit or severe-memory warning on two installations in thirty minutes;
- observation persistence abandonment.

Expected provider/authentication/configuration errors and user cancellations must not page.

## Release verification

Before promoting a release:

1. confirm release version, commit, environment, distribution, platform, and architecture tags;
2. run controlled Electron-main, renderer-JavaScript, renderer-process-gone, Forge-handled, Forge-unhandled, and previous-unclean-exit fixtures;
3. verify at least 95% of controlled frames resolve to expected TypeScript/Svelte source;
4. verify the Forge debug ID in `forge-main.cjs.map` is present in the shipped SEA executable;
5. inspect every field and breadcrumb for forbidden content and the telemetry canary;
6. verify no `.map`, auth token, or vendor secret exists in installed/distribution artifacts;
7. verify unset/disabled builds produce zero vendor requests;
8. reconcile synthetic run starts/finishes with captured events exactly.

Replay, autocapture, logs, and tracing remain disabled unless a later privacy review explicitly approves a narrow change.

## Privacy incident response

If any event contains forbidden content:

1. restrict access to the affected vendor project immediately;
2. disable the event or SDK path using server-side controls and a release hotfix;
3. record the event IDs, release, environment, first/last occurrence, and affected installations without copying the sensitive payload;
4. delete affected events/issues/attachments according to vendor procedure;
5. notify the privacy incident contact and reliability owner;
6. determine whether contractual or legal notification is required;
7. add the leaked shape as a permanent sanitizer/schema regression fixture;
8. complete a post-incident review before restoring capture.

Never paste leaked payloads into tickets, chat, or logs.

## Installation deletion and identity reset

An installation can reset its anonymous identity locally with `ae telemetry reset-identity` after explicit confirmation. Resetting does not send an event and does not itself delete historical vendor data.

For a deletion request:

1. obtain the installation UUID from the local `telemetry.json` through the approved support workflow; do not request the whole file publicly;
2. derive the PostHog distinct ID using the production code's documented `install_<uuid>` form;
3. request deletion in PostHog and verify completion;
4. if Sentry uses a separately salted installation digest, derive it only inside the approved support tool and request deletion there;
5. record completion without retaining the identifier in the support ticket;
6. instruct the installation owner to reset identity if they want future events separated.

Forge pairing tokens, account credentials, machine names, and repository data are never valid lookup keys.

## Retention, access, and cost review

Monthly:

- review event/property cardinality and volume;
- remove unused events and dashboard properties;
- inspect a random sanitized sample from each runtime;
- confirm sandbox/development traffic is absent from production;
- review Sentry grouping noise and expected-error filtering;
- forecast vendor cost.

Quarterly:

- review vendor access, audit logs, retention, DPAs, and data region;
- test deletion/export and local identity reset;
- run the forbidden-canary suite;
- re-approve event owners and business purpose;
- verify source-map access remains restricted.

## Ownership

Before production credentials are enabled, assign names for:

- product analytics owner and backup;
- Editor reliability owner and backup;
- Forge reliability owner and backup;
- privacy incident contact;
- release/source-map owner;
- vendor budget and access owner.

No production rollout is complete while these entries are unassigned in the team's private operations registry.
