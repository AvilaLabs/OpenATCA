# oa-safety

The state of every safety net, and the inhibition objects that are the only way to quiet one.
Rules 1 and 2 of `docs/M5_DESIGN.md`. A core crate (`no_std`, no `unsafe`), class B1 and B7 of
`RULESET.md`. The nets themselves live elsewhere (`oa-ca` for conflict alert); this crate is what
every one of them reports to and what every display reads from.

## Net state (`net`)

There are four nets: conflict alert, MSAW, area proximity warning and approach path monitoring
(`NetId`, `NETS`). Each reports its state every cycle with `state_report`: running, degraded with a
reason, the area affected and a count (pairs or tracks not evaluated, say), or unavailable with a
reason.

The table, not the net, decides what is shown (`net_status`, `net_statuses`):

- A net that has never reported is **unavailable, never reported**.
- A net whose last report is older than the staleness limit is **unavailable, stale**. A net that
  stops reporting can therefore never appear to be running.
- A report dated further ahead than the future horizon is refused and counted.
- **Inhibited** is never reported by a net. It is derived from the inhibition table, so a net
  cannot claim it and an inhibition cannot be hidden.
- Every state other than running needs a banner (`status_needs_banner`) that cannot be dismissed
  while it lasts. The display never shows "no alerts" when the truth is "no alerting".

`state_table_new(stale_limit_ns, future_horizon_ns)` takes both limits from the caller and
refuses a limit that is not positive.

## Inhibitions (`inhibit`, `geom`)

An inhibition has a net, an area (a polygon of up to 32 vertices, `geom`), an altitude band, a
start, an end, a reason and the name of whoever set it. `inhibit_create` refuses one:

- whose net allows no inhibition at all;
- that lasts longer than the net's maximum;
- that starts further ahead than allowed;
- whose area is larger than allowed, or whose polygon is malformed.

All four limits come from `InhibitConfig`, and none has a default. `inhibit_config_none()` allows
nothing on any net.

While an inhibition is in force, `inhibit_suppress` records every alert it suppresses
(`suppressed_log`), so a replay can show what it hid. `inhibit_expire` ends inhibitions at their
end time, and every creation, removal and expiry is an event (`events_take`). An expired
inhibition stops suppressing at once, whether or not anyone has looked at it, and the ones still
waiting for review are counted (`inhibits_expired_waiting`).

Every table is a fixed array: 64 inhibitions, 4,096 suppressed alerts and 256 events, each a
named constant with its reasoning beside it. When a table is full, that is counted and refused,
never silently dropped. No function allocates or panics on any input. Times are signed
nanoseconds, positions 1e-7 degrees and altitudes feet.

## Tests

`tests/safety.rs` holds the five-rule tests of M5_DESIGN §6:
- a net taken down shows unavailable;
- an expired inhibition stops suppressing;
- a stale report is unavailable;
- every limit and refusal is checked.

The display side, banners and inhibition areas drawn on the scope, is in `oa-display`.
