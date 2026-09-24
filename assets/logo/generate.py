import math, os, struct, subprocess
OUT = os.path.dirname(os.path.abspath(__file__))
os.makedirs(OUT + "/png", exist_ok=True)

DARK = dict(line="#dce0e5", acc="#74ade8", bg="#282c33")
LIGHT = dict(line="#1f2329", acc="#2f6fb5", bg="#fafaf9")

def hexpts(cx, cy, r):
    return [(cx + r*math.cos(math.radians(a)), cy + r*math.sin(math.radians(a))) for a in (-90, -30, 30, 90, 150, 210)]

def mark(line, acc, sw=4.5, cx=32, cy=32, r=24, arm=0.79):
    p = hexpts(cx, cy, r)
    hexd = "M" + " L".join(f"{x:.2f},{y:.2f}" for x, y in p) + " Z"
    ul, ur, bo = p[5], p[1], p[3]
    e = lambda v: (cx + (v[0]-cx)*arm, cy + (v[1]-cy)*arm)
    a, b, c = e(ul), e(ur), e(bo)
    yd = f"M{a[0]:.2f},{a[1]:.2f} L{cx},{cy} L{b[0]:.2f},{b[1]:.2f} M{cx},{cy} L{c[0]:.2f},{c[1]:.2f}"
    return (f'<path d="{hexd}" fill="none" stroke="{line}" stroke-width="{sw}" stroke-linejoin="round"/>'
            f'<path d="{yd}" fill="none" stroke="{acc}" stroke-width="{sw}" stroke-linecap="round" stroke-linejoin="round"/>')

def word(line, acc, sw=5):
    base = f'fill="none" stroke-width="{sw}" stroke-linecap="round" stroke-linejoin="round"'
    return (f'<g stroke="{line}" {base}>'
            '<path d="M2,8 V48 M18,22 L3,36 M9,30 L19,48"/>'          # k
            '<path d="M28,22 V37 A9.5,9.5 0 0 0 47,37 M47,22 V48"/>'   # u
            '<path d="M58,8 V48"/><circle cx="71" cy="35" r="13"/>'   # b
            '<path d="M124,8 V48"/>'                                   # l
            f'</g><path d="M92,22 L103,43 M114,22 L97,60" stroke="{acc}" {base}/>')  # y

def svg(w, h, body, vb=None):
    vb = vb or f"0 0 {w} {h}"
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="{vb}">{body}</svg>\n'

def save(name, s):
    open(f"{OUT}/{name}", "w").write(s)

# marks
save("kubyl-mark.svg", svg(64, 64, mark(**{k: DARK[k] for k in ("line", "acc")})))
save("kubyl-mark-light.svg", svg(64, 64, mark(LIGHT["line"], LIGHT["acc"])))
save("kubyl-mark-mono.svg", svg(64, 64, mark("currentColor", "currentColor")))
save("kubyl-mark-small.svg", svg(64, 64, mark(DARK["line"], DARK["acc"], sw=6.5, r=25, arm=0.74)))  # tuned for 16–32 px

# wordmark + lockups
save("kubyl-wordmark.svg", svg(130, 64, f'<g transform="translate(3,0)">{word(DARK["line"], DARK["acc"])}</g>', "0 0 132 64"))
save("kubyl-wordmark-light.svg", svg(130, 64, f'<g transform="translate(3,0)">{word(LIGHT["line"], LIGHT["acc"])}</g>', "0 0 132 64"))
for suf, pal in (("", DARK), ("-light", LIGHT)):
    body = mark(pal["line"], pal["acc"]) + f'<g transform="translate(80,4) scale(0.88)">{word(pal["line"], pal["acc"])}</g>'
    save(f"kubyl-lockup{suf}.svg", svg(200, 64, body, "0 0 200 64"))

# app icon (macOS-style squircle-ish rounded square, 1024 grid, 824 content box)
def appicon(small=False):
    m = mark(DARK["line"], DARK["acc"], sw=6.5 if small else 4.5, r=25 if small else 24, arm=0.74 if small else 0.79)
    s = 10.5 if small else 9.2
    return svg(1024, 1024,
        '<rect x="100" y="100" width="824" height="824" rx="185" fill="#282c33"/>'
        '<rect x="102" y="102" width="820" height="820" rx="183" fill="none" stroke="#464b57" stroke-width="4"/>'
        f'<g transform="translate({512-32*s},{512-32*s}) scale({s})">{m}</g>')
save("kubyl-app-icon.svg", appicon())
save("kubyl-app-icon-small.svg", appicon(True))
# full-bleed square (Windows / Linux / social avatars)
save("kubyl-avatar.svg", svg(512, 512, f'<rect width="512" height="512" fill="#282c33"/><g transform="translate(96,96) scale(5)">{mark(DARK["line"], DARK["acc"])}</g>'))
save("favicon.svg", svg(32, 32, '<style>.l{stroke:#1f2329}.a{stroke:#2f6fb5}@media (prefers-color-scheme:dark){.l{stroke:#dce0e5}.a{stroke:#74ade8}}</style>'
     + mark("x", "y", sw=6.5, r=25, arm=0.74).replace('stroke="x"', 'class="l"').replace('stroke="y"', 'class="a"'), "0 0 64 64"))

# rasterize
def png(src, px, dst, w=None, h=None):
    subprocess.run(["rsvg-convert", "-w", str(w or px), "-h", str(h or px), f"{OUT}/{src}", "-o", dst], check=True)

sizes = [16, 32, 48, 64, 128, 256, 512, 1024]
for px in sizes:
    png("kubyl-app-icon-small.svg" if px <= 48 else "kubyl-app-icon.svg", px, f"{OUT}/png/app-icon-{px}.png")
    png("kubyl-avatar.svg", px, f"{OUT}/png/avatar-{px}.png")
png("kubyl-lockup.svg", 0, f"{OUT}/png/lockup-dark-1200.png", 1200, 384)
png("kubyl-lockup-light.svg", 0, f"{OUT}/png/lockup-light-1200.png", 1200, 384)

# macOS .icns
iset = os.path.join(OUT, ".build", "kubyl.iconset")
os.makedirs(iset, exist_ok=True)
for base in (16, 32, 128, 256, 512):
    for sc in (1, 2):
        px = base * sc
        png("kubyl-app-icon-small.svg" if px <= 48 else "kubyl-app-icon.svg", px, f"{iset}/icon_{base}x{base}{'@2x' if sc == 2 else ''}.png")
subprocess.run(["iconutil", "-c", "icns", iset, "-o", f"{OUT}/kubyl.icns"], check=True)

# Windows .ico (PNG-embedded entries; full-bleed avatar reads better in the taskbar)
ents = [16, 24, 32, 48, 64, 128, 256]
blobs = []
for px in ents:
    p = f"{iset}/ico{px}.png"
    subprocess.run(["rsvg-convert", "-w", str(px), "-h", str(px), f"{OUT}/kubyl-mark-small.svg" if px <= 48 else f"{OUT}/kubyl-avatar.svg", "-o", p], check=True)
    if px <= 48:  # small sizes: mark on dark rounded tile
        t = f'<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><rect width="64" height="64" rx="12" fill="#282c33"/>{mark(DARK["line"], DARK["acc"], sw=6.5, r=25, arm=0.74)}</svg>'
        open(p + ".svg", "w").write(t)
        subprocess.run(["rsvg-convert", "-w", str(px), "-h", str(px), p + ".svg", "-o", p], check=True)
    blobs.append(open(p, "rb").read())
hdr = struct.pack("<HHH", 0, 1, len(ents)); off = 6 + 16 * len(ents); dirs = b""
for px, b in zip(ents, blobs):
    dirs += struct.pack("<BBBBHHII", px % 256, px % 256, 0, 0, 1, 32, len(b), off); off += len(b)
open(f"{OUT}/kubyl.ico", "wb").write(hdr + dirs + b"".join(blobs))

# preview sheet
prev = svg(1200, 760,
  '<rect width="1200" height="760" fill="#1d1f24"/><rect x="600" width="600" height="380" fill="#fafaf9"/><rect y="380" width="1200" height="380" fill="#282c33"/>'
  f'<g transform="translate(90,126) scale(2)">{mark(DARK["line"], DARK["acc"]) + "<g transform=\"translate(80,4) scale(0.88)\">" + word(DARK["line"], DARK["acc"]) + "</g>"}</g>'
  f'<g transform="translate(690,126) scale(2)">{mark(LIGHT["line"], LIGHT["acc"]) + "<g transform=\"translate(80,4) scale(0.88)\">" + word(LIGHT["line"], LIGHT["acc"]) + "</g>"}</g>'
  f'<g transform="translate(80,440) scale(0.25)">{appicon()[appicon().index(">")+1:-7]}</g>'
  + "".join(f'<g transform="translate({400+i*130},{560-s/2}) scale({s/1024})">{appicon(s<=48)[appicon(s<=48).index(">")+1:-7]}</g>' for i, s in enumerate([128, 64, 48, 32, 16]))
  )
save("preview.svg", prev)
png("preview.svg", 0, f"{OUT}/preview.png", 1200, 760)
import shutil; shutil.rmtree(os.path.join(OUT, ".build"), ignore_errors=True)
print("done")
