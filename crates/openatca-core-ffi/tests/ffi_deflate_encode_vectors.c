#define _POSIX_C_SOURCE 200809L
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "openatca.h"

static size_t unhex(const char *text, uint8_t *output) {
    size_t length = strlen(text);
    if (length % 2 != 0) return 0;
    for (size_t index = 0; index < length / 2; ++index) {
        unsigned value = 0;
        if (sscanf(text + 2 * index, "%2x", &value) != 1) return 0;
        output[index] = (uint8_t)value;
    }
    return length / 2;
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
        char *fields[9] = {0};
        fields[0] = line;
        for (size_t index = 0; index < 8; ++index) {
            char *tab = strchr(fields[index], '\t');
            if (!tab) { fprintf(stderr, "bad vector line %u\n", lineno); return 1; }
            *tab = '\0';
            fields[index + 1] = tab + 1;
        }
        fields[8][strcspn(fields[8], "\r\n")] = '\0';
        uint8_t input[32768], encoded[512], plain[512], decoded[512];
        size_t input_n = unhex(fields[1], input);
        size_t plain_n = unhex(fields[8], plain);
        uint32_t level = (uint32_t)strtoul(fields[2], NULL, 10);
        uint64_t max_output = strtoull(fields[3], NULL, 10);
        uint32_t expected_status = (uint32_t)strtoul(fields[4], NULL, 10);
        uint64_t capacity = strtoull(fields[5], NULL, 10);
        int64_t expected_consumed = strtoll(fields[6], NULL, 10);
        uint32_t expected_level = (uint32_t)strtoul(fields[7], NULL, 10);
        uint64_t encoded_n = 777, consumed = 777;
        uint32_t actual_level = 777;
        memset(encoded, 0xa5, sizeof encoded);
        OaErrorInfo error = {0};
        uint32_t status = oa_deflate_raw_compress(input_n ? input : NULL, input_n, level,
            max_output, capacity ? encoded : NULL, capacity, &encoded_n, &consumed,
            &actual_level, &error);
        if (status != expected_status) { fprintf(stderr, "%s status %u != %u\n", fields[0], status, expected_status); return 1; }
        if (status == OA_STATUS_OK) {
            if (consumed != (uint64_t)expected_consumed || actual_level != expected_level) {
                fprintf(stderr, "%s metadata mismatch\n", fields[0]); return 1;
            }
            uint64_t decoded_n = 777, compressed_n = 777;
            status = oa_deflate_raw_inflate(encoded, encoded_n, plain_n,
                plain_n ? decoded : NULL, plain_n, &decoded_n, &compressed_n, &error);
            if (status != OA_STATUS_OK || decoded_n != plain_n ||
                (plain_n && memcmp(decoded, plain, plain_n) != 0) || compressed_n != encoded_n) {
                fprintf(stderr, "%s round trip failed\n", fields[0]); return 1;
            }
        } else if (status == OA_STATUS_BUFFER_TOO_SMALL) {
            if (encoded_n == 0 || consumed != (uint64_t)expected_consumed || actual_level != expected_level || encoded[0] != 0xa5) {
                fprintf(stderr, "%s size query failed\n", fields[0]); return 1;
            }
            uint64_t required = encoded_n;
            status = oa_deflate_raw_compress(input_n ? input : NULL, input_n, level,
                max_output, encoded, required, &encoded_n, &consumed, &actual_level, &error);
            if (status != OA_STATUS_OK || encoded_n != required || consumed != (uint64_t)expected_consumed || actual_level != expected_level) {
                fprintf(stderr, "%s exact-size retry failed\n", fields[0]); return 1;
            }
            uint64_t decoded_n = 777, compressed_n = 777;
            status = oa_deflate_raw_inflate(encoded, encoded_n, plain_n,
                plain_n ? decoded : NULL, plain_n, &decoded_n, &compressed_n, &error);
            if (status != OA_STATUS_OK || decoded_n != plain_n ||
                (plain_n && memcmp(decoded, plain, plain_n) != 0) || compressed_n != encoded_n) {
                fprintf(stderr, "%s size-query round trip failed\n", fields[0]); return 1;
            }
        } else if (encoded_n != 777 || consumed != 777 || actual_level != 777 || encoded[0] != 0xa5) {
            fprintf(stderr, "%s failure was not transactional\n", fields[0]); return 1;
        }
    }
    fclose(file);
    return 0;
}
