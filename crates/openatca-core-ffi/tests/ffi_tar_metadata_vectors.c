#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "openatca.h"

_Static_assert(sizeof(OaTarEntry) == 48, "OaTarEntry ABI size");
_Static_assert(_Alignof(OaTarEntry) == 8, "OaTarEntry ABI alignment");

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

static int check_valid(OaTarEntry *entries, uint64_t count, const uint8_t *names,
                       uint64_t names_length, const uint8_t *expected_names, size_t expected_n,
                       const char *name) {
    if (count != 3 || names_length != expected_n || memcmp(names, expected_names, expected_n) != 0) {
        fprintf(stderr, "%s count/name pool mismatch\n", name); return 1;
    }
    uint64_t mtime = 014000000000ULL;
    if (entries[0].name_offset != 0 || entries[0].name_length != 4 || entries[0].size != 3 ||
        entries[0].mtime != mtime || entries[0].header_offset != 0 || entries[0].typeflag != '0' ||
        entries[1].name_offset != 4 || entries[1].name_length != 4 || entries[1].size != 0 ||
        entries[1].mtime != mtime || entries[1].header_offset != 1024 || entries[1].typeflag != '?' ||
        entries[2].name_offset != 8 || entries[2].name_length != 9 || entries[2].size != 0 ||
        entries[2].mtime != mtime || entries[2].header_offset != 1536 || entries[2].typeflag != '0' ||
        memcmp(entries[0].reserved, "\0\0\0\0\0\0\0", 7) != 0 ||
        memcmp(entries[1].reserved, "\0\0\0\0\0\0\0", 7) != 0 ||
        memcmp(entries[2].reserved, "\0\0\0\0\0\0\0", 7) != 0) {
        fprintf(stderr, "%s row metadata mismatch\n", name); return 1;
    }
    return 0;
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
        char *fields[8] = {0};
        fields[0] = line;
        for (size_t index = 0; index < 7; ++index) {
            char *tab = strchr(fields[index], '\t');
            if (!tab) { fprintf(stderr, "bad vector line %u\n", lineno); return 1; }
            *tab = '\0';
            fields[index + 1] = tab + 1;
        }
        fields[7][strcspn(fields[7], "\r\n")] = '\0';
        uint8_t archive[32768], names_expected[256], names[512];
        size_t archive_n = unhex(fields[1], archive);
        size_t expected_names_n = unhex(fields[7], names_expected);
        uint32_t max_entries = (uint32_t)strtoul(fields[2], NULL, 10);
        uint32_t expected_status = (uint32_t)strtoul(fields[3], NULL, 10);
        uint64_t entry_capacity = strtoull(fields[4], NULL, 10);
        uint64_t names_capacity = strtoull(fields[5], NULL, 10);
        int64_t expected_count = strtoll(fields[6], NULL, 10);
        OaTarEntry entries[8];
        uint8_t sentinel_entries[sizeof entries], sentinel_names[sizeof names];
        memset(entries, 0xa5, sizeof entries);
        memset(sentinel_entries, 0xa5, sizeof sentinel_entries);
        memset(names, 0xa5, sizeof names);
        memset(sentinel_names, 0xa5, sizeof sentinel_names);
        uint64_t count = 777, names_length = 777;
        OaErrorInfo error = {0};
        uint32_t status = oa_tar_parse(archive_n ? archive : NULL, archive_n, max_entries,
            entry_capacity ? entries : NULL, entry_capacity, &count,
            names_capacity ? names : NULL, names_capacity, &names_length, &error);
        if (status != expected_status) { fprintf(stderr, "%s status %u != %u\n", fields[0], status, expected_status); return 1; }
        if (status == OA_STATUS_OK) {
            if ((int64_t)count != expected_count || names_length != expected_names_n) {
                fprintf(stderr, "%s success lengths mismatch\n", fields[0]); return 1;
            }
            if (strcmp(fields[0], "valid_duplicate_unknown") == 0 &&
                check_valid(entries, count, names, names_length, names_expected, expected_names_n, fields[0])) return 1;
        } else if (status == OA_STATUS_BUFFER_TOO_SMALL) {
            if ((int64_t)count != expected_count || names_length != expected_names_n ||
                memcmp(entries, sentinel_entries, sizeof entries) != 0 ||
                memcmp(names, sentinel_names, sizeof names) != 0) {
                fprintf(stderr, "%s query metadata/output mismatch\n", fields[0]); return 1;
            }
            uint64_t exact_count = 777, exact_names_length = 777;
            status = oa_tar_parse(archive, archive_n, max_entries, entries, count, &exact_count,
                names, names_length, &exact_names_length, &error);
            if (status != OA_STATUS_OK || exact_count != count || exact_names_length != names_length ||
                check_valid(entries, exact_count, names, exact_names_length, names_expected, expected_names_n, fields[0])) return 1;
        } else if (count != 777 || names_length != 777 ||
            memcmp(entries, sentinel_entries, sizeof entries) != 0 ||
            memcmp(names, sentinel_names, sizeof names) != 0) {
            fprintf(stderr, "%s failure was not transactional\n", fields[0]); return 1;
        }
    }
    fclose(file);
    return 0;
}
