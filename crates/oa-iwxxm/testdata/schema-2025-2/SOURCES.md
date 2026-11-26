# IWXXM METAR/SPECI encoder schema inputs

The target edition is WMO IWXXM v2025-2 (`wmo-im/iwxxm`, commit
`2c4db03d20f22bca74e55024ff6dc357adfff473`, dated 2025-11-25). The directly pinned
IWXXM schema bytes and hashes are:

| File | Upstream path | SHA-256 |
|---|---|---|
| `metarSpeci.xsd` | `IWXXM/metarSpeci.xsd` | `61f4d97b764f8791967b6e0da6739a69123cf0d9a7c57920cb3a27ba195c53ae` |
| `common.xsd` | `IWXXM/common.xsd` | `21e63487c5787938650fa274383c03bafaee0ef0176dfb7bdccf1f493b56c8da` |

Source URLs use the pinned full commit:

- <https://raw.githubusercontent.com/wmo-im/iwxxm/2c4db03d20f22bca74e55024ff6dc357adfff473/IWXXM/metarSpeci.xsd>
- <https://raw.githubusercontent.com/wmo-im/iwxxm/2c4db03d20f22bca74e55024ff6dc357adfff473/IWXXM/common.xsd>

The relevant referenced schema inputs were also captured for provenance. They are not a
complete transitive schema closure: AIXM and some GML declarations import ISO 19139 GMD schemas
that are not included. Encoder tests use the pinned IWXXM schemas with the repository's bounded
Rust XML parser to check emitted namespace URIs, local declarations, and selected direct-child
sequence/cardinality constraints. This is a structural profile check, not a general XSD
validator, full imported-type validation, Schematron validation, or conformance claim.

## Referenced schema files

The upstream URLs below are canonical schema source locations. Each checked-in file is named
with its relative path and SHA-256 so a local byte change is detectable.

| Relative file | Source URL | SHA-256 |
|---|---|---|
| `external/AIXM_AbstractGML_ObjectTypes.xsd` | <https://www.aixm.aero/schema/5.1.1/AIXM_AbstractGML_ObjectTypes.xsd> | `9c4436bd02deb8ce8055d0e4b2630c870697a22be6bab4fcdbdc911916c8406c` |
| `external/AIXM_DataTypes.xsd` | <https://www.aixm.aero/schema/5.1.1/AIXM_DataTypes.xsd> | `9f0145b918ad44be12518aa14500ae28fca0c7d18e41aa4d88b6234a112fee74` |
| `external/AIXM_Features.xsd` | <https://www.aixm.aero/schema/5.1.1/AIXM_Features.xsd> | `ff66083b02b7e0d9add8bfd7760179f82f9fac53629c651a38d5e90ca6115c4f` |
| `external/gml/basicTypes.xsd` | <https://schemas.opengis.net/gml/3.2.1/basicTypes.xsd> | `2e37d858bd5be1307892add40826632e4e16d5c44b032ba668c510f09588da5c` |
| `external/gml/coordinateOperations.xsd` | <https://schemas.opengis.net/gml/3.2.1/coordinateOperations.xsd> | `d9a96671453711b98301d8f796ba1bb47990b7ab4ea83e8a1803c27110bbc434` |
| `external/gml/coordinateReferenceSystems.xsd` | <https://schemas.opengis.net/gml/3.2.1/coordinateReferenceSystems.xsd> | `0b38a6d9a82b698307215be446cfbec18682f1edcdbdf531d0e9fadb88e3017e` |
| `external/gml/coordinateSystems.xsd` | <https://schemas.opengis.net/gml/3.2.1/coordinateSystems.xsd> | `50658a9b0cd0e8887dab3a54d0e66ea9d74e7737ac15b87fb3a88f6d578c884b` |
| `external/gml/coverage.xsd` | <https://schemas.opengis.net/gml/3.2.1/coverage.xsd> | `8861ca287202f3da462b3d78c71065dc78b3d24ac37304297601bd6599e8362f` |
| `external/gml/datums.xsd` | <https://schemas.opengis.net/gml/3.2.1/datums.xsd> | `a5ac13d8015f8d9c0947e5ad614c482a0f2c632d168f87c631d953273bdeb30d` |
| `external/gml/defaultStyle.xsd` | <https://schemas.opengis.net/gml/3.2.1/defaultStyle.xsd> | `88ddc067143b88010887564883c8fad51bea16e46bd1892480dd93a30367cdc0` |
| `external/gml/deprecatedTypes.xsd` | <https://schemas.opengis.net/gml/3.2.1/deprecatedTypes.xsd> | `cd02f14a1f83be529cb5d7ba8ce6a4089ebf26dcb3e287e4b81488cb6bf6dd5b` |
| `external/gml/dictionary.xsd` | <https://schemas.opengis.net/gml/3.2.1/dictionary.xsd> | `fb5f633e6b441ed19bd17433e28efedfb7a335bdbde04b4e5383582a763beae5` |
| `external/gml/direction.xsd` | <https://schemas.opengis.net/gml/3.2.1/direction.xsd> | `fed7bb5e0fcc13b78add0bdb4b603a278452b70617a207ee084d2427daa17011` |
| `external/gml/dynamicFeature.xsd` | <https://schemas.opengis.net/gml/3.2.1/dynamicFeature.xsd> | `457e07361d175c8c344d20fbed203f051b07dc13f7c637576247872fcf294c43` |
| `external/gml/feature.xsd` | <https://schemas.opengis.net/gml/3.2.1/feature.xsd> | `aa8434aa6b38fe00ab9dd1a1e84c9288b4828484c783c04c8caf4ea5650ecb07` |
| `external/gml/geometryAggregates.xsd` | <https://schemas.opengis.net/gml/3.2.1/geometryAggregates.xsd> | `d1cb25b4e923d06c44d6930ced39848ea0e938e197f95a8794e73692d0ff425b` |
| `external/gml/geometryBasic0d1d.xsd` | <https://schemas.opengis.net/gml/3.2.1/geometryBasic0d1d.xsd> | `b1f9e4c1714546cd1df6f03f459926c3714f6b4346bba58de7950eddedcc7d6c` |
| `external/gml/geometryBasic2d.xsd` | <https://schemas.opengis.net/gml/3.2.1/geometryBasic2d.xsd> | `2fc0914ecd7ca02d41ec090d8202e754d67e40890d8846382e7e25446bf4b181` |
| `external/gml/geometryComplexes.xsd` | <https://schemas.opengis.net/gml/3.2.1/geometryComplexes.xsd> | `26e936aa53efe6e56bbcadd5528967c2b9ac2b08b869252703c757e7ffb9711c` |
| `external/gml/geometryPrimitives.xsd` | <https://schemas.opengis.net/gml/3.2.1/geometryPrimitives.xsd> | `ddde612ddaa13fe8313ac5e0d70090d52459c83777e394b3d3917a205469b2af` |
| `external/gml/gml.xsd` | <https://schemas.opengis.net/gml/3.2.1/gml.xsd> | `eb1e652fdc10fa70352c3fc554cd3bfacc5a863e964a4e64c76f25c762b80a3f` |
| `external/gml/gmlBase.xsd` | <https://schemas.opengis.net/gml/3.2.1/gmlBase.xsd> | `36825875b089ee8f69f42103d31ce52f6eb9489d2f40e11516495f7c152cc79d` |
| `external/gml/grids.xsd` | <https://schemas.opengis.net/gml/3.2.1/grids.xsd> | `c317678414da9a28e0be87bb1544b875875746bc219aa0b7f6c31f32efd0c5f0` |
| `external/gml/measures.xsd` | <https://schemas.opengis.net/gml/3.2.1/measures.xsd> | `3faae40533a200b6999278678ef43c90a508fd7f8ded474f50475b4c3f19f152` |
| `external/gml/observation.xsd` | <https://schemas.opengis.net/gml/3.2.1/observation.xsd> | `514b7489b2aeaf58303e24c0180cba92654f2201e87a6aa7c2fe8e1c73ceff99` |
| `external/gml/referenceSystems.xsd` | <https://schemas.opengis.net/gml/3.2.1/referenceSystems.xsd> | `5bbeb1210ead8b6fba6516c4a5b9e80c1b811b48ec9f3293bbaaa5346aeec0e8` |
| `external/gml/temporal.xsd` | <https://schemas.opengis.net/gml/3.2.1/temporal.xsd> | `e6336faf79732dd8cc79b4f18595a4ccf93ab9d7c55e9f6517b79f4f44572fdd` |
| `external/gml/temporalReferenceSystems.xsd` | <https://schemas.opengis.net/gml/3.2.1/temporalReferenceSystems.xsd> | `f7671997af4fbd098ff53db1f025df3d1a678f707560a9bca83ca54d4ef61e7c` |
| `external/gml/temporalTopology.xsd` | <https://schemas.opengis.net/gml/3.2.1/temporalTopology.xsd> | `ee8fd1e816cb41746c16443e47a0206f71ab77a621d49778dc565a9d6eff4352` |
| `external/gml/topology.xsd` | <https://schemas.opengis.net/gml/3.2.1/topology.xsd> | `aa4e2db656a1dfad2fe50c23cccf5ab0f3bba5894087dca75e7bd25faf31ba12` |
| `external/gml/units.xsd` | <https://schemas.opengis.net/gml/3.2.1/units.xsd> | `b3e2b8f364b9b844dccc0d8314f9c6c952db5155bccfe7316a1742a63dcc29fe` |
| `external/gml/valueObjects.xsd` | <https://schemas.opengis.net/gml/3.2.1/valueObjects.xsd> | `ddf0c234b66363f596492f59b92e307d2d7d864a122c28abc32bcdf87ce7485e` |
| `external/xlink.xsd` | <https://www.w3.org/1999/xlink.xsd> | `c3dbbaa28b884377ecd4a6c49d1f12566d8227cb0dce8ced10c0f66f8fa266e3` |
| `external/xml.xsd` | <https://www.w3.org/2001/xml.xsd> | `61960fb3131e38022caad5360e2f33a3382578ab3c80cd58bd74320ede61b20c` |
