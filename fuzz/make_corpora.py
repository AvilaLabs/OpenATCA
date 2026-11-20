"""Builds the local fuzz corpora under $OPENATCA_DATA/fuzz/<target>/corpus from the raw data, and
refreshes the small synthetic seeds in fuzz/seeds/. Real data never goes into the repository.

    python3 -I fuzz/make_corpora.py
"""
import os, sys, struct, glob, gzip, io, zlib, tarfile, zipfile, random
# The sealed hold-out guard (tools/sealed_guard.py); -I does not put the script's directory on the path.
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tools'))
import sealed_guard
D = os.environ['OPENATCA_DATA']
R = D + '/raw'
S = D + '/scds'
REPO = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'seeds')
random.seed(1)
SEALED_SKIPPED = 0
def gl(pattern):
    """glob() without the sealed paths: each is skipped and counted, never opened (OA-SEAL-012)."""
    global SEALED_SKIPPED
    keep = []
    for p in glob.glob(pattern):
        try:
            sealed_guard.production().check(p)
        except sealed_guard.SealedError:
            SEALED_SKIPPED += 1
            continue
        keep.append(p)
    return keep
def rb(path, mode='rb'):
    """Whole file through the guard's opener."""
    with sealed_guard.open_read(path, mode) as f:
        return f.read()
def out(target, name, data, cap=300_000):
    if len(data) > cap or len(data) == 0: return
    d = f'{D}/fuzz/{target}/corpus'
    sealed_guard.refuse(d)
    os.makedirs(d, exist_ok=True)
    open(f'{d}/{name}', 'wb').write(data)
def repo(target, name, data):
    d = f'{REPO}/{target}'
    os.makedirs(d, exist_ok=True)
    open(f'{d}/{name}', 'wb').write(data)
    out(target, 'syn-' + name, data)

# --- asterix (GPL samples: local only)
A = R + '/asterix-samples'
def blocks(buf):
    i = 0
    while i + 3 <= len(buf):
        n = struct.unpack('>H', buf[i+1:i+3])[0]
        if n < 3 or i + n > len(buf): break
        yield buf[i:i+n]; i += n
for f in ['cat048.raw', 'cat062cat065.raw', 'cat21_re.ast']:
    b = rb(f'{A}/{f}')
    out('asterix', f'{f}-whole', b)
    for k, blk in enumerate(blocks(b)): out('asterix', f'{f}-{k}', blk)
for f in gl(A + '/*.pcap'):
    b = rb(f)
    be = b[:4] == b'\xa1\xb2\xc3\xd4'
    e = '>' if be else '<'
    i = 24; k = 0
    while i + 16 <= len(b):
        incl = struct.unpack(e + 'I', b[i+8:i+12])[0]
        pkt = b[i+16:i+16+incl]; i += 16 + incl
        pay = pkt[42:]
        out('asterix', f'{os.path.basename(f)}-{k}', pay)
        for j, blk in enumerate(blocks(pay)): out('asterix', f'{os.path.basename(f)}-{k}-{j}', blk)
        k += 1

# --- xml family from scds message dumps
def msgs(pattern, n, maxsz=200_000):
    fs = sorted(gl(pattern))
    random.shuffle(fs)
    c = 0
    for f in fs:
        if os.path.getsize(f) <= maxsz:
            yield os.path.basename(f), rb(f); c += 1
            if c >= n: break
for tgt, pat, n in [('tbfm', S+'/tbfm-*/*.msg', 600), ('tfms', S+'/tfms-*/*.msg', 600), ('fixm', S+'/sfdps-*/*.msg', 600)]:
    for nm, d in msgs(pat, n):
        out(tgt, nm, d); out('xml', tgt + '-' + nm, d) if random.random() < 0.3 else None
for nm, d in msgs(S+'/fns-*/*.msg', 113): out('xml', 'fns-' + nm, d)
# a few hostile XML starters (synthetic)
for i, x in enumerate([b'<a/>', b'<a xmlns="u"><b:c xmlns:b="v" d="&amp;&#65;"/></a>', b'<?xml version="1.0"?><!-- c --><a><![CDATA[x]]></a>', b'<a b="1" b="2"/>', b'<!DOCTYPE a [<!ENTITY e "x">]><a>&e;</a>']):
    repo('xml', f'x{i}.xml', x)

# --- feed frames from the recordings
for f in gl(S + '/rec/*.oaf'):
    b = rb(f); i = 0; fr = []
    while i + 40 <= len(b):
        if b[i:i+4] != b'OAF1': break
        n = struct.unpack('<I', b[i+32:i+36])[0]
        fr.append(b[i:i+40+n]); i += 40 + n
    random.shuffle(fr)
    for k, x in enumerate(fr[:200]): out('feed-frame', f'{os.path.basename(f)}-{k}', x, 100_000)
    for k in range(20):
        out('feed-frame', f'{os.path.basename(f)}-run{k}', b''.join(fr[k*5:k*5+5]), 200_000)

# --- adsb traces, json
# The newest part among the unsealed days (the real layout is raw/adsblol/YYYY.MM.DD/v*.tar.aa);
# a sealed day directory is skipped and counted, never listed.
adsb_parts, adsb_skipped, _ = sealed_guard.walk_unsealed(R + '/adsblol') if os.path.isdir(R + '/adsblol') else ([], 0, 0)
SEALED_SKIPPED += adsb_skipped
adsb_parts = sorted(p for p in adsb_parts if p.endswith('.tar.aa'))
n = 0; tot = 0
try:
    if not adsb_parts: raise FileNotFoundError('no unsealed *.tar.aa part')
    with tarfile.open(fileobj=sealed_guard.open_read(adsb_parts[-1], 'rb'), mode='r|') as t:
        for m in t:
            if not (m.isfile() and m.name.endswith('.json')): continue
            if m.size > 120_000: continue
            try:
                raw = gzip.decompress(t.extractfile(m).read())
            except Exception:
                continue
            if len(raw) > 400_000: continue
            out('adsb-trace', f'tr{n}', raw, 400_000); out('json', f'tr{n}', raw, 400_000) if n % 4 == 0 else None
            n += 1; tot += len(raw)
            if n >= 500 or tot > 30_000_000: break
except Exception as e:
    print('tar stop', e)
print('traces', n)
for f in gl(R + '/jsontestsuite/test_parsing/*.json'):
    out('json', 'jts-' + os.path.basename(f), rb(f))
# awc
awc = {0: ['metar'], 1: ['taf'], 2: ['pirep'], 3: ['sigmet', 'isigmet'], 4: ['airsigmet'], 5: ['gairmet']}
for f in gl(R + '/awc/*/*.json'):
    nm = os.path.basename(f)
    for sel, keys in awc.items():
        if any(nm.startswith(k + '-') or nm.startswith(k + '_') for k in keys):
            if nm.startswith('sigmet') and sel != 3: continue
            d = rb(f)
            out('awc', nm, bytes([sel]) + d, 1_500_000)
            out('json', 'awc-' + nm, d, 1_500_000) if len(d) < 400_000 and random.random() < .3 else None
            break

# --- grib2 + j2k
def gsplit(b):
    i = 0
    while i + 16 <= len(b) and b[i:i+4] == b'GRIB':
        n = struct.unpack('>Q', b[i+8:i+16])[0]
        if n < 16 or i + n > len(b): break
        yield b[i:i+n]; i += n
k = 0
for f in sorted(gl(R + '/grib2-testdata/*.grib2')) + sorted(gl(R + '/hrrr/*')) [:0]:
    b = rb(f)
    out('grib2', os.path.basename(f), b, 3_000_000)
    for m in gsplit(b):
        out('grib2', f'm{k}', m, 400_000)
        # sections
        p = 16
        tmpl = None
        while p + 5 <= len(m) - 4:
            ln = struct.unpack('>I', m[p:p+4])[0]; sn = m[p+4]
            if ln < 5: break
            if sn == 5 and ln >= 11: tmpl = struct.unpack('>H', m[p+9:p+11])[0]
            if sn == 7 and tmpl == 40: out('j2k', f'cs{k}', m[p+5:p+ln], 400_000)
            p += ln
        k += 1
print('grib msgs', k)

# --- synthetic: tiff
def tiff(w, h, comp=1, big=False):
    e = '>' if big else '<'
    px = b''.join(struct.pack(e + 'f', 100.0 + x + 3 * y) for y in range(h) for x in range(w))
    if comp == 8: px = zlib.compress(px)
    ents = []
    def ent(tag, typ, cnt, val): ents.append((tag, typ, cnt, val))
    data_off = 8 + 2 + 14 * 12 + 4
    extra = b''
    def blob(b):
        nonlocal extra
        o = data_off + len(extra); extra += b; return o
    scale = blob(struct.pack(e + '3d', 0.001, 0.001, 0))
    tie = blob(struct.pack(e + '6d', 0, 0, 0, -90.0, 45.0, 0))
    keys = blob(struct.pack(e + '16H', 1, 1, 0, 3, 1024, 0, 1, 2, 1025, 0, 1, 1, 2048, 0, 1, 4326))
    pxo = blob(px)
    ent(256, 4, 1, w); ent(257, 4, 1, h); ent(258, 3, 1, 32); ent(259, 3, 1, comp); ent(262, 3, 1, 1)
    ent(273, 4, 1, pxo); ent(277, 3, 1, 1); ent(278, 4, 1, h); ent(279, 4, 1, len(px)); ent(339, 3, 1, 3)
    ent(33550, 12, 3, scale); ent(33922, 12, 6, tie); ent(34735, 3, 16, keys); ent(34736, 12, 0, 0)
    ents.sort()
    ents = ents[:14]
    hdr = (b'MM' if big else b'II') + struct.pack(e + 'HI', 42, 8) + struct.pack(e + 'H', len(ents))
    for t, ty, c, v in ents:
        vb = struct.pack(e + 'HHI', t, ty, c)
        if ty == 3 and c == 1: vb += struct.pack(e + 'H', v) + b'\0\0' if not big else struct.pack(e + 'H', v) + b'\0\0'
        else: vb += struct.pack(e + 'I', v)
        hdr += vb
    hdr += struct.pack(e + 'I', 0)
    return hdr + extra
for name, a in [('le-raw', (8, 6, 1)), ('le-deflate', (8, 6, 8)), ('be-raw', (4, 4, 1, True)), ('le-large', (64, 64, 8))]:
    repo('tiff', name + '.tif', tiff(*a))

# --- synthetic: zip, gzip, tar
def mkzip(method, names):
    bio = io.BytesIO()
    with zipfile.ZipFile(bio, 'w', method) as z:
        for n in names: z.writestr(n, (b'hello zip %d\n' % len(n)) * 40)
    return bio.getvalue()
repo('zip', 'stored.zip', mkzip(zipfile.ZIP_STORED, ['a.txt', 'dir/b.txt']))
repo('zip', 'deflated.zip', mkzip(zipfile.ZIP_DEFLATED, ['a.txt', '../evil', '/abs', 'c/d/e.csv']))
repo('zip', 'empty.zip', mkzip(zipfile.ZIP_STORED, []))
repo('gzip', 'one.gz', gzip.compress(b'hello gzip ' * 50))
repo('gzip', 'two.gz', gzip.compress(b'a' * 100) + gzip.compress(b'b' * 100))
c = zlib.compressobj(9, zlib.DEFLATED, -15); repo('gzip', 'raw.deflate', c.compress(bytes(range(256)) * 8) + c.flush())
c = zlib.compressobj(0, zlib.DEFLATED, -15); repo('gzip', 'stored.deflate', c.compress(b'abc' * 100) + c.flush())
def mktar(fmt, names):
    bio = io.BytesIO()
    with tarfile.open(fileobj=bio, mode='w', format=fmt) as t:
        for n in names:
            ti = tarfile.TarInfo(n); d = b'tar body ' * 30; ti.size = len(d); ti.mtime = 1_700_000_000
            t.addfile(ti, io.BytesIO(d))
    return bio.getvalue()
repo('tar', 'ustar.tar', mktar(tarfile.USTAR_FORMAT, ['a.txt', 'd/b.txt']))
repo('tar', 'pax.tar', mktar(tarfile.PAX_FORMAT, ['x' * 150, 'é.txt']))
repo('tar', 'gnu.tar', mktar(tarfile.GNU_FORMAT, ['y' * 130 + '.json']))
# --- csv, nasr, arinc424, notam, mva (the text parsers)
NA = R + '/nasr-2026-10-29/x'
CI = R + '/cifp-2026-10-29/x/FAACIFP18'
OAR = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'crates')
def rd(path): return rb(path)

# csv: the first byte is the BufReader capacity (1-64), the rest is the file.
for i, (cap, body) in enumerate([
    (5, b'A,B,C\r\n1,2,3\r\n"x,y","q""r",\r\n'),
    (1, b'\xef\xbb\xbfA,B\n1,2\n'),
    (2, b'\xef\xbb\xbfA,B\n1,2\n'),
    (3, b'\xef\xbb\xbf"A","B"\n"multi\nline","2"\n'),
    (7, b'A,B\n1\n'),
    (9, b'A,B\r1,2\r\n3,4\n'),
    (4, b'\xef\xbb'),
    (64, b'"unterminated,1\n'),
]):
    repo('csv', f's{i}.csv', bytes([cap]) + body)
nasr_csvs = sorted(gl(NA + '/*.csv'))
for k, f in enumerate(nasr_csvs):
    lines = rd(f).split(b'\n')
    for j in range(4):
        a = 1 + (random.randrange(max(1, len(lines) - 12)) if j else 0)
        out('csv', f'{os.path.basename(f)}-{j}', bytes([random.randrange(1, 65)]) + b'\n'.join([lines[0]] + lines[a:a + 8]) + b'\n', 20_000)

# nasr: selector byte, then the file (form feed between files for the multi-file parsers).
SEL = {'APT_BASE': 0, 'APT_RWY': 1, 'APT_RWY_END': 2, 'FIX_BASE': 3, 'FIX_NAV': 4, 'NAV_BASE': 5,
       'AWY_BASE': 6, 'AWY_SEG_ALT': 7, 'ARB_BASE': 9, 'ARB_SEG': 10, 'CLS_ARSP': 11,
       'STAR_BASE': 12, 'STAR_APT': 13, 'DP_BASE': 0x8c, 'DP_APT': 0x8d}
def head_rows(name, n, off=0):
    lines = rd(f'{NA}/{name}.csv').split(b'\n')
    return b'\n'.join([lines[0]] + lines[1 + off:1 + off + n]) + b'\n'
for name, sel in SEL.items():
    if not os.path.exists(f'{NA}/{name}.csv'): continue
    n_all = rd(f'{NA}/{name}.csv').count(b'\n')
    for k in range(14):
        off = 0 if k == 0 else random.randrange(max(1, n_all - 40))
        out('nasr', f'{name}-{k}', bytes([sel]) + head_rows(name, [1, 2, 3, 5, 10, 20, 40][k % 7], off), 60_000)
for kind, sel0 in [('STAR', 14), ('DP', 0x8e)]:
    for k in range(40):
        n = [1, 2, 4, 8, 16][k % 5]
        out('nasr', f'{kind}-proc-{k}', bytes([sel0]) + b'\x0c'.join(head_rows(f'{kind}_{p}', n) for p in ('BASE', 'APT', 'RTE')), 60_000)
for k in range(20):
    n = [1, 2, 4, 8][k % 4]
    out('nasr', f'awy-{k}', bytes([8]) + head_rows('AWY_BASE', n) + b'\x0c' + head_rows('AWY_SEG_ALT', n * 2), 60_000)
for st in sorted(gl(NA + '/*_CSV_DATA_STRUCTURE.csv')):
    out('nasr', 'schema-' + os.path.basename(st), bytes([15]) + rd(st), 200_000)
    base = os.path.basename(st).replace('_CSV_DATA_STRUCTURE.csv', '')
    for name in [os.path.basename(x)[:-4] for x in nasr_csvs if os.path.basename(x).startswith(base + '_') and 'STRUCTURE' not in x][:3]:
        out('nasr', f'validate-{name}', bytes([16]) + rd(st) + b'\x0c' + head_rows(name, 10), 300_000)

# nasr seeds (synthetic): a small structure file and rows written for the fixes and the STAR family.
repo('nasr', 'schema.csv', bytes([15]) + b'CSV File,Column Name,Max Length,Data Type,Nullable\nFIX_BASE,EFF_DATE,10,VARCHAR,No\nFIX_BASE,FIX_ID,30,VARCHAR,No\nFIX_BASE,LAT_DEG,2,NUMBER(2,0),Yes\nFIX_BASE,LAT_SEC,5,NUMBER(5,2),Yes\n')
repo('nasr', 'validate.csv', bytes([16]) + b'CSV File,Column Name,Max Length,Data Type,Nullable\nFIX_BASE,EFF_DATE,10,VARCHAR,No\nFIX_BASE,FIX_ID,30,VARCHAR,No\nFIX_BASE,LAT_DEG,2,NUMBER(2,0),Yes\n\x0c"EFF_DATE","FIX_ID","LAT_DEG"\n"2030/01/01","QQQQQ",42\n"2030/01/01","",\n')
FIXH = '"EFF_DATE","FIX_ID","ICAO_REGION_CODE","STATE_CODE","COUNTRY_CODE","LAT_DEG","LAT_MIN","LAT_SEC","LAT_HEMIS","LAT_DECIMAL","LONG_DEG","LONG_MIN","LONG_SEC","LONG_HEMIS","LONG_DECIMAL","FIX_ID_OLD","CHARTING_REMARK","FIX_USE_CODE","ARTCC_ID_HIGH","ARTCC_ID_LOW","PITCH_FLAG","CATCH_FLAG","SUA_ATCAA_FLAG","MIN_RECEP_ALT","COMPULSORY","CHARTS"\n'
FIXR = '"2030/01/01","QQAAA","K6","XX","US",42,7,12.5,"N",42.12013889,71,8,30,"W",-71.14166667,"","","WP   ","ZQQ","ZQQ","N","N","N",,"","IAP"\n'
repo('nasr', 'fix.csv', bytes([3]) + (FIXH + FIXR + FIXR.replace('QQAAA', 'QQAAB').replace('"N",42', '"X",42')).encode())
repo('nasr', 'fix-hostile.csv', bytes([3]) + (FIXH + FIXR.replace('42.12013889', 'NaN').replace(',12.5,', ',99,') + FIXR.replace('"W"', '"E"')).encode())
STB = '"EFF_DATE","ARRIVAL_NAME","AMENDMENT_NO","ARTCC","STAR_AMEND_EFF_DATE","RNAV_FLAG","STAR_COMPUTER_CODE","SERVED_ARPT"\n"2030/01/01","ALPHA","ONE","ZQQ","2030/01/01","N","QQAAA.ALPHA1","QQA QQB"\n'
STA = '"EFF_DATE","STAR_COMPUTER_CODE","ARTCC","BODY_NAME","BODY_SEQ","ARPT_ID","RWY_END_ID"\n"2030/01/01","QQAAA.ALPHA1","ZQQ","QQAAA-ALPHA",1,"QQA","ALL"\n'
STR = '"EFF_DATE","STAR_COMPUTER_CODE","ARTCC","ROUTE_PORTION_TYPE","ROUTE_NAME","BODY_SEQ","TRANSITION_COMPUTER_CODE","POINT_SEQ","POINT","ICAO_REGION_CODE","POINT_TYPE","NEXT_POINT","ARPT_RWY_ASSOC"\n"2030/01/01","QQAAA.ALPHA1","ZQQ","BODY","QQAAA-ALPHA",1,"",10,"QQAAA","K2","RP   ","QQAAB","QQA/09, QQB"\n"2030/01/01","QQAAA.ALPHA1","ZQQ","BODY","QQAAA-ALPHA",1,"",20,"QQAAB","K2","RP   ","","QQA"\n'
repo('nasr', 'star-base.csv', bytes([12]) + STB.encode())
repo('nasr', 'star-all.csv', bytes([14]) + b'\x0c'.join(x.encode() for x in (STB, STA, STR)))
repo('nasr', 'dp-all.csv', bytes([0x8e]) + b'\x0c'.join(x.encode() for x in (STB.replace('ARRIVAL', 'DP').replace('STAR', 'DP'), STA.replace('STAR', 'DP'), STR.replace('STAR', 'DP'))))

# arinc424: the record layouts of the crate's tests with the airport identifiers and names
# replaced (the lines were published by the FAA; nothing here names a real place).
FX = OAR + '/oa-arinc424/tests/fixtures'
SUB = [('KORD', 'KQQA'), ('KMDW', 'KQQB'), ('KMSP', 'KQQC'), ('CHICAGO MIDWAY INTL', 'QUINCY SAMPLE INTL '),
       ('MINNEAPOLIS-ST PAUL INTL/WOLD-', 'SAMPLETON-NORTH INTL/EXAMPLE-'), ('HARE INTL', 'TEST INTL'),
       ('CHICAGO', 'QUINCY ')]
def fx(name):
    t = rb(f'{FX}/{name}.txt', 'r')
    for a, b in SUB:
        t = t.replace(a, b[:len(a)].ljust(len(a)))
    return t
def cifp(body):
    hdr = rb(f'{FX}/header.txt', 'r').split('\n')[:5]
    n = len([l for l in body.split('\n') if l])
    hdr[0] = hdr[0][:28] + f'{n:07d}' + hdr[0][35:]
    return ('\n'.join(hdr) + '\n' + body).encode()
repo('arinc424', 'airports-sid-star.cifp', cifp(fx('airports') + fx('runways') + fx('sid') + fx('star')))
repo('arinc424', 'approach.cifp', cifp(fx('airports') + fx('approach')))
repo('arinc424', 'airspace.cifp', cifp(fx('uc') + fx('ur')))
repo('arinc424', 'enroute.cifp', cifp(fx('airways') + fx('enroute_waypoints') + fx('vhf') + fx('ndb') + fx('terminal_ndb') + fx('terminal_waypoints') + fx('localizers') + fx('mora')))
repo('arinc424', 'records.txt', (fx('airports') + fx('approach')).encode())
repo('arinc424', 'header-only.cifp', cifp(''))
# local corpus: the real file in record-sized chunks, each with the real header and a matching count
if os.path.exists(CI):
    cl = rd(CI).split(b'\n')
    hdr = [l for l in cl[:5]]
    data = [l for l in cl[5:] if l]
    for k in range(400):
        n = [1, 2, 3, 5, 8, 13, 30][k % 7]
        a = random.randrange(len(data) - n)
        h = [bytearray(x) for x in hdr]
        h[0][28:35] = b'%07d' % n
        out('arinc424', f'cifp-{k}', b'\n'.join([bytes(x) for x in h] + data[a:a + n]) + b'\n', 20_000)

# notam: the synthetic message of the oa-notam tests (it is made up, no real feed data).
import re
cm = rb(OAR + '/oa-notam/tests/common/mod.rs', 'r')
def rawstr(name):
    return re.search(r'pub const ' + name + r': &str = r#"(.*?)"#;', cm, re.S).group(1)
nhead, nev, ntail = rawstr('HEAD'), rawstr('EVENT'), '</message:AIXMBasicMessage>'
msg = (nhead + nev + ntail).encode()
repo('notam', 'event.xml', msg)
repo('notam', 'event-twice.xml', (nhead + nev + nev + ntail).encode())
repo('notam', 'no-event.xml', (nhead + ntail).encode())
repo('notam', 'cut.xml', msg[:len(msg) // 2])
repo('notam', 'coords.xml', msg.replace(b'<event:coordinates>3012N09045W', b'<event:coordinates>9912N18145W'))
repo('notam', 'dates.xml', msg.replace(b'203003051000', b'PERM').replace(b'2030-03-05T09:00:00.000Z', b'2030-13-45T25:61:61Z'))

# mva: a synthetic chart (two squares, one with a hole) in the shape the FAA writes
def sq(x0, y0, x1, y1): return [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
def mva_msg(desc, secs):
    nil = ' xsi:nil="true" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"'
    def pl(r): return ' '.join(f'{x:.5f} {y:.5f}' for x, y in r + [r[0]])
    mem = ''
    for name, alt, rings in secs:
        inter = ''.join(f'<ns1:interior><ns1:LinearRing><ns1:posList>{pl(h)}</ns1:posList></ns1:LinearRing></ns1:interior>' for h in rings[1:])
        mem += f'<ns8:hasMember ns2:type="simple"><ns3:Airspace ns1:id="1"><ns1:description ns2:type="simple">MINIMUM VECTORING ALTITUDE (MVA)</ns1:description><ns1:boundedBy{nil}/><ns3:timeSlice><ns3:AirspaceTimeSlice><ns1:validTime ns2:type="simple"><ns1:TimePeriod><ns1:beginPosition>1980-01-01T14:00:00Z</ns1:beginPosition><ns1:endPosition indeterminatePosition="unknown"/></ns1:TimePeriod></ns1:validTime><ns3:interpretation>BASELINE</ns3:interpretation><ns3:sequenceNumber>1</ns3:sequenceNumber><ns3:type>OTHER</ns3:type><ns3:designator{nil}/><ns3:localType{nil}/><ns3:name>{name}</ns3:name><ns3:designatorICAO{nil}/><ns3:controlType{nil}/><ns3:upperLowerSeparation{nil}/><ns3:protectedRoute{nil}/><ns3:geometryComponent><ns3:AirspaceGeometryComponent><ns3:operation{nil}/><ns3:operationSequence{nil}/><ns3:theAirspaceVolume><ns3:AirspaceVolume><ns3:minimumLimit uom="FT">{alt}</ns3:minimumLimit><ns3:minimumLimitReference>MSL</ns3:minimumLimitReference><ns3:horizontalProjection><ns3:Surface srsName="urn:ogc:def:crs:OGC:1.3:CRS84" ns1:id="s"><ns1:patches><ns1:PolygonPatch interpolation="planar"><ns1:exterior><ns1:LinearRing><ns1:posList>{pl(rings[0])}</ns1:posList></ns1:LinearRing></ns1:exterior>{inter}</ns1:PolygonPatch></ns1:patches></ns3:Surface></ns3:horizontalProjection></ns3:AirspaceVolume></ns3:theAirspaceVolume></ns3:AirspaceGeometryComponent></ns3:geometryComponent></ns3:AirspaceTimeSlice></ns3:timeSlice></ns3:Airspace></ns8:hasMember>'
    return (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><ns8:AIXMBasicMessage ns1:id="m" xmlns:ns2="http://www.w3.org/1999/xlink" xmlns:ns8="http://www.aixm.aero/schema/5.1/message" xmlns:ns1="http://www.opengis.net/gml/3.2" xmlns:ns3="http://www.aixm.aero/schema/5.1"><ns1:description ns2:type="simple">{desc}</ns1:description><ns1:boundedBy{nil}/><ns3:sequenceNumber>1</ns3:sequenceNumber><ns3:messageMetadata/>{mem}</ns8:AIXMBasicMessage>').encode()
two = [('A', 2500, [sq(-88.0, 42.0, -87.0, 43.0)]), ('B', 3000, [sq(-87.0, 42.0, -86.0, 43.0), sq(-86.75, 42.25, -86.25, 42.75)])]
repo('mva', 'fus3.xml', mva_msg('XYZ_MVA_FUS3_2024_v1', two))
repo('mva', 'fus5.xml', mva_msg('XYZ_MVA_FUS5_2024_v1', two[:1]))
repo('mva', 'sub.xml', mva_msg('XYZ_SAV_LHW_MVA_2024', two[:1]))
repo('mva', 'open-ring.xml', mva_msg('XYZ_MVA_FUS3_2024_v1', two[:1]).replace(b'-88.00000 42.00000</ns1:posList>', b'-88.00000 42.50000</ns1:posList>'))
for f in sorted(gl(R + '/mva*/*.xml')):
    out('mva', os.path.basename(f), rd(f), 400_000)

# every committed seed also goes into its corpus
for t in os.listdir(REPO):
    for f in os.listdir(f'{REPO}/{t}'):
        out(t, 'seed-' + f, rb(f'{REPO}/{t}/{f}'))
print('sealed entries skipped:', SEALED_SKIPPED)
print('done')
