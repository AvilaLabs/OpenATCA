/* L0 math, SHA-256 and geodesy use shared source vectors. */
/* REQ: OA-FFI-003 */
#include "../../../include/openatca.h"

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static size_t split(char *text, char delimiter, char **fields, size_t capacity) {
    size_t count = 0;
    char *start = text;
    for (char *cursor = text;; ++cursor) {
        if (*cursor == delimiter || *cursor == '\0' || *cursor == '\n') {
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

static size_t numbers(char *text, double *out, size_t capacity) {
    char *parts[8];
    size_t count = split(text, ';', parts, 8);
    if (count > capacity || count > 8) return capacity + 1;
    for (size_t i = 0; i < count; ++i) {
        char *end = NULL;
        out[i] = strtod(parts[i], &end);
        if (end == parts[i] || *end != '\0') return capacity + 1;
    }
    return count;
}

static int hex_value(char value) {
    if (value >= '0' && value <= '9') return value - '0';
    if (value >= 'a' && value <= 'f') return value - 'a' + 10;
    return -1;
}

static size_t decode_hex(const char *text, uint8_t *out, size_t capacity) {
    size_t length = strlen(text);
    if (length % 2 != 0 || length / 2 > capacity) return capacity + 1;
    for (size_t i = 0; i < length / 2; ++i) {
        int hi = hex_value(text[i * 2]);
        int lo = hex_value(text[i * 2 + 1]);
        if (hi < 0 || lo < 0) return capacity + 1;
        out[i] = (uint8_t)((hi << 4) | lo);
    }
    return length / 2;
}

static int check(double got, double want, double tolerance) {
    return isfinite(got) && fabs(got - want) <= tolerance;
}

static int compare_hex(const uint8_t *bytes, size_t count, const char *expected) {
    static const char digits[] = "0123456789abcdef";
    if (strlen(expected) != count * 2) return 0;
    for (size_t i = 0; i < count; ++i) {
        if (expected[i * 2] != digits[bytes[i] >> 4] ||
            expected[i * 2 + 1] != digits[bytes[i] & 15]) return 0;
    }
    return 1;
}

static int compare_ascii_hex(const uint8_t *text, size_t count, const char *expected) {
    if (strlen(expected) != count) return 0;
    for (size_t i = 0; i < count; ++i) {
        if (text[i] != (uint8_t)expected[i]) return 0;
    }
    return 1;
}

static void report_failure(int *failures, const char *family, size_t line_number,
                           const char *case_name, const char *reason) {
    fprintf(stderr, "%s line %zu case=%s: %s\n", family, line_number,
            case_name == NULL ? "<unknown>" : case_name, reason);
    ++*failures;
}

static int run_vectors(const char *path) {
    char line[2048];
    size_t line_number = 0;
    int failures = 0;
    FILE *input = fopen(path, "r");
    if (input == NULL) {
        fprintf(stderr, "cannot open L0 vector file: %s\n", path);
        return 2;
    }
    while (fgets(line, sizeof(line), input) != NULL) {
        char *fields[7];
        double args[8] = {0};
        double expected[4] = {0};
        double got[4] = {0};
        size_t argc, expected_count = 0;
        double tolerance;
        OaErrorInfo error = {99, 99, 99};
        uint32_t status = OA_STATUS_INVALID_ARGUMENT;
        ++line_number;
        size_t count = split(line, ',', fields, 7);
        if (line_number == 1) continue;
        if (count != 7) {
            report_failure(&failures, "L0 vector", line_number,
                           count > 0 ? fields[0] : NULL, "expected exactly seven CSV fields");
            continue;
        }
        argc = strcmp(fields[1], "sha256") == 0 || fields[2][0] == '\0'
                   ? 0
                   : numbers(fields[2], args, 8);
        if (argc > 8) {
            report_failure(&failures, "L0 vector", line_number, fields[0], "invalid or oversized argument list");
            continue;
        }
        if (fields[3][0] != '\0') expected_count = numbers(fields[3], expected, 4);
        if (expected_count > 4) {
            report_failure(&failures, "L0 vector", line_number, fields[0], "invalid or oversized expected output list");
            continue;
        }
        if (fields[4][0] != '\0') {
            char *second = NULL;
            double value;
            if (expected_count >= 4) {
                report_failure(&failures, "L0 vector", line_number, fields[0], "too many expected outputs");
                continue;
            }
            value = strtod(fields[4], &second);
            if (second == fields[4] || *second != '\0') {
                report_failure(&failures, "L0 vector", line_number, fields[0], "invalid second expected output");
                continue;
            }
            expected[expected_count] = value;
            ++expected_count;
        }
        tolerance = fields[5][0] == '\0' ? 0.0 : strtod(fields[5], NULL);

        if (strcmp(fields[1], "math_unary") == 0 && argc == 2) {
            status = oa_math_f64_unary((uint32_t)args[0], args[1], &got[0], &error);
            got[1] = 0; expected_count = 1;
        } else if (strcmp(fields[1], "math_binary") == 0 && argc == 3) {
            status = oa_math_f64_binary((uint32_t)args[0], args[1], args[2], &got[0], &error);
            expected_count = 1;
        } else if (strcmp(fields[1], "math_ternary") == 0 && argc == 4) {
            status = oa_math_f64_ternary((uint32_t)args[0], args[1], args[2], args[3], &got[0], &error);
            expected_count = 1;
        } else if (strcmp(fields[1], "math_powi") == 0 && argc == 2) {
            status = oa_math_f64_powi(args[0], (int32_t)args[1], &got[0], &error);
            expected_count = 1;
        } else if (strcmp(fields[1], "math_sin_cos") == 0 && argc == 1) {
            status = oa_math_f64_sin_cos(args[0], &got[0], &got[1], &error);
            expected_count = 2;
        } else if (strcmp(fields[1], "math_compare") == 0 && argc == 2) {
            int32_t order = 0;
            status = oa_math_f64_compare(args[0], args[1], &order, &error);
            got[0] = (double)order; expected_count = 1;
        } else if (strcmp(fields[1], "math_sign_negative") == 0 && argc == 1) {
            uint32_t negative = 0;
            status = oa_math_f64_is_sign_negative(args[0], &negative, &error);
            got[0] = (double)negative; expected_count = 1;
        } else if (strcmp(fields[1], "math_f32_unary") == 0 && argc == 2) {
            float value = 0;
            status = oa_math_f32_unary((uint32_t)args[0], (float)args[1], &value, &error);
            got[0] = (double)value; expected_count = 1;
        } else if (strcmp(fields[1], "math_f32_binary") == 0 && argc == 3) {
            float value = 0;
            status = oa_math_f32_binary((uint32_t)args[0], (float)args[1], (float)args[2], &value, &error);
            got[0] = (double)value; expected_count = 1;
        } else if (strcmp(fields[1], "math_f32_ternary") == 0 && argc == 4) {
            float value = 0;
            status = oa_math_f32_ternary((uint32_t)args[0], (float)args[1], (float)args[2], (float)args[3], &value, &error);
            got[0] = (double)value; expected_count = 1;
        } else if (strcmp(fields[1], "math_f32_compare") == 0 && argc == 2) {
            int32_t order = 0;
            status = oa_math_f32_compare((float)args[0], (float)args[1], &order, &error);
            got[0] = (double)order; expected_count = 1;
        } else if (strcmp(fields[1], "math_f32_sign_negative") == 0 && argc == 1) {
            uint32_t negative = 0;
            status = oa_math_f32_is_sign_negative((float)args[0], &negative, &error);
            got[0] = (double)negative; expected_count = 1;
        } else if (strcmp(fields[1], "sha256") == 0) {
            uint8_t bytes[4096], digest[32], hex[64];
            size_t length = decode_hex(fields[2], bytes, sizeof(bytes));
            if (length > sizeof(bytes)) {
                report_failure(&failures, "L0 vector", line_number, fields[0], "SHA-256 input is invalid or exceeds the fixed buffer");
                continue;
            }
            status = oa_sha256_digest(length == 0 ? NULL : bytes, (uint64_t)length, digest, &error);
            if (status == OA_STATUS_OK) {
                OaErrorInfo hex_error = {99, 99, 99};
                uint32_t hex_status = oa_sha256_hex(digest, hex, &hex_error);
                if (hex_status != OA_STATUS_OK || hex_error.code != OA_STATUS_OK) {
                    status = hex_status != OA_STATUS_OK ? hex_status : hex_error.code;
                } else if (!compare_hex(digest, sizeof(digest), fields[6]) ||
                           !compare_ascii_hex(hex, sizeof(hex), fields[6])) {
                    report_failure(&failures, "L0 vector", line_number, fields[0],
                                   "SHA-256 digest or ASCII-hex output differs from fixture");
                }
            }
            expected_count = 0;
        } else if (strcmp(fields[1], "rhumb_inverse") == 0 && argc == 4) {
            status = oa_geo_wgs84_rhumb_inverse(args[0], args[1], args[2], args[3], &got[0], &got[1], &error);
            expected_count = 2;
        } else if (strcmp(fields[1], "rhumb_direct") == 0 && argc == 4) {
            status = oa_geo_wgs84_rhumb_direct(args[0], args[1], args[2], args[3], &got[0], &got[1], &error);
            expected_count = 2;
        } else if (strcmp(fields[1], "rhumb_fraction") == 0 && argc == 5) {
            status = oa_geo_wgs84_rhumb_point_at_fraction(args[0], args[1], args[2], args[3], args[4], &got[0], &got[1], &error);
            expected_count = 2;
        } else if (strcmp(fields[1], "separation") == 0 && argc == 5) {
            status = oa_geo_wgs84_geodesic_rhumb_separation(
                args[0], args[1], args[2], args[3], (uint32_t)args[4],
                &got[0], &got[1], &got[2], &got[3], &error);
            expected_count = 4;
        } else {
            report_failure(&failures, "L0 vector", line_number, fields[0], "unknown operation or wrong argument count");
            continue;
        }
        if (status != OA_STATUS_OK || error.code != OA_STATUS_OK || error.location_kind != OA_LOCATION_NONE) {
            report_failure(&failures, "L0 vector", line_number, fields[0], "operation status or error record differs from success contract");
            continue;
        }
        if (strcmp(fields[0], "equator-zero-separation") == 0) {
            if (!isfinite(got[1]) || got[1] < 0.0 || got[1] > 1.0 ||
                !check(got[0], expected[0], tolerance) ||
                !check(got[2], expected[1], tolerance) ||
                !check(got[3], expected[2], tolerance)) {
                report_failure(&failures, "L0 vector", line_number, fields[0],
                               "zero-separation tie result differs from fixture or valid fraction range");
            }
        } else {
            for (size_t i = 0; i < expected_count; ++i) {
                if (!check(got[i], expected[i], tolerance)) {
                    char reason[96];
                    (void)snprintf(reason, sizeof(reason), "output %zu differs from fixture", i + 1);
                    report_failure(&failures, "L0 vector", line_number, fields[0], reason);
                }
            }
        }
    }
    if (fclose(input) != 0) {
        fprintf(stderr, "L0 vector input close failed: %s\n", path);
        return 3;
    }
    return failures == 0 ? 0 : 4;
}

static int run_polygon_vectors(const char *path) {
    char line[512];
    size_t line_number = 0;
    int failures = 0;
    FILE *input = fopen(path, "r");
    if (input == NULL) {
        fprintf(stderr, "cannot open polygon vector file: %s\n", path);
        return 2;
    }
    while (fgets(line, sizeof(line), input) != NULL) {
        char *fields[5];
        double latitudes[8], longitudes[8];
        size_t vertices = 0;
        ++line_number;
        size_t count = split(line, ',', fields, 5);
        if (line_number == 1) continue;
        if (count != 5) {
            report_failure(&failures, "polygon vector", line_number,
                           count > 0 ? fields[0] : NULL, "expected exactly five CSV fields");
            continue;
        }
        char *point_parts[8];
        size_t point_count = split(fields[1], ';', point_parts, 8);
        if (point_count > 8) {
            report_failure(&failures, "polygon vector", line_number, fields[0], "too many vertices");
            continue;
        }
        int coordinates_valid = 1;
        for (size_t i = 0; i < point_count; ++i) {
            char *coordinate[2];
            if (split(point_parts[i], '|', coordinate, 2) != 2) {
                report_failure(&failures, "polygon vector", line_number, fields[0], "malformed latitude/longitude pair");
                coordinates_valid = 0;
                break;
            }
            latitudes[i] = strtod(coordinate[0], NULL);
            longitudes[i] = strtod(coordinate[1], NULL);
            ++vertices;
        }
        if (!coordinates_valid) continue;
        OaPolygon *polygon = NULL;
        OaErrorInfo error = {99, 99, 99};
        uint32_t status = oa_geo_polygon_new_wgs84(latitudes, longitudes, (uint64_t)vertices, &polygon, &error);
        if (status != OA_STATUS_OK || polygon == NULL) {
            report_failure(&failures, "polygon vector", line_number, fields[0], "polygon construction failed");
            continue;
        }
        uint64_t edge_count = 0;
        status = oa_geo_polygon_edge_count(polygon, &edge_count, &error);
        if (status != OA_STATUS_OK || edge_count != vertices) {
            report_failure(&failures, "polygon vector", line_number, fields[0], "edge count differs from vertex count");
        }
        uint32_t inside = 99;
        status = oa_geo_polygon_contains_wgs84(polygon, strtod(fields[2], NULL), strtod(fields[3], NULL), &inside, &error);
        if (status != OA_STATUS_OK || inside != (uint32_t)strtoul(fields[4], NULL, 10)) {
            report_failure(&failures, "polygon vector", line_number, fields[0], "contains result differs from fixture");
        }
        oa_geo_polygon_free(polygon);
    }
    if (fclose(input) != 0) {
        fprintf(stderr, "polygon vector input close failed: %s\n", path);
        return 3;
    }
    return failures == 0 ? 0 : 4;
}

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: %s L0_VECTORS.csv POLYGON_VECTORS.csv\n", argv[0]);
        return 1;
    }
    int vectors = run_vectors(argv[1]);
    if (vectors != 0) return vectors;
    return run_polygon_vectors(argv[2]);
}
