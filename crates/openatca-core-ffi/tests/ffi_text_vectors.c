#include "../../../include/openatca.h"
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

_Static_assert(sizeof(OaTextError) == 32, "OaTextError layout");
_Static_assert(sizeof(OaJsonNode) == 24, "OaJsonNode layout");
_Static_assert(sizeof(OaJsonCounts) == 16, "OaJsonCounts layout");
_Static_assert(sizeof(OaXmlSpan) == 8, "OaXmlSpan layout");
_Static_assert(sizeof(OaXmlElement) == 40, "OaXmlElement layout");
_Static_assert(sizeof(OaXmlAttribute) == 32, "OaXmlAttribute layout");
_Static_assert(sizeof(OaXmlText) == 16, "OaXmlText layout");
_Static_assert(sizeof(OaXmlNamespace) == 24, "OaXmlNamespace layout");
_Static_assert(sizeof(OaXmlCounts) == 40, "OaXmlCounts layout");
_Static_assert(sizeof(OaCsvRow) == 16, "OaCsvRow layout");
_Static_assert(sizeof(OaCsvField) == 8, "OaCsvField layout");
_Static_assert(sizeof(OaCsvCounts) == 24, "OaCsvCounts layout");
_Static_assert(offsetof(OaXmlElement, namespace_head) == 36, "OaXmlElement offsets");
_Static_assert(offsetof(OaCsvRow, first_field) == 8, "OaCsvRow offsets");

static int hex_value(char value) {
    if (value >= '0' && value <= '9') return value - '0';
    if (value >= 'a' && value <= 'f') return value - 'a' + 10;
    if (value >= 'A' && value <= 'F') return value - 'A' + 10;
    return -1;
}

static size_t decode_hex(const char *hex, uint8_t *output, size_t capacity) {
    size_t length = strlen(hex);
    if ((length & 1U) != 0 || length / 2 > capacity) return SIZE_MAX;
    for (size_t index = 0; index < length; index += 2) {
        int high = hex_value(hex[index]);
        int low = hex_value(hex[index + 1]);
        if (high < 0 || low < 0) return SIZE_MAX;
        output[index / 2] = (uint8_t)((high << 4) | low);
    }
    return length / 2;
}

static int counts_match(const char *text, const uint64_t *counts, size_t count) {
    const char *cursor = text;
    for (size_t index = 0; index < count; ++index) {
        uint64_t value = 0;
        if (*cursor < '0' || *cursor > '9') return 0;
        do {
            uint64_t digit = (uint64_t)(*cursor - '0');
            if (value > (UINT64_MAX - digit) / 10) return 0;
            value = value * 10 + digit;
            ++cursor;
        } while (*cursor >= '0' && *cursor <= '9');
        if (value != counts[index]) return 0;
        if (index + 1 < count) {
            if (*cursor != ',') return 0;
            ++cursor;
        }
    }
    return *cursor == '\0';
}

static int run_one(char *line) {
    size_t line_length = strlen(line);
    while (line_length > 0 && (line[line_length - 1] == '\n' || line[line_length - 1] == '\r')) {
        line[--line_length] = '\0';
    }
    char *fields[8] = {0};
    size_t field_count = 0;
    char *cursor = line;
    while (field_count < 8) {
        fields[field_count++] = cursor;
        char *tab = strchr(cursor, '\t');
        if (tab == NULL) break;
        *tab = '\0';
        cursor = tab + 1;
    }
    if (field_count != 8) return 1;
    uint8_t input[4096], expected_pool[4096];
    size_t input_len = decode_hex(fields[2], input, sizeof(input));
    size_t pool_len = strcmp(fields[7], "-") == 0 ? 0 : decode_hex(fields[7], expected_pool, sizeof(expected_pool));
    uint32_t expected_status = (uint32_t)strtoul(fields[3], NULL, 10);
    uint32_t expected_kind = (uint32_t)strtoul(fields[4], NULL, 10);
    uint64_t expected_position = strtoull(fields[5], NULL, 10);
    if (input_len == SIZE_MAX || pool_len == SIZE_MAX) return 2;
    OaErrorInfo error = {0, 0, 0};
    OaTextError parse_error = {0};

    if (strcmp(fields[0], "json") == 0) {
        OaJsonCounts counts = {0, 0};
        uint32_t status = oa_json_parse(input, input_len, NULL, 0, NULL, 0, &counts, &parse_error, &error);
        if (expected_status != 0) {
            return status == expected_status && parse_error.kind == expected_kind && parse_error.position == expected_position ? 0 : 3;
        }
        uint64_t values[] = {counts.node_count, counts.value_bytes};
        if (status != OA_STATUS_BUFFER_TOO_SMALL || !counts_match(fields[6], values, 2)) return 4;
        OaJsonNode *nodes = calloc((size_t)counts.node_count, sizeof(*nodes));
        uint8_t *pool = calloc((size_t)counts.value_bytes, 1);
        if (nodes == NULL || pool == NULL) { free(nodes); free(pool); return 5; }
        status = oa_json_parse(input, input_len, nodes, counts.node_count, pool, counts.value_bytes, &counts, &parse_error, &error);
        int okay = status == 0 && counts.node_count != 0 && nodes[0].kind == OA_JSON_OBJECT
            && counts.value_bytes == pool_len && memcmp(pool, expected_pool, pool_len) == 0;
        free(nodes); free(pool);
        return okay ? 0 : 6;
    }
    if (strcmp(fields[0], "xml") == 0) {
        OaXmlCounts counts = {0};
        uint32_t status = oa_xml_parse(input, input_len, NULL, 0, NULL, 0, NULL, 0, NULL, 0, NULL, 0,
                                       &counts, &parse_error, &error);
        if (expected_status != 0) {
            return status == expected_status && parse_error.kind == expected_kind && parse_error.position == expected_position ? 0 : 7;
        }
        uint64_t values[] = {counts.elements, counts.attributes, counts.text_nodes, counts.namespaces, counts.namespace_uri_bytes};
        if (status != OA_STATUS_BUFFER_TOO_SMALL || !counts_match(fields[6], values, 5)) return 8;
        OaXmlElement *elements = calloc((size_t)counts.elements, sizeof(*elements));
        OaXmlAttribute *attributes = calloc((size_t)counts.attributes, sizeof(*attributes));
        OaXmlText *texts = calloc((size_t)counts.text_nodes, sizeof(*texts));
        OaXmlNamespace *namespaces = calloc((size_t)counts.namespaces, sizeof(*namespaces));
        uint8_t *pool = calloc((size_t)counts.namespace_uri_bytes, 1);
        if (elements == NULL || attributes == NULL || texts == NULL || namespaces == NULL || pool == NULL) {
            free(elements); free(attributes); free(texts); free(namespaces); free(pool); return 9;
        }
        status = oa_xml_parse(input, input_len, elements, counts.elements, attributes, counts.attributes,
            texts, counts.text_nodes, namespaces, counts.namespaces, pool, counts.namespace_uri_bytes,
            &counts, &parse_error, &error);
        int okay = status == 0 && counts.namespace_uri_bytes == pool_len && memcmp(pool, expected_pool, pool_len) == 0;
        if (okay && texts[0].flags != 1) okay = 0;
        if (okay) {
            uint8_t decoded[8] = {0};
            uint64_t written = 0;
            status = oa_xml_decode_span(input, input_len, texts[0].raw_value.start, texts[0].raw_value.length,
                OA_XML_SPAN_TEXT, decoded, sizeof(decoded), &written, &parse_error, &error);
            okay = status == 0 && written == 2 && memcmp(decoded, "v&", 2) == 0;
        }
        free(elements); free(attributes); free(texts); free(namespaces); free(pool);
        return okay ? 0 : 10;
    }
    if (strcmp(fields[0], "csv") == 0) {
        OaCsvCounts counts = {0};
        uint32_t status = oa_csv_parse(input, input_len, NULL, 0, NULL, 0, NULL, 0, &counts, &parse_error, &error);
        if (expected_status != 0) {
            return status == expected_status && parse_error.kind == expected_kind && parse_error.position == expected_position ? 0 : 11;
        }
        uint64_t values[] = {counts.rows, counts.fields, counts.value_bytes};
        if (status != OA_STATUS_BUFFER_TOO_SMALL || !counts_match(fields[6], values, 3)) return 12;
        OaCsvRow *rows = calloc((size_t)counts.rows, sizeof(*rows));
        OaCsvField *fields_out = calloc((size_t)counts.fields, sizeof(*fields_out));
        uint8_t *pool = calloc((size_t)counts.value_bytes, 1);
        if (rows == NULL || fields_out == NULL || pool == NULL) { free(rows); free(fields_out); free(pool); return 13; }
        status = oa_csv_parse(input, input_len, rows, counts.rows, fields_out, counts.fields,
            pool, counts.value_bytes, &counts, &parse_error, &error);
        int okay = status == 0 && counts.value_bytes == pool_len && memcmp(pool, expected_pool, pool_len) == 0;
        free(rows); free(fields_out); free(pool);
        return okay ? 0 : 14;
    }
    return 15;
}

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    FILE *input = fopen(argv[1], "rb");
    if (input == NULL) return 3;
    char line[8192];
    if (fgets(line, sizeof(line), input) == NULL) { fclose(input); return 4; }
    size_t rows = 0;
    int failures = 0;
    while (fgets(line, sizeof(line), input) != NULL) {
        ++rows;
        if (run_one(line) != 0) ++failures;
    }
    fclose(input);
    return rows == 6 && failures == 0 ? 0 : 5;
}
