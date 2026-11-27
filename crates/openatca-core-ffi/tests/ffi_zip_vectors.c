#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "openatca.h"

_Static_assert(sizeof(OaZipEntry) == 56, "OaZipEntry ABI size");
_Static_assert(_Alignof(OaZipEntry) == 8, "OaZipEntry ABI alignment");

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
        char *fields[8] = {0};
        fields[0] = line;
        for (size_t index = 0; index < 7; ++index) {
            char *tab = strchr(fields[index], '\t');
            if (!tab) { fprintf(stderr, "bad ZIP vector line %u\n", lineno); return 1; }
            *tab = '\0'; fields[index + 1] = tab + 1;
        }
        fields[7][strcspn(fields[7], "\r\n")] = '\0';
        uint8_t archive[32768], expected_names[512], expected_output[32768], expected_data[32768];
        size_t archive_n = unhex(fields[1], archive);
        uint32_t expected_list = (uint32_t)strtoul(fields[2], NULL, 10);
        size_t expected_names_n = strcmp(fields[3], "-") == 0 ? 0 : unhex(fields[3], expected_names);
        OaZipEntry entries[8];
        uint8_t names[512];
        memset(entries, 0xa5, sizeof entries); memset(names, 0xa5, sizeof names);
        OaZipEntry initial_entries[8]; uint8_t initial_names[512];
        memcpy(initial_entries, entries, sizeof entries); memcpy(initial_names, names, sizeof names);
        uint64_t count = UINT64_MAX, names_length = UINT64_MAX;
        OaErrorInfo error = {0};
        uint32_t status = oa_zip_list(archive, archive_n, NULL, 0, &count, NULL, 0, &names_length, &error);
        uint32_t expected_query = expected_list == OA_STATUS_OK ? OA_STATUS_BUFFER_TOO_SMALL : expected_list;
        if (status != expected_query) { fprintf(stderr, "%s list status %u != %u\n", fields[0], status, expected_query); return 1; }
        if (expected_list != OA_STATUS_OK) {
            if (count != UINT64_MAX || names_length != UINT64_MAX) { fprintf(stderr, "%s changed failure counts\n", fields[0]); return 1; }
            continue;
        }
        if (count > 8 || names_length > sizeof names || count == 0) { fprintf(stderr, "%s fixture cap\n", fields[0]); return 1; }
        status = oa_zip_list(archive, archive_n, entries, count, &count, names, names_length, &names_length, &error);
        if (status != OA_STATUS_OK || names_length != expected_names_n ||
            memcmp(names, expected_names, expected_names_n) != 0 ||
            memcmp(entries[0].reserved, "\0\0\0\0\0\0", 6) != 0) {
            fprintf(stderr, "%s metadata mismatch\n", fields[0]); return 1;
        }
        if (strcmp(fields[4], "-") == 0) continue;
        uint64_t index = strtoull(fields[4], NULL, 10);
        uint32_t expected_extract = (uint32_t)strtoul(fields[5], NULL, 10);
        uint64_t declared = strtoull(fields[7], NULL, 10), output_length = UINT64_MAX;
        uint8_t *out = declared ? expected_output : NULL;
        status = oa_zip_extract_entry(archive, archive_n, index, NULL, 0, &output_length, &error);
        uint32_t expected_extract_query = declared == 0 ? OA_STATUS_OK : OA_STATUS_BUFFER_TOO_SMALL;
        if (status != expected_extract_query || output_length != declared) {
            fprintf(stderr, "%s extract query mismatch\n", fields[0]); return 1;
        }
        memset(expected_output, 0xa5, sizeof expected_output);
        size_t expected_bytes_n = strcmp(fields[6], "-") == 0 ? 0 : unhex(fields[6], expected_data);
        uint8_t before[32768]; memcpy(before, expected_output, sizeof before);
        status = oa_zip_extract_entry(archive, archive_n, index, out, declared, &output_length, &error);
        if (status != expected_extract) { fprintf(stderr, "%s extract status %u != %u\n", fields[0], status, expected_extract); return 1; }
        if (expected_extract == OA_STATUS_OK) {
            if (output_length != expected_bytes_n ||
                (expected_bytes_n && memcmp(out, expected_data, expected_bytes_n) != 0)) {
                fprintf(stderr, "%s extraction bytes mismatch\n", fields[0]); return 1;
            }
        } else if (memcmp(expected_output, before, sizeof before) != 0 || output_length != declared) {
            fprintf(stderr, "%s failed extraction modified output/length\n", fields[0]); return 1;
        }
    }
    fclose(file);

    /* Capacity shortfall and invalid index remain transactional. */
    file = fopen(argv[1], "r");
    if (!file) return 2;
    while (fgets(line, sizeof line, file) && strncmp(line, "stored\t", 7) != 0) {}
    fclose(file);
    char *fields[8] = {0}; fields[0] = line;
    for (size_t index = 0; index < 7; ++index) { char *tab = strchr(fields[index], '\t'); if (!tab) return 2; *tab = '\0'; fields[index+1] = tab+1; }
    uint8_t archive[32768], output[4] = {0xa5,0xa5,0xa5,0xa5}, names[2] = {0xa5,0xa5};
    size_t archive_n = unhex(fields[1], archive);
    uint64_t count = UINT64_MAX, names_length = UINT64_MAX, out_length = UINT64_MAX;
    OaZipEntry entry = {.name_offset = 77}; OaErrorInfo error = {0};
    uint32_t status = oa_zip_list(archive, archive_n, &entry, 1, &count, names, 2, &names_length, &error);
    if (status != OA_STATUS_BUFFER_TOO_SMALL || count != 1 || entry.name_offset != 77 || names[0] != 0xa5) return 1;
    status = oa_zip_extract_entry(archive, archive_n, 0, output, 2, &out_length, &error);
    if (status != OA_STATUS_BUFFER_TOO_SMALL || out_length != 3 || output[0] != 0xa5) return 1;
    out_length = UINT64_MAX;
    status = oa_zip_extract_entry(archive, archive_n, 1, output, sizeof output, &out_length, &error);
    if (status != OA_STATUS_ZIP_BAD_ENTRY_INDEX || out_length != UINT64_MAX || output[0] != 0xa5) return 1;
    return 0;
}
