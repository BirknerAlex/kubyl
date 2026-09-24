import json, math, os

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "out", "project")
os.makedirs(ROOT, exist_ok=True)
W, H = 1440, 900

ICONS = {
 "search": '<circle cx="11" cy="11" r="7"></circle><path d="m20 20-3.5-3.5"></path>',
 "cr": '<path d="m9 18 6-6-6-6"></path>',
 "cd": '<path d="m6 9 6 6 6-6"></path>',
 "box": '<path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"></path><path d="m3.3 7 8.7 5 8.7-5"></path><path d="M12 22V12"></path>',
 "layers": '<path d="m12 2 10 5-10 5L2 7z"></path><path d="m2 17 10 5 10-5"></path><path d="m2 12 10 5 10-5"></path>',
 "server": '<rect x="2" y="3" width="20" height="8" rx="2"></rect><rect x="2" y="13" width="20" height="8" rx="2"></rect><path d="M6 7h.01M6 17h.01"></path>',
 "globe": '<circle cx="12" cy="12" r="10"></circle><path d="M2 12h20M12 2a15 15 0 0 1 0 20 15 15 0 0 1 0-20"></path>',
 "file": '<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><path d="M14 2v6h6"></path>',
 "sliders": '<path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6"></path>',
 "plus": '<path d="M12 5v14M5 12h14"></path>',
 "minus": '<path d="M5 12h14"></path>',
 "play": '<path d="m6 3 14 9-14 9z"></path>',
 "pause": '<rect x="6" y="4" width="4" height="16"></rect><rect x="14" y="4" width="4" height="16"></rect>',
 "terminal": '<path d="m4 17 6-6-6-6M12 19h8"></path>',
 "download": '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3"></path>',
 "filter": '<path d="M22 3H2l8 9.46V19l4 2v-8.54z"></path>',
 "refresh": '<path d="M21 12a9 9 0 1 1-3-6.7L21 8"></path><path d="M21 3v5h-5"></path>',
 "alert": '<path d="m21.73 18-8-14a2 2 0 0 0-3.46 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"></path><path d="M12 9v4M12 17h.01"></path>',
 "ok": '<circle cx="12" cy="12" r="10"></circle><path d="m9 12 2 2 4-4"></path>',
 "err": '<circle cx="12" cy="12" r="10"></circle><path d="m15 9-6 6M9 9l6 6"></path>',
 "info": '<circle cx="12" cy="12" r="10"></circle><path d="M12 16v-4M12 8h.01"></path>',
 "key": '<circle cx="7.5" cy="15.5" r="5.5"></circle><path d="m21 2-9.6 9.6M15.5 7.5l3 3L22 7l-3-3"></path>',
 "lock": '<rect x="3" y="11" width="18" height="11" rx="2"></rect><path d="M7 11V7a5 5 0 0 1 10 0v4"></path>',
 "cloud": '<path d="M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z"></path>',
 "activity": '<path d="M22 12h-4l-3 9L9 3l-3 9H2"></path>',
 "cpu": '<rect x="4" y="4" width="16" height="16" rx="2"></rect><rect x="9" y="9" width="6" height="6"></rect><path d="M15 2v2M15 20v2M2 15h2M2 9h2M20 15h2M20 9h2M9 2v2M9 20v2"></path>',
 "db": '<ellipse cx="12" cy="5" rx="9" ry="3"></ellipse><path d="M3 5v14a9 3 0 0 0 18 0V5M3 12a9 3 0 0 0 18 0"></path>',
 "bell": '<path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.94 1.94 0 0 0 3.4 0"></path>',
 "list": '<path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"></path>',
 "code": '<path d="m16 18 6-6-6-6M8 6l-6 6 6 6"></path>',
 "eye": '<path d="M2 12s3-7 10-7 10 7 10 7-3 7-10 7S2 12 2 12"></path><circle cx="12" cy="12" r="3"></circle>',
 "trash": '<path d="M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path>',
 "link": '<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"></path><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"></path>',
 "folder": '<path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"></path>',
 "split": '<rect x="3" y="3" width="18" height="18" rx="2"></rect><path d="M12 3v18"></path>',
 "max": '<path d="M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7"></path>',
 "more": '<circle cx="5" cy="12" r="1"></circle><circle cx="12" cy="12" r="1"></circle><circle cx="19" cy="12" r="1"></circle>',
 "left": '<path d="m12 19-7-7 7-7M19 12H5"></path>',
 "right": '<path d="M5 12h14M12 5l7 7-7 7"></path>',
 "shield": '<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10"></path>',
 "blocks": '<rect x="3" y="3" width="7" height="7" rx="1"></rect><rect x="14" y="3" width="7" height="7" rx="1"></rect><rect x="3" y="14" width="7" height="7" rx="1"></rect><path d="M14 17.5h7M17.5 14v7"></path>',
 "clock": '<circle cx="12" cy="12" r="10"></circle><path d="M12 6v6l4 2"></path>',
 "diff": '<path d="M12 3v14M5 10h14M5 21h14"></path>',
 "user": '<circle cx="12" cy="8" r="4"></circle><path d="M20 21a8 8 0 0 0-16 0"></path>',
 "upload": '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M17 8l-5-5-5 5M12 3v12"></path>',
 "wrap": '<path d="M3 6h18M3 12h15a3 3 0 1 1 0 6h-4M16 16l-2 2 2 2M3 18h7"></path>',
 "x": '<path d="M18 6 6 18M6 6l12 12"></path>',
 "wheel": '<circle cx="12" cy="12" r="8"></circle><circle cx="12" cy="12" r="2"></circle><path d="M12 2v7.5M12 14.5V22M22 12h-7.5M9.5 12H2M4.93 4.93l5.3 5.3M13.77 13.77l5.3 5.3M19.07 4.93l-5.3 5.3M10.23 13.77l-5.3 5.3"></path>',
 "network": '<rect x="16" y="16" width="6" height="6" rx="1"></rect><rect x="2" y="16" width="6" height="6" rx="1"></rect><rect x="9" y="2" width="6" height="6" rx="1"></rect><path d="M5 16v-3a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v3M12 12V8"></path>',
 "drive": '<path d="M22 12H2M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11zM6 16h.01M10 16h.01"></path>',
 "zap": '<path d="M13 2 3 14h9l-1 8 10-12h-9z"></path>',
 "star": '<path d="m12 2 3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01z"></path>',
 "gauge": '<path d="m12 14 4-4"></path><path d="M3.34 19a10 10 0 1 1 17.32 0"></path>',
 "up": '<path d="M12 19V5M5 12l7-7 7 7"></path>',
 "copy": '<rect x="9" y="9" width="13" height="13" rx="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>',
 "store": '<path d="M3 9h18l-2-5H5zM4 9v11h16V9M9 20v-6h6v6"></path>',
 "gear": '<circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>',
}

def ic(n, s=14, c="currentColor", sw=1.75, fill="none"):
    return (f'<svg width="{s}" height="{s}" viewBox="0 0 24 24" fill="{fill}" stroke="{c}" stroke-width="{sw}" '
            f'stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" style="flex-shrink: 0">{ICONS[n]}</svg>')

C = dict(bg="#282c33", panel="#2f343e", elev="#3b414d", border="#464b57", bv="#363c46", text="#dce0e5",
         muted="#a9afbc", dim="#959aa6", faint="#5d636f", accent="#74ade8", green="#a1c181", red="#d07277",
         yellow="#dec184", purple="#b477cf", cyan="#6eb4bf", orange="#bf956a")

CSS = """
:root{--bg:#282c33;--panel:#2f343e;--elev:#3b414d;--border:#464b57;--bv:#363c46;--text:#dce0e5;--muted:#a9afbc;--dim:#959aa6;--faint:#5d636f;--accent:#74ade8;--green:#a1c181;--red:#d07277;--yellow:#dec184;--purple:#b477cf;--cyan:#6eb4bf;--orange:#bf956a;--sel:#363c48;--hover:#343944}
body{margin:0;background:#1d1f24}
a{color:#74ade8}a:hover{color:#9cc6f0}
.app{width:1440px;height:900px;display:flex;flex-direction:column;background:var(--bg);color:var(--text);font-family:'IBM Plex Sans',system-ui,sans-serif;font-size:13px;overflow:hidden;position:relative;border-radius:10px;border:1px solid #4b5160;box-sizing:border-box}
.mono{font-family:'IBM Plex Mono',ui-monospace,monospace}
button{font:inherit;color:inherit;background:none;border:0;padding:0;cursor:pointer}
input{font:inherit;color:inherit}
.tb{height:38px;flex-shrink:0;display:flex;align-items:center;gap:6px;padding:0 10px 0 14px;background:var(--elev);border-bottom:1px solid var(--border)}
.lights{display:flex;gap:8px;margin-right:12px}.lights span{width:12px;height:12px;border-radius:50%;display:block}
.tbb{display:flex;align-items:center;gap:6px;height:26px;padding:0 8px;border-radius:5px;color:var(--text)}
.tbb:hover{background:#454b58}
.tbsep{color:var(--faint)}
.search{display:flex;align-items:center;gap:8px;height:26px;width:420px;padding:0 8px;border-radius:6px;background:#2f343e;border:1px solid var(--border);color:var(--dim)}
.kbd{font-family:'IBM Plex Mono',monospace;font-size:11px;color:var(--muted);padding:1px 5px;border-radius:4px;border:1px solid var(--border);background:#343944;line-height:15px}
.prod{font-size:10.5px;font-weight:600;letter-spacing:.06em;color:#1e2127;background:var(--red);padding:1px 6px;border-radius:3px}
.body{flex:1;display:flex;min-height:0}
.side{width:268px;flex-shrink:0;background:var(--panel);border-right:1px solid var(--border);display:flex;flex-direction:column;overflow:hidden}
.phead{height:34px;flex-shrink:0;display:flex;align-items:center;gap:6px;padding:0 8px 0 12px;color:var(--muted);font-size:12px}
.sec{height:24px;display:flex;align-items:center;gap:6px;padding:0 12px;font-size:11px;font-weight:600;letter-spacing:.06em;text-transform:uppercase;color:var(--dim)}
.ti{height:23px;display:flex;align-items:center;gap:6px;padding-right:10px;color:var(--muted);white-space:nowrap;box-sizing:border-box}
.ti .n{flex:1;overflow:hidden;text-overflow:ellipsis}
.ti .c{font-size:11.5px;color:var(--dim);font-family:'IBM Plex Mono',monospace}
.ti.on{background:var(--sel);color:var(--text);outline:1px solid var(--accent);outline-offset:-1px}
.ti.root{color:var(--text);font-weight:500}
.dot{width:7px;height:7px;border-radius:50%;display:inline-block;flex-shrink:0}
.main{flex:1;display:flex;flex-direction:column;min-width:0}
.tabs{height:34px;flex-shrink:0;display:flex;align-items:stretch;background:var(--panel);border-bottom:1px solid var(--border)}
.tab{display:flex;align-items:center;gap:7px;padding:0 14px;border-right:1px solid var(--border);color:var(--dim);white-space:nowrap}
.tab.on{background:var(--bg);color:var(--text);margin-bottom:-1px}
.tabx{margin-left:4px;color:var(--faint)}
.tabtools{margin-left:auto;display:flex;align-items:center;gap:2px;padding:0 8px;color:var(--dim)}
.ib{width:26px;height:26px;display:flex;align-items:center;justify-content:center;border-radius:5px;color:var(--dim)}
.ib:hover{background:var(--hover)}
.tool{height:40px;flex-shrink:0;display:flex;align-items:center;gap:8px;padding:0 12px;border-bottom:1px solid var(--bv)}
.crumb{display:flex;align-items:center;gap:6px;color:var(--dim)}.crumb b{color:var(--text);font-weight:500}
.inp{display:flex;align-items:center;gap:7px;height:26px;padding:0 8px;border-radius:5px;background:#2b3038;border:1px solid var(--border);color:var(--dim);box-sizing:border-box}
.inp.focus{border-color:var(--accent)}
.btn{display:flex;align-items:center;gap:6px;height:26px;padding:0 10px;border-radius:5px;border:1px solid var(--border);background:#343944;color:var(--text);white-space:nowrap;box-sizing:border-box}
.btn.g{border-color:transparent;background:none;color:var(--muted)}
.btn.p{background:var(--accent);border-color:var(--accent);color:#1b1e24;font-weight:600}
.btn.d{color:var(--red)}
.chip{display:inline-flex;align-items:center;gap:5px;height:20px;padding:0 7px;border-radius:4px;background:#353a45;color:var(--muted);font-size:11.5px;white-space:nowrap;box-sizing:border-box}
.chip.on{background:#2d3b4d;color:#a8cdf3;outline:1px solid #3f5a78}
.mchip{font-family:'IBM Plex Mono',monospace;font-size:11px}
.status{height:28px;flex-shrink:0;display:flex;align-items:center;gap:14px;padding:0 10px;background:var(--panel);border-top:1px solid var(--border);color:var(--dim);font-size:12px;white-space:nowrap}
.status span{display:flex;align-items:center;gap:5px}
.hints{height:28px;flex-shrink:0;display:flex;align-items:center;gap:14px;padding:0 12px;border-top:1px solid var(--bv);background:#2a2e36;font-size:12px;color:var(--dim);white-space:nowrap;overflow:hidden}
.hints b{font-family:'IBM Plex Mono',monospace;font-weight:500;color:var(--accent);margin-right:5px}
.th{display:grid;align-items:center;height:28px;padding:0 12px;font-size:11.5px;color:var(--dim);border-bottom:1px solid var(--bv);background:#2a2e36;white-space:nowrap}
.tr{display:grid;align-items:center;height:30px;padding:0 12px;border-bottom:1px solid #2e333b;white-space:nowrap}
.tr > *{overflow:hidden;text-overflow:ellipsis}
.tr.on{background:var(--sel);outline:1px solid var(--accent);outline-offset:-1px}
.tr .mono,.td{font-size:12px}
.dock{flex-shrink:0;background:var(--panel);border-left:1px solid var(--border);display:flex;flex-direction:column;overflow:hidden}
.dsec{padding:12px 14px;border-bottom:1px solid var(--bv)}
.dtitle{font-size:11px;font-weight:600;letter-spacing:.06em;text-transform:uppercase;color:var(--dim);margin:0 0 8px}
.kv{display:grid;grid-template-columns:104px minmax(0,1fr);row-gap:5px;column-gap:8px;font-size:12px}
.kv dt{color:var(--dim)}.kv dd{margin:0;color:var(--text);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.bar{height:5px;border-radius:3px;background:#3a3f4a;overflow:hidden}.bar i{display:block;height:100%;border-radius:3px}
.pill{display:inline-flex;align-items:center;gap:6px}
.card{background:var(--panel);border:1px solid var(--border);border-radius:8px}
.lg{font-family:'IBM Plex Mono',monospace;font-size:12px;line-height:20px;white-space:pre;display:flex;gap:12px;padding:0 12px}
.hl{background:#6b5a2a;color:#f5e2a8;border-radius:2px}
.hl2{background:#8a7331;color:#fff4cf;border-radius:2px;outline:1px solid #dec184}
h1,h2,h3,p{margin:0}
"""

HEAD = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{title}</title>
<script src="./support.js"></script>
</head>
<body>
<x-dc>
<helmet>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link href="https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500&amp;family=IBM+Plex+Sans:wght@400;500;600&amp;display=swap" rel="stylesheet">
<style>{css}</style>
</helmet>
"""
TAIL = """
</x-dc>
<script type="text/x-dc" data-dc-script data-props='{"$preview":{"width":1440,"height":900}}'>
class Component extends DCLogic {
renderVals() {
return {};
}
}
</script>
</body>
</html>
"""

def page(title, inner):
    return HEAD.format(title=title, css=CSS) + inner + TAIL

def dot(c): return f'<span class="dot" style="background:{c}"></span>'

# ---------- chrome ----------
def titlebar(cluster="prod-eu-west-1", ns="payments", prod=True, meta="EKS · v1.30.4"):
    p = '<span class="prod">PROD</span>' if prod else ''
    return f'''<header class="tb">
<div class="lights"><span style="background:#ff5f57"></span><span style="background:#febc2e"></span><span style="background:#28c840"></span></div>
<button class="tbb" aria-label="Switch cluster">{ic("wheel",15,C["accent"])}<span style="font-weight:600">{cluster}</span>{p}{ic("cd",12,C["dim"])}</button>
<span class="tbsep">/</span>
<button class="tbb" aria-label="Switch namespace">{ic("folder",14,C["dim"])}<span>{ns}</span>{ic("cd",12,C["dim"])}</button>
<div style="flex:1"></div>
<button class="search">{ic("search",13)}<span style="flex:1;text-align:left">Search resources, commands, contexts…</span><span class="kbd">⌘K</span></button>
<div style="flex:1"></div>
<span style="display:flex;align-items:center;gap:6px;color:var(--dim);font-size:12px;margin-right:6px">{dot(C["green"])}{meta}</span>
<button class="ib" aria-label="Notifications">{ic("bell",14)}</button>
<button aria-label="Account" style="width:24px;height:24px;border-radius:50%;background:#4a6a8a;color:#e8f0f8;font-size:10.5px;font-weight:600;display:flex;align-items:center;justify-content:center">AB</button>
</header>'''

def statusbar(left_extra="", right_extra=""):
    return f'''<footer class="status">
<span>{ic("split",13)}</span>
<span style="color:var(--text)">{dot(C["green"])}prod-eu-west-1</span>
<span>{ic("folder",12)}payments</span>
<span>{ic("alert",12,C["yellow"])}<span style="color:var(--yellow)">3</span>{ic("err",12,C["red"])}<span style="color:var(--red)">2</span></span>
{left_extra}
<span style="flex:1"></span>
{right_extra}
<span>{ic("activity",12,C["green"])}Prometheus</span>
<span>{ic("zap",12)}14 watches</span>
<span>{ic("terminal",12)}</span>
<span class="mono" style="font-size:11.5px">kube-rs · GPUI</span>
</footer>'''

def ti(label, depth=0, icon=None, count=None, open_=None, on=False, root=False, color=None, extra=""):
    pad = 8 + depth * 14
    chev = ""
    if open_ is True: chev = ic("cd", 12, C["dim"])
    elif open_ is False: chev = ic("cr", 12, C["dim"])
    else: chev = '<span style="width:12px;flex-shrink:0"></span>'
    ico = ic(icon, 14, color or C["dim"]) if icon else ""
    cnt = f'<span class="c">{count}</span>' if count is not None else ""
    cls = "ti" + (" on" if on else "") + (" root" if root else "")
    return f'<div class="{cls}" style="padding-left:{pad}px">{chev}{ico}<span class="n">{label}</span>{extra}{cnt}</div>'

def sidebar(active="Pods", cr_open=True):
    a = lambda n: n == active
    fav = lambda ns, cl, col, src: (f'<div class="ti{" on" if active=="fav:"+ns+cl else ""}" style="padding-left:12px">{ic("star",13,C["yellow"],1.6,C["yellow"])}'
                                   f'<span class="n"><span style="color:var(--text)">{ns}</span> <span style="color:var(--dim)">· {cl}</span></span>'
                                   f'<span title="{src}" style="display:flex">{dot(col)}</span></div>')
    rows = [
        f'<div class="phead"><span style="flex:1;font-weight:500;color:var(--text)">Explorer</span><button class="ib" aria-label="Filter kinds">{ic("search",13)}</button><button class="ib" aria-label="Add kubeconfig">{ic("plus",14)}</button><button class="ib" aria-label="More">{ic("more",14)}</button></div>',
        f'<div class="sec">{ic("cd",11)}Favorites<span style="flex:1"></span><span style="font-weight:400;letter-spacing:0;text-transform:none;color:var(--faint)">4</span></div>',
        fav("payments", "prod-eu-west-1", C["red"], "~/work/kube/eks-prod.yaml"),
        fav("payments", "staging-eu-west-1", C["yellow"], "~/.kube/config"),
        fav("checkout", "gke-analytics", C["cyan"], "~/Downloads/gke-analytics.yaml"),
        fav("ingress", "platform-onprem", C["purple"], "~/work/kube/platform-onprem.yaml"),
        '<div style="height:6px"></div>',
        f'<div class="sec">{ic("cd",11)}Clusters</div>',
        ti("prod-eu-west-1", 0, "wheel", open_=True, root=True, color=C["red"], extra='<span class="prod" style="font-size:9.5px;padding:0 4px">PROD</span>'),
        ti("Overview", 1, "gauge", on=a("Overview")),
        ti("Events", 1, "bell", "23", on=a("Events"), color=C["yellow"] if not a("Events") else None),
        ti("Workloads", 1, open_=True),
        ti("Pods", 2, "box", "17", on=a("Pods")),
        ti("Deployments", 2, "layers", "9", on=a("Deployments")),
        ti("StatefulSets", 2, "db", "2"),
        ti("DaemonSets", 2, "server", "0"),
        ti("Jobs &amp; CronJobs", 2, "clock", "6"),
        ti("Network", 1, open_=False),
        ti("Config &amp; Secrets", 1, open_=False),
        ti("Storage", 1, open_=False),
        ti("Access Control", 1, open_=False),
        ti("Cluster", 1, open_=False),
        ti("Administration", 1, open_=True),
        ti("Installed Operators", 2, "blocks", "7", on=a("Operators")),
        ti("OperatorHub", 2, "store"),
        ti("Cluster Updates", 2, "up", extra=f'<span style="margin-right:2px">{dot(C["accent"])}</span>', on=a("Updates")),
        ti("Custom Resources", 1, open_=True),
        ti("cert-manager.io", 2, open_=True),
        ti("Certificates", 3, "file", "12", on=a("Certificates")),
        ti("Issuers", 3, "file", "3"),
        ti("monitoring.coreos.com", 2, open_=False),
        ti('<span style="color:var(--dim)">14 more API groups…</span>', 2),
        ti("staging-eu-west-1", 0, "wheel", open_=False, root=True, color=C["yellow"]),
        ti("gke-analytics", 0, "wheel", open_=False, root=True, color=C["cyan"]),
        ti("platform-onprem", 0, "wheel", open_=False, root=True, color=C["purple"], extra=f'<span style="color:var(--yellow);display:flex" title="Sign-in required">{ic("key",12,C["yellow"])}</span>'),
        ti("homelab-k3s", 0, "wheel", open_=False, root=True, color=C["faint"], extra=f'<span style="color:var(--red);font-size:11px">offline</span>'),
    ]
    return '<aside class="side">' + "\n".join(rows) + '</aside>'

def tabs(items, tools=True):
    out = []
    for it in items:
        icon, label, on = it[0], it[1], it[2]
        dirty = len(it) > 3 and it[3]
        mark = f'<span class="dot" style="background:{C["accent"]};margin-left:4px"></span>' if dirty else f'<span class="tabx">{ic("x",11)}</span>'
        out.append(f'<div class="tab{" on" if on else ""}">{ic(icon,13,C["accent"] if on else C["dim"])}<span>{label}</span>{mark}</div>')
    t = f'<div class="tabtools"><button class="ib" aria-label="New tab">{ic("plus",14)}</button><button class="ib" aria-label="Split">{ic("split",14)}</button><button class="ib" aria-label="Zoom">{ic("max",13)}</button></div>' if tools else ""
    return '<div class="tabs">' + "".join(out) + t + '</div>'

def hints(items):
    return '<div class="hints">' + "".join(f'<span><b>{k}</b>{v}</span>' for k, v in items) + '</div>'

def st(status):
    m = {"Running": C["green"], "Succeeded": C["green"], "Completed": C["dim"], "Pending": C["yellow"],
         "ContainerCreating": C["accent"], "CrashLoopBackOff": C["red"], "OOMKilled": C["red"], "Error": C["red"],
         "Installing": C["accent"], "Failed": C["red"], "Upgrade available": C["yellow"], "Ready": C["green"], "NotReady": C["red"]}
    c = m.get(status, C["muted"])
    return f'<span class="pill">{dot(c)}<span style="color:{c}">{status}</span></span>'

def bar(pct, c=None, w=None):
    c = c or (C["red"] if pct >= 85 else C["yellow"] if pct >= 70 else C["accent"])
    ws = f'width:{w}px' if w else ''
    return f'<div class="bar" style="{ws}"><i style="width:{pct}%;background:{c}"></i></div>'

def spark(vals, w, h, c, fill=True, sw=1.5):
    mx, mn = max(vals), min(vals)
    rng = (mx - mn) or 1
    pts = [(i * w / (len(vals) - 1), h - 2 - (v - mn) / rng * (h - 4)) for i, v in enumerate(vals)]
    d = "M" + " L".join(f"{x:.1f},{y:.1f}" for x, y in pts)
    area = f'<path d="{d} L{w},{h} L0,{h} Z" fill="{c}" fill-opacity="0.12"></path>' if fill else ""
    return f'<svg width="{w}" height="{h}" viewBox="0 0 {w} {h}" aria-hidden="true">{area}<path d="{d}" fill="none" stroke="{c}" stroke-width="{sw}"></path></svg>'

def series(n, base, amp, seed, noise=0.25):
    return [base + amp * (math.sin(i / 3.1 + seed) * 0.6 + math.sin(i / 1.3 + seed * 2) * noise + math.sin(i / 7.0 + seed / 2) * 0.4) for i in range(n)]

def shell(active, tabbar, content):
    return f'''<div class="app">
{titlebar()}
<div class="body">
{sidebar(active)}
<main class="main">
{tabbar}
{content}
</main>
</div>
{statusbar()}
</div>'''

# ---------- 1. Pods ----------
PODS = [
 ("checkout-api-7d9f8c6b5-x2kqp", "2/2", "Running", 0, 184, 42, "312Mi", 61, "ip-10-0-12-41", "3d4h"),
 ("checkout-api-7d9f8c6b5-m8fzt", "2/2", "Running", 0, 171, 38, "298Mi", 58, "ip-10-0-14-7", "3d4h"),
 ("checkout-api-7d9f8c6b5-qj4wn", "2/2", "Running", 1, 203, 46, "330Mi", 64, "ip-10-0-11-92", "3d4h"),
 ("payment-gateway-5c8b7f9d4-hl2vp", "1/2", "CrashLoopBackOff", 14, 12, 3, "88Mi", 17, "ip-10-0-12-41", "47m"),
 ("payment-gateway-5c8b7f9d4-7tgxs", "2/2", "Running", 0, 96, 24, "204Mi", 40, "ip-10-0-14-7", "47m"),
 ("ledger-writer-0", "1/1", "Running", 0, 322, 64, "1.1Gi", 72, "ip-10-0-13-5", "12d"),
 ("ledger-writer-1", "1/1", "Running", 0, 298, 60, "1.0Gi", 68, "ip-10-0-11-92", "12d"),
 ("ledger-writer-2", "0/1", "Pending", 0, None, 0, "—", 0, "&lt;none&gt;", "2m"),
 ("fraud-scorer-6f77d8c9b-9zxkd", "0/1", "OOMKilled", 3, 402, 80, "1.9Gi", 97, "ip-10-0-13-5", "6h"),
 ("fraud-scorer-6f77d8c9b-wv5hn", "1/1", "Running", 0, 377, 75, "1.6Gi", 81, "ip-10-0-14-7", "6h"),
 ("invoice-renderer-84c6c7c9f-pp4rc", "0/1", "ContainerCreating", 0, None, 0, "—", 0, "ip-10-0-12-41", "8s"),
 ("settlement-batch-28791440-k7x2m", "0/1", "Completed", 0, None, 0, "—", 0, "ip-10-0-11-92", "4h"),
 ("settlement-batch-28791380-n5dqz", "0/1", "Completed", 0, None, 0, "—", 0, "ip-10-0-13-5", "5h"),
 ("currency-rates-28791455-q9w8e", "0/1", "Error", 0, None, 0, "—", 0, "ip-10-0-14-7", "19m"),
 ("redis-payments-master-0", "1/1", "Running", 0, 41, 20, "96Mi", 37, "ip-10-0-13-5", "12d"),
 ("redis-payments-replicas-0", "1/1", "Running", 0, 33, 16, "91Mi", 35, "ip-10-0-11-92", "12d"),
 ("webhook-relay-7b9c5d8f6-rrm4c", "1/1", "Running", 2, 18, 9, "64Mi", 25, "ip-10-0-12-41", "2d1h"),
]
POD_COLS = "grid-template-columns: minmax(0,1fr) 46px 138px 54px 92px 92px 108px 46px"

def pods_screen():
    head = f'<div class="th" style="{POD_COLS}"><span>NAME {ic("cd",10)}</span><span>READY</span><span>STATUS</span><span style="text-align:right;padding-right:10px">RESTARTS</span><span>CPU</span><span>MEMORY</span><span>NODE</span><span>AGE</span></div>'
    rows = []
    for i, (n, rd, s, r, cpu, cp, mem, mp, node, age) in enumerate(PODS):
        rc = C["red"] if r >= 3 else C["text"]
        cpu_cell = f'<div style="display:flex;flex-direction:column;gap:3px;padding-right:12px"><span class="mono">{cpu}m</span>{bar(cp)}</div>' if cpu else '<span class="mono" style="color:var(--faint)">—</span>'
        mem_cell = f'<div style="display:flex;flex-direction:column;gap:3px;padding-right:12px"><span class="mono">{mem}</span>{bar(mp)}</div>' if cpu else '<span class="mono" style="color:var(--faint)">—</span>'
        rows.append(f'<div class="tr{" on" if i==0 else ""}" style="{POD_COLS};height:32px"><span class="mono">{n}</span><span class="mono" style="color:{C["red"] if rd.split("/")[0]!=rd.split("/")[1] and s not in ("Completed",) else C["text"]}">{rd}</span>{st(s)}<span class="mono" style="text-align:right;padding-right:10px;color:{rc}">{r}</span>{cpu_cell}{mem_cell}<span class="mono" style="color:var(--muted)">{node}</span><span class="mono" style="color:var(--muted)">{age}</span></div>')
    toolbar = f'''<div class="tool">
<div class="crumb">{ic("box",14,C["accent"])}<b>Pods</b><span>·</span><span>17 in payments</span><span style="color:var(--red)">· 3 failing</span></div>
<div style="flex:1"></div>
<div class="inp focus" style="width:300px">{ic("filter",12)}<span class="mono" style="color:var(--text);font-size:12px">app in (checkout-api, payment-gateway)</span></div>
<div style="display:flex;gap:4px"><span class="chip on">payments {ic("x",10)}</span><span class="chip">+ namespace</span></div>
<button class="btn g" aria-label="Columns">{ic("sliders",13)}</button>
<span class="chip" style="color:var(--green)">{dot(C["green"])}live</span>
</div>'''
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{toolbar}
{head}
<div style="flex:1;overflow:hidden">{"".join(rows)}</div>
{hints([("l","Logs"),("s","Shell"),("d","Describe"),("e","Edit YAML"),("⇧f","Port-forward"),("⌃k","Kill"),("⌃d","Delete"),(":","Command"),("/","Filter")])}
</div>'''
    cpu = series(40, 180, 25, 1.2); mem = series(40, 300, 12, 2.3, 0.1)
    dock = f'''<aside class="dock" style="width:340px">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Pod details</span><button class="ib" aria-label="Pin">{ic("star",13)}</button><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec">
<div class="mono" style="font-size:12.5px;color:var(--text);margin-bottom:6px">checkout-api-7d9f8c6b5-x2kqp</div>
<div style="display:flex;gap:6px;flex-wrap:wrap">{st("Running")}<span class="chip">Burstable</span><span class="chip">10.0.12.188</span></div>
</div>
<div class="dsec"><p class="dtitle">Owner chain</p>
<div style="display:flex;align-items:center;gap:6px;font-size:12px;flex-wrap:wrap"><a href="#">{"Deployment"}</a><span style="color:var(--faint)">›</span><a href="#" class="mono" style="font-size:11.5px">checkout-api-7d9f8c6b5</a><span style="color:var(--faint)">›</span><span style="color:var(--muted)">Pod</span></div>
</div>
<div class="dsec"><p class="dtitle">Containers</p>
<div style="display:flex;flex-direction:column;gap:8px">
<div style="display:flex;gap:8px;align-items:flex-start">{ic("box",14,C["green"])}<div style="flex:1;min-width:0"><div style="display:flex;justify-content:space-between"><b style="font-weight:500">api</b><span style="color:var(--green);font-size:12px">running · 3d4h</span></div><div class="mono" style="font-size:11px;color:var(--dim);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">registry.example.com/payments/checkout-api:2.14.1</div><div style="font-size:11.5px;color:var(--dim)">:8080/TCP · :9090/TCP metrics</div></div></div>
<div style="display:flex;gap:8px;align-items:flex-start">{ic("box",14,C["green"])}<div style="flex:1;min-width:0"><div style="display:flex;justify-content:space-between"><b style="font-weight:500">istio-proxy</b><span style="color:var(--green);font-size:12px">running · 3d4h</span></div><div class="mono" style="font-size:11px;color:var(--dim)">docker.io/istio/proxyv2:1.23.2</div></div></div>
</div></div>
<div class="dsec"><p class="dtitle">Usage · last 1h <span style="text-transform:none;letter-spacing:0;font-weight:400">· Prometheus</span></p>
<div style="display:flex;flex-direction:column;gap:10px">
<div><div style="display:flex;justify-content:space-between;font-size:12px"><span style="color:var(--muted)">CPU</span><span class="mono" style="font-size:11.5px">184m <span style="color:var(--dim)">/ req 250m · lim 500m</span></span></div>{spark(cpu,310,34,C["accent"])}</div>
<div><div style="display:flex;justify-content:space-between;font-size:12px"><span style="color:var(--muted)">Memory</span><span class="mono" style="font-size:11.5px">312Mi <span style="color:var(--dim)">/ lim 512Mi</span></span></div>{spark(mem,310,34,C["purple"])}</div>
</div></div>
<div class="dsec"><p class="dtitle">Labels</p>
<div style="display:flex;gap:4px;flex-wrap:wrap"><span class="chip mchip">app=checkout-api</span><span class="chip mchip">version=2.14.1</span><span class="chip mchip">team=payments</span><span class="chip mchip">pod-template-hash=7d9f8c6b5</span></div></div>
<div class="dsec" style="border-bottom:0"><p class="dtitle">Conditions</p>
<div style="display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:4px;font-size:12px">{"".join(f'<span class="pill">{ic("ok",12,C["green"])}{c}</span>' for c in ["Initialized","Ready","ContainersReady","PodScheduled"])}</div></div>
</aside>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    tb = tabs([("box", "Pods", True), ("terminal", "checkout-api · logs", False), ("code", "api-tls.yaml", False, True), ("gauge", "Overview", False)])
    return page("Pods — Kubyl", shell("Pods", tb, content))

# ---------- 2. Logs + exec ----------
def logs_screen():
    pods = {"x2kqp": C["cyan"], "m8fzt": C["purple"], "qj4wn": C["orange"]}
    lv = {"INFO": C["green"], "WARN": C["yellow"], "ERROR": C["red"], "DEBUG": C["dim"]}
    L = [
     ("10:42:17.902", "m8fzt", "INFO", 'POST /v1/checkout 200 38ms  {"order":"ord_8F2kQ","items":3}'),
     ("10:42:18.114", "x2kqp", "INFO", 'GET  /v1/cart/u_1932 200 6ms'),
     ("10:42:18.231", "qj4wn", "WARN", 'payment-gateway slow response 1840ms  {"attempt":1}'),
     ("10:42:18.476", "x2kqp", "DEBUG", 'pool stats  {"active":14,"idle":6,"waiting":0}'),
     ("10:42:19.010", "qj4wn", "ERROR", 'upstream <TIMEOUT> after 2000ms calling payment-gateway:8443/authorize'),
     ("10:42:19.011", "qj4wn", "ERROR", '  retrying with backoff  {"attempt":2,"delay_ms":250}'),
     ("10:42:19.290", "m8fzt", "INFO", 'POST /v1/checkout 200 41ms  {"order":"ord_8F2kR","items":1}'),
     ("10:42:19.644", "x2kqp", "INFO", 'GET  /healthz 200 1ms'),
     ("10:42:20.102", "qj4wn", "WARN", 'circuit breaker half-open  {"target":"payment-gateway"}'),
     ("10:42:20.388", "x2kqp", "ERROR", 'upstream <TIMEOUT!> after 2000ms calling payment-gateway:8443/authorize'),
     ("10:42:20.390", "x2kqp", "INFO", 'POST /v1/checkout 502 2004ms  {"order":"ord_8F2kS"}'),
     ("10:42:20.917", "m8fzt", "INFO", 'GET  /v1/cart/u_4410 200 5ms'),
     ("10:42:21.206", "qj4wn", "INFO", 'POST /v1/checkout 200 44ms  {"order":"ord_8F2kT","items":2}'),
     ("10:42:21.552", "m8fzt", "DEBUG", 'cache hit ratio 0.93  {"keys":18842}'),
     ("10:42:21.870", "x2kqp", "WARN", 'retry budget 62% consumed  {"window":"60s"}'),
     ("10:42:22.031", "qj4wn", "INFO", 'GET  /healthz 200 1ms'),
    ]
    lines = []
    for t, p, l, msg in L:
        m = msg.replace("<TIMEOUT!>", '<span class="hl2">timeout</span>').replace("<TIMEOUT>", '<span class="hl">timeout</span>')
        # json-ish tint
        m = m.replace('{"', '<span style="color:var(--dim)">{"').replace('}', '}</span>') if '{"' in m else m
        bg = "background:#3a2e31;" if l == "ERROR" else ""
        lines.append(f'<div class="lg" style="{bg}"><span style="color:var(--faint)">{t}</span><span style="color:{pods[p]}">{p}</span><span style="color:{lv[l]};width:40px;display:inline-block">{l}</span><span style="color:var(--text)">{m}</span></div>')
    toolbar = f'''<div class="tool" style="gap:6px">
<div class="crumb">{ic("layers",14,C["accent"])}<b class="mono" style="font-size:12.5px">deployment/checkout-api</b></div>
<div style="display:flex;gap:4px;margin-left:4px">{"".join(f'<span class="chip mchip">{dot(c)}{p}</span>' for p,c in pods.items())}</div>
<span style="color:var(--faint)">|</span>
<button class="btn" style="height:24px">{ic("box",12)}api{ic("cd",11)}</button>
<button class="btn" style="height:24px">{ic("clock",12)}since 15m{ic("cd",11)}</button>
<div style="flex:1"></div>
<span class="chip on">{ic("play",10)}Follow</span><span class="chip on">Timestamps</span><span class="chip">{ic("wrap",11)}Wrap</span><span class="chip">Previous</span><span class="chip">JSON</span>
<button class="ib" aria-label="Download logs">{ic("download",14)}</button>
</div>
<div class="tool" style="height:38px;gap:8px;background:#2a2e36">
<div class="inp focus" style="width:320px">{ic("search",12)}<span class="mono" style="color:var(--text);font-size:12px">timeout</span><span style="flex:1"></span><span style="font-size:11.5px">2 of 27</span><span class="kbd">.*</span><span class="kbd">Aa</span></div>
<div style="display:flex;gap:4px"><span class="chip on" style="color:var(--red)">ERROR 27</span><span class="chip on" style="color:var(--yellow)">WARN 64</span><span class="chip on">INFO</span><span class="chip">DEBUG</span></div>
<div style="flex:1"></div>
<span style="font-size:12px;color:var(--dim)">{dot(C["green"])} streaming · 3 pods · 1,284 lines · 42/s</span>
</div>'''
    term = f'''<div style="height:268px;flex-shrink:0;border-top:1px solid var(--border);display:flex;flex-direction:column;background:var(--bg)">
<div class="tabs" style="height:32px">
<div class="tab on">{ic("terminal",13,C["accent"])}<span>exec · x2kqp/api · /bin/sh</span><span class="tabx">{ic("x",11)}</span></div>
<div class="tab">{ic("link",13)}<span>port-forward svc/ledger 5432 → localhost:15432</span><span class="tabx">{ic("x",11)}</span></div>
<div class="tabtools"><button class="ib" aria-label="New terminal">{ic("plus",14)}</button><button class="ib" aria-label="Maximize">{ic("max",13)}</button></div>
</div>
<div class="mono" style="padding:10px 14px;font-size:12.5px;line-height:20px;white-space:pre;color:var(--text)"><span style="color:var(--green)">/app $</span> env | grep PAYMENT_
<span style="color:var(--muted)">PAYMENT_GATEWAY_URL=https://payment-gateway.payments.svc:8443
PAYMENT_TIMEOUT_MS=2000</span>
<span style="color:var(--green)">/app $</span> wget -qO- localhost:9090/healthz
<span style="color:var(--muted)">{{"status":"degraded","db":"ok","gateway":"timeout","uptime":"3d4h12m"}}</span>
<span style="color:var(--green)">/app $</span> nslookup payment-gateway
<span style="color:var(--muted)">Name:    payment-gateway.payments.svc.cluster.local
Address: 172.20.41.117</span>
<span style="color:var(--green)">/app $</span> <span style="background:var(--accent);color:var(--bg)"> </span></div>
</div>'''
    center = f'<div style="flex:1;display:flex;flex-direction:column;min-width:0">{toolbar}<div style="flex:1;overflow:hidden;padding:6px 0">{"".join(lines)}</div>{term}</div>'
    stream = lambda icon, title, sub, col, act="": f'<div style="display:flex;gap:10px;padding:9px 14px;border-bottom:1px solid var(--bv)">{ic(icon,14,col)}<div style="flex:1;min-width:0"><div style="font-size:12.5px;color:var(--text);white-space:nowrap;overflow:hidden;text-overflow:ellipsis">{title}</div><div style="font-size:11.5px;color:var(--dim)">{sub}</div></div>{act}</div>'
    stop = f'<button class="ib" aria-label="Stop">{ic("x",12)}</button>'
    dock = f'''<aside class="dock" style="width:300px">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Active sessions</span><span class="c mono" style="font-size:11.5px">5</span></div>
{stream("list","logs · deployment/checkout-api","3 pods · follow · 42 lines/s",C["green"],stop)}
{stream("terminal","exec · x2kqp / api","/bin/sh · websocket · 4m",C["green"],stop)}
{stream("link","svc/ledger :5432 → :15432","port-forward · 2 conns",C["green"],stop)}
{stream("link","pod/redis-payments-master-0 :6379","port-forward · reconnecting…",C["yellow"],stop)}
{stream("bell","events · payments","watch · 23 warnings today",C["accent"],stop)}
<div class="dsec" style="border-bottom:0;margin-top:auto"><p class="dtitle">Pod events · x2kqp</p>
<div style="display:flex;flex-direction:column;gap:6px;font-size:12px">
<div style="display:flex;gap:6px">{ic("alert",12,C["yellow"])}<span><b style="font-weight:500">Unhealthy</b> <span style="color:var(--muted)">Readiness probe failed: 503</span> <span style="color:var(--dim)">×3 · 2m</span></span></div>
<div style="display:flex;gap:6px">{ic("info",12,C["dim"])}<span><b style="font-weight:500">Pulled</b> <span style="color:var(--muted)">image already present</span> <span style="color:var(--dim)">3d</span></span></div>
<div style="display:flex;gap:6px">{ic("info",12,C["dim"])}<span><b style="font-weight:500">Scheduled</b> <span style="color:var(--muted)">to ip-10-0-12-41</span> <span style="color:var(--dim)">3d</span></span></div>
</div></div>
</aside>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    tb = tabs([("box", "Pods", False), ("list", "checkout-api · logs", True), ("code", "api-tls.yaml", False, True), ("gauge", "Overview", False)])
    return page("Live logs and shell — Kubyl", shell("Pods", tb, content))

# ---------- 3. YAML editor ----------
def yaml_screen():
    K, S, N, CM, P = C["red"], C["green"], C["orange"], "#7f8591", C["text"]
    def k(key, ind=0, val=None, vc=S, dash=False, com=None):
        s = " " * ind + (f'<span style="color:var(--faint)">- </span>' if dash else "")
        if key: s += f'<span style="color:{K}">{key}</span><span style="color:var(--muted)">:</span>'
        if val is not None: s += (" " if key else "") + f'<span style="color:{vc}">{val}</span>'
        if com: s += f'  <span style="color:{CM};font-style:italic"># {com}</span>'
        return s
    L = [
     (k("apiVersion",0,"cert-manager.io/v1"), None),
     (k("kind",0,"Certificate"), None),
     (k("metadata"), None),
     (k("name",2,"api-tls"), None),
     (k("namespace",2,"payments"), None),
     (k("labels",2), None),
     (k("app.kubernetes.io/managed-by",4,"Helm"), None),
     ("lens", "managedFields hidden · helm, cert-manager-certificates-issuing, kubyl"),
     (k("spec"), None),
     (k("secretName",2,"api-tls"), None),
     (k("duration",2,"2160h", com="90d"), None),
     (k("renewBefore",2,"720h", com="30d"), "mod"),
     (k("commonName",2,"api.payments.example.com"), None),
     (k("dnsNames",2), None),
     (k(None,4,"api.payments.example.com",dash=True), None),
     (k(None,4,"checkout.payments.example.com",dash=True), "add"),
     (k("issuerRef",2), None),
     (k("name",4,"letsencrypt-prod"), None),
     (k("kind",4,"ClusterIssuer"), None),
     (k("group",4,"cert-manager.io"), None),
     (k("privateKey",2), None),
     (k("algorithm",4,"ECDSA"), None),
     (k("size",4,"256",N), None),
     ('    <span style="color:%s">rotationPolicy</span><span style="color:var(--muted)">:</span> <span style="color:%s;text-decoration:underline wavy %s;text-underline-offset:3px">Allways</span>' % (K, S, C["red"]), "err"),
     (k("usages",2), None),
     (k(None,4,"server auth",dash=True), None),
     (k(None,4,"digital signature",dash=True), None),
     ("lens", "status · read-only · live from watch"),
     ('<span style="opacity:.55">' + k("status") + '</span>', None),
     ('<span style="opacity:.55">' + k("notAfter",2,'"2026-12-01T08:14:52Z"') + '</span>', None),
     ('<span style="opacity:.55">' + k("renewalTime",2,'"2026-11-16T08:14:52Z"') + '</span>', None),
     ('<span style="opacity:.55">' + k("revision",2,"4",N) + '</span>', None),
    ]
    out, n = [], 0
    for text, mark in L:
        if text == "lens":
            out.append(f'<div style="display:flex;height:22px;align-items:center"><span style="width:56px"></span><span style="width:3px"></span><span style="padding-left:14px;font-size:11.5px;color:var(--dim);font-family:IBM Plex Sans">{mark}</span></div>')
            continue
        n += 1
        gut = {"mod": C["accent"], "add": C["green"], "err": C["accent"]}.get(mark, "transparent")
        active = "background:#2f343e;" if n == 11 else ""
        diag = f'<span style="margin-left:18px;padding:0 8px;border-radius:3px;background:#4a2f33;color:#f0b4b8;font-family:IBM Plex Sans;font-size:12px">Unsupported value “Allways”: supported values are “Never”, “Always”</span>' if mark == "err" else ""
        out.append(f'<div class="mono" style="display:flex;height:22px;align-items:center;font-size:13px;white-space:pre;{active}"><span style="width:44px;text-align:right;padding-right:12px;color:{"var(--text)" if n==11 else "var(--faint)"}">{n}</span><span style="width:3px;height:22px;background:{gut}"></span><span style="padding-left:14px">{text}</span>{diag}</div>')
    hover = f'''<div style="position:absolute;left:260px;top:318px;width:430px;background:#2f343e;border:1px solid var(--border);border-radius:7px;box-shadow:0 10px 30px rgba(0,0,0,.45);padding:12px 14px;font-size:12.5px;line-height:18px">
<div class="mono" style="font-size:12px;margin-bottom:6px"><span style="color:{C["red"]}">spec.renewBefore</span><span style="color:var(--dim)">: string (duration) · optional</span></div>
<p style="color:var(--muted)">How long before the certificate expires that renewal should start. Must be shorter than <span class="mono" style="font-size:11.5px;color:var(--text)">spec.duration</span>.</p>
<div style="margin-top:8px;padding-top:8px;border-top:1px solid var(--bv);color:var(--dim);font-size:11.5px">From CRD certificates.cert-manager.io · openAPIV3Schema v1</div>
</div>'''
    toolbar = f'''<div class="tool">
<div class="crumb mono" style="font-size:12px">{ic("file",13,C["accent"])}<span>cert-manager.io/v1</span><span>›</span><span>Certificate</span><span>›</span><b>payments/api-tls</b></div>
<span class="chip" style="color:var(--yellow)">3 changes vs live</span>
<div style="flex:1"></div>
<button class="btn g">{ic("refresh",13)}Revert</button>
<button class="btn">{ic("diff",13)}Diff vs live</button>
<button class="btn">{ic("eye",13)}Dry run</button>
<button class="btn p">{ic("upload",13,"#1b1e24")}Apply <span style="font-weight:400;opacity:.8">(server-side)</span></button>
</div>'''
    diff = f'''<div style="height:188px;flex-shrink:0;border-top:1px solid var(--border);display:flex;flex-direction:column">
<div class="tabs" style="height:32px"><div class="tab on">{ic("diff",13,C["accent"])}<span>Diff vs live</span></div><div class="tab">{ic("err",13,C["red"])}<span>Problems</span><span class="chip" style="height:17px;margin-left:4px">1</span></div><div class="tab">{ic("clock",13)}<span>Revision history</span></div></div>
<div class="mono" style="font-size:12.5px;line-height:21px;white-space:pre;padding:6px 0">
<div style="padding:0 14px;color:var(--dim)">@@ spec @@</div><div style="padding:0 14px;background:#3a2e31;color:#e7a9ad">-  renewBefore: 360h</div><div style="padding:0 14px;background:#2d3a2c;color:#bfd9a6">+  renewBefore: 720h  # 30d</div><div style="padding:0 14px;color:var(--muted)">   dnsNames:</div><div style="padding:0 14px;color:var(--muted)">     - api.payments.example.com</div><div style="padding:0 14px;background:#2d3a2c;color:#bfd9a6">+    - checkout.payments.example.com</div></div>
</div>'''
    editor = f'<div style="flex:1;overflow:hidden;position:relative;padding-top:6px">{"".join(out)}{hover}</div>'
    center = f'<div style="flex:1;display:flex;flex-direction:column;min-width:0">{toolbar}{editor}{diff}</div>'
    fld = lambda name, typ, depth=0, req=False, on=False: f'<div class="ti{" on" if on else ""}" style="padding-left:{12+depth*14}px"><span class="n mono" style="font-size:12px">{name}{"<span style=color:var(--red)> *</span>" if req else ""}</span><span class="mono" style="font-size:11px;color:var(--cyan)">{typ}</span></div>'
    rel = lambda icon, t, s: f'<div style="display:flex;gap:8px;align-items:center;font-size:12px;padding:3px 0">{ic(icon,13,C["dim"])}<a href="#" class="mono" style="font-size:11.5px">{t}</a><span style="color:var(--dim)">{s}</span></div>'
    dock = f'''<aside class="dock" style="width:300px">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Schema</span><span style="font-size:11.5px">from CRD</span></div>
<div style="padding:6px 0">
{fld("spec","object",0,True)}
{fld("secretName","string",1,True)}
{fld("issuerRef","object",1,True)}
{fld("commonName","string",1)}
{fld("dnsNames","[]string",1)}
{fld("duration","duration",1)}
{fld("renewBefore","duration",1,on=True)}
{fld("privateKey","object",1)}
{fld("algorithm","enum",2)}
{fld("rotationPolicy","enum",2)}
{fld("size","integer",2)}
{fld("usages","[]enum",1)}
{fld("keystores","object",1)}
{fld("secretTemplate","object",1)}
{fld("subject","object",1)}
</div>
<div class="dsec" style="border-top:1px solid var(--bv)"><p class="dtitle">Related objects</p>
{rel("key","secret/api-tls","kubernetes.io/tls")}
{rel("file","certificaterequest/api-tls-4","Ready")}
{rel("globe","ingress/payments-public","uses secret")}
{rel("file","clusterissuer/letsencrypt-prod","Ready")}
</div>
<div class="dsec" style="border-bottom:0"><p class="dtitle">Editor</p>
<div style="display:flex;flex-direction:column;gap:6px;font-size:12px;color:var(--muted)">
<span class="pill">{ic("ok",12,C["green"])}Schema validation on</span>
<span class="pill">{ic("eye",12)}Hide managedFields</span>
<span class="pill">{ic("shield",12,C["yellow"])}Confirm apply on PROD</span>
</div></div>
</aside>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    tb = tabs([("file", "Certificates", False), ("code", "api-tls.yaml", True, True), ("list", "checkout-api · logs", False)])
    return page("YAML editor — Kubyl", shell("Certificates", tb, content))

# ---------- 4. Overview + events ----------
def overview_screen():
    def kpi(title, big, sub, pct, vals, col):
        return f'''<div class="card" style="flex:1;padding:12px 14px;display:flex;flex-direction:column;gap:6px;min-width:0">
<div style="display:flex;justify-content:space-between;color:var(--dim);font-size:12px"><span>{title}</span><span class="mono" style="font-size:11.5px">{pct}</span></div>
<div style="display:flex;align-items:baseline;gap:8px"><span style="font-size:24px;font-weight:600;letter-spacing:-.01em">{big}</span><span style="font-size:12px;color:var(--dim)">{sub}</span></div>
{spark(vals,236,30,col)}
</div>'''
    kpis = "".join([
        kpi("CPU", "61%", "118 / 192 cores", "req 74% · lim 138%", series(40, 60, 8, 0.4), C["accent"]),
        kpi("Memory", "72%", "553 / 768 GiB", "req 81%", series(40, 72, 4, 1.9, .1), C["purple"]),
        kpi("Pods", "842", "of 1,320 capacity", "64%", series(40, 830, 14, 3.1), C["cyan"]),
        kpi("Nodes", "11 / 12", "ready", f'<span style="color:var(--red)">1 NotReady</span>', [12]*25+[11]*15, C["green"]),
    ])
    def chart(title, sub, ser, w=402, h=150):
        grid = "".join(f'<line x1="0" x2="{w}" y1="{y}" y2="{y}" stroke="#363c46"></line>' for y in (h*0.25, h*0.5, h*0.75))
        paths = ""
        mx = max(max(v) for v, _, _ in ser) * 1.1
        for vals, col, name in ser:
            pts = " L".join(f"{i*w/(len(vals)-1):.1f},{h - v/mx*h:.1f}" for i, v in enumerate(vals))
            paths += f'<path d="M{pts}" fill="none" stroke="{col}" stroke-width="1.6"></path>'
        legend = "".join(f'<span class="pill" style="font-size:11.5px;color:var(--muted)"><span style="width:10px;height:2px;background:{c};display:inline-block"></span>{n}</span>' for _, c, n in ser)
        return f'''<div class="card" style="flex:1;padding:12px 14px;min-width:0">
<div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:8px"><span style="font-weight:500">{title}</span><span style="font-size:11.5px;color:var(--dim)">{sub}</span></div>
<svg width="{w}" height="{h}" viewBox="0 0 {w} {h}" aria-label="{title} chart">{grid}{paths}</svg>
<div style="display:flex;gap:14px;margin-top:8px;flex-wrap:wrap">{legend}</div>
</div>'''
    cpu = [(series(60, 38, 8, 0.3), C["accent"], "payments"), (series(60, 30, 6, 2.2), C["orange"], "search"), (series(60, 22, 4, 4.1), C["purple"], "platform"), (series(60, 14, 3, 5.5), C["cyan"], "monitoring")]
    mem = [(series(60, 180, 10, 1.1, .1), C["accent"], "payments"), (series(60, 150, 6, 2.6, .1), C["orange"], "search"), (series(60, 120, 5, 3.3, .1), C["purple"], "platform"), (series(60, 90, 4, 4.4, .1), C["cyan"], "monitoring")]
    NC = "grid-template-columns: minmax(0,1fr) 84px 110px 110px 50px 76px 56px"
    nodes = [("ip-10-0-11-92", "Ready", 58, 71, 74, "m6i.2xlarge"), ("ip-10-0-12-41", "Ready", 72, 83, 81, "m6i.2xlarge"),
             ("ip-10-0-13-5", "Ready", 66, 88, 77, "m6i.2xlarge"), ("ip-10-0-14-7", "Ready", 49, 62, 70, "m6i.2xlarge"),
             ("ip-10-0-15-3", "NotReady", 0, 0, 0, "m6i.2xlarge"), ("ip-10-0-21-18", "Ready", 81, 69, 92, "c7i.4xlarge")]
    nrows = "".join(f'<div class="tr" style="{NC}"><span class="mono">{n}</span>{st(s)}<div style="display:flex;align-items:center;gap:6px;padding-right:12px"><span class="mono" style="width:28px">{c}%</span>{bar(c,w=60) if c else ""}</div><div style="display:flex;align-items:center;gap:6px;padding-right:12px"><span class="mono" style="width:28px">{m}%</span>{bar(m,w=60) if m else ""}</div><span class="mono">{p}</span><span class="mono" style="color:var(--muted)">{t}</span><span class="mono" style="color:var(--muted)">v1.30.4</span></div>' for n, s, c, m, p, t in nodes)
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0;padding:16px 18px;gap:14px;overflow:hidden">
<div style="display:flex;align-items:center;gap:10px">
<h1 style="font-size:18px;font-weight:600">prod-eu-west-1</h1><span class="prod">PROD</span>
<span style="color:var(--dim);font-size:12px">Amazon EKS · v1.30.4 · 12 nodes · 38 namespaces</span>
<div style="flex:1"></div>
<span class="chip" style="color:var(--green)">{ic("activity",11,C["green"])}Prometheus · monitoring/prometheus-k8s</span>
<div style="display:flex;border:1px solid var(--border);border-radius:5px;overflow:hidden;font-size:12px">{"".join(f'<span style="padding:3px 9px;{"background:#2d3b4d;color:#a8cdf3" if r=="1h" else "color:var(--dim)"}">{r}</span>' for r in ["15m","1h","6h","24h","7d"])}</div>
</div>
<div style="display:flex;gap:12px">{kpis}</div>
<div style="display:flex;gap:12px">{chart("CPU by namespace","cores · rate(container_cpu_usage_seconds_total[5m])",cpu)}{chart("Memory working set","GiB · by namespace",mem)}</div>
<div class="card" style="overflow:hidden">
<div style="display:flex;align-items:center;padding:10px 12px;gap:8px"><span style="font-weight:500">Nodes</span><span style="color:var(--dim);font-size:12px">12</span><div style="flex:1"></div><button class="btn g">{ic("server",13)}Cordon</button><button class="btn g">Drain…</button></div>
<div class="th" style="{NC}"><span>NAME</span><span>STATUS</span><span>CPU</span><span>MEMORY</span><span>PODS</span><span>TYPE</span><span>KUBELET</span></div>
{nrows}
</div>
</div>'''
    ev = [
     ("w", "BackOff", "pod/payment-gateway-5c8b7f9d4-hl2vp", "Back-off restarting failed container gateway", "×14", "12s"),
     ("w", "OOMKilling", "pod/fraud-scorer-6f77d8c9b-9zxkd", "Memory cgroup out of memory: killed process 1 (python)", "×3", "41s"),
     ("n", "ScalingReplicaSet", "deployment/invoice-renderer", "Scaled up replica set invoice-renderer-84c6c7c9f to 2", "", "58s"),
     ("w", "FailedScheduling", "pod/ledger-writer-2", "0/12 nodes are available: 1 node(s) not ready, 11 Insufficient memory", "×6", "2m"),
     ("w", "NodeNotReady", "node/ip-10-0-15-3", "Node is not ready: kubelet stopped posting status", "", "3m"),
     ("n", "Issuing", "certificate/api-tls", "Renewal scheduled for 2026-11-16", "", "4m"),
     ("w", "Unhealthy", "pod/checkout-api-7d9f8c6b5-x2kqp", "Readiness probe failed: HTTP probe failed with statuscode 503", "×3", "4m"),
     ("n", "SuccessfulCreate", "job/settlement-batch-28791440", "Created pod: settlement-batch-28791440-k7x2m", "", "4h"),
     ("n", "Pulled", "pod/webhook-relay-7b9c5d8f6-rrm4c", "Container image already present on machine", "", "2d"),
    ]
    evrows = "".join(f'''<div style="display:flex;gap:10px;padding:10px 14px;border-bottom:1px solid var(--bv)">{ic("alert" if t=="w" else "info",14,C["yellow"] if t=="w" else C["dim"])}<div style="flex:1;min-width:0"><div style="display:flex;gap:8px;align-items:baseline"><b style="font-weight:500;{"color:var(--yellow)" if t=="w" else ""}">{r}</b><span style="flex:1"></span><span class="mono" style="font-size:11px;color:var(--dim)">{cnt} {age}</span></div><div class="mono" style="font-size:11.5px;color:var(--accent);white-space:nowrap;overflow:hidden;text-overflow:ellipsis">{o}</div><div style="font-size:12px;color:var(--muted);line-height:17px">{m}</div></div></div>''' for t, r, o, m, cnt, age in ev)
    dock = f'''<aside class="dock" style="width:350px">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Events</span><span style="font-size:12px;color:var(--green);display:flex;align-items:center;gap:5px">{dot(C["green"])}live</span><button class="ib" aria-label="Pause">{ic("pause",12)}</button></div>
<div style="display:flex;gap:4px;padding:8px 14px;border-bottom:1px solid var(--bv)"><span class="chip on" style="color:var(--yellow)">Warning 23</span><span class="chip">Normal 212</span><span class="chip">all namespaces {ic("cd",10)}</span></div>
{evrows}
</aside>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    tb = tabs([("gauge", "Overview", True), ("box", "Pods", False), ("code", "api-tls.yaml", False, True)])
    return page("Cluster overview — Kubyl", shell("Overview", tb, content))

# ---------- 5. Clusters & kubeconfigs + OIDC ----------
def clusters_screen():
    src = lambda path, sub, on=False, icon="file", warn="": f'<div style="display:flex;gap:10px;padding:10px 12px;border-radius:6px;{"background:var(--sel);outline:1px solid var(--accent);outline-offset:-1px" if on else ""}">{ic(icon,15,C["accent"] if on else C["dim"])}<div style="flex:1;min-width:0"><div class="mono" style="font-size:12px;color:var(--text)">{path}</div><div style="font-size:11.5px;color:var(--dim)">{sub}</div></div>{warn}</div>'
    left = f'''<div style="width:330px;flex-shrink:0;border-right:1px solid var(--bv);padding:16px 12px;display:flex;flex-direction:column;gap:4px">
<div style="display:flex;align-items:center;margin:0 4px 8px"><h2 style="font-size:14px;font-weight:600;flex:1">Kubeconfig sources</h2><button class="btn" style="height:24px">{ic("plus",12)}Add</button></div>
{src("~/.kube/config","3 contexts · watched for changes")}
{src("$KUBECONFIG","merged · 2 files",icon="zap")}
{src("~/work/kube/eks-prod.yaml","2 contexts · watched",True)}
{src("~/work/kube/platform-onprem.yaml","1 context · OIDC",warn=f'<span style="display:flex">{ic("key",13,C["yellow"])}</span>')}
{src("~/Downloads/gke-analytics.yaml","1 context")}
<div style="margin-top:10px;border:1px dashed var(--border);border-radius:8px;padding:18px 14px;text-align:center;color:var(--dim);font-size:12.5px;line-height:19px">{ic("upload",18)}<br>Drop kubeconfig files here<br><span style="font-size:12px">or <a href="#">browse</a> · <a href="#">paste YAML</a></span></div>
<div style="margin-top:auto;font-size:11.5px;color:var(--dim);line-height:17px;padding:0 4px">Files are never modified. Contexts from every source are merged; name collisions get the file name as suffix.</div>
</div>'''
    CC = "grid-template-columns: 22px minmax(0,1.1fr) minmax(0,1.3fr) minmax(0,1.2fr) 150px"
    ctx = [
     (C["red"], "prod-eu-west-1", "https://3F9C…gr7.eu-west-1.eks.amazonaws.com", "exec · aws eks get-token", ("Connected · 38ms", C["green"]), True),
     (C["red"], "prod-us-east-1", "https://71AD…kq2.us-east-1.eks.amazonaws.com", "exec · aws eks get-token", ("Connected · 96ms", C["green"]), False),
     (C["yellow"], "staging-eu-west-1", "https://C04E…p9x.eu-west-1.eks.amazonaws.com", "exec · aws (profile staging)", ("Connected · 41ms", C["green"]), False),
     (C["cyan"], "gke-analytics", "https://34.90.18.201", "exec · gke-gcloud-auth-plugin", ("Connected · 52ms", C["green"]), False),
     (C["purple"], "platform-onprem", "https://k8s.platform.internal:6443", "OIDC · sso.example.com", ("Signing in…", C["yellow"]), False),
     (C["green"], "kind-dev", "https://127.0.0.1:52341", "client certificate", ("Connected · 2ms", C["green"]), False),
     (C["faint"], "homelab-k3s", "https://192.168.1.40:6443", "bearer token", ("Unreachable", C["red"]), False),
    ]
    crow = "".join(f'<div class="tr{" on" if on else ""}" style="{CC};height:34px"><span>{dot(c)}</span><span style="font-weight:500">{n}</span><span class="mono" style="color:var(--muted)">{s}</span><span style="color:var(--muted)">{a}</span><span class="pill" style="color:{sc}">{dot(sc)}{stx}</span></div>' for c, n, s, a, (stx, sc), on in ctx)
    toggle = lambda on: f'<span style="width:28px;height:16px;border-radius:8px;background:{C["accent"] if on else "#4a505c"};position:relative;display:inline-block;flex-shrink:0"><span style="position:absolute;top:2px;{"right" if on else "left"}:2px;width:12px;height:12px;border-radius:50%;background:#fff"></span></span>'
    opt = lambda t, s, on: f'<div style="display:flex;gap:12px;align-items:center;padding:8px 0;border-bottom:1px solid var(--bv)"><div style="flex:1"><div style="font-size:12.5px">{t}</div><div style="font-size:11.5px;color:var(--dim)">{s}</div></div>{toggle(on)}</div>'
    right = f'''<div style="flex:1;min-width:0;padding:16px 18px;display:flex;flex-direction:column;gap:12px">
<div style="display:flex;align-items:center;gap:10px"><h2 style="font-size:14px;font-weight:600">Contexts</h2><span style="color:var(--dim);font-size:12px">7 from 5 sources</span><div style="flex:1"></div><div class="inp" style="width:220px">{ic("search",12)}Filter contexts</div></div>
<div class="card" style="overflow:hidden"><div class="th" style="{CC}"><span></span><span>CONTEXT</span><span>API SERVER</span><span>AUTH</span><span>STATUS</span></div>{crow}</div>
<div style="display:flex;gap:12px">
<div class="card" style="flex:1;padding:14px 16px"><p class="dtitle">prod-eu-west-1 · safety</p>
{opt("Production cluster","Red accent in title bar, typed confirmation for delete / scale to 0",True)}
{opt("Read-only mode","Block all mutating requests from this app",False)}
{opt("Default namespace","payments",True)}
</div>
<div class="card" style="flex:1;padding:14px 16px"><p class="dtitle">Connection</p>
<dl class="kv" style="margin:0"><dt>Source</dt><dd class="mono" style="font-size:11.5px">~/work/kube/eks-prod.yaml</dd><dt>Auth</dt><dd>exec plugin · aws 2.17</dd><dt>Token</dt><dd>cached · expires in 11m</dd><dt>CA</dt><dd>inline · valid until 2034</dd><dt>Proxy</dt><dd style="color:var(--dim)">none</dd><dt>Discovery</dt><dd>214 kinds · 31 CRDs</dd></dl>
</div></div>
</div>'''
    modal = f'''<div style="position:absolute;inset:0;background:rgba(15,17,21,.55);display:flex;align-items:flex-start;justify-content:center;padding-top:150px">
<div role="dialog" aria-label="Sign in to platform-onprem" style="width:500px;background:#2f343e;border:1px solid var(--border);border-radius:10px;box-shadow:0 20px 60px rgba(0,0,0,.5);overflow:hidden">
<div style="display:flex;align-items:center;gap:10px;padding:14px 16px;border-bottom:1px solid var(--bv)">{ic("key",16,C["yellow"])}<b style="font-weight:600;flex:1">Sign in to platform-onprem</b><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div style="padding:16px;display:flex;flex-direction:column;gap:14px">
<dl class="kv" style="margin:0;grid-template-columns:90px minmax(0,1fr)"><dt>Issuer</dt><dd class="mono" style="font-size:11.5px">https://sso.example.com/realms/platform</dd><dt>Client ID</dt><dd class="mono" style="font-size:11.5px">kubernetes</dd><dt>Scopes</dt><dd class="mono" style="font-size:11.5px">openid groups email offline_access</dd></dl>
<div style="display:flex;gap:12px;align-items:center;padding:12px;border-radius:7px;background:#2a2e36;border:1px solid var(--bv)">
<svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9" fill="none" stroke="#464b57" stroke-width="3"></circle><path d="M12 3a9 9 0 0 1 9 9" fill="none" stroke="#74ade8" stroke-width="3" stroke-linecap="round"></path></svg>
<div style="flex:1"><div style="font-size:12.5px">Waiting for your browser…</div><div class="mono" style="font-size:11px;color:var(--dim)">callback on http://127.0.0.1:8000/callback · PKCE</div></div>
<button class="btn" style="height:24px">Reopen browser</button></div>
<div style="font-size:12px;color:var(--muted)">No browser on this machine? Use a device code instead:</div>
<div style="display:flex;gap:10px;align-items:center"><span class="mono" style="font-size:18px;letter-spacing:.12em;padding:6px 12px;border-radius:6px;background:#23272e;border:1px solid var(--border)">WDJB-MJHT</span><span style="font-size:12px;color:var(--dim)">at <a href="#">sso.example.com/device</a></span><button class="ib" aria-label="Copy code">{ic("copy",13)}</button></div>
<div style="display:flex;gap:8px;align-items:center;font-size:11.5px;color:var(--dim)">{ic("lock",12)}Tokens stored in Keychain · Credential Manager · Secret Service</div>
</div>
<div style="display:flex;justify-content:flex-end;gap:8px;padding:12px 16px;border-top:1px solid var(--bv)"><button class="btn g">Cancel</button><button class="btn p">Use device code</button></div>
</div></div>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{left}{right}</div>'
    tb = tabs([("gear", "Clusters &amp; kubeconfigs", True), ("gauge", "Overview", False), ("box", "Pods", False)])
    inner = f'''<div class="app">
{titlebar()}
<div class="body">
{sidebar("none")}
<main class="main">{tb}{content}</main>
</div>
{statusbar()}
{modal}
</div>'''
    return page("Clusters and kubeconfigs — Kubyl", inner)

# ---------- 6. Command palette over deployments ----------
def palette_screen():
    DC = "grid-template-columns: minmax(0,1fr) 70px 90px 90px 120px minmax(0,1fr) 56px"
    deps = [("checkout-api", "3/3", 3, 3, "RollingUpdate", "checkout-api:2.14.1", "41d"), ("payment-gateway", "1/2", 2, 1, "RollingUpdate", "payment-gateway:5.2.0", "47m"),
            ("fraud-scorer", "1/2", 2, 1, "RollingUpdate", "fraud-scorer:0.19.4", "6h"), ("invoice-renderer", "1/2", 2, 1, "Recreate", "invoice-renderer:1.8.0", "22d"),
            ("webhook-relay", "1/1", 1, 1, "RollingUpdate", "webhook-relay:3.0.2", "90d"), ("currency-rates-api", "2/2", 2, 2, "RollingUpdate", "currency-rates:1.1.0", "90d"),
            ("notifications", "2/2", 2, 2, "RollingUpdate", "notifications:4.4.1", "12d"), ("refunds", "2/2", 2, 2, "RollingUpdate", "refunds:2.0.9", "12d"), ("reports-ui", "1/1", 1, 1, "RollingUpdate", "reports-ui:0.3.2", "3d")]
    rows = "".join(f'<div class="tr{" on" if i==0 else ""}" style="{DC}"><span class="mono">{n}</span><span class="mono" style="color:{C["red"] if r.split("/")[0]!=r.split("/")[1] else C["text"]}">{r}</span><span class="mono">{u}</span><span class="mono">{a}</span><span style="color:var(--muted)">{s}</span><span class="mono" style="color:var(--muted)">{img}</span><span class="mono" style="color:var(--muted)">{age}</span></div>' for i, (n, r, u, a, s, img, age) in enumerate(deps))
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div class="tool"><div class="crumb">{ic("layers",14,C["accent"])}<b>Deployments</b><span>·</span><span>9 in payments</span></div><div style="flex:1"></div><div class="inp" style="width:240px">{ic("filter",12)}Filter</div></div>
<div class="th" style="{DC}"><span>NAME</span><span>READY</span><span>UP-TO-DATE</span><span>AVAILABLE</span><span>STRATEGY</span><span>IMAGE</span><span>AGE</span></div>
<div style="flex:1;overflow:hidden">{rows}</div>
{hints([("s","Scale"),("r","Restart"),("u","Rollback"),("l","Logs"),("e","Edit"),("⌃d","Delete")])}
</div>'''
    rev = lambda n, img, when, cur=False: f'<div style="display:flex;gap:10px;align-items:center;padding:6px 0;font-size:12px"><span class="mono" style="width:28px;color:{C["accent"] if cur else C["dim"]}">#{n}</span><span class="mono" style="flex:1;font-size:11.5px">{img}</span><span style="color:var(--dim)">{when}</span></div>'
    dock = f'''<aside class="dock" style="width:320px">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">checkout-api</span></div>
<div class="dsec"><p class="dtitle">Replicas</p><div style="display:flex;align-items:center;gap:8px"><button class="btn" aria-label="Scale down" style="width:28px;padding:0;justify-content:center">{ic("minus",13)}</button><span class="mono" style="font-size:18px;width:36px;text-align:center">3</span><button class="btn" aria-label="Scale up" style="width:28px;padding:0;justify-content:center">{ic("plus",13)}</button><span style="flex:1"></span><span style="font-size:12px;color:var(--dim)">HPA 3–10 · cpu 70%</span></div></div>
<div class="dsec"><p class="dtitle">Rollout history</p>{rev(14,"checkout-api:2.14.1","41d",True)}{rev(13,"checkout-api:2.14.0","48d")}{rev(12,"checkout-api:2.13.3","63d")}
<div style="display:flex;gap:6px;margin-top:8px"><button class="btn">{ic("refresh",12)}Restart</button><button class="btn">Rollback…</button><button class="btn g">{ic("pause",12)}Pause</button></div></div>
</aside>'''
    item = lambda icon, name, sub, alias, cnt="", on=False: f'<div style="display:flex;align-items:center;gap:10px;height:32px;padding:0 12px;border-radius:5px;{"background:var(--sel)" if on else ""}">{ic(icon,14,C["accent"] if on else C["dim"])}<span style="color:var(--text)">{name}</span><span class="mono" style="font-size:11.5px;color:var(--dim)">{sub}</span><span style="flex:1"></span><span class="mono" style="font-size:11px;color:var(--dim)">{alias}</span><span class="mono" style="font-size:11.5px;color:var(--muted);width:28px;text-align:right">{cnt}</span></div>'
    grp = lambda t: f'<div style="padding:8px 12px 4px;font-size:11px;font-weight:600;letter-spacing:.06em;text-transform:uppercase;color:var(--dim)">{t}</div>'
    pal = f'''<div role="dialog" aria-label="Command palette" style="position:absolute;left:50%;top:60px;margin-left:-300px;width:600px;background:#2f343e;border:1px solid var(--border);border-radius:9px;box-shadow:0 24px 60px rgba(0,0,0,.55);overflow:hidden">
<div style="display:flex;align-items:center;gap:10px;height:44px;padding:0 14px;border-bottom:1px solid var(--bv)"><span class="mono" style="color:var(--accent);font-size:14px">:</span><span class="mono" style="font-size:14px;color:var(--text)">cert<span style="display:inline-block;width:1px;height:16px;background:var(--accent);vertical-align:-3px;margin-left:1px"></span></span><span style="flex:1"></span><span style="font-size:11.5px;color:var(--dim)">in prod-eu-west-1</span></div>
<div style="display:flex;gap:6px;padding:8px 12px;border-bottom:1px solid var(--bv);font-size:11.5px;color:var(--dim)"><span class="chip on"><b class="mono">:</b> resources</span><span class="chip"><b class="mono">@</b> contexts</span><span class="chip"><b class="mono">#</b> namespaces</span><span class="chip"><b class="mono">&gt;</b> actions</span><span class="chip"><b class="mono">*</b> favorites</span></div>
<div style="padding:4px 6px 8px">
{grp("Resource kinds")}
{item("file","certificates","cert-manager.io/v1","cert, certs","12",True)}
{item("file","certificaterequests","cert-manager.io/v1","cr, crs","48")}
{item("file","certificatesigningrequests","certificates.k8s.io/v1","csr","2")}
{item("key","secrets","v1 · filter type=kubernetes.io/tls","sec","19")}
{grp("Actions")}
{item("zap","Renew certificate…","cmctl renew","","")}
{item("eye","Inspect TLS secret…","decode + show chain","","")}
{grp("Favorites")}
{item("star","payments","staging-eu-west-1 · certificates","","3")}
</div>
<div style="display:flex;gap:16px;padding:8px 14px;border-top:1px solid var(--bv);font-size:11.5px;color:var(--dim)"><span><span class="kbd">↵</span> open</span><span><span class="kbd">⌘↵</span> open in split</span><span><span class="kbd">⇥</span> all namespaces</span><span style="flex:1"></span><span><span class="kbd">esc</span></span></div>
</div>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    tb = tabs([("layers", "Deployments", True), ("box", "Pods", False), ("list", "checkout-api · logs", False)])
    inner = f'''<div class="app">
{titlebar()}
<div class="body">
{sidebar("Deployments")}
<main class="main">{tb}{content}</main>
</div>
{statusbar()}
{pal}
</div>'''
    return page("Command palette — Kubyl", inner)

# ---------- 7. Installed operators (OLM) ----------
def operators_screen():
    OC = "grid-template-columns: minmax(0,1.3fr) 110px 150px 110px 96px minmax(0,1.2fr)"
    tile = lambda l, c: f'<span style="width:26px;height:26px;border-radius:6px;background:{c}22;color:{c};display:flex;align-items:center;justify-content:center;font-weight:600;font-size:11px;flex-shrink:0;border:1px solid {c}55">{l}</span>'
    ops = [
     ("cert-manager", "cert-manager", C["green"], "CM", "1.15.3", "Succeeded", "stable", "Automatic", ["Certificate", "Issuer", "ClusterIssuer"]),
     ("Prometheus Operator", "prometheus-operator", C["orange"], "PR", "0.76.1", "Succeeded", "beta", "Automatic", ["Prometheus", "ServiceMonitor", "+5"]),
     ("Strimzi", "strimzi-kafka-operator", C["cyan"], "SZ", "0.43.0", "Upgrade available", "stable", "Manual", ["Kafka", "KafkaTopic", "KafkaUser", "+6"]),
     ("CloudNativePG", "cloudnative-pg", C["accent"], "PG", "1.24.1", "Succeeded", "stable-v1", "Automatic", ["Cluster", "Backup", "Pooler"]),
     ("Argo CD", "argocd-operator", C["orange"], "AR", "0.11.0", "Succeeded", "alpha", "Automatic", ["ArgoCD", "Application"]),
     ("External Secrets", "external-secrets-operator", C["purple"], "ES", "0.10.3", "Installing", "stable", "Automatic", ["ExternalSecret", "SecretStore"]),
     ("Sail (Istio)", "sailoperator", C["accent"], "IS", "0.2.0", "Failed", "candidates", "Manual", ["Istio", "IstioRevision"]),
    ]
    rows = "".join(f'''<div class="tr{" on" if i==2 else ""}" style="{OC};height:46px">
<div style="display:flex;gap:10px;align-items:center;min-width:0">{tile(t,c)}<div style="min-width:0"><div style="font-weight:500">{n}</div><div class="mono" style="font-size:11px;color:var(--dim)">{pkg}</div></div></div>
<span class="mono">{v}</span>{st(s)}<span class="mono" style="color:var(--muted)">{ch}</span><span style="color:{C["yellow"] if ap=="Manual" else C["muted"]}">{ap}</span>
<div style="display:flex;gap:4px;overflow:hidden">{"".join(f'<span class="chip">{a}</span>' for a in apis)}</div></div>''' for i, (n, pkg, c, t, v, s, ch, ap, apis) in enumerate(ops))
    subtabs = f'''<div style="display:flex;gap:2px;padding:0 12px;border-bottom:1px solid var(--bv);height:36px;align-items:stretch">{"".join(f'<span style="display:flex;align-items:center;gap:6px;padding:0 10px;{"color:var(--text);box-shadow:inset 0 -2px 0 var(--accent)" if on else "color:var(--dim)"}">{t}</span>' for t, on in [("Installed","1"),("OperatorHub",""),("Install plans <span class=\"chip\" style=\"height:17px;color:var(--yellow)\">1 pending</span>",""),("Subscriptions",""),("Helm releases","")])}</div>'''
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div class="tool"><div class="crumb">{ic("blocks",14,C["accent"])}<b>Operators</b><span>·</span><span>OLM v0 detected in namespace olm · 7 installed</span></div><div style="flex:1"></div><div class="inp" style="width:220px">{ic("search",12)}Filter operators</div><button class="btn p">{ic("store",13,"#1b1e24")}Browse OperatorHub</button></div>
{subtabs}
<div class="th" style="{OC}"><span>NAME</span><span>VERSION</span><span>STATUS</span><span>CHANNEL</span><span>APPROVAL</span><span>PROVIDED APIS</span></div>
<div style="flex:1;overflow:hidden">{rows}</div>
</div>'''
    api = lambda k, n: f'<div style="display:flex;align-items:center;gap:8px;padding:7px 10px;border:1px solid var(--bv);border-radius:6px;background:#2a2e36"><span class="mono" style="font-size:12px;flex:1">{k}</span><span style="font-size:11.5px;color:var(--dim)">{n}</span><button class="btn g" style="height:22px;padding:0 6px">{ic("plus",11)}Create</button></div>'
    chg = lambda icon, col, t: f'<div style="display:flex;gap:8px;font-size:12px;align-items:flex-start;padding:3px 0">{ic(icon,13,col)}<span style="color:var(--muted)">{t}</span></div>'
    dock = f'''<aside class="dock" style="width:350px">
<div class="phead" style="border-bottom:1px solid var(--bv)">{tile("SZ",C["cyan"])}<span style="flex:1;color:var(--text);font-weight:500;margin-left:4px">Strimzi</span><span style="font-size:11.5px">kafka namespace</span></div>
<div class="dsec" style="background:#35322a"><div style="display:flex;gap:8px;align-items:center;margin-bottom:8px">{ic("up",14,C["yellow"])}<b style="font-weight:600;color:var(--yellow)">Upgrade pending approval</b></div>
<div class="mono" style="font-size:12px;margin-bottom:10px">0.43.0 <span style="color:var(--dim)">→</span> <span style="color:var(--green)">0.44.0</span> <span style="color:var(--dim)">· installplan install-7qk2d</span></div>
{chg("file",C["accent"],"3 CRDs updated · kafkas.kafka.strimzi.io schema adds <span class='mono' style='font-size:11px'>spec.kafka.tieredStorage</span>")}
{chg("shield",C["yellow"],"RBAC: ClusterRole gains <span class='mono' style='font-size:11px'>get,list</span> on nodes")}
{chg("ok",C["green"],"All 4 Kafka clusters on supported versions")}
<div style="display:flex;gap:6px;margin-top:10px"><button class="btn p">Approve</button><button class="btn">{ic("code",12)}View YAML</button><button class="btn g">Diff CRDs</button></div></div>
<div class="dsec"><p class="dtitle">Provided APIs</p><div style="display:flex;flex-direction:column;gap:6px">{api("Kafka","4 instances")}{api("KafkaTopic","112 instances")}{api("KafkaUser","38 instances")}{api("KafkaConnect","2 instances")}</div></div>
<div class="dsec" style="border-bottom:0"><p class="dtitle">Subscription</p><dl class="kv" style="margin:0"><dt>Catalog</dt><dd>operatorhubio-catalog</dd><dt>Channel</dt><dd>stable</dd><dt>CSV</dt><dd class="mono" style="font-size:11.5px">strimzi-cluster-operator.v0.43.0</dd><dt>Approval</dt><dd style="color:var(--yellow)">Manual</dd></dl></div>
</aside>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    tb = tabs([("blocks", "Installed Operators", True), ("up", "Cluster Updates", False), ("gauge", "Overview", False)])
    return page("Installed operators — Kubyl", shell("Operators", tb, content))

# ---------- 8. Cluster updates ----------
def updates_screen():
    # version graph
    gw, gh = 560, 120
    nodes = [(40, 60, "1.30.4", "current", C["green"]), (210, 60, "1.31.2", "recommended", C["accent"]), (380, 30, "1.32.0", "blocked", C["red"]), (380, 92, "1.31.3", "rolling out", C["dim"])]
    edges = [(0, 1), (1, 2), (1, 3)]
    g = "".join(f'<path d="M{nodes[a][0]+14},{nodes[a][1]} C{(nodes[a][0]+nodes[b][0])/2},{nodes[a][1]} {(nodes[a][0]+nodes[b][0])/2},{nodes[b][1]} {nodes[b][0]-14},{nodes[b][1]}" fill="none" stroke="{"#74ade8" if (a,b)==(0,1) else "#464b57"}" stroke-width="2" {"" if (a,b)==(0,1) else "stroke-dasharray=\"4 4\""}></path>' for a, b in edges)
    for x, y, v, lab, c in nodes:
        g += f'<circle cx="{x}" cy="{y}" r="9" fill="{c}" fill-opacity="{1 if lab in ("current","recommended") else 0.35}" stroke="{c}" stroke-width="2"></circle>'
        g += f'<text x="{x+16}" y="{y-4}" fill="#dce0e5" font-family="IBM Plex Mono" font-size="12.5">{v}</text><text x="{x+16}" y="{y+13}" fill="{c if lab!="rolling out" else "#959aa6"}" font-family="IBM Plex Sans" font-size="11.5">{lab}</text>'
    graph = f'<svg width="{gw}" height="{gh}" viewBox="0 0 {gw} {gh}" aria-label="Update graph">{g}</svg>'
    chk = lambda icon, col, t, s, act="": f'<div style="display:flex;gap:10px;padding:10px 0;border-bottom:1px solid var(--bv);align-items:flex-start">{ic(icon,15,col)}<div style="flex:1;min-width:0"><div style="font-size:12.5px">{t}</div><div style="font-size:11.5px;color:var(--dim);line-height:17px">{s}</div></div>{act}</div>'
    NP = "grid-template-columns: minmax(0,1fr) 90px 90px minmax(0,1.3fr) 110px"
    pools = [
      ("Control plane", "managed", "1.30.4", '<span style="color:var(--dim);font-size:12px">updated first · ~12 min</span>', '<button class="btn" style="height:24px">Update</button>'),
      ("general-m6i", "6 nodes", "1.30.4", '<span style="color:var(--dim);font-size:12px">waits for control plane</span>', '<span style="color:var(--dim);font-size:12px">queued</span>'),
      ("compute-c7i", "4 nodes", "1.30.4", '<span style="color:var(--dim);font-size:12px">surge 1 · maxUnavailable 0</span>', '<span style="color:var(--dim);font-size:12px">queued</span>'),
      ("gpu-g5", "2 nodes", "1.29.8", f'<div style="display:flex;align-items:center;gap:8px"><span class="mono" style="font-size:11.5px">1 / 2</span>{bar(50,C["accent"],120)}<span style="font-size:11.5px;color:var(--dim)">draining ip-10-0-31-4</span></div>', f'<span style="color:var(--accent);font-size:12px">catching up</span>'),
    ]
    prows = "".join(f'<div class="tr" style="{NP};height:40px"><span style="font-weight:500">{n}</span><span style="color:var(--muted)">{s}</span><span class="mono">{v}</span>{p}{a}</div>' for n, s, v, p, a in pools)
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0;padding:16px 18px;gap:14px;overflow:hidden">
<div style="display:flex;align-items:center;gap:10px"><h1 style="font-size:18px;font-weight:600">Cluster updates</h1><span style="color:var(--dim);font-size:12px">prod-eu-west-1 · provider Amazon EKS (via AWS API, profile prod)</span></div>
<div style="display:flex;gap:12px">
<div class="card" style="width:250px;padding:14px 16px;display:flex;flex-direction:column;gap:10px">
<div><div style="font-size:12px;color:var(--dim)">Current version</div><div class="mono" style="font-size:24px;font-weight:500">1.30.4</div><div style="font-size:11.5px;color:var(--dim)">platform eks.12 · standard support until 2026-07 <span style="color:var(--yellow)">(extended)</span></div></div>
<div><div style="font-size:12px;color:var(--dim);margin-bottom:4px">Channel</div><button class="btn" style="width:100%;justify-content:space-between">stable{ic("cd",12)}</button></div>
<div style="font-size:12px;color:var(--dim)">Last update 1.29.8 → 1.30.4<br>2026-07-14 · took 42 min</div>
</div>
<div class="card" style="flex:1;padding:14px 16px;min-width:0">
<div style="display:flex;align-items:center;margin-bottom:6px"><span style="font-weight:500;flex:1">Update path</span><button class="btn p">{ic("up",13,"#1b1e24")}Update to 1.31.2…</button></div>
{graph}
</div></div>
<div class="card" style="padding:4px 16px 6px">
<div style="display:flex;align-items:center;padding:10px 0 4px"><span style="font-weight:500;flex:1">Pre-flight checks for 1.31.2</span><span style="font-size:12px;color:var(--dim)">ran 2m ago · <a href="#">re-run</a></span></div>
{chk("err",C["red"],"PodDisruptionBudget blocks node drain","payments/ledger-writer-pdb allows 0 disruptions (3 replicas, minAvailable 3)",'<button class="btn" style="height:24px">Open PDB</button>')}
{chk("alert",C["yellow"],"Deprecated APIs still requested","flowcontrol.apiserver.k8s.io/v1beta3 — 2 clients in the last 24h (from audit metrics)",'<button class="btn g" style="height:24px">Details</button>')}
{chk("alert",C["yellow"],"Add-on needs update: coredns","v1.11.1 installed · v1.11.3 recommended for 1.31",'<button class="btn g" style="height:24px">Update add-on</button>')}
{chk("ok",C["green"],"Installed operators compatible","7 of 7 operators declare support for 1.31 (OLM maxKubeVersion)")}
<div style="display:flex;gap:10px;padding:10px 0;align-items:center">{ic("ok",15,C["green"])}<span style="font-size:12.5px">Node capacity for surge upgrades</span><span style="font-size:11.5px;color:var(--dim)">headroom for +1 node per pool</span></div>
</div>
<div class="card" style="overflow:hidden"><div style="display:flex;align-items:center;padding:10px 14px"><span style="font-weight:500;flex:1">Control plane &amp; node pools</span><span style="font-size:12px;color:var(--dim)">order: control plane → pools, one at a time</span></div>
<div class="th" style="{NP}"><span>POOL</span><span>SIZE</span><span>VERSION</span><span>PROGRESS</span><span></span></div>{prows}</div>
</div>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}</div>'
    tb = tabs([("blocks", "Installed Operators", False), ("up", "Cluster Updates", True), ("gauge", "Overview", False)])
    return page("Cluster updates — Kubyl", shell("Updates", tb, content))


# ---------- 9. File browser: drag & drop to/from pods ----------
def files_screen():
    FC = "grid-template-columns: minmax(0,1fr) 78px 104px"
    PC = "grid-template-columns: minmax(0,1fr) 78px 96px 118px"
    def frow(cols, name, icon, col, cells, sel=False, dim=False, extra=""):
        st_ = "background:#2d3b4d;" if sel else ""
        op = "opacity:.45;" if dim else ""
        cs = "".join(f'<span class="mono" style="color:var(--muted)">{c}</span>' for c in cells)
        return f'<div class="tr" style="{cols};height:28px;{st_}{op}"><span style="display:flex;align-items:center;gap:8px">{ic(icon,14,col)}<span class="mono">{name}</span>{extra}</span>{cs}</div>'
    fo, fi = C["accent"], C["dim"]
    local = "".join([
        frow(FC, "..", "folder", fo, ["", ""]),
        frow(FC, "certs-new/", "folder", fo, ["3 items", "10:31"], True, True),
        frow(FC, "feature-flags.json", "file", fi, ["2.1 KB", "10:28"], True, True),
        frow(FC, "overrides.yaml", "file", fi, ["844 B", "10:27"], True, True),
        frow(FC, "heapdump-0923.hprof", "file", fi, ["388 MB", "yesterday"]),
        frow(FC, "notes.md", "file", fi, ["3.4 KB", "yesterday"]),
        frow(FC, "trace-checkout.json", "file", fi, ["12 MB", "Sep 21"]),
    ])
    ro = f'<span class="chip" style="height:17px;font-size:10.5px">{ic("lock",10)}Secret · read-only</span>'
    cm = f'<span class="chip" style="height:17px;font-size:10.5px">ConfigMap</span>'
    pod = "".join([
        frow(PC, "..", "folder", fo, ["", "", ""]),
        frow(PC, "certs/", "folder", fo, ["—", "drwxr-xr-x", "app · 3d"]),
        frow(PC, "secrets/", "folder", fo, ["—", "dr-xr-xr-x", "root · 3d"], extra=ro),
        frow(PC, "application.yaml", "file", fi, ["6.2 KB", "-rw-r--r--", "app · 3d"], extra=cm),
        frow(PC, "feature-flags.json", "file", fi, ["1.9 KB", "-rw-r--r--", "app · 3d"]),
        frow(PC, "logback.xml", "file", fi, ["1.1 KB", "-rw-r--r--", "app · 3d"]),
        frow(PC, "overrides.yaml", "file", fi, ["612 B", "-rw-r--r--", "app · 2d"]),
        frow(PC, "tmp/", "folder", fo, ["—", "drwxrwxrwt", "app · 1h"]),
    ])
    crumb = lambda parts: '<div class="crumb mono" style="font-size:12px">' + '<span style="color:var(--faint)">/</span>'.join(f'<span style="{"color:var(--text)" if i==len(parts)-1 else ""}">{p}</span>' for i, p in enumerate(parts)) + '</div>'
    lpane = f"""<section style="flex:1;min-width:0;display:flex;flex-direction:column;border-right:1px solid var(--border)">
<div class="tool" style="height:36px">{ic("server",13,C["dim"])}<span style="font-weight:500">This Mac</span>{crumb(["~","Downloads","debug"])}<div style="flex:1"></div><span style="font-size:12px;color:var(--dim)">3 selected</span></div>
<div class="th" style="{FC}"><span>NAME</span><span>SIZE</span><span>MODIFIED</span></div>
<div style="flex:1;overflow:hidden">{local}</div>
</section>"""
    rpane = f"""<section style="flex:1.25;min-width:0;display:flex;flex-direction:column;position:relative">
<div class="tool" style="height:36px">{ic("box",13,C["accent"])}<span style="font-weight:500">api</span>{crumb(["","app","config"])}<div style="flex:1"></div><button class="btn g" style="height:24px">{ic("plus",12)}New folder</button><button class="btn g" style="height:24px">{ic("download",12)}Download</button></div>
<div class="th" style="{PC}"><span>NAME</span><span>SIZE</span><span>MODE</span><span>OWNER · MODIFIED</span></div>
<div style="flex:1;overflow:hidden">{pod}</div>
<div style="position:absolute;left:8px;right:8px;top:44px;bottom:8px;border:2px dashed var(--accent);border-radius:8px;background:rgba(116,173,232,.07);display:flex;align-items:flex-end;justify-content:center;padding-bottom:26px;box-sizing:border-box">
<div style="display:flex;align-items:center;gap:10px;padding:10px 16px;border-radius:8px;background:#2f343e;border:1px solid var(--accent);box-shadow:0 8px 24px rgba(0,0,0,.4)">{ic("upload",16,C["accent"])}<div><div style="font-size:13px">Drop to upload <b style="font-weight:600">3 items</b> into <span class="mono" style="font-size:12px">/app/config</span></div><div style="font-size:11.5px;color:var(--yellow)">overrides.yaml and feature-flags.json already exist · hold ⌥ to keep both</div></div></div>
</div>
</section>"""
    ghost = f"""<div aria-hidden="true" style="position:absolute;left:650px;top:260px;pointer-events:none">
<div style="position:absolute;left:6px;top:6px;width:210px;height:34px;border-radius:6px;background:#3b414d;border:1px solid var(--border)"></div>
<div style="position:absolute;left:3px;top:3px;width:210px;height:34px;border-radius:6px;background:#3b414d;border:1px solid var(--border)"></div>
<div style="position:relative;width:210px;height:34px;border-radius:6px;background:#3b4658;border:1px solid var(--accent);display:flex;align-items:center;gap:8px;padding:0 10px;box-sizing:border-box;box-shadow:0 10px 24px rgba(0,0,0,.45)">{ic("folder",14,C["accent"])}<span class="mono" style="font-size:12px;flex:1">certs-new/ +2</span><span style="background:var(--accent);color:#1b1e24;border-radius:9px;font-size:11px;font-weight:600;padding:0 6px">3</span></div>
<svg width="18" height="18" viewBox="0 0 24 24" style="position:absolute;left:190px;top:22px" aria-hidden="true"><path d="M4 2l16 10-7 1.5L10 21z" fill="#fff" stroke="#1b1e24" stroke-width="1.5"></path></svg>
</div>"""
    def tr(dirn, name, sub, pct, state, col):
        arrow = ic("download" if dirn=="down" else "upload", 14, col)
        pb = f'<div style="width:180px">{bar(pct, col)}</div>' if pct is not None else '<div style="width:180px"></div>'
        return f'<div style="display:flex;align-items:center;gap:12px;height:34px;padding:0 14px;border-bottom:1px solid var(--bv)">{arrow}<span class="mono" style="font-size:12px;width:260px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{name}</span><span style="font-size:12px;color:var(--dim);flex:1;white-space:nowrap;overflow:hidden;text-overflow:ellipsis">{sub}</span>{pb}<span style="font-size:12px;color:{col};width:150px;text-align:right">{state}</span><button class="ib" aria-label="Cancel">{ic("x",12)}</button></div>'
    queue = f"""<div style="height:210px;flex-shrink:0;border-top:1px solid var(--border);display:flex;flex-direction:column">
<div class="tabs" style="height:32px"><div class="tab on">{ic("refresh",13,C["accent"])}<span>Transfers</span><span class="chip" style="height:17px;margin-left:4px">2 active</span></div><div class="tab">{ic("clock",13)}<span>History</span></div>
<div class="tabtools" style="font-size:12px;gap:10px"><span>tar over exec · resumable chunks · sha256 verified</span></div></div>
{tr("down","heapdump-2026-09-24.hprof","x2kqp:/tmp → ~/Downloads (dragged to Finder)",64,"64% · 38 MB/s · 4s",C["accent"])}
{tr("down","/app/logs/ · 128 files","streamed as tar.gz, extracted locally",22,"22% · 9 MB/s",C["accent"])}
{tr("up","application-local.yaml","~/work → x2kqp:/app/config",100,"done · verified",C["green"])}
{tr("up","ca-bundle.pem","→ x2kqp:/app/config/secrets",None,"read-only mount (Secret)",C["red"])}
</div>"""
    toolbar = f"""<div class="tool">
<div class="crumb">{ic("box",14,C["accent"])}<b class="mono" style="font-size:12.5px">checkout-api-7d9f8c6b5-x2kqp</b></div>
<button class="btn" style="height:24px">{ic("box",12)}api{ic("cd",11)}</button>
<span class="chip">{dot(C["green"])}tar available · /bin/sh</span>
<div style="flex:1"></div>
<span class="chip">Show hidden</span>
<button class="btn">{ic("upload",13)}Upload…</button>
<button class="btn">{ic("split",13)}Swap panes</button>
</div>"""
    center = f"""<div style="flex:1;display:flex;flex-direction:column;min-width:0;position:relative">
{toolbar}
<div style="flex:1;display:flex;min-height:0">{lpane}{rpane}</div>
{queue}
{hints([("⏎","Open / preview"),("e","Edit in place"),("F5","Copy to other pane"),("⌘⌫","Delete in pod"),("⌥ drag","Keep both"),("drag out","Download to Finder / Explorer")])}
{ghost}
</div>"""
    tb = tabs([("box", "Pods", False), ("folder", "x2kqp · files", True), ("list", "checkout-api · logs", False)])
    return page("Pod file browser — Kubyl", shell("Pods", tb, center))

# ---------- 10. Service web view over a temporary port-forward ----------
def webview_screen():
    # generic internal web app (placeholder content, not any vendor's UI)
    def panel(title, body, span=1):
        return f'<div style="grid-column:span {span};background:#ffffff;border:1px solid #dfe2e7;border-radius:6px;padding:12px 14px;display:flex;flex-direction:column;gap:8px;min-width:0"><div style="font-size:12.5px;font-weight:600;color:#2a2f36">{title}</div>{body}</div>'
    def wchart(seed, col, w=420, h=120):
        v = series(48, 50, 18, seed)
        mx = max(v) * 1.15
        pts = " L".join(f"{i*w/47:.1f},{h - x/mx*h:.1f}" for i, x in enumerate(v))
        grid = "".join(f'<line x1="0" x2="{w}" y1="{y}" y2="{y}" stroke="#eceef2"></line>' for y in (h*.25, h*.5, h*.75))
        return f'<svg width="100%" height="{h}" viewBox="0 0 {w} {h}" preserveAspectRatio="none" aria-hidden="true">{grid}<path d="M{pts} L{w},{h} L0,{h} Z" fill="{col}" fill-opacity=".12"></path><path d="M{pts}" fill="none" stroke="{col}" stroke-width="1.8"></path></svg>'
    stat = lambda big, sub, col: f'<div style="font-size:30px;font-weight:600;color:{col};letter-spacing:-.01em">{big}</div><div style="font-size:12px;color:#6b7280">{sub}</div>'
    rows = "".join(f'<div style="display:grid;grid-template-columns:minmax(0,1fr) 70px 70px;font-size:12px;padding:5px 0;border-top:1px solid #eef0f3;color:#2a2f36"><span class="mono" style="font-size:11.5px">{n}</span><span style="text-align:right">{c}</span><span style="text-align:right">{m}</span></div>' for n, c, m in [("checkout-api-7d9f8c6b5-x2kqp", "184m", "312Mi"), ("fraud-scorer-6f77d8c9b-wv5hn", "377m", "1.6Gi"), ("ledger-writer-0", "322m", "1.1Gi"), ("payment-gateway-5c8b7f9d4-7tgxs", "96m", "204Mi")])
    top_head = '<div style="display:grid;grid-template-columns:minmax(0,1fr) 70px 70px;font-size:11px;color:#6b7280"><span>POD</span><span style="text-align:right">CPU</span><span style="text-align:right">MEM</span></div>'
    web = f"""<div style="flex:1;min-height:0;background:#f4f5f7;color:#2a2f36;font-family:'IBM Plex Sans',system-ui,sans-serif;display:flex;flex-direction:column;overflow:hidden">
<div style="height:44px;flex-shrink:0;background:#ffffff;border-bottom:1px solid #dfe2e7;display:flex;align-items:center;gap:14px;padding:0 16px">
<span style="width:22px;height:22px;border-radius:5px;background:#e8590c;display:block"></span><span style="font-weight:600">Dashboards</span><span style="color:#9aa1ab">/</span><span>Kubernetes · Pods</span>
<div style="flex:1"></div>
<span style="font-size:12px;color:#4b5563;border:1px solid #dfe2e7;border-radius:4px;padding:3px 8px">namespace: payments</span>
<span style="font-size:12px;color:#4b5563;border:1px solid #dfe2e7;border-radius:4px;padding:3px 8px">Last 1 hour</span>
<span style="width:26px;height:26px;border-radius:50%;background:#cbd5e1;display:block"></span>
</div>
<div style="flex:1;padding:14px 16px;display:grid;grid-template-columns:repeat(3,minmax(0,1fr));grid-auto-rows:min-content;gap:12px;overflow:hidden">
{panel("Pods running", stat("17", "3 not ready", "#15803d"))}
{panel("CPU usage", stat("1.84", "cores · 38% of requests", "#1d4ed8"))}
{panel("Memory working set", stat("7.9 GiB", "71% of limits", "#b45309"))}
{panel("CPU by pod", wchart(0.8, "#2563eb"), 2)}
{panel("Restarts (1h)", stat("14", "payment-gateway-5c8b7f9d4-hl2vp", "#b91c1c"))}
{panel("Memory by pod", wchart(2.4, "#d97706"), 2)}
{panel("Top pods", top_head + rows)}
</div>
</div>"""
    toolbar = f"""<div class="tool" style="gap:4px;height:42px">
<button class="ib" aria-label="Back">{ic("left",14)}</button><button class="ib" aria-label="Forward">{ic("right",14)}</button><button class="ib" aria-label="Reload">{ic("refresh",13)}</button>
<div class="inp" style="flex:1;height:28px;margin:0 6px;gap:8px">
<span class="prod" style="font-size:9.5px;padding:0 4px">PROD</span>
<span class="chip mchip" style="height:19px">{ic("link",11,C["green"])}svc/grafana:80</span>
<span class="mono" style="font-size:12px;color:var(--dim)">http://127.0.0.1:52871</span><span class="mono" style="font-size:12px;color:var(--text);margin-left:-8px">/d/k8s-pods/pods?orgId=1&amp;var-namespace=payments</span>
<span style="flex:1"></span>
<span style="font-size:11.5px;color:var(--dim);display:flex;align-items:center;gap:4px">{ic("lock",11)}isolated session</span>
</div>
<span class="chip">100%</span>
<button class="btn g" style="height:26px">{ic("globe",13)}Open in browser</button>
<button class="ib" aria-label="Developer tools">{ic("code",14)}</button>
<button class="ib" aria-label="More">{ic("more",14)}</button>
</div>"""
    center = f'<div style="flex:1;display:flex;flex-direction:column;min-width:0">{toolbar}{web}</div>'
    port = lambda name, spec, btn: f'<div style="display:flex;align-items:center;gap:10px;padding:8px 0;border-bottom:1px solid var(--bv)"><div style="flex:1;min-width:0"><div class="mono" style="font-size:12px">{name}</div><div style="font-size:11.5px;color:var(--dim)">{spec}</div></div>{btn}</div>'
    openbtn = f'<button class="btn" style="height:24px;border-color:var(--accent);color:#a8cdf3">{ic("globe",12,C["accent"])}Open · 2 tabs</button>'
    wvbtn = f'<button class="btn" style="height:24px">{ic("globe",12)}Web view</button>'
    other = lambda svc, p, sub: f'<div style="display:flex;align-items:center;gap:10px;padding:7px 0"><div style="flex:1;min-width:0"><div class="mono" style="font-size:12px">{svc}<span style="color:var(--dim)">:{p}</span></div><div style="font-size:11.5px;color:var(--dim)">{sub}</div></div>{wvbtn}</div>'
    notweb = '<button class="btn g" style="height:24px">Open as web view…</button>'
    dock = f"""<aside class="dock" style="width:330px">
<div class="phead" style="border-bottom:1px solid var(--bv)">{ic("globe",13,C["accent"])}<span style="flex:1;color:var(--text);font-weight:500">svc/grafana</span><span style="font-size:11.5px">monitoring</span></div>
<div class="dsec"><p class="dtitle">Ports</p>
{port("http-web · 80 → 3000/TCP", "appProtocol http · start path /d/k8s-pods", openbtn)}
{port("grpc · 3001/TCP", "not HTTP", notweb)}
</div>
<div class="dsec"><p class="dtitle">Temporary forward</p>
<div style="display:flex;gap:10px;align-items:flex-start">{ic("link",14,C["green"])}<div style="flex:1;min-width:0;font-size:12px;line-height:18px">
<div class="mono" style="font-size:12px">127.0.0.1:52871 → svc/grafana:80</div>
<div style="color:var(--dim)">via pod grafana-6d8f9b7c4-k2x9p · 3 connections · 1.2 MB</div>
<div style="color:var(--muted);margin-top:4px">Hidden from saved forwards. Stops when the last web view tab closes.</div></div></div>
<div style="display:flex;gap:6px;margin-top:10px"><button class="btn d" style="height:24px">{ic("x",12)}Stop and close tabs</button></div>
</div>
<div class="dsec"><p class="dtitle">Session</p>
<dl class="kv" style="margin:0"><dt>Storage</dt><dd>isolated · prod-eu-west-1 / monitoring / grafana</dd><dt>Cookies</dt><dd>kept between sessions</dd><dt>Idle stop</dt><dd>after 30 min in background</dd></dl>
<div style="display:flex;gap:6px;margin-top:10px"><button class="btn g" style="height:24px">Clear site data</button><button class="btn g" style="height:24px">Private mode</button></div>
</div>
<div class="dsec" style="border-bottom:0"><p class="dtitle">Other web UIs in monitoring</p>
{other("prometheus-k8s", "9090", "appProtocol http · preset /graph")}
{other("alertmanager-main", "9093", "port name web")}
{other("kube-state-metrics", "8080", "port name http-metrics")}
</div>
</aside>"""
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    tb = f"""<div class="tabs">
<div class="tab">{ic("globe",13)}<span>Services</span><span class="tabx">{ic("x",11)}</span></div>
<div class="tab on">{dot(C["red"])}{ic("globe",13,C["accent"])}<span>grafana · Kubernetes / Pods</span><span class="tabx">{ic("x",11)}</span></div>
<div class="tab">{dot(C["red"])}{ic("globe",13)}<span>prometheus-k8s · /graph</span><span class="tabx">{ic("x",11)}</span></div>
<div class="tab">{ic("box",13)}<span>Pods</span><span class="tabx">{ic("x",11)}</span></div>
<div class="tabtools"><button class="ib" aria-label="New tab">{ic("plus",14)}</button><button class="ib" aria-label="Split">{ic("split",14)}</button><button class="ib" aria-label="Zoom">{ic("max",13)}</button></div>
</div>"""
    webfwd = f'<span style="color:var(--text)">{ic("globe",12,C["accent"])}2 web forwards</span>'
    inner = f"""<div class="app">
{titlebar(ns="monitoring")}
<div class="body">
{sidebar("none")}
<main class="main">{tb}{content}</main>
</div>
{statusbar(right_extra=webfwd)}
</div>"""
    return page("Service web view — Kubyl", inner)

SCREENS = [
 ("Main.dc.html", "1 · Pods (k9s-style table + details)", pods_screen),
 ("Logs.dc.html", "2 · Live logs, exec shell, port-forwards", logs_screen),
 ("Yaml.dc.html", "3 · YAML editor with CRD schema", yaml_screen),
 ("Overview.dc.html", "4 · Overview, Prometheus metrics, events", overview_screen),
 ("Clusters.dc.html", "5 · Kubeconfigs, contexts, OIDC sign-in", clusters_screen),
 ("Palette.dc.html", "6 · Command palette (:resources, @contexts)", palette_screen),
 ("Operators.dc.html", "7 · Operators (OLM) — OpenShift-style", operators_screen),
 ("Updates.dc.html", "8 · Cluster updates — OpenShift-style", updates_screen),
 ("Files.dc.html", "9 · Pod file browser — drag & drop upload/download", files_screen),
 ("Webview.dc.html", "10 · Service web view over a temporary port-forward", webview_screen),
]

boards, order = {}, []
for i, (fn, title, fnc) in enumerate(SCREENS):
    with open(os.path.join(ROOT, fn), "w") as f:
        f.write(fnc())
    col, row = i % 4, i // 4
    boards[fn] = {"x": col * (W + 80), "y": row * (H + 120 + 60), "w": W, "h": H, "title": title}
    order.append(fn)

canvas = {"v": 3, "createdOnFiles": {"v": 1, "at": "2026-09-24T08:44:10Z"}, "title": "Kubyl — Mockups",
          "launch": {"view": "canvas"}, "pages": [], "boards": boards, "order": order,
          "notes": {"t": {"x": 0, "y": -300, "text": "Kubyl — native Kubernetes client on GPUI (Zed One Dark)", "kind": "title1", "maxW": 5840}},
          "designSystems": []}
with open(os.path.join(ROOT, "canvas.json"), "w") as f:
    json.dump(canvas, f, indent=1)
print("ok", [os.path.getsize(os.path.join(ROOT, s[0])) for s in SCREENS])
