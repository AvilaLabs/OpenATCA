/* C11 checks for the shared synthetic FAA FNS AIXM profile vector. */
#include "../../../include/openatca.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

_Static_assert(sizeof(OaNotamRow) == 272, "OaNotamRow ABI size");
_Static_assert(sizeof(OaNotamFeature) == 56, "OaNotamFeature ABI size");
_Static_assert(sizeof(OaNotamPolygon) == 16, "OaNotamPolygon ABI size");
_Static_assert(sizeof(OaNotamVertex) == 8, "OaNotamVertex ABI size");
_Static_assert(sizeof(OaNotamUnknownPath) == 184, "OaNotamUnknownPath ABI size");
_Static_assert(sizeof(OaNotamSummary) == 512, "OaNotamSummary ABI size");
_Static_assert(sizeof(OaNotamBuffers) == 96, "OaNotamBuffers ABI size");

static int fail(const char *message) {
    fprintf(stderr, "NOTAM C vector: %s\n", message);
    return 1;
}

static int contains_bytes(const uint8_t *haystack, size_t haystack_len,
                          const char *needle, size_t needle_len) {
    size_t i;
    if (needle_len > haystack_len) return 0;
    for (i = 0; i <= haystack_len - needle_len; ++i) {
        if (memcmp(haystack + i, needle, needle_len) == 0) return 1;
    }
    return 0;
}

int main(int argc, char **argv) {
    FILE *file;
    long size;
    uint8_t *input;
    OaNotamSummary summary = {0};
    OaErrorInfo error = {0};
    OaNotamBuffers buffers = {0};
    OaNotamRow *rows = NULL;
    OaNotamFeature *features = NULL;
    OaNotamPolygon *polygons = NULL;
    OaNotamVertex *vertices = NULL;
    uint8_t *strings = NULL;
    OaNotamUnknownPath *unknown = NULL;
    uint32_t status;

    if (argc != 2) return fail("usage: ffi_notam_vectors synthetic-notam-v1.xml");
    file = fopen(argv[1], "rb");
    if (file == NULL || fseek(file, 0, SEEK_END) != 0 || (size = ftell(file)) < 0 ||
        (uint64_t)size > OA_NOTAM_MAX_INPUT_BYTES || fseek(file, 0, SEEK_SET) != 0)
        return fail("cannot open bounded fixture");
    input = (uint8_t *)malloc((size_t)size);
    if (input == NULL) return fail("fixture allocation failed");
    if (fread(input, 1, (size_t)size, file) != (size_t)size || fclose(file) != 0)
        return fail("cannot read fixture");

    status = oa_notam_fns_decode(input, (uint64_t)size, 0, 0, NULL, &summary, &error);
    if (status != OA_STATUS_BUFFER_TOO_SMALL || summary.status != status ||
        error.code != status || summary.row_count != 1 || summary.polygon_count != 1 ||
        summary.vertex_count != 4 || summary.feature_count == 0)
        return fail("query did not return exact required capacities");

    rows = (OaNotamRow *)calloc((size_t)summary.row_count, sizeof(*rows));
    features = (OaNotamFeature *)calloc((size_t)summary.feature_count, sizeof(*features));
    polygons = (OaNotamPolygon *)calloc((size_t)summary.polygon_count, sizeof(*polygons));
    vertices = (OaNotamVertex *)calloc((size_t)summary.vertex_count, sizeof(*vertices));
    strings = (uint8_t *)calloc((size_t)summary.string_bytes, 1);
    unknown = (OaNotamUnknownPath *)calloc((size_t)summary.unknown_path_count, sizeof(*unknown));
    if (rows == NULL || features == NULL || polygons == NULL || vertices == NULL ||
        (summary.string_bytes != 0 && strings == NULL) ||
        (summary.unknown_path_count != 0 && unknown == NULL))
        return fail("output allocation failed");
    buffers.row = rows;
    buffers.row_capacity = summary.row_count;
    buffers.features = features;
    buffers.feature_capacity = summary.feature_count;
    buffers.polygons = polygons;
    buffers.polygon_capacity = summary.polygon_count;
    buffers.vertices = vertices;
    buffers.vertex_capacity = summary.vertex_count;
    buffers.strings = strings;
    buffers.string_capacity = summary.string_bytes;
    buffers.unknown_paths = unknown;
    buffers.unknown_path_capacity = summary.unknown_path_count;

    status = oa_notam_fns_decode(input, (uint64_t)size, 0, 0, &buffers, &summary, &error);
    if (status != OA_STATUS_OK || summary.status != status || error.code != status ||
        summary.row_count != 1 || rows[0].year != 2030 || rows[0].number != 42 ||
        (rows[0].present & OA_NOTAM_FIELD_MASK(OA_NOTAM_FIELD_TEXT)) == 0 ||
        polygons[0].vertex_count != 4 || vertices[0].lat_e7 != 301000000 ||
        vertices[0].lon_e7 != -901000000 || summary.unknown_path_count != 1 ||
        summary.unknown_elements != 1 || unknown[0].count != 1 ||
        !contains_bytes(unknown[0].text, unknown[0].text_len, "event:novel", 11))
        return fail("decoded typed values or polygon geometry disagree");
    if (rows[0].text_off + rows[0].text_len > summary.string_bytes ||
        !contains_bytes(strings + rows[0].text_off, rows[0].text_len, "RWY 09/27 CLSD", 14))
        return fail("text arena span does not point at the retained NOTAM text");

    rows[0].present = UINT64_MAX;
    features[0].kind = UINT8_MAX;
    vertices[0].lat_e7 = INT32_MAX;
    buffers.row_capacity = 0;
    status = oa_notam_fns_decode(input, (uint64_t)size, 0, 0, &buffers, &summary, &error);
    if (status != OA_STATUS_BUFFER_TOO_SMALL || rows[0].present != UINT64_MAX ||
        features[0].kind != UINT8_MAX || vertices[0].lat_e7 != INT32_MAX)
        return fail("short row capacity wrote partial output");

    status = oa_notam_fns_decode(input, (uint64_t)size, INT64_C(1898931600000),
        INT64_C(1898931600001), NULL, &summary, &error);
    if (status != OA_STATUS_NOTAM_SEALED_DROP || summary.row_count != 0 ||
        summary.sealed_dropped != 1)
        return fail("explicit half-open drop window did not count its dropped document");

    free(input);
    free(rows);
    free(features);
    free(polygons);
    free(vertices);
    free(strings);
    free(unknown);
    puts("NOTAM C vector passed");
    return 0;
}
