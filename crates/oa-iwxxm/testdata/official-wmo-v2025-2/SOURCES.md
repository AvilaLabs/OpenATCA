# Official IWXXM example bytes

Source release: WMO IWXXM repository tag `v2025-2` (schema namespace `http://icao.int/iwxxm/2025-2`).
Files are checked in byte-for-byte from the repository's `IWXXM/examples/` directory.
No license statement is inferred here; consult the upstream repository's applicable terms.

| File | Upstream URL | Bytes | SHA-256 | Result in this profile |
|---|---|---|---|
| `metar-A3-1.xml` | `https://raw.githubusercontent.com/wmo-im/iwxxm/v2025-2/IWXXM/examples/metar-A3-1.xml` | 6,854 | `2fb2e17d26cebc238addaf1f9a3b6dec5159b8703669728cb19654a512a9ac27` | Decoded METAR observation and trend values |
| `speci-A3-2.xml` | `https://raw.githubusercontent.com/wmo-im/iwxxm/v2025-2/IWXXM/examples/speci-A3-2.xml` | 5,858 | `fb31b69a64c3f322367f7a86aaa1a70d81697cf7dbed1736a6aa3108a3df970e` | Decoded SPECI observation and trend values |
| `taf-A5-1.xml` | `https://raw.githubusercontent.com/wmo-im/iwxxm/v2025-2/IWXXM/examples/taf-A5-1.xml` | 9,048 | `71cb0b5d92b62b4f7eb5eb58f18770bf6238e4c6dea51d0d427432cb59a990c4` | Decoded TAF valid period, base, BECOMING/TEMPO/FROM groups |
| `taf-A5-2.xml` | `https://raw.githubusercontent.com/wmo-im/iwxxm/v2025-2/IWXXM/examples/taf-A5-2.xml` | 1,790 | `86a3c2b14362d9abd73b5c0c8f6fe5e1d53360581a3f2996f274c3ddf4b62544` | Stable refusal: `taf.cancelledReportValidPeriod` |
| `metar-NIL-collect.xml` | `https://raw.githubusercontent.com/wmo-im/iwxxm/v2025-2/IWXXM/examples/metar-NIL-collect.xml` | 2,608 | `8c8557f87b8f152ba45b88089dadb68d240f3d4d0e267986826f2337f78d7fac` | Stable refusal: `collect.MeteorologicalBulletin` (collection wrapper, not a single report) |
| `metar-translation-failed.xml` | `https://raw.githubusercontent.com/wmo-im/iwxxm/v2025-2/IWXXM/examples/metar-translation-failed.xml` | 1,983 | `7491c7aca5a26025dc300a49ddc7b8a1b69acdab4d364294ab80a76b74466ab2` | Stable refusal: `root.translationFailedTAC` |
| `taf-NIL-collect.xml` | `https://raw.githubusercontent.com/wmo-im/iwxxm/v2025-2/IWXXM/examples/taf-NIL-collect.xml` | 2,226 | `2808154ac9dce63cc045fa0a82094f69a6c78b5eb0e3527d8d8b94f22164957d` | Stable refusal: `collect.MeteorologicalBulletin` (collection wrapper, not a single report) |
| `taf-translation-failed.xml` | `https://raw.githubusercontent.com/wmo-im/iwxxm/v2025-2/IWXXM/examples/taf-translation-failed.xml` | 1,987 | `c8ea7e883068d8228bbcd4b457d0052c230f0faf59ae0bdbe52c3b74d04a04c1` | Stable refusal: `root.translationFailedTAC` |

The complete `IWXXM/examples/` listing for this release contains the eight files above.
The official examples contain no separate probability-group or wind-shear example; those
behaviors have separate schema-derived synthetic tests and are not counted as official-example
coverage.
