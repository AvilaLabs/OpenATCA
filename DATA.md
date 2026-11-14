# Data provenance

Raw downloads are kept outside the repository in `../openatca-data/raw/` and are never
committed. Each entry records where it came from and its SHA-256.

| Cycle | Source | File | SHA-256 | URL |
|---|---|---|---|---|
| 2026-10-29 | NASR 28-day subscription, CSV edition | `NASR_CSV_29_Oct_2026.zip` | `60b072454319f166797c5f4667520e4cf9c54d2e4e5faffbcdbae846aea276da` | https://nfdc.faa.gov/webContent/28DaySub/extra/29_Oct_2026_CSV.zip |
| 2026-10-29 | CIFP (ARINC 424-18) | `CIFP_261029.zip` | `e3ff2b31a4f6ad3eda1342b83ada1ae399aae7bd8c1e67f3cacd5b288b5e141d` | https://aeronav.faa.gov/Upload_313-d/cifp/CIFP_261029.zip |
| n/a | GeographicLib geodesic test set (MIT, C. F. F. Karney) | `GeodTest-short.dat.gz` | `31d376e8158f7af26277d887d7a1e7726e14d172db5a848a1730dd4886ab6121` | https://downloads.sourceforge.net/project/geographiclib/testdata/GeodTest-short.dat.gz |
| n/a | NIST CAVP SHA byte-oriented test vectors | `shabytetestvectors.zip` | `929ef80b7b3418aca026643f6f248815913b60e01741a44bba9e118067f4c9b8` | https://csrc.nist.gov/CSRC/media/Projects/Cryptographic-Algorithm-Validation-Program/documents/shs/shabytetestvectors.zip |

The NASR server returns 503 to clients without a browser-like User-Agent.

## Terms

FAA aeronautical data is public. The CIFP customer agreement obliges recipients to notify their
own users of reported CIFP errors and to report CIFP errors they discover to FAA Aeronautical
Information Services. See `docs/M0_DESIGN.md`, "Obligations from the data".
