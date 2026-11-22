# Synthetic ADS-B recording

`synthetic-adsb.oaf` was generated from the [OAF1](../../../../docs/FEED_FRAMES.md) and
[ADS3](../../../../docs/ADSB_PAYLOAD.md) format tables. It contains 30 one-second frames
beginning at Unix second 1750000000. Two synthetic aircraft, `A00001` and `A00002`, fly east
at 250 kt at 10,000 ft and 11,000 ft respectively. Each starts at longitude −88.0048° and
advances 0.00157° per second. Their latitudes are 41.9786° and 41.9986°. The first aircraft's
reports are absent from seconds 10 through 19, leaving 50 reports total. Receive time equals
measurement time. All positions, addresses, and timestamps were generated for this fixture.
