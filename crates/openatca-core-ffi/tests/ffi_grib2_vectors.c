#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
/* REQ: OA-FFI-001, OA-FFI-002, OA-FFI-005 */
#include "openatca.h"

_Static_assert(sizeof(OaGrib2Time) == 8, "GRIB2 time ABI size");
_Static_assert(sizeof(OaGrib2Field) == 168, "GRIB2 field ABI size");
_Static_assert(_Alignof(OaGrib2Field) == 8, "GRIB2 field ABI alignment");
_Static_assert(offsetof(OaGrib2Field, reference_time) == 12, "GRIB2 time offset");
_Static_assert(offsetof(OaGrib2Field, value_offset) == 144, "GRIB2 value offset");

static size_t unhex(const char *s, uint8_t *out) {
    size_t n = strlen(s); if ((n & 1u) != 0) return 0;
    for (size_t i = 0; i < n / 2; ++i) {
        unsigned v = 0; if (sscanf(s + 2 * i, "%2x", &v) != 1) return 0;
        out[i] = (uint8_t)v;
    }
    return n / 2;
}

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    FILE *f = fopen(argv[1], "r"); if (!f) return 2;
    char line[4096];
    if (!fgets(line, sizeof line, f)) return 2;
    if (!fgets(line, sizeof line, f)) return 2;
    char *tab = strchr(line, '\t'); if (!tab) return 2;
    char *hex = tab + 1; hex[strcspn(hex, "\r\n")] = '\0';
    uint8_t bytes[512]; size_t n = unhex(hex, bytes); fclose(f);
    if (!n) return 2;
    uint64_t nf = UINT64_MAX, nv = UINT64_MAX; OaErrorInfo err = {0};
    uint32_t status = oa_grib2_inspect_message(bytes, n, &nf, &nv, &err);
    if (status || nf != 1 || nv != 12) return 1;
    OaGrib2Field field; memset(&field, 0xa5, sizeof field);
    float values[12]; for (size_t i = 0; i < 12; ++i) values[i] = 42.0f;
    status = oa_grib2_decode_message(bytes, n, &field, 1, values, 12, &nf, &nv, &err);
    if (status || field.nx != 4 || field.ny != 3 || field.missing_count != 5 ||
        field.first_latitude_deg != 42.0 || field.first_longitude_deg != -90.0 ||
        field.i_increment_deg != 0.25 || field.j_increment_deg != 0.5 ||
        field.reference_time.year != 2026 || field.forecast_time != 6 ||
        values[0] != 1.0f || !isnan(values[1]) || values[10] != 7.0f || !isnan(values[11])) return 1;
    OaGrib2Field sentinel; memset(&sentinel, 0xa5, sizeof sentinel);
    float untouched[12]; for (size_t i = 0; i < 12; ++i) untouched[i] = 77.0f;
    status = oa_grib2_decode_message(bytes, n, &sentinel, 0, untouched, 12, &nf, &nv, &err);
    if (status != OA_STATUS_BUFFER_TOO_SMALL || ((uint8_t *)&sentinel)[0] != 0xa5 || untouched[0] != 77.0f) return 1;
    status = oa_grib2_inspect_message(bytes, n - 1, &nf, &nv, &err);
    if (status == OA_STATUS_OK) return 1;
    uint8_t unsupported[512]; memcpy(unsupported, bytes, n);
    size_t p = 16; int found = 0;
    while (p + 11 < n - 4) {
        uint32_t length = ((uint32_t)unsupported[p] << 24) | ((uint32_t)unsupported[p + 1] << 16) |
                          ((uint32_t)unsupported[p + 2] << 8) | unsupported[p + 3];
        if (unsupported[p + 4] == 5) { unsupported[p + 9] = 0; unsupported[p + 10] = 41; found = 1; break; }
        p += length;
    }
    if (!found) return 1;
    status = oa_grib2_decode_message(unsupported, n, &field, 1, values, 12, &nf, &nv, &err);
    if (status != OA_STATUS_GRIB2_UNSUPPORTED) return 1;
    return 0;
}
