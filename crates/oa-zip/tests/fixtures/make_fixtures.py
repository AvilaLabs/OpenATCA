# Regenerates the ZIP fixtures and expected extractions. Standard library only.
# Run from this directory:  python3 -I make_fixtures.py
import io, os, struct, zipfile, zlib

OUT = os.path.dirname(os.path.abspath(__file__))
DT = (1980, 1, 1, 0, 0, 0)


def lcg_bytes(n, seed):
    out = bytearray()
    x = seed
    while len(out) < n:
        x = (x * 6364136223846793005 + 1442695040888963407) % (1 << 64)
        out.append((x >> 33) & 0xFF)
    return bytes(out)


def text(n):
    words = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
             "india", "juliet", "kilo", "lima", "mike", "november", "oscar", "papa"]
    raw = lcg_bytes(n, 7)
    s = []
    size = 0
    i = 0
    while size < n:
        w = words[raw[i % len(raw)] % len(words)] + ("\n" if raw[(i * 3) % len(raw)] % 11 == 0 else " ")
        s.append(w)
        size += len(w)
        i += 1
    return "".join(s).encode()[:n]


expected = {}


def add(z, name, data, method=zipfile.ZIP_DEFLATED, level=None, keep=True):
    zi = zipfile.ZipInfo(name, DT)
    zi.compress_type = method
    if level is not None:
        zi._compresslevel = level
    z.writestr(zi, data)
    if keep and "/" not in name and ".." not in name and ":" not in name:
        expected[name] = data


def save(name, buf):
    with open(os.path.join(OUT, name), "wb") as f:
        f.write(buf)


# stored.zip: stored entries, an empty file, a directory entry.
b = io.BytesIO()
with zipfile.ZipFile(b, "w") as z:
    add(z, "a.txt", b"stored contents\n", zipfile.ZIP_STORED)
    add(z, "empty.txt", b"", zipfile.ZIP_STORED)
    zi = zipfile.ZipInfo("dir/", DT)
    z.writestr(zi, b"")
save("stored.zip", b.getvalue())

# deflate.zip: fixed, dynamic, stored-block, empty, long runs, 32 KiB-distance copies.
b = io.BytesIO()
rnd = lcg_bytes(30000, 11)
with zipfile.ZipFile(b, "w") as z:
    add(z, "fixed.txt", b"hello hello hello hello\n")
    add(z, "dynamic.txt", text(20000), level=9)
    add(z, "storedblock.bin", lcg_bytes(5000, 3), level=0)
    add(z, "random.bin", lcg_bytes(5000, 5))
    add(z, "empty.txt", b"")
    add(z, "run.bin", b"a" * 100000)
    add(z, "far.bin", rnd + rnd, level=9)
    add(z, "mixed.bin", text(3000) + lcg_bytes(2000, 9) + text(3000))
save("deflate.zip", b.getvalue())

# many.zip: 300 small entries.
b = io.BytesIO()
with zipfile.ZipFile(b, "w") as z:
    for i in range(300):
        add(z, "f%03d.txt" % i, ("entry %d\n" % i).encode() * (i % 7 + 1), keep=False)
save("many.zip", b.getvalue())

# bomb.zip: 10 MB of zeros in ~10 KB (ratio ~1000).
b = io.BytesIO()
with zipfile.ZipFile(b, "w") as z:
    add(z, "zeros.bin", bytes(10_000_000), level=9, keep=False)
save("bomb.zip", b.getvalue())

# traversal.zip: hostile names, plus one good one.
b = io.BytesIO()
with zipfile.ZipFile(b, "w") as z:
    for n in ["../evil.txt", "/abs.txt", "a/../../b.txt", "C:\\x.txt", "..\\win.txt", "ok.txt"]:
        zi = zipfile.ZipInfo(n, DT)
        zi.filename = n
        z.writestr(zi, b"x")
save("traversal.zip", b.getvalue())


# descriptor.zip: written to a non-seekable stream, so the entries use data descriptors.
class NoSeek(io.RawIOBase):
    def __init__(self):
        self.buf = bytearray()

    def writable(self):
        return True

    def write(self, d):
        self.buf += d
        return len(d)

    def tell(self):
        return len(self.buf)


ns = NoSeek()
with zipfile.ZipFile(ns, "w") as z:
    add(z, "streamed.txt", text(5000))
    add(z, "streamed2.bin", lcg_bytes(300, 21), zipfile.ZIP_STORED)
save("descriptor.zip", bytes(ns.buf))


# zip64.zip: hand-built archive using ZIP64 extra fields, EOCD record and locator.
def zip64_archive(name, data, fake_usize=None):
    crc = zlib.crc32(data)
    co = zlib.compressobj(9, zlib.DEFLATED, -15)
    comp = co.compress(data) + co.flush()
    nb = name.encode()
    local = struct.pack("<IHHHHHIIIHH", 0x04034B50, 45, 0, 8, 0, 0x21, crc, 0xFFFFFFFF, 0xFFFFFFFF, len(nb), 20)
    local += nb + struct.pack("<HHQQ", 1, 16, len(data), len(comp))
    usz = len(data) if fake_usize is None else fake_usize
    body = local + comp
    cd_off = len(body)
    extra = struct.pack("<HHQQ", 1, 16, usz, len(comp))
    cd = struct.pack("<IHHHHHHIIIHHHHHII", 0x02014B50, 45, 45, 0, 8, 0, 0x21, crc, 0xFFFFFFFF, 0xFFFFFFFF,
                     len(nb), len(extra), 0, 0, 0, 0, 0)
    cd += nb + extra
    z64_off = cd_off + len(cd)
    z64 = struct.pack("<IQHHIIQQQQ", 0x06064B50, 44, 45, 45, 0, 0, 1, 1, len(cd), cd_off)
    loc = struct.pack("<IIQI", 0x07064B50, 0, z64_off, 1)
    eocd = struct.pack("<IHHHHIIH", 0x06054B50, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFFFFFF, 0xFFFFFFFF, 0)
    return body + cd + z64 + loc + eocd


data = text(4000)
save("zip64.zip", zip64_archive("big64.txt", data))
expected["big64.txt"] = data
save("zip64_huge.zip", zip64_archive("huge.bin", data, fake_usize=5 * 1024**3))

for n, d in expected.items():
    with open(os.path.join(OUT, "expected", n), "wb") as f:
        f.write(d)
