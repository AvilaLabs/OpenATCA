"""Builds the local fuzz corpora under $OPENATCA_DATA/fuzz/<target>/corpus from the raw data, and
refreshes the small synthetic seeds in fuzz/seeds/. Real data never goes into the repository.

    python3 -I fuzz/make_corpora.py
"""
import os, sys, struct, glob, gzip, io, zlib, tarfile, zipfile, random
D = os.environ['OPENATCA_DATA']
R = D + '/raw'
S = D + '/scds'
REPO = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'seeds')
random.seed(1)
def out(target, name, data, cap=300_000):
    if len(data) > cap or len(data) == 0: return
    d = f'{D}/fuzz/{target}/corpus'
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
    b = open(f'{A}/{f}', 'rb').read()
    out('asterix', f'{f}-whole', b)
    for k, blk in enumerate(blocks(b)): out('asterix', f'{f}-{k}', blk)
for f in glob.glob(A + '/*.pcap'):
    b = open(f, 'rb').read()
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
    fs = sorted(glob.glob(pattern))
    random.shuffle(fs)
    c = 0
    for f in fs:
        if os.path.getsize(f) <= maxsz:
            yield os.path.basename(f), open(f, 'rb').read(); c += 1
            if c >= n: break
for tgt, pat, n in [('tbfm', S+'/tbfm-*/*.msg', 600), ('tfms', S+'/tfms-*/*.msg', 600), ('fixm', S+'/sfdps-*/*.msg', 600)]:
    for nm, d in msgs(pat, n):
        out(tgt, nm, d); out('xml', tgt + '-' + nm, d) if random.random() < 0.3 else None
for nm, d in msgs(S+'/fns-*/*.msg', 113): out('xml', 'fns-' + nm, d)
# a few hostile XML starters (synthetic)
for i, x in enumerate([b'<a/>', b'<a xmlns="u"><b:c xmlns:b="v" d="&amp;&#65;"/></a>', b'<?xml version="1.0"?><!-- c --><a><![CDATA[x]]></a>', b'<a b="1" b="2"/>', b'<!DOCTYPE a [<!ENTITY e "x">]><a>&e;</a>']):
    repo('xml', f'x{i}.xml', x)

# --- feed frames from the recordings
for f in glob.glob(S + '/rec/*.oaf'):
    b = open(f, 'rb').read(); i = 0; fr = []
    while i + 40 <= len(b):
        if b[i:i+4] != b'OAF1': break
        n = struct.unpack('<I', b[i+32:i+36])[0]
        fr.append(b[i:i+40+n]); i += 40 + n
    random.shuffle(fr)
    for k, x in enumerate(fr[:200]): out('feed-frame', f'{os.path.basename(f)}-{k}', x, 100_000)
    for k in range(20):
        out('feed-frame', f'{os.path.basename(f)}-run{k}', b''.join(fr[k*5:k*5+5]), 200_000)

# --- adsb traces, json
tf = glob.glob(R + '/adsblol/*.tar.aa')[0]
n = 0; tot = 0
try:
    with tarfile.open(tf, 'r|') as t:
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
for f in glob.glob(R + '/jsontestsuite/test_parsing/*.json'):
    out('json', 'jts-' + os.path.basename(f), open(f, 'rb').read())
# awc
awc = {0: ['metar'], 1: ['taf'], 2: ['pirep'], 3: ['sigmet', 'isigmet'], 4: ['airsigmet'], 5: ['gairmet']}
for f in glob.glob(R + '/awc/*/*.json'):
    nm = os.path.basename(f)
    for sel, keys in awc.items():
        if any(nm.startswith(k + '-') or nm.startswith(k + '_') for k in keys):
            if nm.startswith('sigmet') and sel != 3: continue
            d = open(f, 'rb').read()
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
for f in sorted(glob.glob(R + '/grib2-testdata/*.grib2')) + sorted(glob.glob(R + '/hrrr/*')) [:0]:
    b = open(f, 'rb').read()
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
# every committed seed also goes into its corpus
for t in os.listdir(REPO):
    for f in os.listdir(f'{REPO}/{t}'):
        out(t, 'seed-' + f, open(f'{REPO}/{t}/{f}', 'rb').read())
print('done')
