# Artisan Broker

Artisan Broker is the native, cross-platform startup heuristics engine for
Artisan Forge. The executable writes exactly `true` when Forge must block and
`false` when startup may continue. Invalid configuration exits nonzero, which
Forge treats as a failed-closed preflight.

The general DEKSA geographic sanctions index is deliberately not compiled into
the binary as a blanket deny list. Deployment policy supplies the reviewed ISO
3166-1 alpha-2 country set through `ARTISAN_BROKER_BLOCKED_COUNTRIES`.

## Inputs

| Variable | Meaning |
| --- | --- |
| `ARTISAN_BROKER_BLOCKED_COUNTRIES` | Comma-separated reviewed country policy |
| `ARTISAN_BROKER_ACCOUNT_COUNTRY` | Verified account country signal |
| `ARTISAN_BROKER_BILLING_COUNTRY` | Verified billing country signal |
| `ARTISAN_BROKER_NETWORK_COUNTRY` | Egress-network country signal |
| `ARTISAN_BROKER_DEVICE_COUNTRY` | Device-region signal |
| `ARTISAN_BROKER_SANCTIONS_MATCH` | Exact screened entity match (`1` or `true`) |
| `ARTISAN_BROKER_FAIL_CLOSED` | Block when no reliable signal exists |

The Broker also observes the system and environment locales as weak supporting
signals. Repeating a correlated source cannot increase its score. Locale
signals alone never cross the ordinary block threshold.
