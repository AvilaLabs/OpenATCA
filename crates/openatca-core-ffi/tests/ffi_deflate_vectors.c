#define _POSIX_C_SOURCE 200809L
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "openatca.h"

static size_t unhex(const char *s, uint8_t *out) {
    size_t n = strlen(s) / 2;
    for (size_t i = 0; i < n; ++i) {
        unsigned value = 0;
        if (sscanf(s + 2 * i, "%2x", &value) != 1) return 0;
        out[i] = (uint8_t)value;
    }
    return n;
}

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    FILE *file = fopen(argv[1], "r");
    if (!file) return 2;
    char line[65536];
    unsigned lineno = 0;
    while (fgets(line, sizeof line, file)) {
        ++lineno;
        if (line[0] == '#' || line[0] == '\n') continue;
        char *fields[7] = {0};
        char *save = NULL;
        char *token = strtok_r(line, "\t\n", &save);
        for (size_t i = 0; token && i < 7; ++i) { fields[i] = token; token = strtok_r(NULL, "\t\n", &save); }
        if (!fields[6]) { fprintf(stderr, "bad vector line %u\n", lineno); return 1; }
        uint8_t input[32768], output[256], expected_output[256];
        size_t input_n = unhex(fields[1], input);
        size_t expected_n = unhex(fields[5], expected_output);
        uint64_t limit = strtoull(fields[2], NULL, 10), cap = strtoull(fields[4], NULL, 10);
        uint32_t expected = (uint32_t)strtoul(fields[3], NULL, 10);
        uint64_t out_n = 777, consumed = 777;
        memset(output, 0xa5, sizeof output);
        OaErrorInfo error = {0};
        uint32_t status = oa_deflate_raw_inflate(input_n ? input : NULL, input_n, limit,
            cap ? output : NULL, cap, &out_n, &consumed, &error);
        if (status != expected) { fprintf(stderr, "%s status %u != %u\n", fields[0], status, expected); return 1; }
        if (status == OA_STATUS_OK || status == OA_STATUS_BUFFER_TOO_SMALL) {
            if (out_n != expected_n || consumed != strtoull(fields[6], NULL, 10) ||
                (status == OA_STATUS_OK && memcmp(output, expected_output, expected_n) != 0)) {
                fprintf(stderr, "%s lengths mismatch\n", fields[0]); return 1;
            }
        } else if (out_n != 777 || consumed != 777 || output[0] != 0xa5) {
            fprintf(stderr, "%s failure was not transactional\n", fields[0]); return 1;
        }
    }
    fclose(file);
    return 0;
}
