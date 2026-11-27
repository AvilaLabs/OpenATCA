#include <assert.h>
#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "../../../include/openatca.h"

typedef uint32_t (*ParseCifp)(const uint8_t *, uint64_t, const uint8_t *, uint64_t,
    const OaCifpBuffers *, OaCifpCounts *, OaCifpError *, OaErrorInfo *);

static void parse_diagnostics(const char *text, uint64_t out[11]) {
    const char *cursor = text;
    for (size_t i = 0; i < 11; i++) {
        char *end = NULL;
        unsigned long long value = strtoull(cursor, &end, 10);
        assert(end != cursor);
        out[i] = (uint64_t)value;
        if (i < 10) {
            assert(*end == '\t');
            cursor = end + 1;
        } else {
            assert(*end == '\n' || *end == '\0');
        }
    }
}

static void assert_diagnostics(const OaCifpError *detail, const uint64_t expected[11]) {
    const uint64_t actual[11] = {
        detail->status, detail->kind, detail->line, detail->first_column,
        detail->last_column, detail->limit, detail->actual, detail->previous,
        detail->found, detail->code, detail->flags
    };
    assert(memcmp(actual, expected, sizeof actual) == 0);
}

static uint8_t *decode_hex(const char *hex, size_t *length) {
    size_t n = strlen(hex);
    assert((n % 2) == 0);
    uint8_t *bytes = malloc(n / 2);
    assert(bytes != NULL);
    for (size_t i = 0; i < n; i += 2) {
        char pair[3] = {hex[i], hex[i + 1], '\0'};
        char *end = NULL;
        unsigned long value = strtoul(pair, &end, 16);
        assert(end == pair + 2 && value <= UINT8_MAX);
        bytes[i / 2] = (uint8_t)value;
    }
    *length = n / 2;
    return bytes;
}

int main(int argc, char **argv) {
    assert(argc == 3);
    void *library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    assert(library != NULL);
    ParseCifp parse = (ParseCifp)dlsym(library, "oa_cifp_parse");
    assert(parse != NULL);

    FILE *fixture = fopen(argv[2], "rb");
    assert(fixture != NULL);
    char line[8192];
    assert(fgets(line, sizeof line, fixture) != NULL);
    assert(strcmp(line,
        "case\thex\tstatus\tkind\tline\tfirst_column\tlast_column\tlimit\tactual\tprevious\tfound\tcode\tflags\n") == 0);
    assert(fgets(line, sizeof line, fixture) != NULL);
    char *hex = strchr(line, '\t');
    assert(hex != NULL && strncmp(line, "valid\t", 6) == 0);
    hex++;
    char *expected_text = strchr(hex, '\t');
    assert(expected_text != NULL);
    *expected_text++ = '\0';
    uint64_t valid_diagnostics[11] = {0};
    parse_diagnostics(expected_text, valid_diagnostics);
    char *newline = strchr(hex, '\n');
    assert(newline != NULL);
    *newline = '\0';
    size_t input_length = 0;
    uint8_t *input = decode_hex(hex, &input_length);
    const uint8_t source[] = "synthetic.cifp";
    OaCifpCounts counts = {0};
    OaCifpError detail = {0};
    OaErrorInfo generic = {0};
    OaCifpBuffers query = {0};
    uint32_t status = parse(input, input_length, source, sizeof source - 1, &query,
                            &counts, &detail, &generic);
    assert(status == OA_STATUS_BUFFER_TOO_SMALL);
    assert_diagnostics(&detail, valid_diagnostics);
    assert(counts.airports == 1 && counts.header_records == 1);
    assert(counts.runways == 1 && counts.localizers == 1 && counts.vhf_navaids == 1);
    assert(counts.ndbs == 2 && counts.waypoints == 2 && counts.moras == 1);
    assert(counts.airspaces == 1 && counts.boundaries == 1);
    assert(counts.airways == 1 && counts.airway_fixes == 1);
    assert(counts.sections == 14 && counts.cycles == 1 && counts.errors == 0);
    assert(counts.procedures == 3 && counts.routes == 3 && counts.legs == 3);
    assert(counts.continuations == 1 && counts.levels_of_service == 3);

    OaCifpHeader header = {0};
    OaCifpHeaderText header_text[1] = {0};
    OaCifpAirport airport[1] = {0};
    OaCifpRunway runways[1] = {0};
    OaCifpLocalizer localizers[1] = {0};
    OaCifpVhfNavaid vhf_navaids[1] = {0};
    OaCifpNdb ndbs[2] = {0};
    OaCifpWaypoint waypoints[2] = {0};
    OaCifpMora moras[1] = {0};
    OaCifpAirspace airspaces[1] = {0};
    OaCifpBoundary boundaries[1] = {0};
    OaCifpAirway airways[1] = {0};
    OaCifpAirwayFix airway_fixes[1] = {0};
    OaCifpSectionCount sections[14] = {0};
    OaCifpCycleCount cycles[1] = {0};
    OaCifpProcedure procedures[3] = {0};
    OaCifpRoute routes[3] = {0};
    OaCifpLeg legs[3] = {0};
    OaCifpContinuation continuations[1] = {0};
    OaCifpLevelOfService levels[3] = {0};
    uint8_t strings[4096] = {0};
    assert(counts.strings_bytes <= sizeof strings);
    OaCifpBuffers out = {0};
    out.header = &header; out.header_capacity = 1;
    out.header_text = header_text; out.header_text_capacity = 1;
    out.airports = airport; out.airports_capacity = 1;
    out.runways = runways; out.runways_capacity = 1;
    out.localizers = localizers; out.localizers_capacity = 1;
    out.vhf_navaids = vhf_navaids; out.vhf_navaids_capacity = 1;
    out.ndbs = ndbs; out.ndbs_capacity = 2;
    out.waypoints = waypoints; out.waypoints_capacity = 2;
    out.moras = moras; out.moras_capacity = 1;
    out.airspaces = airspaces; out.airspaces_capacity = 1;
    out.boundaries = boundaries; out.boundaries_capacity = 1;
    out.airways = airways; out.airways_capacity = 1;
    out.airway_fixes = airway_fixes; out.airway_fixes_capacity = 1;
    out.sections = sections; out.sections_capacity = 14;
    out.cycles = cycles; out.cycles_capacity = 1;
    out.procedures = procedures; out.procedures_capacity = 3;
    out.routes = routes; out.routes_capacity = 3;
    out.legs = legs; out.legs_capacity = 3;
    out.continuations = continuations; out.continuations_capacity = 1;
    out.levels_of_service = levels; out.levels_of_service_capacity = 3;
    out.strings = strings; out.strings_capacity = sizeof strings;
    status = parse(input, input_length, source, sizeof source - 1, &out,
                   &counts, &detail, &generic);
    assert(status == OA_STATUS_OK && detail.status == OA_STATUS_OK);
    assert(header.record_length == 132 && header.cycle_year == 26);
    assert(airport[0].meta.line == 2 && (airport[0].present & 1) != 0);
    assert(memcmp(strings + airport[0].ident.offset, "KZZZ", 4) == 0);
    assert(runways[0].length_ft == 100);
    assert(localizers[0].frequency_mhz == 111.75);
    assert(vhf_navaids[0].frequency_mhz == 113.0);
    assert(ndbs[0].frequency_khz == 365.0);
    assert(ndbs[1].terminal == 1);
    assert(waypoints[0].kind == 1);
    assert(waypoints[1].kind == 2);
    assert(moras[0].cells_ft[0] == 10000);
    assert(airspaces[0].kind == 1 && airspaces[0].boundary_count == 1);
    assert(airways[0].fix_count == 1 && airway_fixes[0].sequence == 1);
    assert(sections[0].count == 1 && cycles[0].year == 26);
    assert(procedures[0].kind == 1 && procedures[1].kind == 2 && procedures[2].kind == 3);
    assert(routes[0].leg_count == 1 && routes[1].leg_count == 1 && routes[2].leg_count == 1);
    assert(legs[0].sequence == 10 && continuations[0].application_type == 'W');
    assert(levels[0].authorized == 1 && levels[1].authorized == 0);

    OaCifpAirport preserved = airport[0];
    out.airports_capacity = 0;
    status = parse(input, input_length, source, sizeof source - 1, &out,
                   &counts, &detail, &generic);
    assert(status == OA_STATUS_BUFFER_TOO_SMALL);
    assert_diagnostics(&detail, valid_diagnostics);
    assert(memcmp(&preserved, &airport[0], sizeof preserved) == 0);

    assert(fgets(line, sizeof line, fixture) != NULL);
    hex = strchr(line, '\t');
    assert(hex != NULL && strncmp(line, "missing_header\t", 15) == 0);
    hex++;
    expected_text = strchr(hex, '\t');
    assert(expected_text != NULL);
    *expected_text++ = '\0';
    uint64_t malformed_diagnostics[11] = {0};
    parse_diagnostics(expected_text, malformed_diagnostics);
    newline = strchr(hex, '\n');
    if (newline != NULL) *newline = '\0';
    size_t bad_length = 0;
    uint8_t *bad = decode_hex(hex, &bad_length);
    OaCifpCounts sentinel = {0}; sentinel.airports = 99;
    counts = sentinel;
    status = parse(bad, bad_length, source, sizeof source - 1, &query,
                   &counts, &detail, &generic);
    assert(status == OA_STATUS_PARSE_ERROR);
    assert(counts.airports == sentinel.airports && detail.status == status);
    assert_diagnostics(&detail, malformed_diagnostics);

    free(bad); free(input); fclose(fixture); dlclose(library);
    return 0;
}
