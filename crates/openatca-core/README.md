# OpenATCA Core

`openatca-core` is an experimental Rust library for offline replay of ADS-B surveillance clips
in OpenATCA's OAF format. It runs the clip through the OpenATCA tracker and exposes measurement
steps, track updates, and input and tracker health to the calling application.

For ASTERIX CAT021, CAT048 and CAT062 encoding and decoding, use the `oa-asterix` crate.
The OpenATCA Developer Kit includes both libraries and the replay example.

## Add the dependency

The package is version `0.1.0` and experimental. It is part of the OpenATCA workspace and can be
used by path from a project checked out beside a local OpenATCA checkout. Adjust the path to
match the location of your checkout.

```toml
[dependencies]
openatca-core = { path = "../OpenATCA/crates/openatca-core" }
```

For a Git dependency, pin a revision that contains the crate on the repository you use.

```toml
[dependencies]
openatca-core = { git = "https://github.com/AvilaLabs/OpenATCA.git", rev = "<revision>" }
```

Run the example from the workspace root.

```sh
cargo run -p openatca-core --example replay
```

Without arguments, the example uses a [synthetic recording](tests/fixtures/README.md) with two
aircraft and a ten-second gap in one aircraft's reports. To replay your own ADS-B clip, add
`-- path/to/clip.oaf` to the command.

## Replay a clip

The API is synchronous and offline. `open` initializes a replay from a clip path and settings;
`next_frame` advances through measurement-time steps until the clip is exhausted.

```rust
use openatca_core::{ReplaySettings, SurveillanceReplay};

fn replay(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut replay = SurveillanceReplay::open(path, ReplaySettings::default())?;

    while let Some(frame) = replay.next_frame() {
        println!("measurement time: {} ns", frame.time_ns);
        for track in frame.tracks() {
            println!("track update: {track:?}");
        }
        println!("tracker health: {:?}", frame.health);
    }

    println!("input health: {:?}", replay.input_health());
    Ok(())
}
```

Each call groups the input measurements with the same `time_ns`, expressed in Unix nanoseconds.
`tracks()` returns the updates the tracker emitted while processing that group. Each update has
its own timestamp, position in 10⁻⁷ degrees, optional barometric altitude in feet, uncertainty
radius in metres, and tracking flags. A group can produce zero updates while tracks are forming.
`health` contains cumulative tracker counters, including lost rows. `input_health()` reports
damaged frames, dropped records, quality rejections, and sealed-window drops after `open`.

`open` reads the clip into memory before playback. The defaults cap the input at 64 MiB at
open and 500,000 declared report rows. `ReplaySettings` can change those limits and the output
buffer capacity. An input over either limit returns a `ReplayError`. As in OpenATCA's scope
replay, Core uses report intervals measured from the clip to configure track confirmation and
coasting. Playback ends at the last measurement group. This version is intended for research,
simulation, and offline analysis.
