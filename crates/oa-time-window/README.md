# `oa-time-window`

This small, pure core crate represents half-open Unix-millisecond windows and converts
nanosecond spans with outward rounding. It is used by decoder APIs to carry caller-supplied
time-drop policy; it does not create or authenticate that policy. See the [time-window API proposal](https://github.com/AvilaLabs/OpenATCA/blob/main/docs/core-api/foundations/time-window.md) for units, bounds and behavior.

**Tier: Preview supporting capability.** The proposed external package name is not reserved or
published.
