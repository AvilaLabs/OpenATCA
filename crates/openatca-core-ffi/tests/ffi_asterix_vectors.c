#define _POSIX_C_SOURCE 200809L
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "openatca.h"

_Static_assert(OA_ASTERIX_MAX_RECORDS == 21844, "ASTERIX record bound matches Rust core");
_Static_assert(sizeof(OaAsterixPosition) == 12 && _Alignof(OaAsterixPosition) == 4, "position ABI");
_Static_assert(offsetof(OaAsterixPosition, item) == 0 && offsetof(OaAsterixPosition, kind) == 2 &&
    offsetof(OaAsterixPosition, scale) == 3 && offsetof(OaAsterixPosition, a_raw) == 4 &&
    offsetof(OaAsterixPosition, b_raw) == 8, "position offsets");
_Static_assert(sizeof(OaAsterixVertical) == 12 && _Alignof(OaAsterixVertical) == 4, "vertical ABI");
_Static_assert(offsetof(OaAsterixVertical, item) == 0 && offsetof(OaAsterixVertical, subitem) == 2 &&
    offsetof(OaAsterixVertical, kind) == 3 && offsetof(OaAsterixVertical, scale) == 4 &&
    offsetof(OaAsterixVertical, flags) == 5 && offsetof(OaAsterixVertical, auxiliary) == 6 &&
    offsetof(OaAsterixVertical, reserved) == 7 && offsetof(OaAsterixVertical, raw) == 8,
    "vertical offsets");
_Static_assert(sizeof(OaAsterixRecord) == 192 && _Alignof(OaAsterixRecord) == 8, "record ABI");
_Static_assert(offsetof(OaAsterixRecord, category) == 0 && offsetof(OaAsterixRecord, cat021_edition) == 1 &&
    offsetof(OaAsterixRecord, source_sac) == 2 && offsetof(OaAsterixRecord, source_sic) == 3 &&
    offsetof(OaAsterixRecord, reserved0) == 4 && offsetof(OaAsterixRecord, present_frn_mask) == 8 &&
    offsetof(OaAsterixRecord, raw_untyped_frn_mask) == 16 &&
    offsetof(OaAsterixRecord, typed_unprojected_frn_mask) == 24 &&
    offsetof(OaAsterixRecord, present_field_mask) == 32 && offsetof(OaAsterixRecord, target_address) == 40 &&
    offsetof(OaAsterixRecord, target_ident) == 44 && offsetof(OaAsterixRecord, track_number) == 52 &&
    offsetof(OaAsterixRecord, time_item) == 54 && offsetof(OaAsterixRecord, time_raw) == 56 &&
    offsetof(OaAsterixRecord, position_count) == 60 && offsetof(OaAsterixRecord, velocity_kind) == 61 &&
    offsetof(OaAsterixRecord, velocity_flags) == 62 && offsetof(OaAsterixRecord, mode3a_flags) == 63 &&
    offsetof(OaAsterixRecord, positions) == 64 && offsetof(OaAsterixRecord, velocity_a_raw) == 88 &&
    offsetof(OaAsterixRecord, velocity_b_raw) == 92 && offsetof(OaAsterixRecord, mode3a_code) == 96 &&
    offsetof(OaAsterixRecord, reserved3) == 98 &&
    offsetof(OaAsterixRecord, raw_untyped_subitem_mask) == 100 &&
    offsetof(OaAsterixRecord, reserved4) == 104 && offsetof(OaAsterixRecord, reserved5) == 108 &&
    offsetof(OaAsterixRecord, vertical_count) == 112 && offsetof(OaAsterixRecord, reserved6) == 113 &&
    offsetof(OaAsterixRecord, vertical) == 120, "record offsets");
_Static_assert(sizeof(OaAsterixCounts) == 32 && _Alignof(OaAsterixCounts) == 8, "counts ABI");
_Static_assert(offsetof(OaAsterixCounts, category) == 0 && offsetof(OaAsterixCounts, edition) == 1 &&
    offsetof(OaAsterixCounts, reserved) == 2 && offsetof(OaAsterixCounts, reserved_bytes) == 4 &&
    offsetof(OaAsterixCounts, consumed_bytes) == 8 &&
    offsetof(OaAsterixCounts, required_record_count) == 16 &&
    offsetof(OaAsterixCounts, written_record_count) == 24, "counts offsets");
_Static_assert(sizeof(OaAsterixError) == 32 && _Alignof(OaAsterixError) == 8, "error ABI");
_Static_assert(offsetof(OaAsterixError, status) == 0 && offsetof(OaAsterixError, kind) == 4 &&
    offsetof(OaAsterixError, category) == 8 && offsetof(OaAsterixError, subitem) == 9 &&
    offsetof(OaAsterixError, item) == 10 && offsetof(OaAsterixError, record) == 12 &&
    offsetof(OaAsterixError, reserved_bytes) == 16 && offsetof(OaAsterixError, offset) == 24,
    "error offsets");

static size_t unhex(const char *text, uint8_t *output) {
    size_t length = strlen(text);
    if ((length & 1u) != 0) return 0;
    for (size_t i = 0; i < length / 2; ++i) {
        unsigned value = 0;
        if (sscanf(text + 2 * i, "%2x", &value) != 1) return 0;
        output[i] = (uint8_t)value;
    }
    return length / 2;
}

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    FILE *file = fopen(argv[1], "r");
    if (!file) return 2;
    char *line = NULL;
    size_t cap = 0;
    unsigned seen = 0;
    if (getline(&line, &cap, file) < 0) return 2;
    while (getline(&line, &cap, file) >= 0) {
        char *fields[12], *save = NULL;
        unsigned n = 0;
        for (char *part = strtok_r(line, "\t\n", &save); part && n < 12;
             part = strtok_r(NULL, "\t\n", &save)) fields[n++] = part;
        if (n != 12) return 3;
        const char *name = fields[0];
        unsigned selector = (unsigned)strtoul(fields[1], NULL, 10);
        uint8_t input[65535];
        size_t length = unhex(fields[2], input);
        unsigned status_want = (unsigned)strtoul(fields[3], NULL, 10);
        unsigned kind_want = (unsigned)strtoul(fields[4], NULL, 10);
        unsigned category_want = (unsigned)strtoul(fields[5], NULL, 10);
        uint64_t records_want = strtoull(fields[6], NULL, 10);
        uint64_t consumed_want = strtoull(fields[7], NULL, 10);
        uint32_t record_want = (uint32_t)strtoul(fields[8], NULL, 10);
        uint16_t item_want = (uint16_t)strtoul(fields[9], NULL, 10);
        uint8_t subitem_want = (uint8_t)strtoul(fields[10], NULL, 10);
        uint64_t offset_want = strtoull(fields[11], NULL, 10);
        OaAsterixRecord rows[2];
        memset(rows, 0xa5, sizeof rows);
        OaAsterixCounts counts = {0};
        OaAsterixError detail = {0};
        OaErrorInfo generic = {0};
        uint32_t status = oa_asterix_decode_block(input, length, (uint8_t)selector,
            rows, 2, &counts, &detail, &generic);
        if (status != status_want || generic.code != status_want) goto mismatch;
        if (status == OA_STATUS_OK) {
            if (counts.category != category_want || counts.required_record_count != records_want ||
                counts.written_record_count != records_want || counts.consumed_bytes != consumed_want)
                goto mismatch;
            OaAsterixCounts query = {0};
            OaAsterixError query_detail = {0};
            OaErrorInfo query_error = {0};
            uint32_t query_status = oa_asterix_decode_block(input, length, (uint8_t)selector,
                NULL, 0, &query, &query_detail, &query_error);
            if (query_status != OA_STATUS_BUFFER_TOO_SMALL ||
                query.required_record_count != records_want || query.written_record_count != 0 ||
                query.consumed_bytes != consumed_want) goto mismatch;
        } else if (detail.kind != kind_want || detail.category != category_want ||
                   detail.record != record_want || detail.item != item_want ||
                   detail.subitem != subitem_want || detail.offset != offset_want) {
            goto mismatch;
        }
        if (status != OA_STATUS_OK) {
            const uint8_t *bytes = (const uint8_t *)rows;
            for (size_t i = 0; i < sizeof rows; ++i) if (bytes[i] != 0xa5) goto mismatch;
        }
        ++seen;
        continue;
mismatch:
        fprintf(stderr, "ASTERIX vector failed: %s\n", name);
        free(line);
        fclose(file);
        return 1;
    }
    free(line);
    fclose(file);
    if (seen != 9) return 4;
    printf("ASTERIX vectors passed: %u\n", seen);
    return 0;
}
