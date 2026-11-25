/* Source-only C ABI contract and shared-vector runner. */
/* REQ: OA-FFI-001 */
#include "../../../include/openatca.h"

#include <stdint.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

_Static_assert(sizeof(OaErrorInfo) == 16, "OaErrorInfo must remain 16 bytes");
_Static_assert(_Alignof(OaErrorInfo) == 8, "OaErrorInfo must remain 8-byte aligned");

typedef uint32_t (*inverse_signature)(double, double, double, double,
                                      double *, double *, double *, OaErrorInfo *);
typedef uint32_t (*direct_signature)(double, double, double, double,
                                     double *, double *, double *, OaErrorInfo *);
typedef uint32_t (*version_signature)(void);
typedef uint32_t (*message_signature)(uint32_t, uint8_t *, uint64_t, uint64_t *);
static inverse_signature const inverse_shape = &oa_geo_wgs84_inverse;
static direct_signature const direct_shape = &oa_geo_wgs84_direct;
static version_signature const version_shape = &oa_core_abi_version;
static message_signature const message_shape = &oa_core_error_message;

static size_t split_fields(char *line, char **fields, size_t capacity) {
    size_t count = 0;
    char *start = line;
    for (char *cursor = line;; ++cursor) {
        if (*cursor == ',' || *cursor == '\0' || *cursor == '\n') {
            if (count >= capacity) return capacity + 1;
            fields[count++] = start;
            if (*cursor == '\0' || *cursor == '\n') {
                *cursor = '\0';
                return count;
            }
            *cursor = '\0';
            start = cursor + 1;
        }
    }
}

static int same_output(double got, const char *expected, double tolerance) {
    char *end = NULL;
    double want;
    double delta;
    if (!isfinite(got)) return 0;
    if (expected[0] == '\0') return 1;
    want = strtod(expected, &end);
    if (end == expected || *end != '\0' || !isfinite(want)) return 0;
    delta = got - want;
    if (delta < 0.0) delta = -delta;
    return delta <= tolerance;
}

static int test_error_message_buffers(void) {
    uint8_t buffer[64];
    uint64_t required = UINT64_MAX;
    const uint64_t expected = (uint64_t)sizeof("success");
    uint32_t status = oa_core_error_message(OA_STATUS_OK, NULL, 0, &required);
    if (status != OA_STATUS_BUFFER_TOO_SMALL || required != expected) return 1;

    memset(buffer, 0xA5, sizeof(buffer));
    required = UINT64_MAX;
    status = oa_core_error_message(OA_STATUS_OK, NULL, 1, &required);
    if (status != OA_STATUS_INVALID_ARGUMENT || required != UINT64_MAX) return 1;

    required = 0;
    status = oa_core_error_message(OA_STATUS_OK, buffer, expected - 1, &required);
    if (status != OA_STATUS_BUFFER_TOO_SMALL || required != expected) return 1;
    for (size_t i = 0; i < sizeof(buffer); ++i) {
        if (buffer[i] != 0xA5) return 1;
    }

    required = 0;
    status = oa_core_error_message(OA_STATUS_OK, buffer, expected, &required);
    if (status != OA_STATUS_OK || required != expected) return 1;
    if (memcmp(buffer, "success", sizeof("success")) != 0) return 1;
    for (size_t i = (size_t)expected; i < sizeof(buffer); ++i) {
        if (buffer[i] != 0xA5) return 1;
    }
    return 0;
}

static int run_vectors(const char *path) {
    char line[512];
    char *fields[12];
    FILE *input = fopen(path, "r");
    size_t line_number = 0;
    int failures = 0;
    if (input == NULL) return 2;
    while (fgets(line, sizeof(line), input) != NULL) {
        size_t count;
        double a, b, c, d, o1 = 1234.0, o2 = 1234.0, o3 = 1234.0;
        double tolerance;
        uint32_t expected_status;
        uint64_t expected_location;
        uint32_t status;
        OaErrorInfo error = {99, 99, 99};
        ++line_number;
        count = split_fields(line, fields, 12);
        if (line_number == 1) continue;
        if (count != 12) { ++failures; continue; }
        a = strtod(fields[1], NULL);
        b = strtod(fields[2], NULL);
        c = strtod(fields[3], NULL);
        d = strtod(fields[4], NULL);
        expected_status = (uint32_t)strtoul(fields[5], NULL, 10);
        expected_location = (uint64_t)strtoull(fields[6], NULL, 10);
        tolerance = fields[10][0] == '\0' ? 0.0 : strtod(fields[10], NULL);
        if (strcmp(fields[0], "inverse") == 0) {
            status = oa_geo_wgs84_inverse(a, b, c, d, &o1, &o2, &o3, &error);
        } else if (strcmp(fields[0], "direct") == 0) {
            status = oa_geo_wgs84_direct(a, b, c, d, &o1, &o2, &o3, &error);
        } else {
            ++failures;
            continue;
        }
        if (status != expected_status || error.code != expected_status) {
            ++failures;
            continue;
        }
        if (expected_status == OA_STATUS_OK) {
            if (error.location_kind != OA_LOCATION_NONE ||
                !same_output(o1, fields[7], tolerance) ||
                !same_output(o2, fields[8], tolerance) ||
                !same_output(o3, fields[9], tolerance)) ++failures;
        } else if (error.location_kind != OA_LOCATION_ARGUMENT ||
                   error.location != expected_location ||
                   o1 != 1234.0 || o2 != 1234.0 || o3 != 1234.0) {
            ++failures;
        }
    }
    if (fclose(input) != 0) return 3;
    if (oa_core_abi_version() != OA_CORE_ABI_VERSION_PROVISIONAL) ++failures;
    if (test_error_message_buffers() != 0) ++failures;
    (void)inverse_shape;
    (void)direct_shape;
    (void)version_shape;
    (void)message_shape;
    return failures == 0 ? 0 : 4;
}

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    return run_vectors(argv[1]);
}
