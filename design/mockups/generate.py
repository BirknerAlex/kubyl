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
 "branch": '<path d="M15 6a9 9 0 0 0-9 9V3"></path><circle cx="18" cy="6" r="3"></circle><circle cx="6" cy="18" r="3"></circle>',
 "commit": '<circle cx="12" cy="12" r="3"></circle><path d="M3 12h6M15 12h6"></path>',
 "history": '<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"></path><path d="M3 3v5h5"></path><path d="M12 7v5l4 2"></path>',
 "rollback": '<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"></path><path d="M3 3v5h5"></path>',
 "fork": '<circle cx="12" cy="18" r="3"></circle><circle cx="6" cy="6" r="3"></circle><circle cx="18" cy="6" r="3"></circle><path d="M18 9v2c0 .6-.4 1-1 1H7c-.6 0-1-.4-1-1V9"></path><path d="M12 12v3"></path>',
 "kanban": '<path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"></path><path d="M8 10v4M12 10v2M16 10v6"></path>',
 "tree": '<path d="M8 5h13M13 12h8M13 19h8"></path><path d="M3 10a2 2 0 0 0 2 2h3"></path><path d="M3 5v12a2 2 0 0 0 2 2h3"></path>',
 "ext": '<path d="M15 3h6v6M10 14 21 3"></path><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"></path>',
 "square": '<rect width="18" height="18" x="3" y="3" rx="2"></rect>',
 "check": '<path d="M20 6 9 17l-5-5"></path>',
 "save": '<path d="M15.2 3a2 2 0 0 1 1.4.6l3.8 3.8a2 2 0 0 1 .6 1.4V19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"></path><path d="M17 21v-7a1 1 0 0 0-1-1H8a1 1 0 0 0-1 1v7"></path><path d="M7 3v4a1 1 0 0 0 1 1h7"></path>',
 "flask": '<path d="M14.5 2v17.5c0 1.4-1.1 2.5-2.5 2.5s-2.5-1.1-2.5-2.5V2"></path><path d="M8.5 2h7"></path><path d="M14.5 16h-5"></path>',
 "fingerprint": '<path d="M2 12C2 6.5 6.5 2 12 2a10 10 0 0 1 8 4"></path><path d="M5 19.5C5.5 18 6 15 6 12c0-.7.12-1.37.34-2"></path><path d="M17.29 21.02c.12-.6.43-2.3.5-3.02"></path><path d="M12 10a2 2 0 0 0-2 2c0 1.02-.1 2.51-.26 4"></path><path d="M8.65 22c.21-.66.45-1.32.57-2"></path><path d="M14 13.12c0 2.38 0 6.38-1 8.88"></path><path d="M2 16h.01"></path><path d="M21.8 16c.2-2 .131-5.354 0-6"></path><path d="M9 6.8a6 6 0 0 1 9 5.2c0 .47 0 1.17-.02 2"></path>',
 "undo": '<path d="M9 14 4 9l5-5"></path><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11"></path>',
 "pencil": '<path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"></path>',
 "fileplus": '<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><path d="M14 2v6h6"></path><path d="M12 18v-6M9 15h6"></path>',
 "shieldalert": '<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10"></path><path d="M12 8v4M12 16h.01"></path>',
 "siren": '<path d="M7 18v-6a5 5 0 1 1 10 0v6"></path><path d="M5 21a1 1 0 0 1-1-1v-1a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v1a1 1 0 0 1-1 1z"></path><path d="M21 12h1"></path><path d="M18.5 4.5 18 5"></path><path d="M2 12h1"></path><path d="M12 2v1"></path><path d="m4.929 4.929.707.707"></path><path d="M12 12v6"></path>',
 "belloff": '<path d="M10.268 21a2 2 0 0 0 3.464 0"></path><path d="M17 17H4a1 1 0 0 1-.74-1.673C4.59 13.956 6 12.499 6 8a6 6 0 0 1 .258-1.742"></path><path d="m2 2 20 20"></path><path d="M8.668 3.01A6 6 0 0 1 18 8c0 2.687.77 4.653 1.707 6.05"></path>',
 "listchecks": '<path d="M13 5h8"></path><path d="M13 12h8"></path><path d="M13 19h8"></path><path d="m3 17 2 2 4-4"></path><path d="m3 7 2 2 4-4"></path>',
 "anchor": '<path d="M12 22V8"></path><path d="M5 12H2a10 10 0 0 0 20 0h-3"></path><circle cx="12" cy="5" r="3"></circle>',
 "route": '<circle cx="6" cy="19" r="3"></circle><path d="M9 19h8.5a3.5 3.5 0 0 0 0-7h-11a3.5 3.5 0 0 1 0-7H15"></path><circle cx="18" cy="5" r="3"></circle>',
 "flows": '<circle cx="12" cy="4.5" r="2.5"></circle><path d="m10.2 6.3-3.9 3.9"></path><circle cx="4.5" cy="12" r="2.5"></circle><path d="M7 12h10"></path><circle cx="19.5" cy="12" r="2.5"></circle><path d="m13.8 17.7 3.9-3.9"></path><circle cx="12" cy="19.5" r="2.5"></circle>',
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

def statusbar(left_extra="", right_extra="", cluster="prod-eu-west-1", ns="payments"):
    return f'''<footer class="status">
<span>{ic("split",13)}</span>
<span style="color:var(--text)">{dot(C["green"])}{cluster}</span>
<span>{ic("folder",12)}{ns}</span>
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

STATUS_TIPS = {"on": "Connected · 38 ms · v1.30.4", "connecting": "Connecting…", "key": "Sign-in required",
               "err": "Unreachable: dial tcp 192.168.1.40:6443: i/o timeout · retry in 20s", None: "Not connected"}

def status_slot(state):
    """Phase 15: the connection state at the right end of a cluster row. Green dot connected, a
    pulsing dim dot connecting, the yellow key for a sign-in, a red dot unreachable or forbidden,
    nothing when not connected. The error text lives in the tooltip and the expanded status row."""
    inner = {"on": dot(C["green"]),
             "connecting": f'<span class="dot" style="background:{C["dim"]};box-shadow:0 0 0 3px #959aa633"></span>',
             "key": ic("key", 12, C["yellow"]),
             "err": dot(C["red"])}.get(state, "")
    return f'<span title="{STATUS_TIPS[state]}" style="width:12px;display:flex;justify-content:center;flex-shrink:0">{inner}</span>'

PROD_MINI = '<span class="prod" style="font-size:9.5px;padding:0 4px">PROD</span>'

def alert_marker(n):
    """Phase 14: a red siren before the status slot while critical alerts fire (visible when the root is collapsed)."""
    return f'<span title="{n} critical alerts firing" style="display:flex">{ic("siren", 12, C["red"], 2)}</span>'

def alert_badge(n, col):
    """Phase 14: the Alerts row's badge: firing alerts in the color of the most severe, a check when all is clear."""
    if n == "ok":
        return f'<span title="No alerts firing" style="display:flex;margin-right:1px">{ic("check", 12, C["green"], 2.5)}</span>'
    return f'<span class="mono" style="font-size:10.5px;font-weight:600;line-height:15px;padding:0 5px;border-radius:8px;background:{col};color:#1e2127">{n}</span>'

def root(name, state=None, open_=False, color=C["dim"], prod=False, marker="", on=False):
    """A cluster root: the icon keeps the cluster's color tag in every state."""
    return ti(name, 0, "wheel", open_=open_, root=True, color=color, on=on,
              extra=(PROD_MINI if prod else "") + marker + status_slot(state))

def sidebar(active="Pods", cr_open=True, alerts=False, flows=False):
    a = lambda n: n == active
    fav = lambda ns, cl, col, src, faint=False: (f'<div class="ti{" on" if active=="fav:"+ns+cl else ""}" style="padding-left:12px" title="{src}{" · not connected" if faint else ""}">{ic("star",13,C["yellow"],1.6,C["yellow"])}'
                                   f'<span class="n"><span style="color:{C["dim"] if faint else C["text"]}">{ns}</span> <span style="color:{C["faint"] if faint else C["dim"]}">· {cl}</span></span>'
                                   f'<span style="display:flex">{dot(col)}</span></div>')
    rows = [
        f'<div class="phead"><span style="flex:1;font-weight:500;color:var(--text)">Explorer</span><button class="ib" aria-label="Filter kinds">{ic("search",13)}</button><button class="ib" aria-label="Add kubeconfig">{ic("plus",14)}</button><button class="ib" aria-label="More">{ic("more",14)}</button></div>',
        f'<div class="sec">{ic("cd",11)}Favorites<span style="flex:1"></span><span style="font-weight:400;letter-spacing:0;text-transform:none;color:var(--faint)">4</span></div>',
        fav("payments", "prod-eu-west-1", C["red"], "~/work/kube/eks-prod.yaml"),
        fav("payments", "staging-eu-west-1", C["yellow"], "~/.kube/config"),
        fav("checkout", "gke-analytics", C["cyan"], "~/Downloads/gke-analytics.yaml", True),
        fav("ingress", "platform-onprem", C["purple"], "~/work/kube/platform-onprem.yaml", True),
        '<div style="height:6px"></div>',
        f'<div class="sec">{ic("cd",11)}Clusters</div>',
        root("prod-eu-west-1", "on", True, C["red"], prod=True, marker=alert_marker(3) if alerts else ""),
        ti("Overview", 1, "gauge", on=a("Overview")),
        *([ti("Alerts", 1, "siren", on=a("Alerts"), extra=alert_badge("3", C["red"]))] if alerts else []),
        ti("Events", 1, "bell", "23", on=a("Events"), color=C["yellow"] if not a("Events") else None),
        *([ti("Network Flows", 1, "flows", on=a("Flows"))] if flows else []),
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
        ti("OperatorHub", 2, "store", on=a("OperatorHub")),
        ti("Helm Releases", 2, "anchor", on=a("Helm")),
        ti("Cluster Updates", 2, "up", extra=f'<span style="margin-right:2px">{dot(C["accent"])}</span>', on=a("Updates")),
        ti("Custom Resources", 1, open_=True),
        ti("cert-manager.io", 2, open_=True),
        ti("Certificates", 3, "file", "12", on=a("Certificates")),
        ti("Issuers", 3, "file", "3"),
        ti("monitoring.coreos.com", 2, open_=False),
        ti('<span style="color:var(--dim)">14 more API groups…</span>', 2),
        root("staging-eu-west-1", "on", color=C["yellow"]),
        root("gke-analytics", None, color=C["cyan"]),
        root("platform-onprem", "key", color=C["purple"]),
        root("homelab-k3s", "err"),
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
def clusters_screen(overlay=None, title="Clusters and kubeconfigs — Kubyl"):
    """Board 5. `overlay` replaces the OIDC sign-in dialog (board 11 reuses this screen behind its wizard)."""
    src = lambda path, sub, on=False, icon="file", warn="": f'<div style="display:flex;gap:10px;padding:10px 12px;border-radius:6px;{"background:var(--sel);outline:1px solid var(--accent);outline-offset:-1px" if on else ""}">{ic(icon,15,C["accent"] if on else C["dim"])}<div style="flex:1;min-width:0"><div class="mono" style="font-size:12px;color:var(--text)">{path}</div><div style="font-size:11.5px;color:var(--dim)">{sub}</div></div>{warn}</div>'
    left = f'''<div style="width:330px;flex-shrink:0;border-right:1px solid var(--bv);padding:16px 12px;display:flex;flex-direction:column;gap:4px">
<div style="display:flex;align-items:center;margin:0 4px 8px"><h2 style="font-size:14px;font-weight:600;flex:1">Kubeconfig sources</h2><button class="btn" style="height:24px">{ic("plus",12)}Add</button></div>
{src("~/.kube/config","3 contexts · watched for changes")}
{src("$KUBECONFIG","merged · 2 files",icon="zap")}
{src("~/work/kube/eks-prod.yaml","2 contexts · watched",True)}
{src("~/work/kube/platform-onprem.yaml","1 context · OIDC",warn=f'<span style="display:flex">{ic("key",13,C["yellow"])}</span>')}
{src("~/Downloads/gke-analytics.yaml","1 context")}
<div style="margin-top:10px;border:1px dashed var(--border);border-radius:8px;padding:18px 14px;text-align:center;color:var(--dim);font-size:12.5px;line-height:19px">{ic("upload",18)}<br>Drop kubeconfig files here<br><span style="font-size:12px">or <a href="#">browse</a> · <a href="#">paste YAML</a></span></div>
<div style="margin-top:auto;font-size:11.5px;color:var(--dim);line-height:17px;padding:0 4px">Only the kubeconfig editor writes files: Kubyl&#39;s own freely, others after you turn on editing. Contexts from every source are merged; name collisions get the file name as suffix.</div>
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
    content = f'<div style="flex:1;display:flex;min-height:0;min-width:0">{left}{right}</div>'
    tb = tabs([("gear", "Clusters &amp; kubeconfigs", True), ("gauge", "Overview", False), ("box", "Pods", False)])
    inner = f'''<div class="app">
{titlebar()}
<div class="body">
{sidebar("none")}
<main class="main">{tb}{content}</main>
</div>
{statusbar()}
{modal if overlay is None else overlay}
</div>'''
    return page(title, inner)

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

# ---------- 7. Operators (OLM), OperatorHub and Helm releases (phase 12) ----------
def op_tile(l, c, s=26):
    """A package's letter tile (the catalog's icon when it has one)."""
    return f'<span style="width:{s}px;height:{s}px;border-radius:6px;background:{c}22;color:{c};display:flex;align-items:center;justify-content:center;font-weight:600;font-size:{11 if s < 30 else 13}px;flex-shrink:0;border:1px solid {c}55">{l}</span>'

def op_subtabs(active, pending=1, v1=False, counts=("7", "8", "5"), olm=True):
    """The Operators tab's sub-tabs. OperatorHub is its own tab (the toolbar button opens it).
    Without OLM only Installed (which explains it) and Helm releases."""
    items = [("Installed", "blocks", counts[0]), ("Install plans", "listchecks", None), ("Subscriptions", "list", counts[1]), ("Helm releases", "anchor", counts[2])]
    if not olm:
        items = [items[0], items[3]]
    if v1:
        items.append(("Extensions", "blocks", "1"))
    out = []
    for t, i, n in items:
        on = t == active
        badge = ""
        if t == "Install plans" and pending:
            badge = f'<span class="chip" style="height:17px;color:var(--yellow)">{pending} pending</span>'
        elif n:
            badge = f'<span class="chip" style="height:17px">{n}</span>'
        style = "color:var(--text);box-shadow:inset 0 -2px 0 var(--accent)" if on else "color:var(--dim)"
        out.append(f'<span style="display:flex;align-items:center;gap:6px;padding:0 10px;white-space:nowrap;{style}">{ic(i,13,C["accent"] if on else C["dim"])}{t}{badge}</span>')
    return f'<div style="display:flex;gap:2px;padding:0 8px;border-bottom:1px solid var(--bv);height:34px;align-items:stretch;flex-shrink:0">{"".join(out)}</div>'

def op_header(summary, filter_text="Filter operators", button=True):
    b = f'<button class="btn p">{ic("store",13,"#1b1e24")}Browse OperatorHub</button>' if button else ""
    return (f'<div class="tool"><div class="crumb">{ic("blocks",14,C["accent"])}<b>Operators</b><span>·</span><span>{summary}</span></div><div style="flex:1"></div>'
            f'<div class="inp" style="width:220px">{ic("search",12)}{filter_text}</div>{b}</div>')

def op_shell(active, tabbar, content, overlay=""):
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
{overlay}
</div>'''

def op_tabs(on):
    return tabs([("blocks", "Operators", on == "ops"), ("store", "OperatorHub", on == "hub"), ("up", "Cluster Updates", False)])

OPS = [
 ("cert-manager", "cert-manager", C["green"], "CM", "1.16.5", "Succeeded", "stable", "Automatic", "operators", ["Certificate", "Issuer", "ClusterIssuer", "+3"]),
 ("Prometheus Operator", "prometheus", C["orange"], "PR", "0.76.1", "Succeeded", "beta", "Automatic", "operators", ["Prometheus", "ServiceMonitor", "+5"]),
 ("Strimzi", "strimzi-kafka-operator", C["cyan"], "SZ", "0.43.0", "Upgrade available", "stable", "Manual", "kafka", ["Kafka", "KafkaTopic", "KafkaUser", "+6"]),
 ("CloudNativePG", "cloudnative-pg", C["accent"], "PG", "1.30.0", "Succeeded", "stable-v1", "Automatic", "databases", ["Cluster", "Backup", "Pooler", "+8"]),
 ("Argo CD", "argocd-operator", C["orange"], "AR", "0.11.0", "Succeeded", "alpha", "Automatic", "operators", ["ArgoCD", "Application"]),
 ("External Secrets", "external-secrets-operator", C["purple"], "ES", "0.10.3", "Installing", "stable", "Automatic", "operators", ["ExternalSecret", "SecretStore"]),
 ("Sail (Istio)", "sailoperator", C["accent"], "IS", "0.2.0", "Failed", "candidates", "Manual", "istio-system", ["Istio", "IstioRevision"]),
]
OC = "grid-template-columns: minmax(190px,1.3fr) 76px 150px 90px 86px 96px minmax(0,1.1fr)"

def op_rows(selected=2):
    out = []
    for i, (n, pkg, c, t, v, s, ch, ap, ns, apis) in enumerate(OPS):
        chips = "".join(f'<span class="chip">{a}</span>' for a in apis)
        apc = C["yellow"] if ap == "Manual" else C["muted"]
        out.append(f'''<div class="tr{" on" if i == selected else ""}" style="{OC};height:46px">
<div style="display:flex;gap:10px;align-items:center;min-width:0">{op_tile(t,c)}<div style="min-width:0"><div style="font-weight:500">{n}</div><div class="mono" style="font-size:11px;color:var(--dim)">{pkg}</div></div></div>
<span class="mono">{v}</span>{st(s)}<span class="mono" style="color:var(--muted)">{ch}</span><span style="color:{apc}">{ap}</span><span class="mono" style="font-size:11.5px;color:var(--muted)">{ns}</span>
<div style="display:flex;gap:4px;overflow:hidden">{chips}</div></div>''')
    return "".join(out)

OP_HEAD = f'<div class="th" style="{OC}"><span>NAME {ic("cd",10)}</span><span>VERSION</span><span>STATUS</span><span>CHANNEL</span><span>APPROVAL</span><span>NAMESPACE</span><span>PROVIDED APIS</span></div>'
OP_HINTS = [("↵", "Details"), ("a", "Approve…"), ("d", "Review changes"), ("c", "Create instance…"), ("y", "View YAML"), ("⌃d", "Uninstall…"), ("/", "Filter")]

def op_installed_center(selected=2):
    return f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{op_header("OLM v0 · 7 installed · 1 upgrade waiting")}
{op_subtabs("Installed")}
{OP_HEAD}
<div style="flex:1;overflow:hidden">{op_rows(selected)}</div>
{hints(OP_HINTS)}
</div>'''

def op_api(k, n, g):
    return (f'<div style="display:flex;align-items:center;gap:8px;padding:6px 10px;border:1px solid var(--bv);border-radius:6px;background:#2a2e36">'
            f'<div style="flex:1;min-width:0"><div class="mono" style="font-size:12px">{k}</div><div class="mono" style="font-size:10.5px;color:var(--dim)">{g}</div></div>'
            f'<a href="#" style="font-size:11.5px;text-decoration:none">{n}</a><button class="btn g" style="height:22px;padding:0 6px">{ic("plus",11)}Create</button></div>')

def op_chg(icon, col, t):
    return f'<div style="display:flex;gap:8px;font-size:12px;align-items:flex-start;padding:3px 0">{ic(icon,13,col)}<span style="color:var(--muted)">{t}</span></div>'

MONO11 = 'class="mono" style="font-size:11px"'

def operators_screen():
    dock = f'''<aside class="dock" style="width:360px">
<div class="phead" style="border-bottom:1px solid var(--bv)">{op_tile("SZ",C["cyan"])}<div style="flex:1;margin-left:4px;min-width:0"><div style="color:var(--text);font-weight:500">Strimzi</div><div style="font-size:11px;color:var(--dim)">0.43.0 · kafka · Manual approval</div></div><button class="ib" aria-label="More">{ic("more",14)}</button><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec" style="background:#35322a"><div style="display:flex;gap:8px;align-items:center;margin-bottom:8px">{ic("up",14,C["yellow"])}<b style="font-weight:600;color:var(--yellow)">Upgrade waiting for approval</b></div>
<div class="mono" style="font-size:12px;margin-bottom:10px">0.43.0 <span style="color:var(--dim)">→</span> <span style="color:var(--green)">0.44.0</span> <span style="color:var(--dim)">· install-7qk2d</span></div>
{op_chg("file",C["accent"],f"3 CRDs change · kafkas.kafka.strimzi.io adds <span {MONO11}>spec.kafka.tieredStorage</span>")}
{op_chg("shield",C["yellow"],f"RBAC: gains <span {MONO11}>get, list</span> on <span {MONO11}>nodes</span> (cluster-wide)")}
{op_chg("ok",C["green"],"Kubernetes 1.30.4 meets minKubeVersion 1.25.0")}
<div style="display:flex;gap:6px;margin-top:10px"><button class="btn p">{ic("check",12,"#1b1e24")}Approve…</button><button class="btn">{ic("diff",12)}Review changes</button><button class="btn g">{ic("code",12)}YAML</button></div></div>
<div class="dsec"><p class="dtitle">Provided APIs</p><div style="display:flex;flex-direction:column;gap:6px">{op_api("Kafka","4 instances","kafka.strimzi.io/v1beta2")}{op_api("KafkaTopic","112 instances","kafka.strimzi.io/v1beta2")}{op_api("KafkaUser","38 instances","kafka.strimzi.io/v1beta2")}{op_api("KafkaConnect","none","kafka.strimzi.io/v1beta2")}</div></div>
<div class="dsec"><p class="dtitle">Subscription</p><dl class="kv" style="margin:0"><dt>Catalog</dt><dd>operatorhubio-catalog <span style="color:var(--dim)">· olm</span></dd><dt>Channel</dt><dd>stable</dd><dt>Installed CSV</dt><dd class="mono" style="font-size:11.5px">strimzi-cluster-operator.v0.43.0</dd><dt>OperatorGroup</dt><dd class="mono" style="font-size:11.5px">kafka/kafka-og <span style="color:var(--dim);font-family:IBM Plex Sans">· OwnNamespace</span></dd></dl></div>
<div class="dsec" style="border-bottom:0"><div style="display:flex;gap:6px"><button class="btn g" style="height:24px">{ic("code",12)}CSV YAML</button><button class="btn g" style="height:24px">{ic("list",12)}Operator logs</button><span style="flex:1"></span><button class="btn d" style="height:24px">{ic("trash",12,C["red"])}Uninstall…</button></div></div>
</aside>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{op_installed_center()}{dock}</div>'
    return page("Installed operators — Kubyl", op_shell("Operators", op_tabs("ops"), content))

# name, provider, tile, color, description, capability, installed
HUB = [
 ("cert-manager", "The cert-manager maintainers", "CM", C["green"], "Cloud native certificate management: X.509 certificates from ACME, Vault, Venafi or self-signed.", "Deep Insights", True),
 ("CloudNativePG", "CloudNativePG", "PG", C["accent"], "PostgreSQL clusters with streaming replication, backups to object storage and rolling updates.", "Auto Pilot", True),
 ("Strimzi", "Strimzi", "SZ", C["cyan"], "Apache Kafka clusters, topics and users as Kubernetes resources.", "Deep Insights", True),
 ("Grafana Operator", "Grafana Labs", "GR", C["orange"], "Deploys and manages Grafana instances, dashboards and data sources.", "Deep Insights", False),
 ("Keycloak Operator", "Red Hat", "KC", C["red"], "An operator for Keycloak identity and access management servers and realms.", "Deep Insights", False),
 ("MariaDB Operator", "mariadb-operator", "MD", C["purple"], "MariaDB and MaxScale clusters with backups, restores and replication.", "Seamless Upgrades", False),
 ("OpenTelemetry Operator", "Community", "OT", C["yellow"], "Collectors and auto-instrumentation for OpenTelemetry.", "Seamless Upgrades", False),
 ("Redis Operator", "OT-Container-Kit", "RD", C["red"], "Redis standalone, cluster and replication setups with sentinel.", "Basic Install", False),
 ("Sealed Secrets", "Bitnami", "SS", C["accent"], "Encrypt Secrets into SealedSecrets, safe to store in Git.", "Basic Install", False),
]

def hub_card(n, prov, t, c, desc, cap, inst, on=False):
    badge = f'<span class="chip" style="height:18px;color:var(--green)">{ic("check",10,C["green"],2.5)}Installed</span>' if inst else ""
    border = C["accent"] if on else "var(--border)"
    bg = "var(--sel)" if on else "#2a2e36"
    return f'''<div style="display:flex;flex-direction:column;gap:8px;padding:12px;border:1px solid {border};background:{bg};border-radius:8px;min-width:0">
<div style="display:flex;gap:10px;align-items:center">{op_tile(t,c,34)}<div style="flex:1;min-width:0"><div style="font-weight:500;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{n}</div><div style="font-size:11.5px;color:var(--dim);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{prov}</div></div></div>
<div style="font-size:12px;color:var(--muted);line-height:17px;height:34px;overflow:hidden">{desc}</div>
<div style="display:flex;gap:4px;align-items:center"><span class="chip" style="height:18px">{cap}</span>{badge}</div></div>'''

def hub_filters():
    cats = [("All", 449, True), ("Cloud Provider", 87, False), ("Integration & Delivery", 76, False), ("Database", 61, False), ("Developer Tools", 54, False),
            ("Security", 49, False), ("Monitoring", 45, False), ("Storage", 39, False), ("Networking", 31, False), ("Streaming & Messaging", 21, False), ("AI/Machine Learning", 19, False)]
    crow = "".join(f'<div style="display:flex;align-items:center;height:24px;padding:0 8px;border-radius:5px;font-size:12.5px;{"background:var(--sel);color:var(--text)" if on else "color:var(--muted)"}"><span style="flex:1">{n.replace("&", "&amp;")}</span><span class="mono" style="font-size:11px;color:var(--dim)">{k}</span></div>' for n, k, on in cats)
    caps = "".join(check(on, n) for n, on in [("Basic Install", False), ("Seamless Upgrades", False), ("Full Lifecycle", False), ("Deep Insights", True), ("Auto Pilot", True)])
    return f'''<div style="width:220px;flex-shrink:0;border-right:1px solid var(--bv);padding:12px 10px;display:flex;flex-direction:column;gap:14px;overflow:hidden">
<div><p class="dtitle" style="padding-left:8px">Category</p>{crow}</div>
<div style="padding-left:8px"><p class="dtitle">Capability level</p><div style="display:flex;flex-direction:column;gap:6px">{caps}</div></div>
<div style="padding-left:8px"><p class="dtitle">Provider</p><button class="btn" style="width:100%;justify-content:space-between;height:26px">Any provider{ic("cd",12)}</button></div>
<div style="padding-left:8px"><p class="dtitle">Catalog</p><div style="display:flex;flex-direction:column;gap:6px">{check(True, "Community Operators", "olm/operatorhubio-catalog · READY")}</div></div>
</div>'''

def hub_center(selected=0, query=""):
    cards = "".join(hub_card(*h, on=i == selected) for i, h in enumerate(HUB))
    return f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div class="tool"><div class="crumb">{ic("store",14,C["accent"])}<b>OperatorHub</b><span>·</span><span>449 packages from 1 catalog · 3 installed</span></div><div style="flex:1"></div>
<div class="inp{" focus" if query else ""}" style="width:260px">{ic("search",12)}<span style="color:{"var(--text)" if query else "var(--dim)"}">{query or "Search packages, providers, APIs"}</span></div><button class="btn g" style="height:26px;padding:0 6px">Sort: Relevance{ic("cd",11)}</button></div>
<div style="flex:1;display:flex;min-height:0">{hub_filters()}
<div style="flex:1;min-width:0;padding:14px;display:grid;grid-template-columns:repeat(3,minmax(0,1fr));grid-auto-rows:min-content;gap:12px;overflow:hidden">{cards}</div></div>
{hints([("↵","Details"),("i","Install…"),("/","Search"),("⇧r","Reload catalogs")])}
</div>'''

def hub_details():
    kinds = "".join(f'<span class="chip mchip">{k}</span>' for k in ["Certificate", "CertificateRequest", "Issuer", "ClusterIssuer", "Challenge", "Order"])
    return f'''<aside class="dock" style="width:360px">
<div class="phead" style="border-bottom:1px solid var(--bv);height:auto;padding:12px 12px 12px 14px;align-items:flex-start">{op_tile("CM",C["green"],34)}<div style="flex:1;margin-left:6px;min-width:0"><div style="color:var(--text);font-weight:600;font-size:14px">cert-manager</div><div style="font-size:11.5px;color:var(--dim)">The cert-manager maintainers · Community Operators</div></div><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec"><div style="display:flex;gap:8px;align-items:center"><button class="btn p">{ic("download",13,"#1b1e24")}Install…</button><span style="font-size:12px;color:var(--green);display:flex;gap:5px;align-items:center">{ic("check",12,C["green"],2.5)}1.16.5 installed in operators</span></div></div>
<div class="dsec"><dl class="kv" style="margin:0"><dt>Latest</dt><dd class="mono" style="font-size:11.5px">1.16.5 <span style="color:var(--dim);font-family:IBM Plex Sans">· stable (default)</span></dd><dt>Channels</dt><dd>stable · candidate</dd><dt>Capability</dt><dd>Deep Insights</dd><dt>Install modes</dt><dd>All namespaces</dd><dt>Min Kubernetes</dt><dd class="mono" style="font-size:11.5px">1.19.0</dd><dt>Repository</dt><dd><a href="#">github.com/cert-manager/cert-manager</a></dd></dl></div>
<div class="dsec"><p class="dtitle">Description</p><div style="font-size:12.5px;line-height:18px;color:var(--muted)">cert-manager is a Kubernetes add-on to automate the management and issuance of TLS certificates from various issuing sources. It periodically ensures certificates are valid and up to date, and attempts to renew them before they expire. <a href="#" style="text-decoration:none">More</a></div></div>
<div class="dsec" style="border-bottom:0"><p class="dtitle">Provided APIs</p><div style="display:flex;gap:4px;flex-wrap:wrap">{kinds}</div></div>
</aside>'''

def operatorhub_screen():
    content = f'<div style="flex:1;display:flex;min-height:0">{hub_center()}{hub_details()}</div>'
    return page("OperatorHub — Kubyl", op_shell("OperatorHub", op_tabs("hub"), content))

def radio(on, label, sub=""):
    dotc = '<span style="width:7px;height:7px;border-radius:50%;background:var(--accent)"></span>' if on else ""
    border = C["accent"] if on else "var(--border)"
    ring = C["accent"] if on else C["faint"]
    subl = f'<div style="font-size:11.5px;color:var(--dim)">{sub}</div>' if sub else ""
    return f'''<div style="display:flex;gap:10px;align-items:flex-start;padding:9px 10px;border-radius:7px;border:1px solid {border};{"background:var(--sel)" if on else ""}">
<span style="margin-top:2px;width:14px;height:14px;border-radius:50%;border:1px solid {ring};display:flex;align-items:center;justify-content:center;flex-shrink:0;box-sizing:border-box">{dotc}</span>
<div style="min-width:0"><div style="font-size:12.5px">{label}</div>{subl}</div></div>'''

def form_row(k, v):
    return f'<div style="display:flex;gap:12px;align-items:flex-start"><span style="width:104px;flex-shrink:0;font-size:12px;color:var(--dim);padding-top:5px">{k}</span><div style="flex:1;min-width:0">{v}</div></div>'

def install_screen():
    channel = f'<div style="display:flex;gap:8px"><button class="btn" style="width:190px;justify-content:space-between">v5 <span style="color:var(--dim)">(default)</span>{ic("cd",12)}</button><button class="btn" style="width:170px;justify-content:space-between"><span class="mono" style="font-size:12px">5.20.0</span> <span style="color:var(--dim)">latest</span>{ic("cd",12)}</button></div>'
    mode = (radio(False, "All namespaces", f"Into <span {MONO11}>operators</span> with the OperatorGroup <span {MONO11}>global-operators</span>; watches every namespace")
            + '<div style="height:6px"></div>' + radio(True, "A specific namespace", "The operator watches only the namespace it's installed in"))
    ns = '<div class="inp focus" style="height:28px"><span class="mono" style="font-size:12.5px;color:var(--text)">grafana</span></div><div style="font-size:11.5px;color:var(--dim);margin-top:5px">Suggested by the operator. It doesn\'t exist yet: Kubyl creates it with an OperatorGroup for it.</div>'
    approval = '<div style="display:flex;gap:6px"><span class="chip on" style="height:24px;padding:0 10px">Automatic</span><span class="chip" style="height:24px;padding:0 10px">Manual</span></div><div style="font-size:11.5px;color:var(--dim);margin-top:5px">Upgrades in the channel install as soon as the catalog has them.</div>'
    creates = "".join(f'<div style="display:flex;gap:8px;align-items:center">{ic("plus",12,C["green"])}{k} <span class="mono" style="font-size:11.5px">{n}</span><span style="color:var(--dim)">{d}</span></div>'
                      for k, n, d in [("Namespace", "grafana", ""), ("OperatorGroup", "grafana/grafana", "· targets grafana"), ("Subscription", "grafana/grafana-operator", "· v5 · Automatic · starts at 5.20.0")])
    modal = f'''<div style="position:absolute;inset:0;background:rgba(15,17,21,.55);display:flex;align-items:flex-start;justify-content:center;padding-top:70px">
<div role="dialog" aria-label="Install Grafana Operator" style="width:600px;background:#2f343e;border:1px solid var(--border);border-radius:10px;box-shadow:0 20px 60px rgba(0,0,0,.5);overflow:hidden">
<div style="display:flex;align-items:center;gap:10px;padding:14px 16px;border-bottom:1px solid var(--bv)">{op_tile("GR",C["orange"])}<b style="font-weight:600;flex:1">Install Grafana Operator</b><span style="font-size:12px;color:var(--dim)">Community Operators</span></div>
<div style="padding:16px;display:flex;flex-direction:column;gap:14px">
{form_row("Channel", channel)}
{form_row("Install mode", mode)}
{form_row("Namespace", ns)}
{form_row("Approval", approval)}
<div class="card" style="background:#2a2e36;padding:10px 12px;display:flex;flex-direction:column;gap:5px;font-size:12.5px">
<div style="font-size:11px;font-weight:600;letter-spacing:.06em;color:var(--dim);text-transform:uppercase;margin-bottom:2px">What Kubyl creates</div>
{creates}
<div style="color:var(--dim);font-size:12px;margin-top:2px">OLM then installs the CRDs, a Deployment and its RBAC (the install plan lists them).</div></div>
</div>
<div style="display:flex;align-items:center;gap:8px;padding:12px 16px;border-top:1px solid var(--bv)"><span style="font-size:11.5px;color:var(--dim)">prod-eu-west-1 asks for its name on the next step</span><span style="flex:1"></span><button class="btn g">Cancel</button><button class="btn p">{ic("download",12,"#1b1e24")}Install</button></div>
</div></div>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{hub_center(3, "grafana")}</div>'
    return page("Install an operator — Kubyl", op_shell("OperatorHub", op_tabs("hub"), content, modal))

def diff_line(kind, text):
    bg, fg = {"+": ("#2f3b2c", C["green"]), "-": ("#3e2c2f", C["red"]), " ": ("transparent", C["muted"]), "@": ("#2a2e36", C["dim"])}[kind]
    sign = kind if kind in "+-" else ""
    return f'<div class="mono" style="display:flex;height:20px;align-items:center;font-size:12px;white-space:pre;background:{bg};color:{fg}"><span style="width:18px;text-align:center;color:var(--faint)">{sign}</span>{text}</div>'

def upgrade_screen():
    crds = [("kafkas.kafka.strimzi.io", "+ 6 lines · adds spec.kafka.tieredStorage", True), ("kafkanodepools.kafka.strimzi.io", "v1beta1 no longer served", False), ("kafkarebalances.kafka.strimzi.io", "~ 2 lines", False)]
    clist = "".join(f'<div style="display:flex;flex-direction:column;padding:7px 10px;border-radius:6px;{"background:var(--sel);outline:1px solid var(--accent);outline-offset:-1px" if on else ""}"><span class="mono" style="font-size:11.5px">{n}</span><span style="font-size:11.5px;color:var(--dim)">{d}</span></div>' for n, d, on in crds)
    lines = [("@", "   versions[v1beta2].schema.openAPIV3Schema.properties.spec.properties.kafka"), (" ", "   properties:"), (" ", "     storage:"), (" ", "       type: object"),
             ("+", "     tieredStorage:"), ("+", "       description: Configure the tiered storage feature."), ("+", "       properties:"), ("+", "         remoteStorageManager:"), ("+", "           type: object"), ("+", "       type: object"),
             (" ", "     version:"), (" ", "       type: string"), ("@", "   versions[v1beta1]"), ("-", " served: true"), ("+", " served: false"), (" ", " storage: false")]
    dl = "".join(diff_line(k, t) for k, t in lines)
    rbac = [("+", "get, list", "nodes", "", "cluster-wide"), ("+", "create", "events", "events.k8s.io", "kafka"), ("-", "delete", "pods/exec", "", "kafka")]
    rl = "".join(f'<div style="display:grid;grid-template-columns:16px 110px minmax(0,1fr) 90px;gap:8px;align-items:center;font-size:12px;padding:4px 0;border-bottom:1px solid var(--bv)"><span style="color:{C["green"] if s == "+" else C["red"]};font-weight:600">{s}</span><span class="mono" style="font-size:11.5px">{v}</span><span class="mono" style="font-size:11.5px">{r}{f" <span style=color:var(--dim)>({g})</span>" if g else ""}</span><span style="color:var(--dim)">{sc}</span></div>' for s, v, r, g, sc in rbac)
    modal = f'''<div style="position:absolute;inset:0;background:rgba(15,17,21,.55);display:flex;align-items:flex-start;justify-content:center;padding-top:50px">
<div role="dialog" aria-label="Approve the Strimzi upgrade" style="width:1040px;background:#2f343e;border:1px solid var(--border);border-radius:10px;box-shadow:0 20px 60px rgba(0,0,0,.5);overflow:hidden">
<div style="display:flex;align-items:center;gap:10px;padding:14px 16px;border-bottom:1px solid var(--bv)">{op_tile("SZ",C["cyan"])}<b style="font-weight:600">Upgrade Strimzi</b><span class="mono" style="font-size:12.5px">0.43.0 <span style="color:var(--dim)">→</span> <span style="color:var(--green)">0.44.0</span></span><span style="flex:1"></span><span style="font-size:12px;color:var(--dim)">install plan kafka/install-7qk2d · RequiresApproval</span><span class="prod" style="font-size:9.5px;padding:0 4px">PROD</span></div>
<div style="display:flex;height:440px">
<div style="width:250px;flex-shrink:0;border-right:1px solid var(--bv);padding:10px;display:flex;flex-direction:column;gap:4px">
<p class="dtitle" style="padding-left:4px">CRDs · 3 of 4 change</p>{clist}
<p class="dtitle" style="padding-left:4px;margin-top:10px">Also in the plan</p>
<div style="font-size:12px;color:var(--muted);padding:0 4px;line-height:19px">CSV strimzi-cluster-operator.v0.44.0<br>2 ClusterRoles, 1 Role and their bindings<br>ServiceAccount strimzi-cluster-operator</div>
<div style="flex:1"></div><div style="font-size:11px;color:var(--dim);padding:0 4px;line-height:16px">From the bundle OLM unpacked (ConfigMap in olm), compared with the live CRDs.</div></div>
<div style="flex:1;min-width:0;display:flex;flex-direction:column">
<div style="display:flex;align-items:center;gap:8px;height:34px;padding:0 12px;border-bottom:1px solid var(--bv);font-size:12px"><span class="mono">kafkas.kafka.strimzi.io</span><span style="color:var(--dim)">· live → bundle</span><span style="flex:1"></span><span class="chip on" style="height:18px">spec only</span><span class="chip" style="height:18px">side by side</span></div>
<div style="flex:1;overflow:hidden;padding:4px 0">{dl}</div></div>
<div style="width:330px;flex-shrink:0;border-left:1px solid var(--bv);padding:12px 14px;display:flex;flex-direction:column;gap:12px">
<div><p class="dtitle">RBAC · operator service account</p>{rl}</div>
<div><p class="dtitle">Compatibility</p>
{op_chg("ok",C["green"],"Kubernetes 1.30.4 meets minKubeVersion 1.25.0")}
{op_chg("alert",C["yellow"],"kafkanodepools: v1beta1 stops being served, but is still in storedVersions")}
{op_chg("ok",C["green"],"Install mode OwnNamespace is still supported")}</div>
<div><p class="dtitle">Instances</p><div style="font-size:12px;color:var(--muted);line-height:18px">4 Kafka, 112 KafkaTopic, 38 KafkaUser in 3 namespaces keep running; the new operator reconciles them.</div></div></div>
</div>
<div style="display:flex;align-items:center;gap:10px;padding:12px 16px;border-top:1px solid var(--bv)"><span style="font-size:12px;color:var(--muted)">Type <span class="mono" style="color:var(--text)">strimzi-kafka-operator</span> to approve on prod-eu-west-1</span><div class="inp focus" style="width:220px;height:28px"><span class="mono" style="font-size:12.5px;color:var(--text)">strimzi-kafka</span></div><span style="flex:1"></span><button class="btn g">{ic("code",12)}Plan YAML</button><button class="btn g">Close</button><button class="btn p" style="opacity:.55">{ic("check",12,"#1b1e24")}Approve</button></div>
</div></div>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{op_installed_center()}</div>'
    return page("Approve an operator upgrade — Kubyl", op_shell("Operators", op_tabs("ops"), content, modal))

def yl(key, ind=0, val=None, vc=None):
    vc = vc or C["green"]
    out = " " * ind + f'<span style="color:{C["red"]}">{key}</span><span style="color:var(--muted)">:</span>'
    if val is not None:
        out += f' <span style="color:{vc}">{val}</span>'
    return out

def create_screen():
    N = C["orange"]
    L = [yl("apiVersion", 0, "kafka.strimzi.io/v1beta2"), yl("kind", 0, "Kafka"), yl("metadata"), yl("name", 2, "my-cluster"), yl("namespace", 2, "kafka"), yl("spec"), yl("kafka", 2), yl("version", 4, "3.8.0"),
         yl("replicas", 4, "3", N), yl("listeners", 4), '    <span style="color:var(--faint)">- </span>' + yl("name", 0, "plain"), yl("port", 6, "9092", N), yl("type", 6, "internal"), yl("tls", 6, "false", N),
         yl("config", 4), yl("offsets.topic.replication.factor", 6, "3", N), yl("storage", 4), yl("type", 6, "ephemeral"), yl("zookeeper", 2), yl("replicas", 4, "3", N), yl("storage", 4), yl("type", 6, "ephemeral"),
         yl("entityOperator", 2), yl("topicOperator", 4, "{}", C["muted"]), yl("userOperator", 4, "{}", C["muted"])]
    rows = "".join(f'<div class="mono" style="display:flex;height:22px;align-items:center;font-size:13px;white-space:pre"><span style="width:44px;text-align:right;padding-right:12px;color:var(--faint)">{i + 1}</span><span style="width:3px;height:22px;background:{C["green"]}"></span><span style="padding-left:14px">{t}</span></div>' for i, t in enumerate(L))
    editor = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div class="tool"><div class="crumb mono" style="font-size:12px">{ic("fileplus",13,C["accent"])}<span>kafka.strimzi.io/v1beta2</span><span>›</span><b>New Kafka</b><span style="font-family:IBM Plex Sans">in</span><b>kafka</b></div><div style="flex:1"></div>
<button class="btn g">{ic("play",12)}Dry run</button><button class="btn p">Apply{ic("cd",11,"#1b1e24")}</button></div>
<div style="display:flex;gap:10px;align-items:center;padding:8px 14px;background:#2d3b4d;border-bottom:1px solid #3f5a78;font-size:12.5px">{ic("info",14,C["accent"])}<span style="flex:1">Example from <b style="font-weight:500">strimzi-cluster-operator.v0.43.0</b> (alm-examples). Review it before applying: examples are often minimal.</span></div>
<div style="flex:1;overflow:hidden;padding-top:6px;background:var(--bg)">{rows}</div>
<div class="hints"><span><b>⌘S</b>Apply</span><span><b>⌘⇧S</b>Dry run</span><span><b>⌘⇧M</b>Problems</span><span style="color:var(--green);display:flex;gap:5px;align-items:center">{ic("ok",12,C["green"])}valid against the CRD schema</span></div></div>'''
    apis = [("Kafka", "4 instances"), ("KafkaTopic", "112 instances"), ("KafkaUser", "38 instances"), ("KafkaConnect", "none"), ("KafkaNodePool", "6 instances"), ("KafkaRebalance", "none")]
    dock = f'''<aside class="dock" style="width:330px">
<div class="phead" style="border-bottom:1px solid var(--bv)">{op_tile("SZ",C["cyan"])}<span style="flex:1;color:var(--text);font-weight:500;margin-left:4px">Strimzi</span><span style="font-size:11.5px">0.43.0 · kafka</span></div>
<div class="dsec"><p class="dtitle">Provided APIs</p><div style="display:flex;flex-direction:column;gap:6px">{"".join(op_api(k, n, "kafka.strimzi.io/v1beta2") for k, n in apis)}</div>
<div style="font-size:11.5px;color:var(--dim);margin-top:8px;line-height:17px">Create opens the YAML editor with the operator's example for the kind (its <span class="mono">alm-examples</span>), in the namespace the operator watches. Kinds without an example get the schema skeleton.</div></div>
</aside>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{editor}{dock}</div>'
    tb = tabs([("blocks", "Operators", False), ("fileplus", "New Kafka", True, True), ("store", "OperatorHub", False)])
    return page("Create an instance from alm-examples — Kubyl", op_shell("Operators", tb, content))

PLANS = [
 ("install-7qk2d", "kafka", "strimzi-cluster-operator.v0.44.0", "Manual", "RequiresApproval", "12m"),
 ("install-54wd4", "databases", "cloudnative-pg.v1.30.1", "Manual", "RequiresApproval", "3h"),
 ("install-hx9f2", "operators", "external-secrets-operator.v0.10.3", "Automatic", "Installing", "2m"),
 ("install-sb2lq", "istio-system", "sailoperator.v0.2.0", "Manual", "Failed", "1d"),
 ("install-fq4sx", "databases", "cloudnative-pg.v1.30.0", "Manual", "Complete", "3h"),
 ("install-9zzt4", "operators", "cert-manager.v1.16.5", "Automatic", "Complete", "2d"),
 ("install-mk2c7", "operators", "prometheusoperator.0.76.1", "Automatic", "Complete", "6d"),
 ("install-7w2vn", "kafka", "strimzi-cluster-operator.v0.43.0", "Manual", "Complete", "21d"),
]
PC = "grid-template-columns: 130px 110px minmax(0,1.4fr) 90px 150px 60px"

def plans_screen():
    pcol = {"RequiresApproval": C["yellow"], "Installing": C["accent"], "Failed": C["red"], "Complete": C["green"]}
    def prow(i, n, ns, csv, ap, ph, age):
        return (f'<div class="tr{" on" if i == 0 else ""}" style="{PC};height:32px"><span class="mono">{n}</span><span class="mono" style="color:var(--muted)">{ns}</span><span class="mono" style="font-size:11.5px">{csv}</span>'
                f'<span style="color:{C["yellow"] if ap == "Manual" else C["muted"]}">{ap}</span><span class="pill">{dot(pcol[ph])}<span style="color:{pcol[ph]}">{ph}</span></span><span class="mono" style="color:var(--muted)">{age}</span></div>')
    group = lambda t: f'<div class="tr" style="{PC};height:26px;background:#2a2e36"><span style="grid-column:1/-1;font-size:11.5px;color:var(--dim)">{t}</span></div>'
    rows = [prow(i, *p) for i, p in enumerate(PLANS)]
    body = group("Waiting for approval · 2") + "".join(rows[:2]) + group("Installing and failed · 2") + "".join(rows[2:4]) + group("Complete · 4") + "".join(rows[4:])
    steps = [("CustomResourceDefinition", "kafkas.kafka.strimzi.io", "update"), ("CustomResourceDefinition", "kafkanodepools.kafka.strimzi.io", "update"), ("CustomResourceDefinition", "kafkarebalances.kafka.strimzi.io", "update"),
             ("ClusterServiceVersion", "strimzi-cluster-operator.v0.44.0", "create"), ("ClusterRole", "strimzi-cluster-operator.v0.44.0-7hd…", "create"), ("ClusterRoleBinding", "strimzi-cluster-operator.v0.44.0-7hd…", "create"), ("ServiceAccount", "strimzi-cluster-operator", "update")]
    srows = "".join(f'<div style="display:grid;grid-template-columns:minmax(0,1fr) 54px;gap:6px;align-items:center;font-size:12px;padding:4px 0;border-bottom:1px solid var(--bv)"><div style="min-width:0"><div style="color:var(--dim);font-size:11px">{k}</div><div class="mono" style="font-size:11.5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{n}</div></div><span style="color:{C["accent"] if a == "update" else C["green"]};font-size:11.5px">{a}</span></div>' for k, n, a in steps)
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{op_header("OLM v0 · 8 install plans · 2 waiting", "Filter install plans")}
{op_subtabs("Install plans", 2)}
<div class="th" style="{PC}"><span>NAME</span><span>NAMESPACE</span><span>CLUSTER SERVICE VERSIONS</span><span>APPROVAL</span><span>PHASE</span><span>AGE</span></div>
<div style="flex:1;overflow:hidden">{body}</div>
{hints([("↵","Details"),("a","Approve…"),("d","Review changes"),("y","View YAML"),("/","Filter")])}
</div>'''
    dock = f'''<aside class="dock" style="width:340px">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500" class="mono">install-7qk2d</span><span style="font-size:11.5px">kafka</span><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec"><div style="display:flex;gap:10px;align-items:center;margin-bottom:10px">{st("Upgrade available")}<span style="font-size:12px;color:var(--dim)">Manual approval · created 12m ago</span></div>
<dl class="kv" style="margin:0"><dt>Operator</dt><dd>Strimzi <span class="mono" style="font-size:11.5px;color:var(--dim)">0.43.0 → 0.44.0</span></dd><dt>Subscription</dt><dd><a href="#">kafka/strimzi-kafka-operator</a></dd><dt>Catalog</dt><dd>operatorhubio-catalog</dd></dl>
<div style="display:flex;gap:6px;margin-top:10px"><button class="btn p">{ic("check",12,"#1b1e24")}Approve…</button><button class="btn">{ic("diff",12)}Review changes</button><button class="btn g">{ic("code",12)}YAML</button></div></div>
<div class="dsec" style="border-bottom:0"><p class="dtitle">Steps · 24 resources</p>{srows}<div style="font-size:11.5px;color:var(--dim);padding-top:6px">and 17 more</div></div>
</aside>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    return page("Install plans — Kubyl", op_shell("Operators", op_tabs("ops"), content))

SUBS = [
 ("cert-manager", "operators", "stable", "operatorhubio-catalog", "Automatic", "cert-manager.v1.16.5", "AtLatestKnown"),
 ("cloudnative-pg", "databases", "stable-v1", "operatorhubio-catalog", "Manual", "cloudnative-pg.v1.30.0", "UpgradePending"),
 ("external-secrets-operator", "operators", "stable", "operatorhubio-catalog", "Automatic", "external-secrets-operator.v0.10.3", "UpgradePending"),
 ("prometheus", "operators", "beta", "operatorhubio-catalog", "Automatic", "prometheusoperator.0.76.1", "AtLatestKnown"),
 ("sailoperator", "istio-system", "candidates", "operatorhubio-catalog", "Manual", "—", "ResolutionFailed"),
 ("strimzi-kafka-operator", "kafka", "stable", "operatorhubio-catalog", "Manual", "strimzi-cluster-operator.v0.43.0", "UpgradePending"),
 ("argocd-operator", "operators", "alpha", "operatorhubio-catalog", "Automatic", "argocd-operator.v0.11.0", "AtLatestKnown"),
 ("grafana-operator", "grafana", "v5", "operatorhubio-catalog", "Automatic", "grafana-operator.v5.20.0", "AtLatestKnown"),
]
SC = "grid-template-columns: minmax(0,1fr) 100px 90px minmax(0,1fr) 86px minmax(0,1.3fr) 130px"

def subscriptions_screen():
    scol = {"AtLatestKnown": C["green"], "UpgradePending": C["yellow"], "ResolutionFailed": C["red"]}
    rows = "".join(f'<div class="tr{" on" if i == 4 else ""}" style="{SC};height:32px"><span class="mono">{n}</span><span class="mono" style="color:var(--muted)">{ns}</span><span class="mono" style="color:var(--muted)">{ch}</span><span class="mono" style="font-size:11.5px;color:var(--muted)">{src}</span><span style="color:{C["yellow"] if ap == "Manual" else C["muted"]}">{ap}</span><span class="mono" style="font-size:11.5px">{csv}</span><span class="pill">{dot(scol[stt])}<span style="color:{scol[stt]}">{stt}</span></span></div>' for i, (n, ns, ch, src, ap, csv, stt) in enumerate(SUBS))
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{op_header("OLM v0 · 8 subscriptions · 1 failing", "Filter subscriptions")}
{op_subtabs("Subscriptions", 2)}
<div class="th" style="{SC}"><span>NAME</span><span>NAMESPACE</span><span>CHANNEL</span><span>CATALOG</span><span>APPROVAL</span><span>INSTALLED CSV</span><span>STATE</span></div>
<div style="flex:1;overflow:hidden">{rows}</div>
{hints([("↵","Details"),("y","View YAML"),("e","Edit YAML"),("⌃d","Uninstall…"),("/","Filter")])}
</div>'''
    cond = lambda ok, t, m: f'<div style="display:flex;gap:8px;font-size:12px;padding:5px 0;border-bottom:1px solid var(--bv);align-items:flex-start">{ic("ok" if ok else "err",13,C["green"] if ok else C["red"])}<div style="min-width:0"><div>{t}</div><div style="color:var(--dim);font-size:11.5px;line-height:16px">{m}</div></div></div>'
    dock = f'''<aside class="dock" style="width:350px">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500" class="mono">sailoperator</span><span style="font-size:11.5px">istio-system</span><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec"><div style="display:flex;gap:10px;align-items:center;margin-bottom:10px">{st("Failed")}<span style="font-size:12px;color:var(--dim)">no CSV installed</span></div>
<dl class="kv" style="margin:0"><dt>Package</dt><dd>sailoperator</dd><dt>Channel</dt><dd>candidates</dd><dt>Catalog</dt><dd>operatorhubio-catalog <span style="color:var(--dim)">· olm</span></dd><dt>Starting CSV</dt><dd class="mono" style="font-size:11.5px">sailoperator.v0.2.0</dd><dt>Approval</dt><dd style="color:var(--yellow)">Manual</dd></dl></div>
<div class="dsec"><p class="dtitle">Conditions</p>
{cond(False, "ResolutionFailed", "constraints not satisfiable: no operators found in channel candidates of package sailoperator in the catalog referenced by subscription sailoperator")}
{cond(True, "CatalogSourcesUnhealthy · False", "all available catalogsources are healthy")}</div>
<div class="dsec" style="border-bottom:0"><div style="display:flex;gap:6px"><button class="btn g" style="height:24px">{ic("code",12)}YAML</button><button class="btn g" style="height:24px">{ic("store",12)}Open in OperatorHub</button><span style="flex:1"></span><button class="btn d" style="height:24px">{ic("trash",12,C["red"])}Uninstall…</button></div></div>
</aside>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    return page("Subscriptions — Kubyl", op_shell("Operators", op_tabs("ops"), content))

HELM = [
 ("kube-prometheus-stack", "monitoring", "kube-prometheus-stack-84.1.0", "v0.86.1", "6", "deployed", "7h"),
 ("metrics-server", "kube-system", "metrics-server-3.13.0", "0.8.0", "3", "deployed", "29h"),
 ("ingress-nginx", "ingress-nginx", "ingress-nginx-4.13.3", "1.13.3", "12", "deployed", "4d"),
 ("payments-api", "payments", "payments-api-2.14.1", "2.14.1", "41", "failed", "38m"),
 ("redis-payments", "payments", "redis-21.2.13", "8.2.1", "7", "pending-upgrade", "2m"),
]
HC = "grid-template-columns: minmax(0,1.1fr) 110px minmax(0,1.2fr) 86px 76px 140px 70px"

def helm_status(s):
    col = {"deployed": C["green"], "failed": C["red"], "pending-upgrade": C["accent"], "superseded": C["dim"], "uninstalling": C["yellow"]}.get(s, C["muted"])
    return f'<span class="pill">{dot(col)}<span style="color:{col}">{s}</span></span>'

def helm_screen():
    rows = "".join(f'<div class="tr{" on" if i == 0 else ""}" style="{HC};height:32px"><span class="mono">{n}</span><span class="mono" style="color:var(--muted)">{ns}</span><span class="mono" style="font-size:11.5px">{ch}</span><span class="mono" style="color:var(--muted)">{av}</span><span class="mono" style="text-align:right;padding-right:16px">{rev}</span>{helm_status(s)}<span class="mono" style="color:var(--muted)">{up}</span></div>' for i, (n, ns, ch, av, rev, s, up) in enumerate(HELM))
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{op_header("5 Helm releases in 5 namespaces", "Filter releases", False)}
{op_subtabs("Helm releases", 2)}
<div class="th" style="{HC}"><span>NAME</span><span>NAMESPACE</span><span>CHART</span><span>APP VERSION</span><span style="text-align:right;padding-right:16px">REVISION</span><span>STATUS</span><span>UPDATED</span></div>
<div style="flex:1;overflow:hidden">{rows}</div>
{hints([("↵","Open release"),("v","Values"),("m","Manifest"),("h","History"),("c","Copy helm command"),("/","Filter")])}
</div>'''
    hist = [("6", "deployed", "Upgrade complete", "7h"), ("5", "superseded", "Upgrade complete", "7h"), ("4", "superseded", "Upgrade complete", "24h"), ("3", "superseded", "Upgrade complete", "25h")]
    hrows = "".join(f'<div style="display:grid;grid-template-columns:26px 100px minmax(0,1fr) 34px;gap:6px;align-items:center;font-size:12px;padding:4px 0;border-bottom:1px solid var(--bv)"><span class="mono">{r}</span>{helm_status(s)}<span style="color:var(--dim);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{d}</span><span class="mono" style="color:var(--dim)">{a}</span></div>' for r, s, d, a in hist)
    res = [("layers", "Deployment", "kube-prometheus-stack-operator"), ("db", "StatefulSet", "prometheus-kube-prometheus-stack-prometheus"), ("server", "DaemonSet", "kube-prometheus-stack-prometheus-node-exporter"), ("network", "Service", "kube-prometheus-stack-prometheus"), ("file", "ConfigMap", "kube-prometheus-stack-grafana")]
    rrows = "".join(f'<div style="display:flex;gap:8px;align-items:center;font-size:12px;padding:3px 0">{ic(i,12,C["accent"])}<span style="color:var(--dim);width:84px;flex-shrink:0">{k}</span><a href="#" class="mono" style="font-size:11.5px;text-decoration:none;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{n}</a></div>' for i, k, n in res)
    dock = f'''<aside class="dock" style="width:350px">
<div class="phead" style="border-bottom:1px solid var(--bv)">{ic("anchor",14,C["accent"])}<span style="flex:1;color:var(--text);font-weight:500;margin-left:4px" class="mono">kube-prometheus-stack</span><span style="font-size:11.5px">monitoring</span></div>
<div class="dsec"><div style="display:flex;gap:10px;align-items:center;margin-bottom:10px">{helm_status("deployed")}<span style="font-size:12px;color:var(--dim)">revision 6 · 7h ago</span></div>
<dl class="kv" style="margin:0"><dt>Chart</dt><dd class="mono" style="font-size:11.5px">kube-prometheus-stack 84.1.0</dd><dt>App version</dt><dd class="mono" style="font-size:11.5px">v0.86.1</dd><dt>First deployed</dt><dd>29h ago</dd><dt>Description</dt><dd>Upgrade complete</dd></dl>
<div style="display:flex;gap:6px;margin-top:10px"><button class="btn p">{ic("anchor",12,"#1b1e24")}Open release</button><button class="btn">{ic("copy",12)}Copy helm command{ic("cd",11)}</button></div></div>
<div class="dsec"><p class="dtitle">History</p>{hrows}</div>
<div class="dsec" style="border-bottom:0"><p class="dtitle">Resources · 118</p>{rrows}<div style="font-size:11.5px;color:var(--dim);padding-top:4px">and 113 more</div></div>
</aside>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    return page("Helm releases — Kubyl", op_shell("Helm", op_tabs("ops"), content))

def helm_release_screen():
    N = C["orange"]
    M = '<span style="color:var(--dim);letter-spacing:1px">••••••••</span>'
    dash = '<span style="color:var(--faint)">- </span>'
    L = [yl("alertmanager"), yl("enabled", 2, "true", N), yl("alertmanagerSpec", 2), yl("replicas", 4, M), yl("config", 2), yl("route", 4), yl("receiver", 6, M), yl("receivers", 4),
         "      " + dash + yl("name", 0, M), "        " + yl("pagerduty_configs", 0), "          " + dash + yl("routing_key", 0, M), yl("grafana"), yl("enabled", 2, "true", N), yl("adminPassword", 2, M),
         yl("ingress", 2), yl("enabled", 4, "false", N), yl("prometheus"), yl("prometheusSpec", 2), yl("retention", 4, M), yl("serviceMonitorSelectorNilUsesHelmValues", 4, "false", N),
         yl("storageSpec", 4), yl("volumeClaimTemplate", 6), yl("spec", 8), yl("resources", 10), yl("requests", 12), yl("storage", 14, M)]
    rows = "".join(f'<div class="mono" style="display:flex;height:22px;align-items:center;font-size:13px;white-space:pre"><span style="width:44px;text-align:right;padding-right:12px;color:var(--faint)">{i + 1}</span><span style="padding-left:14px">{t}</span></div>' for i, t in enumerate(L))
    sub = "".join(f'<span style="display:flex;align-items:center;gap:6px;padding:0 10px;{"color:var(--text);box-shadow:inset 0 -2px 0 var(--accent)" if t == "Values" else "color:var(--dim)"}">{t}</span>' for t in ["Values", "Manifest", "Notes", "History", "Resources"])
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div style="display:flex;align-items:center;gap:10px;padding:0 12px 0 16px;height:44px;flex-shrink:0;border-bottom:1px solid var(--bv)">{ic("anchor",15,C["accent"])}<span class="mono" style="font-size:14px;font-weight:600">kube-prometheus-stack</span><span style="color:var(--dim)">monitoring</span>{helm_status("deployed")}<span class="mono" style="font-size:12px;color:var(--muted)">kube-prometheus-stack-84.1.0 · app v0.86.1 · revision</span><button class="btn" style="height:24px;padding:0 8px"><span class="mono" style="font-size:12px">6</span>{ic("cd",11)}</button><div style="flex:1"></div><button class="btn g" style="height:26px">{ic("copy",12)}Copy helm command{ic("cd",11)}</button></div>
<div style="display:flex;gap:2px;padding:0 8px;border-bottom:1px solid var(--bv);height:34px;align-items:stretch;flex-shrink:0">{sub}</div>
<div style="display:flex;gap:10px;align-items:center;padding:0 14px;height:36px;border-bottom:1px solid var(--bv);font-size:12.5px"><span class="chip on" style="height:22px">User-supplied</span><span class="chip" style="height:22px">All (with chart defaults)</span><span style="flex:1"></span><span style="display:flex;gap:6px;align-items:center;color:var(--dim)">{ic("lock",12)}Values can hold passwords: strings and numbers are masked</span><button class="btn" style="height:24px">{ic("eye",12)}Reveal</button><button class="btn g" style="height:24px">{ic("copy",12)}Copy</button></div>
<div style="flex:1;overflow:hidden;padding-top:6px">{rows}</div>
{hints([("1–5","Tabs"),("r","Reveal/mask"),("[ ]","Older/newer revision"),("c","Copy helm command"),("⌘F","Find")])}
</div>'''
    cmds = [("Rollback to revision 5", "helm rollback kube-prometheus-stack 5 -n monitoring --kube-context kind-kubyl-dev"), ("Uninstall", "helm uninstall kube-prometheus-stack -n monitoring --kube-context kind-kubyl-dev"),
            ("Get values", "helm get values kube-prometheus-stack -n monitoring --kube-context kind-kubyl-dev"), ("History", "helm history kube-prometheus-stack -n monitoring --kube-context kind-kubyl-dev")]
    items = "".join(f'<div style="display:flex;flex-direction:column;padding:5px 8px;border-radius:5px;{"background:var(--sel)" if i == 0 else ""}"><span style="font-size:12.5px">{t}</span><span class="mono" style="font-size:11px;color:var(--dim)">{c}</span></div>' for i, (t, c) in enumerate(cmds))
    menu = f'''<div style="position:absolute;right:14px;top:150px;width:490px;background:#353b45;border:1px solid var(--border);border-radius:7px;box-shadow:0 10px 30px rgba(0,0,0,.45);padding:4px">
{items}
<div style="font-size:11.5px;color:var(--dim);padding:6px 8px 4px;border-top:1px solid var(--bv);margin-top:4px">Kubyl shows Helm releases read-only. The command goes to the clipboard; run it in a terminal.</div></div>'''
    tb = tabs([("blocks", "Operators", False), ("anchor", "kube-prometheus-stack", True), ("store", "OperatorHub", False)])
    content = f'<div style="flex:1;display:flex;min-height:0">{center}</div>'
    return page("Helm release values — Kubyl", op_shell("Helm", tb, content, menu))

def olm_states_screen():
    none = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0;border-right:1px solid var(--border)">
{tabs([("blocks", "Operators · homelab-k3s", True)], tools=False)}
{op_subtabs("Installed", 0, counts=(None, None, "3"), olm=False)}
<div style="flex:1;display:flex;flex-direction:column;gap:14px;padding:30px 34px">
<div style="display:flex;gap:12px;align-items:center">{ic("blocks",22,C["dim"])}<div><div style="font-size:16px;font-weight:600">OLM isn't installed on this cluster</div><div style="font-size:12.5px;color:var(--muted);margin-top:2px">No <span class="mono" style="font-size:11.5px">operators.coreos.com</span> or <span class="mono" style="font-size:11.5px">olm.operatorframework.io</span> APIs. Helm releases work without it.</div></div></div>
<div class="card" style="padding:10px 14px;display:flex;flex-direction:column;gap:6px;font-size:12.5px;color:var(--muted);line-height:19px">
<div><b style="font-weight:500;color:var(--text)">OpenShift</b> ships OLM. Other clusters install it from the operator-framework releases:</div>
<div class="mono" style="font-size:11.5px;padding:6px 8px;border-radius:5px;background:#23272e">operator-sdk olm install <span style="color:var(--dim)"># or the release's install.sh</span></div>
<div>Kubyl's dev cluster: <span class="mono" style="font-size:11.5px;color:var(--text)">script/olm-dev.sh</span> (OLM, the operatorhub.io catalog and a subscription waiting for approval).</div></div>
<div style="display:flex;gap:8px"><button class="btn">{ic("anchor",13)}Show Helm releases</button><button class="btn g">{ic("ext",13)}OLM documentation</button></div>
</div></div>'''
    XC = "grid-template-columns: minmax(0,1fr) minmax(0,1fr) 80px 120px 130px"
    ext = [("kubyl-v1-sample", "argocd-operator", "0.13.0", "kubyl-v1-sample", "Succeeded"), ("grafana", "grafana-operator", "5.20.0", "grafana", "Installing")]
    xrows = "".join(f'<div class="tr{" on" if i == 0 else ""}" style="{XC};height:32px"><span class="mono">{n}</span><span class="mono" style="color:var(--muted)">{p}</span><span class="mono">{v}</span><span class="mono" style="color:var(--muted)">{ns}</span>{st(s)}</div>' for i, (n, p, v, ns, s) in enumerate(ext))
    cats = f'<div class="tr" style="{XC};height:32px"><span class="mono">operatorhubio</span><span class="mono" style="font-size:11.5px;color:var(--muted)">quay.io/operatorhubio/catalog:latest</span><span></span><span style="color:var(--muted)">every 10m</span>{st("Ready")}</div>'
    v1 = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{tabs([("blocks", "Operators · staging", True)], tools=False)}
{op_subtabs("Extensions", 0, True)}
<div style="display:flex;gap:8px;align-items:center;padding:8px 14px;border-bottom:1px solid var(--bv);font-size:12.5px;color:var(--muted)">{ic("info",13,C["accent"])}<span style="flex:1">OLM v1: Kubyl lists extensions and catalogs. Install or upgrade one from a YAML template.</span><button class="btn" style="height:24px">{ic("fileplus",12)}New ClusterExtension…</button></div>
<div class="th" style="{XC}"><span>CLUSTEREXTENSION</span><span>PACKAGE</span><span>VERSION</span><span>NAMESPACE</span><span>STATUS</span></div>
{xrows}
<div class="th" style="{XC};margin-top:14px"><span>CLUSTERCATALOG</span><span>IMAGE</span><span></span><span>POLL</span><span>STATUS</span></div>
{cats}
<div style="flex:1"></div>
<div class="dsec" style="border-top:1px solid var(--bv);border-bottom:0"><p class="dtitle">kubyl-v1-sample · conditions</p>
<div style="display:flex;flex-direction:column;gap:4px;font-size:12px">{op_chg("ok",C["green"],"Installed · True · Installed bundle quay.io/operatorhubio/argocd-operator@sha256:… successfully")}{op_chg("ok",C["green"],"Progressing · True · Desired state reached")}</div>
<div style="font-size:11.5px;color:var(--dim);margin-top:8px">Upgrade: edit <span class="mono">spec.source.catalog.version</span> in the YAML (Edit YAML, <span class="mono">e</span>).</div></div>
</div>'''
    inner = f'''<div class="app">
{titlebar("staging-eu-west-1", "payments", False, "EKS · v1.30.4")}
<div class="body">{sidebar("")}<main class="main" style="flex-direction:row">{none}{v1}</main></div>
{statusbar()}
</div>'''
    return page("OLM not installed; OLM v1 extensions — Kubyl", inner)

# ---------- 8. Cluster updates (phase 13) ----------
OCP_PROD = "ocp-prod.example.com"
OCP_PROD_CTX = f"{OCP_PROD} · kube:admin"
OCP_DEV = "ocp-dev.example.com"
OCP_DEV_CTX = f"{OCP_DEV} · jane.doe@example.com"
INK = "#1b1e24"  # text and icons on accent-filled controls
M11 = 'class="mono" style="font-size:11px"'
M115 = 'class="mono" style="font-size:11.5px"'

# Explorer groups for the phase-13 frames (Routes only on clusters that serve route.openshift.io)
NAV = {
 "Workloads": [("Pods", "box", "17"), ("Deployments", "layers", "9"), ("StatefulSets", "db", "2"), ("DaemonSets", "server", "0"), ("Jobs &amp; CronJobs", "clock", "6")],
 "Network": [("Services", "network", "12"), ("Endpoints", "commit", "12"), ("Ingresses", "globe", "3"), ("Routes", "route", "8"), ("IngressClasses", "file", "1"), ("NetworkPolicies", "shield", "4")],
 "Administration": [("Installed Operators", "blocks", "12"), ("OperatorHub", "store", None), ("Helm Releases", "anchor", None), ("Cluster Updates", "up", None)],
}
NAV_GROUPS = ["Workloads", "Network", "Config &amp; Secrets", "Storage", "Access Control", "Cluster", "Administration", "Custom Resources"]
NAV_CLUSTERS = [(OCP_PROD_CTX, "on", C["red"], True), ("prod-eu-west-1", "on", C["red"], True), (OCP_DEV_CTX, "on", C["accent"], False),
                ("k3s-edge", "on", C["green"], False), ("platform-onprem", "on", C["purple"], False), ("gke-analytics", "on", C["cyan"], False)]

def nav_sidebar(cluster, color, prod=False, active="", open_=("Administration",), ocp=True, upd_dot="accent"):
    """A sidebar for any cluster: its root expanded, the given groups open, the other clusters below."""
    rows = [
        f'<div class="phead"><span style="flex:1;font-weight:500;color:var(--text)">Explorer</span><button class="ib" aria-label="Filter kinds">{ic("search",13)}</button><button class="ib" aria-label="Add kubeconfig">{ic("plus",14)}</button><button class="ib" aria-label="More">{ic("more",14)}</button></div>',
        f'<div class="sec">{ic("cr",11)}Favorites<span style="flex:1"></span><span style="font-weight:400;letter-spacing:0;text-transform:none;color:var(--faint)">4</span></div>',
        f'<div class="sec">{ic("cd",11)}Clusters</div>',
        root(cluster, "on", True, color, prod=prod),
        ti("Overview", 1, "gauge"),
        ti("Events", 1, "bell", "4", color=C["yellow"]),
    ]
    for g in NAV_GROUPS:
        is_open = g in open_
        rows.append(ti(g, 1, open_=is_open))
        for n, icon, cnt in (NAV[g] if is_open else []):
            if n == "Routes" and not ocp:
                continue
            extra = f'<span style="margin-right:2px">{dot(C[upd_dot])}</span>' if n == "Cluster Updates" and upd_dot else ""
            rows.append(ti(n, 2, icon, cnt, on=n == active, extra=extra))
    rows += [root(n, s, color=c, prod=p) for n, s, c, p in NAV_CLUSTERS if n != cluster]
    return '<aside class="side">' + "\n".join(rows) + '</aside>'

UPD_TABS = [("blocks", "Installed Operators", False), ("up", "Cluster Updates", True), ("gauge", "Overview", False)]

def upd_app(title, content, cluster=OCP_PROD_CTX, color=C["red"], prod=True, meta="OpenShift · v1.30.6", ns="payments",
            overlay="", sidebar_html=None, tabbar=None, right_extra="", ocp=True, upd_dot="accent"):
    side = sidebar_html or nav_sidebar(cluster, color, prod, "Cluster Updates", ocp=ocp, upd_dot=upd_dot)
    inner = f'''<div class="app">
{titlebar(cluster, ns, prod, meta)}
<div class="body">{side}<main class="main">{tabbar or tabs(UPD_TABS)}{content}</main></div>
{statusbar(right_extra=right_extra, cluster=cluster.split(" · ")[0], ns=ns)}
{overlay}
</div>'''
    return page(title, inner)

def upd_header(cluster, provider, how, chips="", recheck=True, sticky=False, size=18):
    """The page header. It stays put while the page scrolls (a shadow once scrolled)."""
    rc = f'<button class="btn g">{ic("refresh",13)}Re-check</button>' if recheck else ""
    sh = "border-bottom:1px solid var(--bv);box-shadow:0 6px 10px -8px rgba(0,0,0,.6);position:relative;z-index:2;" if sticky else ""
    return (f'<div style="display:flex;align-items:center;gap:10px;padding:14px 18px 12px;flex-shrink:0;min-width:0;{sh}">'
            f'<h1 style="font-size:{size}px;font-weight:600;white-space:nowrap">Cluster updates</h1>{chips}'
            f'<span style="color:var(--dim);font-size:12px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;min-width:0"><span style="color:var(--muted)">{cluster}</span> · provider {provider} ({how})</span>'
            f'<div style="flex:1"></div>{rc}</div>')

def upd_page(header, sections, scroll=0, thumb=(2, 520)):
    """One scrollable page: `scroll` shifts the content up, the thumb shows where we are."""
    top, h = thumb
    return (f'<div style="flex:1;display:flex;flex-direction:column;min-width:0;min-height:0">{header}'
            f'<div style="flex:1;min-height:0;overflow:hidden;position:relative">'
            f'<div style="display:flex;flex-direction:column;gap:14px;padding:{0 if scroll else 2}px 18px 18px;margin-top:-{scroll}px">{"".join(sections)}</div>'
            f'<div aria-hidden="true" style="position:absolute;right:3px;top:{top}px;width:6px;height:{h}px;border-radius:3px;background:#4b5160"></div>'
            f'</div></div>')

def upd_row(*cards, align="stretch"):
    return f'<div style="display:flex;gap:14px;align-items:{align}">{"".join(cards)}</div>'

def sbtn(label, icon=None, kind="", h=24):
    i = ic(icon, 12, INK if kind == "p" else "currentColor") if icon else ""
    return f'<button class="btn{" " + kind if kind else ""}" style="height:{h}px">{i}{label}</button>'

def dimt(t, size=11.5):
    return f'<span style="font-size:{size}px;color:var(--dim)">{t}</span>'

TAGC = {"rec": ("#2d3b4d", "#a8cdf3"), "cond": ("#3d3727", C["yellow"]), "block": ("#3e2c2f", C["red"]), "avail": ("#353a45", C["muted"]),
        "latest": ("#28393d", C["cyan"]), "ro": ("#353a45", C["muted"])}
GCOL = {"rec": C["accent"], "avail": "#7a808c", "cond": C["yellow"], "block": C["red"], "latest": C["cyan"]}

def tag(kind, text):
    bg, fg = TAGC[kind]
    return f'<span class="chip" style="height:18px;background:{bg};color:{fg}">{text}</span>'

def spinner(s=14, col=None):
    return (f'<svg width="{s}" height="{s}" viewBox="0 0 24 24" aria-hidden="true" style="flex-shrink:0"><circle cx="12" cy="12" r="9" fill="none" stroke="#464b57" stroke-width="3"></circle>'
            f'<path d="M12 3a9 9 0 0 1 9 9" fill="none" stroke="{col or C["accent"]}" stroke-width="3" stroke-linecap="round"></path></svg>')

# ----- version card -----
def vcard(blocks, width=260):
    w = f"width:{width}px;flex-shrink:0;" if width else "flex:1;min-width:0;"
    return f'<div class="card" style="{w}padding:14px 16px;display:flex;flex-direction:column;gap:12px;box-sizing:border-box">{"".join(blocks)}</div>'

def vc_label(t):
    return f'<div style="font-size:12px;color:var(--dim);margin-bottom:4px">{t}</div>'

def vc_current(ver, sub, extra="", size=24):
    return (f'<div><div style="font-size:12px;color:var(--dim)">Current version</div><div class="mono" style="font-size:{size}px;font-weight:500;line-height:1.35">{ver}</div>'
            f'<div style="font-size:11.5px;color:var(--muted);line-height:17px">{sub}</div>{extra}</div>')

def vc_line(label, body):
    return f'<div>{vc_label(label)}<div style="font-size:12px;line-height:18px">{body}</div></div>'

def dropdown(value, open_=False, menu="", mono=True):
    v = f'<span class="mono" style="font-size:12px">{value}</span>' if mono else value
    border = "border-color:var(--accent);" if open_ else ""
    return f'<div style="position:relative"><button class="btn" style="width:100%;justify-content:space-between;{border}">{v}{ic("cd",12)}</button>{menu if open_ else ""}</div>'

def ocp_channel_menu():
    items = [("candidate-4.17", "release candidates", False), ("fast-4.17", "GA, as soon as it's released", False), ("stable-4.17", "GA, after fast-4.17 feedback", True),
             ("eus-4.18", "extended update support", False), ("stable-4.18", "for the update to 4.18", False)]
    rows = "".join(f'<div style="display:flex;align-items:center;gap:8px;height:28px;padding:0 8px;border-radius:4px;{"background:var(--sel)" if on else ""}">'
                   f'<span style="width:12px;display:flex">{ic("check",12,C["accent"],2.5) if on else ""}</span><span class="mono" style="font-size:12px;width:108px">{n}</span>'
                   f'<span style="font-size:11.5px;color:var(--dim)">{d}</span></div>' for n, d, on in items)
    return f'''<div role="listbox" aria-label="Channel" style="position:absolute;left:0;top:30px;width:340px;z-index:6;background:#353b45;border:1px solid var(--border);border-radius:7px;box-shadow:0 12px 34px rgba(0,0,0,.5);padding:4px">
<div style="padding:6px 8px 4px;font-size:11px;font-weight:600;letter-spacing:.06em;text-transform:uppercase;color:var(--dim)">Channels offered for 4.17.8</div>
{rows}
<div style="display:flex;gap:8px;padding:8px 8px 6px;margin-top:4px;border-top:1px solid var(--bv);font-size:11.5px;color:var(--muted);line-height:16px"><span style="display:flex;padding-top:2px">{ic("info",12,C["accent"])}</span><span>Changing the channel patches ClusterVersion <span {M11}>spec.channel</span>; you confirm it first.</span></div>
</div>'''

def ocp_version_card(menu=False):
    cid = (f'<div style="display:flex;align-items:center;gap:6px;font-size:11.5px;color:var(--dim);margin-top:2px">Cluster ID'
           f'<span class="mono" style="font-size:11px;color:var(--muted)">3f9c1e2a…7b1d</span>'
           f'<button class="ib" aria-label="Copy cluster ID" style="width:18px;height:18px">{ic("copy",11)}</button></div>')
    status = (f'<div style="display:flex;flex-direction:column;gap:3px;font-size:11.5px">'
              f'<span class="pill" style="color:var(--green)">{ic("ok",12,C["green"])}Available</span>'
              f'<span class="pill" style="color:var(--yellow)">{ic("alert",12,C["yellow"])}Upgradeable: False · AdminAckRequired</span></div>')
    return vcard([
        vc_current("4.17.8", "OpenShift 4.17.8 · Kubernetes v1.30.6", cid),
        vc_line("Last update", f'<span {M115}>4.17.6 → 4.17.8</span><br><span style="color:var(--dim)">2026-08-30 · took 1 h 12 min</span>'),
        f'<div>{vc_label("Status")}{status}</div>',
        f'<div>{vc_label("Channel")}{dropdown("stable-4.17", menu, ocp_channel_menu())}</div>',
    ])

# ----- update path graph -----
def graph_legend():
    def sample(col, dash="", w=1.6, fill="#2f343e"):
        return (f'<svg width="26" height="10" viewBox="0 0 26 10" aria-hidden="true"><path d="M1 5h15" stroke="{col}" stroke-width="{w}"{dash}></path>'
                f'<circle cx="20" cy="5" r="3.5" fill="{fill}" stroke="{col}" stroke-width="1.6"></circle></svg>')
    d = ' stroke-dasharray="3 2"'
    items = [(sample(C["accent"], "", 2.2, C["accent"]), "recommended"), (sample("#7a808c"), "available"), (sample(C["yellow"], d), "conditional"), (sample(C["red"], d), "blocked")]
    return '<div style="display:flex;gap:12px;font-size:11.5px;color:var(--dim)">' + "".join(f'<span style="display:flex;align-items:center;gap:5px">{s}{t}</span>' for s, t in items) + '</div>'

def upd_graph(cur, cur_sub, targets, lw=52, span=90, sel=None, pad=4):
    """The current version on the left; edges fan out to a column of targets, newest first.
    targets: (version, kind, head_html, row_height, body_html). kind: rec, avail, cond, block, latest."""
    x0 = lw + 18
    x1 = x0 + span
    tops, y = [], pad
    for t in targets:
        tops.append(y)
        y += t[3]
    H = y + pad
    ys = [tp + 14 for tp in tops]
    cy = (ys[0] + ys[-1]) / 2
    mx = (x0 + x1) / 2
    edges, nodes = [], []
    for i in sorted(range(len(targets)), key=lambda i: targets[i][1] == "rec"):  # recommended edge on top
        kind, ty = targets[i][1], ys[i]
        col = GCOL[kind]
        dash = ' stroke-dasharray="5 4"' if kind in ("cond", "block") else ""
        edges.append(f'<path d="M{x0+9},{cy:.1f} C{mx:.1f},{cy:.1f} {mx:.1f},{ty:.1f} {x1-7},{ty:.1f}" fill="none" stroke="{"#5d636f" if kind == "avail" else col}" stroke-width="{2.4 if kind == "rec" else 1.5}"{dash}></path>')
        nodes.append(f'<circle cx="{x1}" cy="{ty}" r="6" fill="{col if kind == "rec" else "#2f343e"}" stroke="{col}" stroke-width="2"></circle>')
    nodes.append(f'<circle cx="{x0}" cy="{cy:.1f}" r="13" fill="none" stroke="{C["green"]}" stroke-opacity=".35" stroke-width="2"></circle><circle cx="{x0}" cy="{cy:.1f}" r="8" fill="{C["green"]}"></circle>')
    svg = f'<svg width="{x1+8}" height="{H}" viewBox="0 0 {x1+8} {H}" style="position:absolute;left:0;top:0;pointer-events:none" aria-hidden="true">{"".join(edges)}{"".join(nodes)}</svg>'
    rows = ""
    for (v, kind, head, h, body), top in zip(targets, tops):
        if v == sel:
            bg = "background:var(--sel);outline:1px solid var(--accent);outline-offset:-1px;"
        elif kind == "rec":
            bg = "background:rgba(116,173,232,.08);"
        else:
            bg = ""
        vc = C["muted"] if kind in ("avail", "block") else C["text"]
        rows += (f'<div style="position:absolute;left:{x1-14}px;right:0;top:{top}px;height:{h}px;padding:0 10px 0 28px;box-sizing:border-box;border-radius:6px;overflow:hidden;{bg}">'
                 f'<div style="display:flex;align-items:center;gap:8px;height:28px;white-space:nowrap"><span class="mono" style="font-size:13px;color:{vc};font-weight:{600 if kind == "rec" else 400}">{v}</span>{head}</div>{body}</div>')
    cur_label = (f'<div style="position:absolute;left:0;top:{cy-17:.0f}px;width:{lw}px;text-align:right;white-space:nowrap">'
                 f'<div class="mono" style="font-size:13px;font-weight:600">{cur}</div><div style="font-size:11px;color:var(--green)">{cur_sub}</div></div>')
    return f'<div style="position:relative;height:{H}px;flex-shrink:0">{rows}{svg}{cur_label}</div>'

def path_card(graph, right="", extra="", legend=True, title="Update path"):
    return (f'<div class="card" style="flex:1;min-width:0;padding:12px 16px 14px;display:flex;flex-direction:column;gap:10px">'
            f'<div style="display:flex;align-items:center;gap:14px;min-height:26px"><span style="font-weight:500;white-space:nowrap">{title}</span>{graph_legend() if legend else ""}<div style="flex:1"></div>{right}</div>'
            f'{graph}{extra}</div>')

def path_note(text):
    return f'<div style="font-size:11.5px;color:var(--dim);display:flex;gap:6px;align-items:center;margin-top:auto">{ic("info",12)}{text}</div>'

def upd_steps(title, steps):
    items = "".join(f'<div style="display:flex;gap:8px;align-items:flex-start;font-size:12px;line-height:18px"><span style="width:16px;height:16px;border-radius:50%;background:#353a45;color:var(--muted);font-size:10.5px;display:flex;align-items:center;justify-content:center;flex-shrink:0;margin-top:1px">{i}</span><span style="color:var(--muted)">{t}</span></div>'
                    for i, t in enumerate(steps, 1))
    return (f'<div style="display:flex;flex-direction:column;gap:5px;padding-top:10px;border-top:1px solid var(--bv)">'
            f'<div style="font-size:11px;font-weight:600;letter-spacing:.06em;text-transform:uppercase;color:var(--dim)">{title}</div>{items}</div>')

def gline(icon, col, t):
    return (f'<div style="display:flex;gap:7px;align-items:flex-start;font-size:12px;line-height:18px;color:var(--muted);white-space:nowrap">'
            f'<span style="display:flex;padding-top:3px">{ic(icon,12,col)}</span><span>{t}</span></div>')

def ocp_targets(rich=False):
    d = dimt
    if not rich:
        return [
            ("4.18.2", "block", tag("block", "blocked") + d("AdminAckRequired · cert-utils-operator"), 28, ""),
            ("4.17.13", "cond", tag("cond", "conditional · 1 risk") + d("ExampleStorageDriverRegression"), 28, ""),
            ("4.17.12", "rec", tag("rec", "recommended") + d("latest without known risks"), 28, ""),
            ("4.17.11", "avail", d("available"), 26, ""),
            ("4.17.10", "avail", d("available"), 26, ""),
            ("4.17.9", "avail", d("available"), 26, ""),
        ]
    sp = '<span style="flex:1"></span>'
    return [
        ("4.18.2", "block", tag("block", "blocked") + d("minor update · in stable-4.18 and eus-4.18"), 92,
         gline("err", C["red"], f'<b style="font-weight:500;color:var(--text)">Upgradeable=False: AdminAckRequired</b> — Kubernetes 1.31 removes APIs still in use;<br>acknowledge in <span {M11}>openshift-config/admin-acks</span>')
         + gline("err", C["red"], f'<span {M11}>cert-utils-operator</span> declares <span {M11}>olm.maxOpenShiftVersion</span> 4.17')),
        ("4.17.13", "cond", tag("cond", "conditional · 1 risk") + d("released 2026-09-24") + sp + sbtn("Accept risk…", h=22), 72,
         gline("alert", C["yellow"], f'<span class="mono" style="font-size:11px;color:var(--yellow)">ExampleStorageDriverRegression</span><span style="color:var(--dim)"> · applies to this cluster</span><br>'
               'Volumes on the example CSI driver can fail to attach after a node reboot. <a href="#">Learn more</a>')),
        ("4.17.12", "rec", tag("rec", "recommended") + d("released 2026-09-18"), 50,
         '<div style="font-size:12px;color:var(--muted);line-height:18px;white-space:nowrap">Latest in stable-4.17 without known risks · 23 bug fixes, 4 security fixes</div>'),
        ("4.17.11", "avail", d("available · released 2026-09-10"), 28, ""),
        ("4.17.10", "avail", d("available · released 2026-09-03"), 28, ""),
        ("4.17.9", "avail", d("available · released 2026-08-27"), 28, ""),
    ]

def ocp_node_details():
    kv = (f'<dl class="kv" style="margin:0;grid-template-columns:92px minmax(0,1fr);width:384px;flex-shrink:0">'
          f'<dt>Version</dt><dd><span {M115}>4.17.13</span> · Kubernetes v1.30.9</dd>'
          f'<dt>Released</dt><dd>2026-09-24 · 2 days ago</dd>'
          f'<dt>Release notes</dt><dd><a href="#">errata.example.com/RHBA-2026:4471</a></dd>'
          f'<dt>Channels</dt><dd class="mono" style="font-size:11.5px">candidate-4.17 · fast-4.17 · stable-4.17</dd>'
          f'<dt>Image</dt><dd class="mono" style="font-size:11.5px">ocp-release@sha256:9c1e…4f2a</dd></dl>')
    risk = (f'<div style="flex:1;min-width:0;border:1px solid #6b5a2a;background:#35322a;border-radius:7px;padding:10px 12px;display:flex;flex-direction:column;gap:6px">'
            f'<div style="display:flex;align-items:center;gap:8px">{ic("alert",14,C["yellow"])}<span class="mono" style="font-size:12px;color:var(--yellow)">ExampleStorageDriverRegression</span><span class="chip" style="height:18px">applies to this cluster</span></div>'
            f'<div style="font-size:12px;color:var(--muted);line-height:17px">Volumes provisioned by the example CSI driver can fail to attach after a node reboot, until the driver is updated to 2.4.</div>'
            f'<div style="font-size:11.5px;color:var(--dim);line-height:16px">Matched by PromQL <span {M11}>group(csv_succeeded&#123;name=~"example-csi-driver.*"&#125;)</span> · evaluated 2m ago</div>'
            f'<div style="display:flex;gap:8px;align-items:center;margin-top:2px"><a href="#" style="font-size:12px;display:flex;gap:4px;align-items:center;white-space:nowrap">Learn more{ic("ext",11)}</a><span style="flex:1"></span>'
            f'<button class="ib" aria-label="Copy oc adm upgrade command" title="Copy oc adm upgrade --to 4.17.13 --allow-not-recommended">{ic("copy",13)}</button>{sbtn("Accept risk and update…")}</div></div>')
    return (f'<div style="border-top:1px solid var(--bv);padding-top:12px;display:flex;flex-direction:column;gap:10px">'
            f'<div style="display:flex;align-items:center;gap:8px"><span class="mono" style="font-size:14px;font-weight:600">4.17.13</span>{tag("cond", "conditional update")}{dimt("selected in the graph · Esc clears")}</div>'
            f'<div style="display:flex;gap:18px;align-items:flex-start">{kv}{risk}</div></div>')

def ocp_path_card(rich=False):
    g = upd_graph("4.17.8", "current", ocp_targets(rich), lw=52, span=100 if rich else 90, sel="4.17.13" if rich else None)
    extra = ocp_node_details() if rich else path_note("From the update service for stable-4.17 · checked 4 min ago · 3 more versions in fast-4.17")
    return path_card(g, sbtn("Update to 4.17.12…", "up", "p", 26), extra)

# ----- pre-flight checks -----
PF_ICON = {"fail": ("err", C["red"]), "warn": ("alert", C["yellow"]), "pass": ("ok", C["green"]), "info": ("info", C["dim"]), "deny": ("lock", C["orange"])}

def pf_row(kind, title, expl, action="", detail="", last=False, pad=10, open_=None):
    icon, col = PF_ICON[kind]
    chev = f'<span style="display:flex">{ic("cd" if open_ else "cr", 12, C["dim"])}</span>' if open_ is not None else ""
    return (f'<div style="display:flex;gap:10px;padding:{pad}px 0;{"" if last else "border-bottom:1px solid var(--bv);"}align-items:flex-start">'
            f'<span style="display:flex;padding-top:1px">{ic(icon,15,col)}</span>'
            f'<div style="flex:1;min-width:0"><div style="display:flex;align-items:center;gap:6px;font-size:12.5px;color:{"var(--dim)" if kind == "info" else "var(--text)"}">{title}{chev}</div>'
            f'<div style="font-size:11.5px;color:var(--dim);line-height:17px">{expl}</div>{detail}</div>{action}</div>')

def pf_summary(fail=0, warn=0, ok=0, na=0, other=""):
    parts = []
    if fail:
        parts.append(f'<span style="color:var(--red)">{fail} failed</span>')
    if warn:
        parts.append(f'<span style="color:var(--yellow)">{warn} warning{"s" if warn > 1 else ""}</span>')
    parts.append(f'<span style="color:var(--green)">{ok} passed</span>')
    if na:
        parts.append(f'<span style="color:var(--dim)">{na} not applicable</span>')
    if other:
        parts.append(other)
    return '<span style="font-size:12px;white-space:nowrap">' + '<span style="color:var(--faint)"> · </span>'.join(parts) + '</span>'

def pf_card(title, summary, rows, right=None, summary_line=False):
    if right is None:
        right = f'<span style="font-size:12px;color:var(--dim)">ran 2m ago</span>{sbtn("Re-run", "refresh", "g")}'
    head = (f'<div style="display:flex;align-items:center;gap:10px;padding:10px 0 {4 if summary_line else 8}px;{"" if summary_line else "border-bottom:1px solid var(--bv);"}">'
            f'<span style="font-weight:500;white-space:nowrap">{title}</span>{"" if summary_line else summary}<div style="flex:1"></div>{right}</div>')
    line = f'<div style="padding:0 0 8px;border-bottom:1px solid var(--bv)">{summary}</div>' if summary_line else ""
    return f'<div class="card" style="padding:2px 16px">{head}{line}{rows}</div>'

def pf_passed_line(text, last=True):
    return (f'<div style="display:flex;gap:10px;align-items:center;padding:9px 0;font-size:12px;color:var(--dim);{"" if last else "border-bottom:1px solid var(--bv);"}">{ic("ok",15,C["green"])}'
            f'<span style="min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{text}</span><a href="#" style="margin-left:auto;text-decoration:none;white-space:nowrap">Show all</a></div>')

def pf_requests():
    RC = "grid-template-columns: minmax(0,.9fr) minmax(0,1.2fr) minmax(0,1.5fr) 110px 76px"
    reqs = [("v1beta3 flowschemas", "flow-exporter/v0.9.2 (linux/amd64)", "system:serviceaccount:monitoring:flow-exporter", "1,284", "3m ago"),
            ("v1beta3 flowschemas", "kubectl/v1.27.4 (darwin/arm64)", "jane.doe@example.com", "6", "19h ago")]
    cell = 'style="font-size:11.5px;padding-right:12px'
    head = (f'<div class="th" style="{RC};height:26px;padding:0 10px;background:#262a31"><span>RESOURCE</span><span>USER AGENT</span><span>USER</span>'
            f'<span style="text-align:right;padding-right:18px">REQUESTS · 24 H</span><span>LAST</span></div>')
    body = "".join(f'<div class="tr" style="{RC};height:28px;padding:0 10px"><span class="mono" {cell}">{r}</span><span class="mono" {cell};color:var(--muted)">{ua}</span>'
                   f'<span class="mono" {cell};color:var(--muted)">{u}</span><span class="mono" {cell};text-align:right;padding-right:18px">{n}</span><span style="font-size:12px;color:var(--dim)">{t}</span></div>'
                   for r, ua, u, n, t in reqs)
    return (f'<div style="margin-top:8px;border:1px solid var(--bv);border-radius:6px;overflow:hidden;background:#2a2e36">{head}{body}'
            f'<div style="display:flex;align-items:center;gap:6px;padding:5px 10px;font-size:11.5px;color:var(--dim)">From APIRequestCount <span {M11}>flowschemas.v1beta3.flowcontrol.apiserver.k8s.io</span> · removedInRelease 1.32'
            f'<span style="flex:1"></span><button class="btn g" style="height:22px">{ic("code",12)}APIRequestCount YAML</button></div></div>')

def ocp_pf_rows(expand=False, collapsed=False):
    issues = [
        pf_row("fail", "PodDisruptionBudget blocks node drain",
               f'<span {M11}>payments/ledger-writer-pdb</span> allows 0 disruptions: 3 replicas, minAvailable 3. The machine-config operator can\'t drain the node that runs a ledger-writer pod.', sbtn("Open PDB")),
        pf_row("warn", "Deprecated APIs still requested",
               f'<span {M11}>flowcontrol.apiserver.k8s.io/v1beta3</span> flowschemas · 2 clients in the last 24 h, from APIRequestCount; removed in 1.32',
               sbtn("Show requests"), pf_requests() if expand else "", open_=expand),
        pf_row("warn", "Helm release uses removed APIs",
               f'<span {M11}>payments/legacy-app</span> revision 7: <span {M11}>extensions/v1beta1</span> Ingress, removed in 1.22', sbtn("Open release")),
    ]
    if collapsed:
        return "".join(issues) + pf_passed_line("5 passed: installed operators, version skew, surge capacity, cluster operators, machine config pools · 1 not applicable")
    passed = [
        pf_row("pass", "Installed operators compatible", f'12 of 12 allow OpenShift 4.17 / Kubernetes 1.30 (<span {M11}>olm.maxOpenShiftVersion</span>, <span {M11}>maxKubeVersion</span>)'),
        pf_row("pass", "Version skew", "kubelets 1.30.6 on 6 of 6 nodes, within n-3"),
        pf_row("pass", "Node capacity for surge", "headroom for one node per pool: 38% CPU, 44% memory free"),
        pf_row("pass", "Cluster operators healthy", "33 of 33 Available, none Degraded"),
        pf_row("pass", "Machine config pools ready", "master 3 / 3 and worker 3 / 3 updated, none paused or degraded"),
        pf_row("info", "Add-on compatibility", "provided by the provider: none for OpenShift", last=True),
    ]
    return "".join(issues + passed)

def ocp_pf_card(expand=False, collapsed=True):
    return pf_card("Pre-flight checks for 4.17.12", pf_summary(1, 2, 5, 1), ocp_pf_rows(expand, collapsed), summary_line=not collapsed)

# ----- OpenShift: cluster operators, machine config pools, history -----
MCP_COLS = "grid-template-columns: 110px minmax(0,1.1fr) 70px 86px minmax(0,1.7fr) 124px"
MCP_HEAD = f'<div class="th" style="{MCP_COLS}"><span>POOL</span><span>NODES UPDATED</span><span>READY</span><span>DEGRADED</span><span>STATE</span><span>MAX UNAVAILABLE</span></div>'

def mcp_row(pool, upd, pct, ready, degr, state, maxu, col=None):
    return (f'<div class="tr" style="{MCP_COLS};height:36px"><span style="font-weight:500">{pool}</span>'
            f'<div style="display:flex;align-items:center;gap:8px;padding-right:16px"><span class="mono" style="width:40px;flex-shrink:0">{upd}</span>{bar(pct, col or C["green"], 120)}</div>'
            f'<span class="mono">{ready}</span><span class="mono" style="color:var(--dim)">{degr}</span>{state}<span class="mono">{maxu}</span></div>')

def mcp_updated(v="4.17.8"):
    return f'<span class="pill" style="color:var(--green)">{ic("check",12,C["green"],2.5)}updated · {v}</span>'

def card_head(title, sub="", right=""):
    s = f'<span style="font-size:12px;color:var(--dim);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;min-width:0">{sub}</span>' if sub else ""
    return f'<div style="display:flex;align-items:center;gap:10px;padding:10px 16px;min-height:26px"><span style="font-weight:500;white-space:nowrap">{title}</span>{s}<div style="flex:1"></div>{right}</div>'

def ocp_ops_card():
    return (f'<div class="card" style="overflow:hidden">'
            f'{card_head("Cluster operators &amp; machine config pools", "OpenShift", sbtn("Show operators", "list", "g"))}'
            f'<div style="display:flex;gap:8px;align-items:center;padding:0 16px 10px;font-size:12.5px">{ic("ok",14,C["green"])}<span>33 of 33 cluster operators at 4.17.8</span><span style="color:var(--dim)">· all Available, none Progressing or Degraded</span></div>'
            f'<div style="display:flex;gap:8px;align-items:center;padding:7px 16px;background:#35322a;border-top:1px solid var(--bv);font-size:12.5px">{ic("alert",14,C["yellow"])}'
            f'<span>Admin acknowledgement needed before 4.18:</span><span class="mono" style="font-size:11.5px;color:var(--muted)">ack-4.17-kube-1.31-api-removals-in-4.18</span><div style="flex:1"></div>{sbtn("Acknowledge…")}</div>'
            f'{MCP_HEAD}{mcp_row("master", "3 / 3", 100, "3", "0", mcp_updated(), "1")}{mcp_row("worker", "3 / 3", 100, "3", "0", mcp_updated(), "1")}</div>')

def ocp_history_card():
    HC8 = "grid-template-columns: 96px 118px 150px 150px 100px 86px minmax(0,1fr)"
    hist = [("4.17.8", "Completed", "2026-08-30 09:12", "2026-08-30 10:24", "1 h 12 min", True, "4b0e…91ac"),
            ("4.17.6", "Completed", "2026-07-19 08:40", "2026-07-19 09:38", "58 min", True, "e27d…0c3f"),
            ("4.17.3", "Completed", "2026-06-02 07:05", "2026-06-02 09:09", "2 h 4 min", True, "8a41…d2e0"),
            ("4.16.21", "Partial", "2026-05-10 18:02", "superseded", "—", False, "51c9…7a6b"),
            ("4.16.19", "Completed", "2026-04-21 08:15", "2026-04-21 09:20", "1 h 5 min", True, "c03a…44f1")]
    def row(v, s, a, b, took, ver, img):
        col = C["green"] if s == "Completed" else C["yellow"]
        vf = f'<span style="color:var(--green)">{ic("check",12,C["green"],2.5)}</span>' if ver else '<span style="color:var(--dim)">no</span>'
        return (f'<div class="tr" style="{HC8}"><span class="mono">{v}</span><span class="pill" style="color:{col}">{dot(col)}{s}</span>'
                f'<span class="mono" style="color:var(--muted)">{a}</span><span class="mono" style="color:{"var(--dim)" if b == "superseded" else "var(--muted)"}">{b}</span>'
                f'<span class="mono">{took}</span>{vf}<span class="mono" style="color:var(--dim);font-size:11.5px">quay.example.com/ocp-release@sha256:{img}</span></div>')
    return (f'<div class="card" style="overflow:hidden">{card_head("Update history", "from ClusterVersion status.history · 5 of 11", sbtn("Copy as text", "copy", "g"))}'
            f'<div class="th" style="{HC8}"><span>VERSION</span><span>STATE</span><span>STARTED</span><span>COMPLETED</span><span>TOOK</span><span>VERIFIED</span><span>IMAGE</span></div>'
            + "".join(row(*h) for h in hist) + '</div>')

def ocp_header(sticky=False, chips=""):
    return upd_header(OCP_PROD, "OpenShift", "ClusterVersion, read through the Kubernetes API", chips, sticky=sticky)

def ocp_sections(menu=False, rich=False, pf_expand=False, pf_collapsed=True):
    top = upd_row(ocp_version_card(menu), ocp_path_card(rich), align="flex-start" if rich else "stretch")
    return [top, ocp_pf_card(pf_expand, pf_collapsed), ocp_ops_card(), ocp_history_card()]

def updates_screen():
    """Board 8 overview: the OpenShift page scrolled down to the provider card and the update history."""
    content = upd_page(ocp_header(sticky=True), ocp_sections(), scroll=318, thumb=(226, 520))
    return upd_app("Cluster updates — Kubyl", content)

def updates_channel_screen():
    content = upd_page(ocp_header(), ocp_sections(menu=True), thumb=(2, 520))
    return upd_app("OpenShift version and channel — Kubyl", content)

def updates_graph_screen():
    content = upd_page(ocp_header(), ocp_sections(rich=True), thumb=(2, 440))
    return upd_app("Update graph — Kubyl", content)

def updates_preflight_screen():
    content = upd_page(ocp_header(sticky=True), ocp_sections(pf_expand=True, pf_collapsed=False), scroll=296, thumb=(170, 420))
    return upd_app("Pre-flight checks — Kubyl", content)

# ----- progress during an update -----
CO_COLS = "grid-template-columns: minmax(0,.75fr) 158px 84px 100px 84px minmax(0,1.8fr)"

def cond(v, good_col):
    return f'<span class="mono" style="color:{good_col}">{v}</span>'

def co_row(name, state, avail, prog, degr, msg):
    if state == "prog":
        ver = f'<span class="mono"><span style="color:var(--dim)">4.17.8</span> <span style="color:var(--faint)">→</span> <span style="color:var(--accent)">4.17.12</span></span>'
    elif state == "queued":
        ver = f'<span class="mono">4.17.8 <span style="color:var(--faint)">→ 4.17.12</span></span>'
    else:
        ver = f'<span class="mono"><span style="color:var(--dim)">4.17.8 →</span> 4.17.12</span>'
    a = cond("True", C["green"]) if avail else cond("False", C["red"])
    p = f'<span class="pill" style="color:var(--accent)">{spinner(11)}<span class="mono">True</span></span>' if prog else cond("False", C["dim"])
    dg = cond("True", C["red"]) if degr else cond("False", C["dim"])
    op = "opacity:.55;" if state == "done" else ""
    return (f'<div class="tr" style="{CO_COLS};{op}"><span class="mono" style="font-weight:{500 if state == "prog" else 400}">{name}</span>{ver}{a}{p}{dg}'
            f'<span style="font-size:12px;color:{"var(--muted)" if state == "prog" else "var(--dim)"}">{msg}</span></div>')

def updates_progress_screen():
    chips = f'<span class="chip" style="background:#2d3b4d;color:#a8cdf3">{spinner(11)}updating</span>'
    stat = lambda k, v: f'<span style="white-space:nowrap">{k} <b style="font-weight:500;color:var(--text)">{v}</b></span>'
    banner = f'''<div class="card" style="padding:14px 16px;display:flex;flex-direction:column;gap:10px;border-color:#3f5a78;background:#2c3440">
<div style="display:flex;align-items:center;gap:10px">{spinner(16)}<span style="font-size:15px;font-weight:600">Updating to 4.17.12</span><span style="font-size:13px;color:var(--muted)">· 61% · started 38 min ago</span><div style="flex:1"></div><span class="mono" style="font-size:12px;color:var(--dim)">4.17.8 → 4.17.12 · stable-4.17</span></div>
<div class="bar" style="height:6px"><i style="width:61%;background:var(--accent)"></i></div>
<div style="display:flex;gap:10px;align-items:baseline;font-size:12px"><span style="color:var(--dim);white-space:nowrap">Progressing</span><span class="mono" style="font-size:12px;color:var(--text)">Working towards 4.17.12: 512 of 845 done (61% complete), waiting on machine-config</span></div>
<div style="display:flex;gap:20px;font-size:12px;color:var(--dim)">{stat("Elapsed", "38 min")}{stat("Estimate", "about 25 min left")}<span>(the last update took 1 h 12 min)</span>{stat("Cluster operators", "28 / 33")}{stat("Nodes", "5 / 6")}<span style="flex:1"></span><span>Closing Kubyl doesn't stop the update.</span></div>
</div>'''
    cos = [("machine-config", "prog", True, True, False, "Working towards 4.17.12: pool worker, 2 of 3 nodes updated"),
           ("network", "prog", True, True, False, "DaemonSet openshift-ovn-kubernetes/ovnkube-node is rolling out: 5 of 6 updated"),
           ("dns", "prog", True, True, False, "DNS default is updating: 4 of 6 pods at the new version"),
           ("node-tuning", "queued", True, False, False, "waits for machine-config"),
           ("storage", "queued", True, False, False, "waits for machine-config"),
           ("kube-apiserver", "done", True, False, False, "NodeInstallerProgressing: 3 nodes are at revision 14"),
           ("etcd", "done", True, False, False, "EtcdMembersAvailable: 3 members are available"),
           ("authentication", "done", True, False, False, "All is well"),
           ("ingress", "done", True, False, False, "The deployment has Available status condition set to True")]
    show_all = '<a href="#" style="font-size:12px;text-decoration:none">Show all 33</a>'
    co = (f'<div class="card" style="overflow:hidden">{card_head("Cluster operators · 28 of 33 updated", "progressing first", show_all)}'
          f'<div class="th" style="{CO_COLS}"><span>NAME</span><span>VERSION</span><span>AVAILABLE</span><span>PROGRESSING</span><span>DEGRADED</span><span>MESSAGE</span></div>'
          + "".join(co_row(*c) for c in cos)
          + f'<div style="height:30px;display:flex;align-items:center;padding:0 12px;font-size:12px;color:var(--dim)">and 24 more at 4.17.12</div></div>')
    draining = f'<span class="pill" style="color:var(--accent);white-space:nowrap">{spinner(12)}draining <span {M115}>ip-10-0-31-4.example.internal</span></span>'
    mcp = (f'<div class="card" style="overflow:hidden">{card_head("Machine config pools", "nodes update one at a time per pool (maxUnavailable)", sbtn("Pause worker pool…", "pause", "g"))}'
           f'{MCP_HEAD}{mcp_row("master", "3 / 3", 100, "3", "0", mcp_updated("4.17.12"), "1")}{mcp_row("worker", "2 / 3", 67, "2", "0", draining, "1", C["accent"])}</div>')
    content = upd_page(ocp_header(chips=chips), [banner, co, mcp], thumb=(2, 470))
    return upd_app("Cluster update in progress — Kubyl", content)

# ----- confirmation -----
def updates_confirm_screen():
    row = lambda k, v: f'<div style="display:flex;gap:10px;align-items:flex-start;font-size:12.5px;line-height:18px"><span style="color:var(--dim);width:96px;flex-shrink:0">{k}</span><div style="flex:1;min-width:0">{v}</div></div>'
    chk = lambda icon, col, t, s: (f'<div style="display:flex;gap:7px;align-items:flex-start;font-size:12px;line-height:18px"><span style="display:flex;padding-top:3px">{ic(icon,12,col)}</span>'
                                   f'<span>{t} <span style="color:var(--dim)">· {s}</span></span></div>')
    checks = (f'<div style="display:flex;flex-direction:column;gap:2px">{pf_summary(1, 2, 5)}'
              + chk("err", C["red"], "PodDisruptionBudget blocks node drain", f'<span {M11}>payments/ledger-writer-pdb</span>')
              + chk("alert", C["yellow"], "Deprecated APIs still requested", "v1beta3 flowschemas, removed in 1.32")
              + chk("alert", C["yellow"], "Helm release uses removed APIs", f'<span {M11}>payments/legacy-app</span>') + '</div>')
    modal = f'''<div style="position:absolute;inset:0;background:rgba(15,17,21,.55);display:flex;align-items:flex-start;justify-content:center;padding-top:58px">
<div role="dialog" aria-label="Update {OCP_PROD} to 4.17.12" style="width:580px;background:#2f343e;border:1px solid var(--border);border-radius:10px;box-shadow:0 20px 60px rgba(0,0,0,.5);overflow:hidden">
<div style="display:flex;align-items:center;gap:10px;padding:14px 16px;border-bottom:1px solid var(--bv)">{ic("up",16,C["accent"])}<b style="font-weight:600;flex:1">Update {OCP_PROD} to 4.17.12?</b><span class="prod" style="font-size:9.5px;padding:0 4px">PROD</span></div>
<div style="padding:16px;display:flex;flex-direction:column;gap:10px">
{row("Cluster", f'<span {M115}>{OCP_PROD_CTX}</span>')}
{row("Provider", "OpenShift (ClusterVersion)")}
{row("From → To", '<span class="mono" style="font-size:12px">4.17.8 → <span style="color:var(--accent)">4.17.12</span></span> <span style="color:var(--dim)">(z-stream, recommended)</span>')}
{row("Channel", f'<span {M115}>stable-4.17</span>')}
{row("What changes", f'<span {M115}>spec.desiredUpdate = 4.17.12</span> <span style="color:var(--dim)">(like <span {M11}>oc adm upgrade --to 4.17.12</span>)</span>')}
{row("Checks", checks)}
<div style="display:flex;gap:10px;padding:10px 12px;border-radius:7px;background:#3a2a2d;border:1px solid #6a3a3f">{ic("alert",15,C["red"])}<div style="font-size:12.5px;line-height:18px"><b style="font-weight:600;color:var(--red)">Updates can't be undone.</b> OpenShift doesn't roll back a cluster; a failed update is fixed forward.</div></div>
{check(False, "I've read the pre-flight results", "Required because a check failed")}
<div style="display:flex;flex-direction:column;gap:6px"><div style="font-size:12px;color:var(--muted)">Type <span class="mono" style="color:var(--text)">{OCP_PROD}</span> to confirm</div>
<div class="inp focus" style="height:28px"><span class="mono" style="font-size:12.5px;color:var(--text)">ocp-prod.exa</span><span style="display:inline-block;width:1px;height:15px;background:var(--accent);margin-left:-6px"></span></div></div>
</div>
<div style="display:flex;align-items:center;gap:8px;padding:12px 16px;border-top:1px solid var(--bv)"><span style="font-size:11.5px;color:var(--dim)">The cluster runs the update; Kubyl tracks it.</span><span style="flex:1"></span><button class="btn g">Cancel</button><button class="btn" style="background:var(--red);border-color:var(--red);color:{INK};font-weight:600;opacity:.45">{ic("up",12,INK)}Start update</button></div>
</div></div>'''
    content = upd_page(ocp_header(), ocp_sections(), thumb=(2, 520))
    return upd_app("Update confirmation — Kubyl", content, overlay=modal)

# ----- Amazon EKS -----
def updates_eks_screen():
    vc = vcard([
        vc_current("1.30", "Kubernetes v1.30.4 · platform eks.12", f'<div class="pill" style="color:var(--green);font-size:11.5px;margin-top:3px">{ic("ok",12,C["green"])}ACTIVE · no health issues</div>'),
        vc_line("Standard support", '<span style="color:var(--yellow)">until 2026-07-23 · extended after</span>'),
        vc_line("Upgrade policy", f'<span {M115}>STANDARD</span><span style="color:var(--dim)"> · EKS has no channels</span>'),
        vc_line("Last update", f'<span {M115}>1.29 → 1.30</span><br><span style="color:var(--dim)">2026-03-14 · took 38 min</span>'),
    ])
    targets = [("1.32", "block", tag("block", "blocked") + dimt("skip-version: EKS updates one minor at a time"), 28, ""),
               ("1.31", "rec", tag("rec", "recommended") + dimt("platform eks.8 · standard support until 2027-11"), 28, "")]
    path = path_card(upd_graph("1.30", "current", targets, lw=40, span=90),
                     sbtn("Update control plane to 1.31…", "up", "p", 26),
                     upd_steps("What the update to 1.31 does", [
                         "Control plane 1.30 → 1.31 · about 10 min, the API stays available",
                         f"Add-ons: <span {M11}>kube-proxy</span> → v1.31.2 and <span {M11}>coredns</span> → v1.11.3, each confirmed",
                         f"Node groups one at a time: <span {M11}>general-m6i</span>, then <span {M11}>gpu-g5</span> · surge 1, maxUnavailable 0"])
                     + path_note("1.32 is offered once the cluster runs 1.31."))
    pf_rows = (pf_row("fail", "PodDisruptionBudget blocks node drain", f'<span {M11}>payments/ledger-writer-pdb</span> allows 0 disruptions', sbtn("Open PDB"), pad=7)
               + pf_row("warn", "Deprecated APIs still requested", f'<span {M11}>flowcontrol.apiserver.k8s.io/v1beta3</span> · 2 clients in 24 h', sbtn("Show requests"), pad=7)
               + pf_row("warn", "Add-on not ready for 1.31", f'<span {M11}>kube-proxy</span> v1.30.3 → v1.31.2 after the control plane', sbtn("Show add-on"), pad=7)
               + pf_passed_line("4 passed: operators, version skew, surge capacity, node groups healthy"))
    pf = pf_card("Pre-flight checks for 1.31", pf_summary(1, 2, 4), pf_rows)
    AD = "grid-template-columns: minmax(0,1fr) minmax(0,1.1fr) 128px 116px"
    ok = f'<span class="pill" style="color:var(--green)">{ic("check",12,C["green"],2.5)}compatible</span>'
    adds = [("vpc-cni", "v1.18.3-eksbuild.1", ok, ""),
            ("coredns", '<span style="color:var(--dim)">v1.11.1 →</span> v1.11.3', '<span style="color:var(--accent);font-size:12px">recommended</span>', sbtn("Update add-on…")),
            ("kube-proxy", '<span style="color:var(--dim)">v1.30.3 →</span> v1.31.2', f'<span class="pill" style="color:var(--yellow)">{ic("alert",12,C["yellow"])}update needed</span>', dimt("after control plane", 11.5)),
            ("aws-ebs-csi-driver", "v1.34.0-eksbuild.1", ok, "")]
    addons = (f'<div class="card" style="flex:1;min-width:0;overflow:hidden">{card_head("Add-ons", "4 managed add-ons")}'
              f'<div class="th" style="{AD}"><span>NAME</span><span>VERSION</span><span>COMPATIBLE WITH 1.31</span><span></span></div>'
              + "".join(f'<div class="tr" style="{AD};height:34px"><span class="mono">{n}</span><span class="mono">{v}</span>{c}<span style="display:flex;justify-content:flex-end">{a}</span></div>' for n, v, c, a in adds)
              + '</div>')
    NG = "grid-template-columns: minmax(0,.8fr) 90px 70px minmax(0,1.1fr) minmax(0,1.5fr) 150px"
    dimc = lambda t: f'<span style="color:var(--dim);font-size:12px">{t}</span>'
    groups = [("Control plane", "managed", "1.30", f'<span {M115}>eks.12</span>', dimc("updated first · about 10 min"), sbtn("Update to 1.31…", "up")),
              ("general-m6i", "6 nodes", "1.30", '<span class="mono" style="font-size:11.5px;color:var(--muted)">AL2023 · 1.30.4-20260812</span>', dimc("waits for the control plane"), dimc("queued")),
              ("gpu-g5", "2 nodes", "1.29", '<span class="mono" style="font-size:11.5px;color:var(--muted)">AL2023 · 1.29.8-20260702</span>',
               f'<div style="display:flex;align-items:center;gap:8px;min-width:0"><span class="mono" style="font-size:11.5px">1 / 2</span>{bar(50, C["accent"], 90)}<span style="font-size:11.5px;color:var(--dim);overflow:hidden;text-overflow:ellipsis">draining ip-10-0-31-4</span></div>',
               f'<span class="pill" style="color:var(--accent)">{spinner(12)}updating to 1.30</span>')]
    cp = (f'<div class="card" style="overflow:hidden">{card_head("Control plane &amp; node groups", "control plane first, then node groups one at a time · surge 1, maxUnavailable 0")}'
          f'<div class="th" style="{NG}"><span>POOL</span><span>SIZE</span><span>VERSION</span><span>AMI / RELEASE</span><span>PROGRESS</span><span></span></div>'
          + "".join(f'<div class="tr" style="{NG};height:36px"><span style="font-weight:500">{n}</span><span style="color:var(--muted)">{s}</span><span class="mono">{v}</span>{ami}{prog}<span style="display:flex;justify-content:flex-end">{a}</span></div>' for n, s, v, ami, prog, a in groups)
          + '</div>')
    head = upd_header("prod-eu-west-1", "Amazon EKS", "AWS API · profile prod · eu-west-1")
    content = upd_page(head, [upd_row(vc, path), upd_row(f'<div style="flex:1;min-width:0">{pf}</div>', addons, align="flex-start"), cp], thumb=(2, 600))
    return upd_app("Cluster updates on Amazon EKS — Kubyl", content, cluster="prod-eu-west-1", color=C["red"], prod=True, meta="EKS · v1.30.4", ocp=False)

# ----- k3s -----
def updates_k3s_screen():
    vc = vcard([
        vc_current("v1.33.4+k3s1", "Kubernetes v1.33.4 · k3s", size=20),
        f'<div>{vc_label("Channel")}{dropdown("stable")}<div style="font-size:11.5px;color:var(--dim);margin-top:5px">channel server: <span {M11}>update.k3s.io</span></div></div>',
        vc_line("Last update", f'<span {M115}>v1.32.7+k3s1 → v1.33.4+k3s1</span><br><span style="color:var(--dim)">2026-08-11 · took 14 min</span>'),
        f'<div>{vc_label("Status")}<span class="pill" style="color:var(--green);font-size:11.5px">{ic("ok",12,C["green"])}4 of 4 nodes at v1.33.4+k3s1</span></div>',
    ])
    targets = [("v1.34.2+k3s1", "latest", tag("latest", "latest") + dimt("latest channel · next minor"), 28, ""),
               ("v1.33.6+k3s1", "rec", tag("rec", "recommended") + dimt("stable channel"), 28, ""),
               ("v1.33.5+k3s1", "avail", dimt("available"), 26, "")]
    path = path_card(upd_graph("v1.33.4+k3s1", "current", targets, lw=96, span=90), sbtn("Release notes", "ext", "g"),
                     upd_steps("What updating the plans does", [
                         f"<span {M11}>server-plan</span> → v1.33.6+k3s1 on k3s-server-1: cordon, k3s restarts",
                         f"<span {M11}>agent-plan</span> → v1.33.6+k3s1 on 3 agents, one at a time: cordon + drain"])
                     + path_note('From <span class="mono" style="font-size:11px">https://update.k3s.io/v1-release/channels</span> · checked 12 min ago'), legend=False)
    pf = pf_card("Pre-flight checks for v1.33.6+k3s1", pf_summary(0, 1, 5, 1),
                 pf_row("warn", "API unavailable while the server restarts", "k3s-server-1 is the only server: the API is down for about a minute while k3s restarts on it", pad=8)
                 + pf_passed_line("5 passed: PodDisruptionBudgets, version skew, surge capacity, deprecated APIs, Helm releases · 1 not applicable"))
    PL = "grid-template-columns: 110px minmax(0,1.6fr) 124px 136px minmax(0,1fr) 84px"
    done = lambda n: f'<div style="display:flex;align-items:center;gap:8px"><span class="mono" style="font-size:11.5px;white-space:nowrap">{n} / {n} nodes</span>{bar(100, C["green"], 60)}<span style="color:var(--green);font-size:12px">complete</span></div>'
    plans = [("server-plan", "node-role.kubernetes.io/control-plane in (true)", "1", done(1)),
             ("agent-plan", "node-role.kubernetes.io/control-plane notin (true)", '1 · <span class="chip" style="height:18px">cordon + drain</span>', done(3))]
    prows = "".join(f'<div class="tr" style="{PL};height:38px"><span class="mono" style="font-weight:500">{n}</span><span class="mono" style="font-size:11.5px;color:var(--muted);padding-right:12px">{sel}</span>'
                    f'<span class="mono">v1.33.4+k3s1</span><span style="display:flex;align-items:center;gap:6px">{c}</span>{s}<span style="display:flex;justify-content:flex-end"><button class="btn g" style="height:24px">{ic("code",12)}YAML</button></span></div>'
                    for n, sel, c, s in plans)
    preview = "".join(diff_line(k, t) for k, t in [("@", " server-plan, agent-plan · spec"), ("-", " version: v1.33.4+k3s1"), ("+", " version: v1.33.6+k3s1")])
    right = f'{sbtn("New plan…", "plus")}{sbtn("Update plans to v1.33.6+k3s1…", "up", "p", 26)}'
    plans_card = (f'<div class="card" style="overflow:hidden">{card_head("Upgrade plans", "upgrade.cattle.io/v1 Plan in system-upgrade", right)}'
                  f'<div class="th" style="{PL}"><span>PLAN</span><span>NODES (SELECTOR)</span><span>VERSION</span><span>CONCURRENCY</span><span>STATUS</span><span></span></div>{prows}'
                  f'<div style="display:flex;gap:16px;padding:12px 16px;align-items:flex-start">'
                  f'<div style="flex:1;display:flex;gap:8px;font-size:12px;color:var(--muted);line-height:18px"><span style="display:flex;padding-top:2px">{ic("info",13,C["accent"])}</span>'
                  f'<span>Plans are created and edited in the YAML editor with a preview: the diff against the live Plans, applied after you confirm. '
                  f'<span {M11}>agent-plan</span> waits for <span {M11}>server-plan</span> (<span {M11}>spec.prepare</span>).</span></div>'
                  f'<div style="width:420px;flex-shrink:0;border:1px solid var(--bv);border-radius:6px;overflow:hidden;background:#2a2e36;padding:3px 0">'
                  f'<div style="font-size:11px;color:var(--dim);padding:2px 10px 4px;text-transform:uppercase;letter-spacing:.06em;font-weight:600">Preview · Update plans</div>{preview}</div></div></div>')
    head = upd_header("k3s-edge", "k3s", "system-upgrade-controller in system-upgrade")
    content = upd_page(head, [upd_row(vc, path), pf, plans_card], thumb=(2, 640))
    return upd_app("Cluster updates on k3s — Kubyl", content, cluster="k3s-edge", color=C["green"], prod=False, meta="k3s · v1.33.4+k3s1", ns="default", ocp=False)

# ----- self-managed and read-only -----
def col_tabs(label, icon="up"):
    return tabs([(icon, label, True)], tools=False)

def updates_selfmanaged_screen():
    nodes = (f'<div style="display:flex;flex-direction:column;gap:4px;font-size:12px">'
             f'<div style="display:flex;gap:8px;white-space:nowrap"><span class="mono" style="font-size:11.5px;width:56px">v1.37.0</span><span style="color:var(--muted);width:50px">5 nodes</span><span style="color:var(--dim);overflow:hidden;text-overflow:ellipsis">cp-1–3, worker-1–2</span></div>'
             f'<div style="display:flex;gap:8px;white-space:nowrap"><span class="mono" style="font-size:11.5px;width:56px;color:var(--yellow)">v1.36.4</span><span style="color:var(--muted);width:50px">1 node</span><span style="color:var(--dim)">worker-3</span></div></div>')
    vc = vcard([vc_current("v1.37.0", "Kubernetes v1.37.0 · kubeadm"),
                f'<div>{vc_label("Kubelets")}{nodes}</div>',
                vc_line("Control plane", f'kube-apiserver <span {M115}>v1.37.0</span> on 3 nodes<br><span style="color:var(--dim)">etcd 3.6.4 · CoreDNS 1.12.1</span>')], width=280)
    info = f'''<div class="card" style="flex:1;min-width:0;padding:14px 16px;display:flex;flex-direction:column;gap:10px">
<div style="display:flex;gap:10px;align-items:flex-start">{ic("info",18,C["accent"])}<div style="min-width:0"><div style="font-size:14px;font-weight:600">Kubyl can't update this cluster</div>
<div style="font-size:12.5px;color:var(--muted);line-height:19px;margin-top:4px">It doesn't know how this cluster was installed. Update it with the tool that installed it (<span {M115}>kubeadm upgrade plan</span>, your distribution's docs).</div></div></div>
<div><div class="mono" style="font-size:11.5px;padding:7px 10px;border-radius:5px;background:#23272e;display:flex;align-items:center;white-space:nowrap">kubeadm upgrade plan<span style="flex:1"></span><button class="ib" aria-label="Copy command" style="width:20px;height:20px">{ic("copy",12)}</button></div>
<div style="font-size:11.5px;color:var(--dim);margin-top:4px">Run it on a control-plane node.</div></div>
<div style="display:flex;gap:16px;font-size:12px"><a href="#" style="display:flex;gap:4px;align-items:center">kubeadm upgrade docs{ic("ext",11)}</a><a href="#" style="display:flex;gap:4px;align-items:center">Version skew policy{ic("ext",11)}</a></div>
<div style="font-size:11.5px;color:var(--dim);margin-top:auto;line-height:17px">Detected from the <span {M11}>kubeadm-config</span> ConfigMap in <span {M11}>kube-system</span>. Version info and pre-flight checks work on every cluster.</div>
</div>'''
    target = f'<button class="btn" style="height:24px;padding:0 8px">Check against <span class="mono" style="font-size:12px">v1.38</span>{ic("cd",12)}</button>'
    rows = (pf_row("warn", "Deprecated APIs still requested", f'<span {M11}>resource.k8s.io/v1beta1</span> resourceclaims · 1 client in the last 24 h, from <span {M11}>apiserver_requested_deprecated_apis</span>', sbtn("Show requests"))
            + pf_row("pass", "No PodDisruptionBudget blocks a drain", "14 checked, each allows at least 1 disruption")
            + pf_row("pass", "Version skew", "kubelets v1.37.0 on 5 nodes and v1.36.4 on worker-3: within n-3 of v1.38")
            + pf_row("pass", "Node capacity for drains", "41% CPU, 52% memory free: one node at a time fits")
            + pf_row("info", "Installed operators", "OLM isn't installed: nothing to check")
            + pf_row("info", "Add-on compatibility", "provided by the provider: none for self-managed clusters", last=True))
    pf = pf_card(f'Pre-flight checks', pf_summary(0, 1, 3, 2), rows,
                 right=f'{target}<span style="font-size:12px;color:var(--dim)">ran 1m ago</span>{sbtn("Re-run", "refresh", "g")}', summary_line=True)
    left = f'''<div style="flex:1.5;min-width:0;display:flex;flex-direction:column;border-right:1px solid var(--border)">
{col_tabs("Cluster Updates · platform-onprem")}
{upd_page(upd_header("platform-onprem", "self-managed", "kubeadm"), [upd_row(vc, info), pf], thumb=(2, 700))}
</div>'''
    ro_chip = f'<span class="chip" style="color:var(--muted)">{ic("lock",11)}read-only</span>'
    banner = (f'<div style="display:flex;gap:10px;align-items:flex-start;padding:10px 12px;border-radius:7px;background:#2c3440;border:1px solid #3f5a78;font-size:12.5px;line-height:18px">'
              f'<span style="display:flex;padding-top:2px">{ic("lock",14,C["accent"])}</span><div><b style="font-weight:600">This cluster is read-only in Kubyl:</b> updates and channel changes are hidden.'
              f'<div style="font-size:11.5px;color:var(--dim)">Set in Clusters &amp; kubeconfigs › Safety. Version info and checks still work.</div></div></div>')
    ro_channel = f'<span {M115}>stable-4.17</span> <span style="color:var(--dim)">{ic("lock",11)} locked</span>'
    ro_last = f'<span {M115}>4.17.6 → 4.17.8</span><span style="color:var(--dim)"> · 2026-08-30</span>'
    ro_vc = (f'<div class="card" style="padding:14px 16px;display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr);gap:12px 16px">'
             f'{vc_current("4.17.8", "OpenShift 4.17.8 · Kubernetes v1.30.6")}'
             f'<div style="display:flex;flex-direction:column;gap:10px">{vc_line("Channel", ro_channel)}{vc_line("Last update", ro_last)}</div></div>')
    ro_targets = [("4.18.2", "block", tag("block", "blocked"), 26, ""), ("4.17.13", "cond", tag("cond", "conditional"), 26, ""),
                  ("4.17.12", "rec", tag("rec", "recommended"), 26, ""), ("4.17.11", "avail", dimt("+ 2 more"), 26, "")]
    ro_path = path_card(upd_graph("4.17.8", "current", ro_targets, lw=52, span=70), dimt("no update button on read-only clusters", 12), legend=False)
    ro_pf = pf_card("Pre-flight checks for 4.17.12", pf_summary(1, 2, 5),
                    pf_row("fail", "PodDisruptionBudget blocks node drain", f'<span {M11}>payments/ledger-writer-pdb</span> allows 0 disruptions', pad=8, last=True),
                    right=sbtn("Re-run", "refresh", "g"), summary_line=True)
    right = f'''<div style="flex:1;min-width:0;display:flex;flex-direction:column">
{col_tabs(f"Cluster Updates · {OCP_PROD}")}
{upd_page(upd_header(OCP_PROD, "OpenShift", "ClusterVersion", ro_chip, recheck=False, size=16), [banner, ro_vc, ro_path, ro_pf], thumb=(2, 690))}
</div>'''
    inner = f'''<div class="app">
{titlebar("platform-onprem", "default", False, "kubeadm · v1.37.0")}
<div class="body">{nav_sidebar("platform-onprem", C["purple"], False, "Cluster Updates", ocp=False, upd_dot=None)}<main class="main" style="flex-direction:row">{left}{right}</main></div>
{statusbar(cluster="platform-onprem", ns="default")}
</div>'''
    return page("Self-managed and read-only clusters — Kubyl", inner)

# ----- credentials missing, provider not built in -----
def updates_credentials_screen():
    creds = f'''<div class="card" style="padding:14px 16px;display:flex;flex-direction:column;gap:10px;border-color:#6b5a2a;background:#33302a">
<div style="display:flex;gap:10px;align-items:flex-start">{ic("key",18,C["yellow"])}<div style="flex:1;min-width:0">
<div style="font-size:14px;font-weight:600">AWS credentials aren't available</div>
<div style="font-size:12.5px;color:var(--muted);line-height:19px;margin-top:3px">The profile <span {M115}>prod</span> has no valid session. Sign in, then retry:</div></div></div>
<div class="mono" style="font-size:12px;padding:7px 10px;border-radius:5px;background:#23272e;display:flex;align-items:center">aws sso login --profile prod<span style="flex:1"></span><button class="ib" aria-label="Copy command" style="width:20px;height:20px">{ic("copy",12)}</button></div>
<dl class="kv" style="margin:0;grid-template-columns:96px minmax(0,1fr)"><dt>Detected</dt><dd>Amazon EKS, from the kubeconfig's exec plugin</dd><dt>Exec plugin</dt><dd class="mono" style="font-size:11.5px">aws eks get-token --cluster-name prod-eu-west-1</dd><dt>Cluster</dt><dd>prod-eu-west-1</dd><dt>Region</dt><dd>eu-west-1</dd><dt>Profile</dt><dd>prod</dd></dl>
<div style="display:flex;gap:8px;align-items:center">{sbtn("Retry", "refresh", "", 26)}{sbtn("Copy command", "copy", "g", 26)}<span style="flex:1"></span>{dimt("no control plane, node group or add-on data until then")}</div>
</div>'''
    k8s = pf_card("Kubernetes-side checks still run", pf_summary(1, 1, 3, 2, '<span style="color:var(--orange)">1 not allowed</span>'),
                  pf_row("fail", "PodDisruptionBudget blocks node drain", f'<span {M11}>payments/ledger-writer-pdb</span> allows 0 disruptions', sbtn("Open PDB"), pad=8)
                  + pf_row("warn", "Deprecated APIs still requested", f'<span {M11}>flowcontrol.apiserver.k8s.io/v1beta3</span> · 2 clients in 24 h', sbtn("Show requests"), pad=8)
                  + pf_row("deny", "Can't read installed operators", f'missing <span {M11}>list clusterserviceversions.operators.coreos.com</span> at cluster scope (403 Forbidden)', sbtn("Copy RBAC rule", "copy", "g"), pad=8)
                  + pf_passed_line("3 passed: version skew, surge capacity, Helm releases", last=False)
                  + pf_row("info", "Add-on compatibility", "needs AWS credentials", pad=8)
                  + pf_row("info", "Control plane &amp; node groups", "needs AWS credentials", pad=8, last=True),
                  right=f'<span style="font-size:12px;color:var(--dim)">against 1.31 · ran 1m ago</span>', summary_line=True)
    left = f'''<div style="flex:1.1;min-width:0;display:flex;flex-direction:column;border-right:1px solid var(--border)">
{col_tabs("Cluster Updates · prod-eu-west-1")}
{upd_page(upd_header("prod-eu-west-1", "Amazon EKS", "AWS API · profile prod", size=16), [creds, k8s], thumb=(2, 700))}
</div>'''
    avail = lambda t: f'<div style="display:flex;gap:7px;align-items:center;font-size:12.5px">{ic("check",13,C["green"],2.5)}{t}</div>'
    gke = f'''<div class="card" style="padding:14px 16px;display:flex;flex-direction:column;gap:10px">
<div style="display:flex;gap:10px;align-items:flex-start">{ic("blocks",18,C["dim"])}<div style="flex:1;min-width:0">
<div style="font-size:14px;font-weight:600">This build doesn't include the GKE provider</div>
<div style="font-size:12.5px;color:var(--muted);line-height:19px;margin-top:3px">It was built without the <span {M115}>updates-gke</span> feature, so Kubyl can't read release channels or start control plane and node pool upgrades.</div></div></div>
<div style="display:flex;flex-direction:column;gap:5px;padding:2px 0 2px 28px"><div style="font-size:12px;color:var(--dim)">Still available</div>{avail("Read-only version info")}{avail("Pre-flight checks")}</div>
<div style="font-size:12px;color:var(--muted);line-height:18px;padding-left:28px">The official releases include every provider. From source:</div>
<div class="mono" style="font-size:11.5px;padding:7px 10px;border-radius:5px;background:#23272e;margin-left:28px">cargo build --release --features updates-gke</div>
</div>'''
    gvc = (f'<div class="card" style="padding:12px 16px;display:flex;gap:18px;align-items:flex-start">{vc_current("v1.31.5-gke.1023000", "Kubernetes v1.31.5 · from /version", size=18)}'
           f'<div style="display:flex;flex-direction:column;gap:8px;min-width:0">{vc_line("Kubelets", f"<span {M115}>v1.31.5</span> on 9 of 9 nodes")}{vc_line("Release channel", "<span style=color:var(--dim)>unknown without the provider</span>")}</div></div>')
    gpf = pf_card("Pre-flight checks for 1.32", pf_summary(0, 0, 2, 1, '<span style="color:var(--orange)">1 not allowed</span>'),
                  pf_row("pass", "Version skew", "kubelets v1.31.5 on 9 of 9 nodes", pad=8)
                  + pf_row("pass", "No PodDisruptionBudget blocks a drain", "22 checked", pad=8)
                  + pf_row("deny", "Can't read deprecated API usage", f'missing <span {M11}>get</span> on <span {M11}>/metrics</span> (nonResourceURLs), and no Prometheus found', sbtn("Copy RBAC rule", "copy", "g"), pad=8)
                  + pf_row("info", "Add-on compatibility", "needs the GKE provider", pad=8, last=True),
                  right=f'<span style="font-size:12px;color:var(--dim)">ran 3m ago</span>', summary_line=True)
    right = f'''<div style="flex:1;min-width:0;display:flex;flex-direction:column">
{col_tabs("Cluster Updates · gke-analytics")}
{upd_page(upd_header("gke-analytics", "GKE", "from gke-gcloud-auth-plugin", size=16), [gke, gvc, gpf], thumb=(2, 700))}
</div>'''
    inner = f'''<div class="app">
{titlebar()}
<div class="body">{nav_sidebar("prod-eu-west-1", C["red"], True, "Cluster Updates", ocp=False, upd_dot=None)}<main class="main" style="flex-direction:row">{left}{right}</main></div>
{statusbar()}
</div>'''
    return page("Update provider unavailable — Kubyl", inner)


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

# ---------- 1 and 10. OpenShift Routes (phase 13, added on request) ----------
# name, host, path, backends (service, weight), target port, tls (termination, insecure policy), admitted (state, reason), age
ROUTES = [
 ("shop-api", "shop.apps.ocp-dev.example.com", "/api", [("shop-web", 80), ("shop-canary", 20)], "http", ("edge", "Redirect"), ("ok", ""), "12d"),
 ("shop-web", "shop.apps.ocp-dev.example.com", "", [("shop-web", None)], "8080", ("edge", "Redirect"), ("ok", ""), "12d"),
 ("payments-gw", "pay.apps.ocp-dev.example.com", "", [("payments-gw", None)], None, ("passthrough", ""), ("ok", ""), "30d"),
 ("admin-console", "admin.apps.ocp-dev.example.com", "", [("admin-ui", None)], "https", ("reencrypt", "Allow"), ("ok", ""), "6d"),
 ("tenant-wildcard", "*.apps.example.com", "", [("tenant-router", None)], "http", ("edge", "None"), ("ok", ""), "3d"),
 ("shop-ingress-x7k2p", "legacy.apps.ocp-dev.example.com", "", [("shop-legacy", None)], "http", ("edge", "Redirect"), ("ok", ""), "45d"),
 ("shop-api-v2", "shop.apps.ocp-dev.example.com", "/api", [("shop-web-v2", None)], "http", ("edge", "Redirect"), ("err", "HostAlreadyClaimed"), "2h"),
 ("metrics-internal", "metrics.apps.ocp-dev.example.com", "", [("prometheus-shop", None)], "9090", None, ("pending", ""), "8s"),
]
# Sized so that no cell needs an ellipsis next to a 320 px details dock: hosts in the UI font, the rest mono 11.5 px,
# several backends on two lines.
RT_COLS = "grid-template-columns: 130px minmax(0,1fr) 124px 78px 106px 164px 28px"
RT_M = 'class="mono" style="font-size:11.5px'

def rt_cells(name, host, path, backends, port, tls, adm, age):
    h = f'<span style="font-size:11.5px;padding-right:8px" title="{host}{path}">{host}<span style="color:var(--dim)">{path}</span></span>'
    if len(backends) == 1:
        s = f'<span {RT_M}">{backends[0][0]}</span>'
    else:
        s = (f'<span {RT_M};line-height:15px;display:flex;flex-direction:column">'
             + "".join(f'<span>{n} <span style="color:var(--dim)">({w}%)</span>{"," if i < len(backends) - 1 else ""}</span>' for i, (n, w) in enumerate(backends)) + '</span>')
    p = f'<span {RT_M}">{port}</span>' if port else f'<span {RT_M};color:var(--faint)">&lt;all&gt;</span>'
    if tls:
        t = f'<span {RT_M}">{tls[0]}{"<span style=color:var(--dim)>/" + tls[1] + "</span>" if tls[1] else ""}</span>'
    else:
        t = f'<span {RT_M};color:var(--faint)">none</span>'
    state, reason = adm
    if state == "ok":
        a = f'<span class="pill" style="color:var(--green);font-size:11.5px;padding-left:6px">{ic("check",12,C["green"],2.5)}default</span>'
    elif state == "err":
        a = f'<span class="pill" style="color:var(--red);font-size:11px;gap:4px;padding-left:6px" title="Route shop-api is older and already claims this host and path">{dot(C["red"])}default: {reason}</span>'
    else:
        a = '<span style="color:var(--faint);font-size:11.5px;padding-left:6px">pending</span>'
    return f'<span {RT_M}">{name}</span>{h}{s}{p}{t}{a}<span {RT_M};color:var(--muted)">{age}</span>'

def routes_center():
    head = (f'<div class="th" style="{RT_COLS}"><span>NAME {ic("cd",10)}</span><span>HOST</span><span>SERVICES</span><span>TARGET PORT</span>'
            f'<span>TLS</span><span style="padding-left:6px">ADMITTED</span><span>AGE</span></div>')
    rows = "".join(f'<div class="tr{" on" if i == 0 else ""}" style="{RT_COLS};height:{42 if len(r[3]) > 1 else 32}px">{rt_cells(*r)}</div>' for i, r in enumerate(ROUTES))
    toolbar = f'''<div class="tool">
<div class="crumb">{ic("route",14,C["accent"])}<b>Routes</b><span>·</span><span>8 in shop</span><span style="color:var(--red)">· 1 not admitted</span></div>
<div style="flex:1"></div>
<div class="inp" style="width:210px">{ic("filter",12)}Filter</div>
<div style="display:flex;gap:4px"><span class="chip on">shop {ic("x",10)}</span><span class="chip">+ namespace</span></div>
<button class="btn g" aria-label="Columns">{ic("sliders",13)}</button>
<span class="chip" style="color:var(--green)">{dot(C["green"])}live</span>
</div>'''
    return f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{toolbar}
{head}
<div style="flex:1;overflow:hidden">{rows}</div>
{hints([("↵","Details"),("w","Web view"),("⇧f","Forward"),("o","Open in browser"),("e","Edit YAML"),("⌃d","Delete"),("/","Filter")])}
</div>'''

def rkv(k, v, w=76):
    return f'<div style="display:flex;gap:8px;font-size:12px;line-height:18px"><span style="width:{w}px;flex-shrink:0;color:var(--dim)">{k}</span><div style="flex:1;min-width:0">{v}</div></div>'

ROUTE_URL = "https://shop.apps.ocp-dev.example.com/api"

def routes_screen():
    mono = lambda t, c="var(--text)": f'<span class="mono" style="font-size:11.5px;color:{c}">{t}</span>'
    ok = f'{ic("check",11,C["green"],2.5)}'
    backend = lambda name, w, extra, sub: (f'<div style="display:flex;flex-direction:column;gap:2px;padding:6px 0;border-bottom:1px solid var(--bv)">'
                                           f'<div style="display:flex;align-items:center;gap:7px;font-size:12px;white-space:nowrap">{ic("network",13,C["dim"])}<a href="#" class="mono" style="font-size:11.5px;text-decoration:none">{name}</a>'
                                           f'<span class="chip" style="height:18px">weight {w}%</span><span style="flex:1"></span>{extra}</div>'
                                           f'<div style="display:flex;align-items:center;gap:8px;font-size:11.5px;color:var(--dim);padding-left:20px;white-space:nowrap">port 80/TCP → 8080{sub}</div></div>')
    fwd_btn = f'<button class="btn" style="height:22px;padding:0 8px">{ic("link",11)}Forward</button>'
    running = (f'<span style="flex:1"></span><span style="display:flex;align-items:center;gap:4px;color:var(--green)">{ic("link",11,C["green"])}<span class="mono" style="font-size:11.5px">→ localhost:8080</span></span>'
               f'<button class="ib" aria-label="Stop forward" style="width:20px;height:20px">{ic("x",11)}</button>')
    eps = "".join(f'<span class="chip mchip">{e}</span>' for e in ["10.128.2.14:8080", "10.131.0.22:8080", "10.129.4.7:8080"])
    pods = "".join(f'<div style="display:flex;align-items:center;gap:7px;font-size:12px;padding:1px 0">{dot(C["green"])}<a href="#" class="mono" style="font-size:11.5px;text-decoration:none">{p}</a><span style="color:var(--dim)">{s}</span></div>'
                   for p, s in [("shop-web-6d8f9b7c4-k2x9p", "shop-web"), ("shop-web-6d8f9b7c4-p8r2m", "shop-web"), ("shop-canary-5b7c9d8f6-q4zt1", "shop-canary")])
    dock = f'''<aside class="dock" style="width:314px">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Route details</span><button class="ib" aria-label="Pin">{ic("star",13)}</button><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec"><div class="mono" style="font-size:12.5px;margin-bottom:6px">shop-api</div>
<div style="display:flex;gap:6px;flex-wrap:wrap"><span class="chip" style="color:var(--green)">{ok}admitted · default</span><span class="chip mchip">edge/Redirect</span><span class="chip">12d</span></div></div>
<div class="dsec"><p class="dtitle">Route</p>
<div style="display:flex;flex-direction:column;gap:4px">
{rkv("Host", mono("shop.apps.ocp-dev.example.com"))}
{rkv("Path", mono("/api"))}
{rkv("TLS", "edge · insecure: Redirect")}
{rkv("Wildcard", "None <span style=color:var(--dim)>(policy)</span>")}
{rkv("Target port", mono("http") + ' → Service port 80 → container 8080')}
</div>
<a href="#" class="mono" style="display:block;margin-top:8px;font-size:11.5px;text-decoration:none;white-space:nowrap;overflow:hidden;text-overflow:ellipsis">{ROUTE_URL}</a>
<div style="display:flex;gap:6px;margin-top:6px"><button class="btn" style="height:24px">{ic("ext",12)}Open in browser</button><button class="btn g" style="height:24px">{ic("globe",12)}Web view</button><button class="ib" aria-label="Copy URL" title="Copy URL">{ic("copy",13)}</button></div></div>
<div class="dsec"><p class="dtitle">Routers · 1</p>
<div style="border:1px solid var(--bv);border-radius:6px;background:#2a2e36;padding:7px 10px;display:flex;flex-direction:column;gap:2px;font-size:12px">
<div style="display:flex;align-items:center;gap:6px"><span class="mono" style="font-size:12px;font-weight:500">default</span><span style="flex:1"></span><span class="pill" style="color:var(--green)">{ok}Admitted · 12d</span></div>
<div style="color:var(--dim);font-size:11.5px">canonical hostname</div><div class="mono" style="font-size:11px;color:var(--muted);white-space:nowrap;overflow:hidden;text-overflow:ellipsis">router-default.apps.ocp-dev.example.com</div>
<div style="color:var(--dim);white-space:nowrap;overflow:hidden;text-overflow:ellipsis">host {mono("shop.apps.ocp-dev.example.com", "var(--muted)")}</div></div></div>
<div class="dsec"><p class="dtitle">TLS</p>
<div style="display:flex;flex-direction:column;gap:4px">
{rkv("Certificate", '<span style="color:var(--green)">expires in 81 days</span><span style="color:var(--dim)"> · 2026-12-16</span><div class="mono" style="font-size:11px;color:var(--muted);white-space:nowrap;overflow:hidden;text-overflow:ellipsis">CN=shop.apps.ocp-dev.example.com</div>')}
{rkv("CA", "present · 1 certificate")}
{rkv("Key", f'<div style="display:flex;align-items:center;gap:4px"><span class="mono" style="font-size:12px;color:var(--dim);letter-spacing:1px">••••••••</span><span style="flex:1"></span><button class="ib" aria-label="Reveal key" style="width:20px;height:20px">{ic("eye",12)}</button><button class="ib" aria-label="Copy key" style="width:20px;height:20px">{ic("copy",12)}</button></div>')}
</div></div>
<div class="dsec" style="border-bottom:0;padding-top:10px"><p class="dtitle" style="margin-bottom:2px">Backends · 2</p>
{backend("shop-web", 80, fwd_btn, "")}
{backend("shop-canary", 20, "", running)}
<div style="font-size:11.5px;color:var(--dim);margin:8px 0 4px">Endpoints · 3 ready</div>
<div style="display:flex;gap:4px;flex-wrap:wrap">{eps}</div>
<div style="font-size:11.5px;color:var(--dim);margin:8px 0 3px">Pods · 3</div>
{pods}
</div>
</aside>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{routes_center()}{dock}</div>'
    tb = tabs([("route", "Routes", True), ("network", "Services", False), ("box", "Pods", False)])
    fwd = f'<span style="color:var(--text)">{ic("link",12,C["green"])}1 forward</span>'
    inner = f'''<div class="app">
{titlebar(OCP_DEV_CTX, "shop", False, "OpenShift · v1.30.6")}
<div class="body">{nav_sidebar(OCP_DEV_CTX, C["accent"], False, "Routes", open_=("Network",), upd_dot=None)}<main class="main">{tb}{content}</main></div>
{statusbar(right_extra=fwd, cluster=OCP_DEV, ns="shop")}
</div>'''
    return page("OpenShift Routes — Kubyl", inner)

def route_webview_screen():
    # a generic API reference served by the backend (placeholder content, not any vendor's UI)
    method = lambda m, col: f'<span style="font-family:\'IBM Plex Mono\',monospace;font-size:11px;font-weight:600;color:#fff;background:{col};border-radius:3px;padding:1px 6px;display:inline-block;width:40px;text-align:center">{m}</span>'
    GET, POST, DEL, PUT = "#2563eb", "#15803d", "#b91c1c", "#b45309"
    nav = [("Products", [("GET", GET, "/api/products", True), ("GET", GET, "/api/products/{id}", False)]),
           ("Cart", [("GET", GET, "/api/cart", False), ("POST", POST, "/api/cart/items", False), ("DEL", DEL, "/api/cart/items/{id}", False)]),
           ("Orders", [("POST", POST, "/api/orders", False), ("GET", GET, "/api/orders/{id}", False), ("PUT", PUT, "/api/orders/{id}/cancel", False)])]
    navh = "".join(f'<div style="font-size:11px;font-weight:600;color:#6b7280;text-transform:uppercase;letter-spacing:.05em;margin:12px 0 6px">{g}</div>'
                   + "".join(f'<div style="display:flex;gap:8px;align-items:center;padding:4px 6px;border-radius:4px;{"background:#e8eefc" if on else ""}">{method(m, c)}<span style="font-family:\'IBM Plex Mono\',monospace;font-size:11.5px;color:#2a2f36">{p}</span></div>' for m, c, p, on in items)
                   for g, items in nav)
    js = lambda k, v: f'<div>&nbsp;&nbsp;&nbsp;&nbsp;<span style="color:#b91c1c">"{k}"</span>: {v},</div>'
    sv = lambda v: f'<span style="color:#15803d">"{v}"</span>'
    nv = lambda v: f'<span style="color:#b45309">{v}</span>'
    body = ('<div>{</div><div>&nbsp;&nbsp;<span style="color:#b91c1c">"items"</span>: [</div><div>&nbsp;&nbsp;&nbsp;&nbsp;{</div>'
            + js("id", sv("prd_1042")) + js("name", sv("Trail Runner 3")) + js("category", sv("shoes")) + js("price", nv("89.90")) + js("stock", nv("14"))
            + '<div>&nbsp;&nbsp;&nbsp;&nbsp;},</div><div>&nbsp;&nbsp;&nbsp;&nbsp;<span style="color:#9aa1ab">… 19 more</span></div><div>&nbsp;&nbsp;],</div>'
            + '<div>&nbsp;&nbsp;<span style="color:#b91c1c">"next"</span>: <span style="color:#15803d">"/api/products?cursor=eyJpZCI6MTA2Mn0"</span></div><div>}</div>')
    param = lambda n, t, d: f'<div style="display:grid;grid-template-columns:110px 80px minmax(0,1fr);font-size:12px;padding:6px 0;border-top:1px solid #eef0f3"><span style="font-family:\'IBM Plex Mono\',monospace;font-size:11.5px">{n}</span><span style="color:#6b7280">{t}</span><span style="color:#4b5563">{d}</span></div>'
    web = f"""<div style="flex:1;min-height:0;background:#f4f5f7;color:#2a2f36;font-family:'IBM Plex Sans',system-ui,sans-serif;display:flex;flex-direction:column;overflow:hidden">
<div style="height:44px;flex-shrink:0;background:#ffffff;border-bottom:1px solid #dfe2e7;display:flex;align-items:center;gap:12px;padding:0 16px">
<span style="width:22px;height:22px;border-radius:5px;background:#0f766e;display:block"></span><span style="font-weight:600">Shop API</span><span style="font-size:11.5px;color:#4b5563;border:1px solid #dfe2e7;border-radius:10px;padding:1px 8px">v2.3.1</span><span style="font-size:12px;color:#6b7280">OpenAPI 3.1</span>
<div style="flex:1"></div><span style="font-size:12px;color:#4b5563;border:1px solid #dfe2e7;border-radius:4px;padding:3px 10px">Search endpoints</span><span style="font-size:12px;color:#ffffff;background:#0f766e;border-radius:4px;padding:4px 10px">Authorize</span>
</div>
<div style="flex:1;display:flex;min-height:0">
<div style="width:240px;flex-shrink:0;background:#ffffff;border-right:1px solid #dfe2e7;padding:4px 12px;overflow:hidden">{navh}</div>
<div style="flex:1;min-width:0;padding:16px 20px;display:flex;flex-direction:column;gap:12px;overflow:hidden">
<div style="display:flex;align-items:center;gap:10px">{method("GET", GET)}<span style="font-family:'IBM Plex Mono',monospace;font-size:14px;font-weight:500">/api/products</span><span style="color:#6b7280;font-size:12.5px">List products</span></div>
<div style="background:#ffffff;border:1px solid #dfe2e7;border-radius:6px;padding:12px 14px">
<div style="font-size:12.5px;font-weight:600;margin-bottom:4px">Query parameters</div>
{param("category", "string", "Only products in this category")}{param("limit", "integer", "Page size, 1–100 (default 20)")}{param("cursor", "string", "From the previous page's next field")}
<div style="display:flex;gap:8px;margin-top:10px"><span style="font-size:12px;color:#ffffff;background:#2563eb;border-radius:4px;padding:4px 12px">Try it out</span><span style="font-size:12px;color:#4b5563;border:1px solid #dfe2e7;border-radius:4px;padding:3px 10px">Copy as curl</span></div>
</div>
<div style="background:#ffffff;border:1px solid #dfe2e7;border-radius:6px;padding:12px 14px;display:flex;flex-direction:column;gap:8px">
<div style="display:flex;align-items:center;gap:8px;font-size:12.5px"><span style="font-weight:600">Response</span><span style="color:#15803d;font-weight:600">200</span><span style="color:#6b7280">application/json · 42 ms · served by shop-web-6d8f9b7c4-k2x9p</span></div>
<div style="font-family:'IBM Plex Mono',monospace;font-size:12px;line-height:19px;background:#f8fafc;border:1px solid #eef0f3;border-radius:4px;padding:8px 12px;color:#2a2f36">{body}</div>
</div>
</div></div>
</div>"""
    toolbar = f"""<div class="tool" style="gap:4px;height:42px">
<button class="ib" aria-label="Back">{ic("left",14)}</button><button class="ib" aria-label="Forward">{ic("right",14)}</button><button class="ib" aria-label="Reload">{ic("refresh",13)}</button>
<div class="inp" style="flex:1;height:28px;margin:0 6px;gap:8px">
<span class="chip mchip" style="height:19px">{ic("link",11,C["green"])}svc/shop-web:80</span>
<span class="mono" style="font-size:12px;color:var(--dim);white-space:nowrap">http://127.0.0.1:18080</span><span class="mono" style="font-size:12px;color:var(--text);margin-left:-8px;white-space:nowrap">/api/docs</span>
<span style="flex:1"></span>
<span style="font-size:11.5px;color:var(--dim);display:flex;align-items:center;gap:4px;white-space:nowrap">{ic("lock",11)}isolated session</span>
</div>
<span class="chip">100%</span>
<button class="btn g" style="height:26px">{ic("ext",13)}Open in browser</button>
<button class="ib" aria-label="Developer tools">{ic("code",14)}</button>
<button class="ib" aria-label="More">{ic("more",14)}</button>
</div>"""
    center = f'<div style="flex:1;display:flex;flex-direction:column;min-width:0">{toolbar}{web}</div>'
    openbtn = f'<button class="btn" style="height:24px;border-color:var(--accent);color:#a8cdf3;flex-shrink:0">{ic("globe",12,C["accent"])}Open · 1 tab</button>'
    wvbtn = f'<button class="btn" style="height:24px;flex-shrink:0">{ic("globe",12)}Web view</button>'
    wrow = lambda top, sub, btn, border=True: (f'<div style="display:flex;align-items:center;gap:10px;padding:8px 0;{"border-bottom:1px solid var(--bv)" if border else ""}">'
                                               f'<div style="flex:1;min-width:0"><div class="mono" style="font-size:11.5px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis">{top}</div>'
                                               f'<div style="font-size:11.5px;color:var(--dim)">{sub}</div></div>{btn}</div>')
    dock = f"""<aside class="dock" style="width:340px">
<div class="phead" style="border-bottom:1px solid var(--bv)">{ic("route",13,C["accent"])}<span style="flex:1;color:var(--text);font-weight:500">route/shop-api</span><span style="font-size:11.5px">shop</span></div>
<div class="dsec"><p class="dtitle">Web views</p>
<div class="mono" style="font-size:11.5px;color:var(--muted);padding-bottom:2px">shop.apps.ocp-dev.example.com/api</div>
{wrow('<span style="color:var(--dim)">→</span> shop-web:80', "80% · port http → 8080", openbtn)}
{wrow('<span style="color:var(--dim)">→</span> shop-canary:80', "20% · port http → 8080", wvbtn, False)}
<div style="display:flex;gap:8px;font-size:11.5px;color:var(--muted);line-height:17px;margin-top:6px"><span style="display:flex;padding-top:1px">{ic("info",12,C["accent"])}</span><span>Opens the backend Service through a temporary forward, even when the Route host isn't reachable from here.</span></div>
</div>
<div class="dsec"><p class="dtitle">Open in browser</p>
<div style="display:flex;align-items:center;gap:10px"><a href="#" class="mono" style="flex:1;min-width:0;font-size:11.5px;text-decoration:none;white-space:nowrap;overflow:hidden;text-overflow:ellipsis">{ROUTE_URL}</a><button class="btn" style="height:24px;flex-shrink:0">{ic("ext",12)}Open in browser</button></div>
<div style="font-size:11.5px;color:var(--dim);line-height:17px;margin-top:5px">Your default browser, through the Route's public host (https: edge TLS).</div>
<div style="display:flex;gap:8px;align-items:flex-start;font-size:11.5px;color:var(--dim);line-height:17px;margin-top:8px;padding-top:8px;border-top:1px dashed var(--bv)"><span style="display:flex;padding-top:1px">{ic("info",12)}</span><span>On <span class="mono" style="font-size:11px">tenant-wildcard</span> (<span class="mono" style="font-size:11px">*.apps.example.com</span>): No browser link for wildcard hosts. The web view still works.</span></div>
</div>
<div class="dsec"><p class="dtitle">Temporary forward</p>
<div style="display:flex;gap:10px;align-items:flex-start">{ic("link",14,C["green"])}<div style="flex:1;min-width:0;font-size:12px;line-height:18px">
<div class="mono" style="font-size:12px">127.0.0.1:18080 → svc/shop-web:80</div>
<div style="color:var(--dim)">via pod shop-web-6d8f9b7c4-k2x9p · 2 connections · 340 KB</div>
<div style="color:var(--muted);margin-top:4px">Hidden from saved forwards. Stops when the last web view tab closes.</div></div></div>
<div style="display:flex;gap:6px;margin-top:10px"><button class="btn d" style="height:24px">{ic("x",12)}Stop and close tabs</button></div>
</div>
<div class="dsec" style="border-bottom:0"><p class="dtitle">Session</p>
<dl class="kv" style="margin:0"><dt>Storage</dt><dd>isolated · {OCP_DEV} / shop / shop-web</dd><dt>Cookies</dt><dd>kept between sessions</dd><dt>Idle stop</dt><dd>after 30 min in background</dd></dl>
</div>
</aside>"""
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    tb = f"""<div class="tabs">
<div class="tab">{ic("route",13)}<span>Routes</span><span class="tabx">{ic("x",11)}</span></div>
<div class="tab on">{ic("globe",13,C["accent"])}<span>shop-web · /api/docs</span><span class="tabx">{ic("x",11)}</span></div>
<div class="tab">{ic("box",13)}<span>Pods</span><span class="tabx">{ic("x",11)}</span></div>
<div class="tabtools"><button class="ib" aria-label="New tab">{ic("plus",14)}</button><button class="ib" aria-label="Split">{ic("split",14)}</button><button class="ib" aria-label="Zoom">{ic("max",13)}</button></div>
</div>"""
    webfwd = f'<span style="color:var(--text)">{ic("globe",12,C["accent"])}1 web forward</span>'
    inner = f"""<div class="app">
{titlebar(OCP_DEV_CTX, "shop", False, "OpenShift · v1.30.6")}
<div class="body">{nav_sidebar(OCP_DEV_CTX, C["accent"], False, "Routes", open_=("Network",), upd_dot=None)}<main class="main">{tb}{content}</main></div>
{statusbar(right_extra=webfwd, cluster=OCP_DEV, ns="shop")}
</div>"""
    return page("Route web view — Kubyl", inner)

# ---------- 11. Kubeconfig editor ----------
KC_INK = "#1b1e24"  # text and icons on accent-filled controls
KC_COMMENT = "#7f8591"
KC_SWATCHES = [C["red"], C["orange"], C["yellow"], C["green"], C["cyan"], C["accent"], C["purple"], "#7a808c"]

# One kubeconfig file open in the editor: its entries and the form of the selected context.
KC_PROD = dict(
    dir="~/work/kube/", file="eks-prod.yaml", changes=2,
    contexts=[("prod-eu-west-1", C["red"], "eks-prod-eu · aws-prod", "ok", True),
              ("prod-us-east-1", C["red"], "eks-prod-us · aws-prod", "ok", False),
              ("staging-eu-west-1", C["yellow"], "eks-staging-eu · aws-staging", "warn", False)],
    clusters=[("eks-prod-eu", "3F9C…gr7.eu-west-1.eks.amazonaws.com", None),
              ("eks-prod-us", "71AD…yl4.us-east-1.eks.amazonaws.com", None),
              ("eks-staging-eu", "C04E…p9x.eu-west-1.eks.amazonaws.com", None)],
    users=[("aws-prod", "exec · aws", None), ("aws-staging", "exec plugin not found", "warn"), ("break-glass", "token", None)],
    ctx="prod-eu-west-1", color=0, cluster=("eks-prod-eu", "3F9C…gr7.eu-west-1.eks…"), user=("aws-prod", "exec · aws eks get-token"),
    ns=("payments", "default", "12 namespaces from the last test"), prod=True, readonly=False,
    problems=[("alert", C["yellow"], 'User <b style="font-weight:500;color:var(--text)">aws-staging</b>: <span class="mono" style="font-size:12px;color:var(--text)">aws</span> wasn\'t found in PATH (login shell)', "Edit user"),
              ("info", C["dim"], 'Cluster <b style="font-weight:500;color:var(--text)">eks-prod-us</b>: CA valid until 2034-05-02', "")],
    last=("2 min ago", 'Connected · 38 ms · v1.30.4-eks · as <span class="mono" style="font-size:12px">arn:aws:iam::…:user/alex</span>'),
)
KC_HOME = dict(
    dir="~/.kube/", file="config", changes=1,
    contexts=[("kind-dev", C["green"], "kind-dev · kind-dev", "ok", True),
              ("docker-desktop", C["accent"], "docker-desktop · docker-desktop", None, False),
              ("homelab-k3s", C["faint"], "homelab · homelab-admin", "err", False)],
    clusters=[("kind-dev", "https://127.0.0.1:52341", None), ("docker-desktop", "kubernetes.docker.internal:6443", None),
              ("homelab", "https://192.168.1.40:6443", None)],
    users=[("kind-dev", "client certificate", None), ("docker-desktop", "client certificate", None), ("homelab-admin", "token", None)],
    ctx="kind-dev", color=3, cluster=("kind-dev", "https://127.0.0.1:52341"), user=("kind-dev", "client certificate"),
    ns=("payments", "default", "6 namespaces from the last test"), prod=False, readonly=False,
    problems=[("err", C["red"], 'Context <b style="font-weight:500;color:var(--text)">homelab-k3s</b>: <span class="mono" style="font-size:12px;color:var(--text)">192.168.1.40:6443</span> unreachable at the last test', "Test again")],
    last=("just now", 'Connected · 2 ms · v1.31.0 · as <span class="mono" style="font-size:12px">kubernetes-admin</span>'),
)
KC_HINTS = [("⌘S", "Save…"), ("⌘T", "Test connection"), ("⌘↵", "Test all"), ("⌘1", "Form"), ("⌘2", "YAML"), ("⌫", "Delete…")]

def kc_toggle(on):
    return (f'<span style="width:28px;height:16px;border-radius:8px;background:{C["accent"] if on else "#4a505c"};position:relative;display:inline-block;flex-shrink:0">'
            f'<span style="position:absolute;top:2px;{"right" if on else "left"}:2px;width:12px;height:12px;border-radius:50%;background:#fff"></span></span>')

def kc_opt(title, sub, on, last=False):
    line = "" if last else "border-bottom:1px solid var(--bv);"
    return (f'<div style="display:flex;gap:12px;align-items:center;padding:8px 0;{line}"><div style="flex:1"><div style="font-size:12.5px">{title}</div>'
            f'<div style="font-size:11.5px;color:var(--dim)">{sub}</div></div>{kc_toggle(on)}</div>')

def kc_seg(items, h=24, stretch=False):
    """Segmented control. items: (label, on) or (label, on, icon)."""
    out = []
    for i, it in enumerate(items):
        label, on = it[0], it[1]
        icon = ic(it[2], 12, C["accent"] if on else C["dim"]) if len(it) > 2 else ""
        look = "background:#2d3b4d;color:#a8cdf3" if on else "color:var(--dim)"
        out.append(f'<span style="display:flex;align-items:center;gap:6px;height:{h}px;padding:0 10px;font-size:12px;white-space:nowrap;box-sizing:border-box;'
                   f'{"flex:1;justify-content:center;" if stretch else ""}{"border-left:1px solid var(--border);" if i else ""}{look}">{icon}{label}</span>')
    return f'<span style="display:flex;border:1px solid var(--border);border-radius:5px;overflow:hidden;flex-shrink:0">{"".join(out)}</span>'

def kc_mark(state, s=12):
    """Result of the last test or validation, shown at the end of list rows."""
    return {"ok": ic("check", s, C["green"], 2.5), "warn": ic("alert", s, C["yellow"]), "err": ic("err", s, C["red"])}.get(state, "")

def kc_badge(state, s=16):
    """Round step marker: ok / err / no (not granted, not an error) / skip."""
    tint = {"ok": ("#a1c18126", "#a1c18166", "check", C["green"]), "err": ("#d0727726", "#d0727766", "x", C["red"])}
    if state in tint:
        bg, bd, icon, col = tint[state]
        return (f'<span style="width:{s}px;height:{s}px;border-radius:50%;background:{bg};border:1px solid {bd};box-sizing:border-box;'
                f'display:flex;align-items:center;justify-content:center;flex-shrink:0">{ic(icon, s - 6, col, 3)}</span>')
    return f'<span style="width:{s}px;height:{s}px;border-radius:50%;border:1px dashed #5d636f;box-sizing:border-box;flex-shrink:0"></span>'

def kc_shell(tabbar, content, overlay=""):
    return f'''<div class="app">
{titlebar()}
<div class="body">
{sidebar("none")}
<main class="main">{tabbar}{content}</main>
</div>
{statusbar()}
{overlay}
</div>'''

def kc_tabs(doc):
    return tabs([("file", doc["file"], True, True), ("gear", "Clusters &amp; kubeconfigs", False), ("box", "Pods", False)])

def kc_toolbar(doc, mode):
    n = doc["changes"]
    return f'''<div class="tool">
<div class="crumb mono" style="font-size:12px;white-space:nowrap">{ic("file",14,C["accent"])}<span>{doc["dir"]}<b>{doc["file"]}</b></span></div>
<span class="chip">{dot(C["green"])}External file · editing on</span>
<span class="chip" style="color:var(--yellow)">{n} unsaved change{"s" if n != 1 else ""}</span>
<div style="flex:1"></div>
{kc_seg([("Form", mode == "form", "sliders"), ("YAML", mode == "yaml", "code")])}
<button class="btn">{ic("flask",13)}Test connection</button>
<button class="btn">Test all</button>
<button class="btn g">{ic("undo",13)}Revert</button>
<button class="btn p">{ic("save",13,KC_INK)}Save…<span class="mono" style="font-weight:400;font-size:11px;opacity:.7">⌘S</span></button>
</div>'''

def kc_list(doc):
    def sec(title, n, first=False):
        return (f'<div class="sec" style="height:28px;padding-right:6px;{"" if first else "margin-top:8px"}">{ic("cd",11)}{title}'
                f'<span style="font-weight:400;letter-spacing:0;text-transform:none;color:var(--faint)">{n}</span><span style="flex:1"></span>'
                f'<button class="ib" aria-label="New {title.lower()[:-1]}" style="width:22px;height:22px">{ic("plus",13)}</button></div>')
    def row(lead, name, sub, state=None, on=False):
        subc = C["yellow"] if state == "warn" and "not found" in sub else C["dim"]
        return (f'<div class="ti{" on" if on else ""}" style="height:26px;padding:0 12px 0 16px;gap:8px">{lead}'
                f'<span style="color:var(--text);flex-shrink:0">{name}</span>'
                f'<span style="flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;text-align:right;font-size:11.5px;color:{subc}">{sub}</span>'
                f'<span style="width:12px;display:flex;justify-content:center;flex-shrink:0">{kc_mark(state)}</span></div>')
    ctx = "".join(row(dot(c), n, s, st, on) for n, c, s, st, on in doc["contexts"])
    cls = "".join(row(ic("server", 13, C["dim"]), n, s, st) for n, s, st in doc["clusters"])
    usr = "".join(row(ic("key", 13, C["yellow"] if st == "warn" else C["dim"]), n, s, st) for n, s, st in doc["users"])
    return f'''<div style="width:290px;flex-shrink:0;border-right:1px solid var(--border);background:var(--panel);display:flex;flex-direction:column;min-height:0">
<div style="padding:10px 10px 6px"><div class="inp">{ic("filter",12)}Filter contexts, clusters, users</div></div>
{sec("Contexts", len(doc["contexts"]), True)}{ctx}
{sec("Clusters", len(doc["clusters"]))}{cls}
{sec("Users", len(doc["users"]))}{usr}
<div style="margin-top:auto;padding:12px 14px;border-top:1px solid var(--bv);display:flex;gap:8px;font-size:11.5px;color:var(--dim);line-height:17px">{ic("history",13)}<span>Comments and key order are kept. Backups:<br><span class="mono" style="font-size:11px;color:var(--muted);white-space:nowrap">~/Library/Application Support/<br>kubyl/kubeconfig-backups</span></span></div>
</div>'''

def kc_form(doc):
    grid = "display:grid;grid-template-columns:120px minmax(0,1fr);gap:8px 14px;align-items:center"
    def lab(t, extra=""):
        return f'<div style="font-size:12.5px;color:var(--muted);height:28px;display:flex;align-items:center;gap:6px">{t}{extra}</div>'
    def select(val, sub=""):
        return (f'<div class="inp" style="width:320px;height:28px;flex-shrink:0"><span class="mono" style="font-size:12px;color:var(--text)">{val}</span>'
                f'<span style="flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:11.5px;text-align:right">{sub}</span>{ic("cd",12)}</div>')
    def text_input(val, placeholder=False):
        return f'<div class="inp" style="width:320px;height:28px"><span class="{"" if placeholder else "mono"}" style="font-size:12px;color:{C["faint"] if placeholder else C["text"]}">{val}</span></div>'
    def line(*parts):
        return '<div style="display:flex;align-items:center;gap:12px;min-width:0">' + "".join(parts) + '</div>'
    link = lambda t: f'<a href="#" style="font-size:12px;text-decoration:none;white-space:nowrap">{t}</a>'
    hint = lambda t: f'<span style="font-size:11.5px;color:var(--dim);white-space:nowrap">{t}</span>'
    ns, was, nshint = doc["ns"]
    changed = f'<span class="dot" style="background:{C["accent"]}" title="Unsaved change"></span>'
    color = next(c for n, c, s, st, on in doc["contexts"] if on)
    prod = '<span class="prod">PROD</span>' if doc["prod"] else ""
    head = f'''<div style="display:flex;align-items:center;gap:10px">
{dot(color)}<span style="font-size:12px;color:var(--dim)">Context</span><h2 class="mono" style="font-size:15px;font-weight:500">{doc["ctx"]}</h2>{prod}
<div style="flex:1"></div>
<button class="btn g">{ic("copy",13)}Duplicate</button><button class="btn g">{ic("pencil",13)}Rename…</button><button class="btn g d">{ic("trash",13,C["red"])}Delete…</button>
</div>'''
    ctx_card = f'''<div class="card" style="padding:12px 16px 14px">
<p class="dtitle">Context</p>
<div style="{grid}">
{lab("Name")}{text_input(doc["ctx"])}
{lab("Cluster")}{line(select(*doc["cluster"]), link("Edit cluster"))}
{lab("User")}{line(select(*doc["user"]), link("Edit user"))}
{lab("Namespace", changed)}{line(select(ns), hint(f'{nshint} · was <span class="mono" style="font-size:11px">{was}</span>'))}
</div></div>'''
    swatches = "".join(f'<span style="width:14px;height:14px;border-radius:50%;background:{c};display:block;{"box-shadow:0 0 0 2px var(--panel),0 0 0 3.5px " + c if i == doc["color"] else ""}"></span>'
                       for i, c in enumerate(KC_SWATCHES))
    over_card = f'''<div class="card" style="padding:12px 16px 4px">
<p class="dtitle">Kubyl overrides <span style="text-transform:none;letter-spacing:0;font-weight:400">· settings.json (not written to the kubeconfig)</span></p>
<div style="{grid}">
{lab("Display name")}{text_input(doc["ctx"], True)}
{lab("Color")}<div style="display:flex;gap:11px;align-items:center;padding-left:3px">{swatches}</div>
</div>
<div style="margin-top:4px">{kc_opt("Production cluster", "Red accent in title bar, typed confirmation for delete / scale to 0", doc["prod"])}{kc_opt("Read-only mode", "Block all mutating requests from this app", doc["readonly"], True)}</div>
</div>'''
    def problem(icon, col, text, act):
        a = f'<a href="#" style="font-size:12px;text-decoration:none">{act}</a>' if act else ""
        return f'<div style="display:flex;gap:10px;align-items:center;padding:4px 0;font-size:12.5px">{ic(icon,14,col)}<span style="flex:1;min-width:0;color:var(--muted)">{text}</span>{a}</div>'
    probs = doc["problems"]
    prob_card = f'''<div class="card" style="padding:12px 16px 8px">
<p class="dtitle" style="margin-bottom:4px">Problems <span style="text-transform:none;letter-spacing:0;font-weight:400">· {len(probs)}</span></p>
{"".join(problem(*p) for p in probs)}
</div>'''
    when, result = doc["last"]
    last = f'''<div style="display:flex;align-items:center;gap:10px;padding:9px 16px;border:1px solid var(--bv);border-radius:8px;background:#2a2e36;font-size:12.5px;white-space:nowrap">
<span class="dtitle" style="margin:0">Last test · {when}</span>{ic("ok",14,C["green"])}<span style="flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis">{result}</span><a href="#" style="font-size:12px;text-decoration:none">Details</a></div>'''
    return f'<div style="flex:1;min-width:0;padding:16px 18px;display:flex;flex-direction:column;gap:12px;overflow:hidden">{head}{ctx_card}{over_card}{prob_card}{last}</div>'

def kc_editor(doc):
    return f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0;min-height:0">
{kc_toolbar(doc, "form")}
<div style="flex:1;display:flex;min-height:0">{kc_list(doc)}{kc_form(doc)}</div>
{hints(KC_HINTS)}
</div>'''

def kubeconfig_screen():
    return page("Kubeconfig editor — Kubyl", kc_shell(kc_tabs(KC_PROD), kc_editor(KC_PROD)))

# YAML tokens, same colors as board 3.
def kc_key(k): return f'<span style="color:{C["red"]}">{k}</span><span style="color:var(--muted)">:</span>'
def kc_str(v): return f'<span style="color:{C["green"]}">{v}</span>'
def kc_punct(p): return f'<span style="color:var(--muted)">{p}</span>'
def kc_com(t): return f'<span style="color:{KC_COMMENT};font-style:italic"># {t}</span>'

def kc_y(key=None, ind=0, val=None, dash=False, com=None):
    s = " " * ind + ('<span style="color:var(--faint)">- </span>' if dash else "")
    if key: s += kc_key(key)
    if val is not None: s += (" " if key else "") + kc_str(val)
    if com: s += ("  " if (key or val) else "") + kc_com(com)
    return s

def kc_yaml_rows():
    args = ["eks", "get-token", "--cluster-name", "prod-eu", "--region", "eu-west-1"]
    flow_args = "      " + kc_key("args") + " " + kc_punct("[") + kc_punct(", ").join(kc_str(a) for a in args) + kc_punct("]")
    flow_env = ("      " + kc_key("env") + " " + kc_punct("[{") + kc_key("name") + " " + kc_str("AWS_PROFILE") + kc_punct(", ")
                + kc_key("value") + " " + kc_str("prod") + kc_punct("}]"))
    caret = '<span style="display:inline-block;width:1px;height:15px;background:var(--accent);vertical-align:-3px"></span>'
    masked = (f'    {kc_key("token")} <span style="color:var(--dim);letter-spacing:.08em">••••••••</span>'
              f'<span class="chip" style="height:17px;margin-left:10px;font-family:\'IBM Plex Sans\';font-size:11px">{ic("eye",11)}Reveal</span>')
    # (line number, html, change mark, folded line count)
    return [
        (1, kc_y(com="prod clusters are managed by the platform team (#platform-oncall)"), None, 0),
        (2, kc_y("apiVersion", 0, "v1"), None, 0),
        (3, kc_y("kind", 0, "Config"), None, 0),
        (4, kc_y("clusters"), None, 0),
        (5, kc_y("name", 0, "eks-prod-eu", dash=True), None, 0),
        (6, kc_y("cluster", 2), None, 0),
        (7, kc_y("server", 4, "https://3F9C0A7B51E2C8D4.gr7.eu-west-1.eks.amazonaws.com"), None, 0),
        (8, kc_y("certificate-authority-data", 4, "LS0tLS1CRUdJTiBDRVJUSUZJQ0FURS0tLS0t…", com="expires 2034"), None, 0),
        (9, kc_y("name", 0, "eks-prod-us", dash=True), None, 3),
        (13, kc_y("name", 0, "eks-staging-eu", dash=True), None, 3),
        (17, kc_y("contexts"), None, 0),
        (18, kc_y("name", 0, "prod-eu-west-1", dash=True), None, 0),
        (19, kc_y("context", 2), None, 0),
        (20, kc_y("cluster", 4, "eks-prod-eu"), None, 0),
        (21, kc_y("user", 4, "aws-prod"), None, 0),
        (22, kc_y("namespace", 4, "payments") + caret, "mod", 0),
        (23, kc_y("name", 0, "prod-us-east-1", dash=True), None, 4),
        (28, kc_y("name", 0, "staging-eu-west-1", dash=True), None, 0),
        (29, kc_y("context", 2), None, 0),
        (30, kc_y("cluster", 4, "eks-staging-eu"), None, 0),
        (31, kc_y("user", 4, "aws-staging"), None, 0),
        (32, kc_y("namespace", 4, "payments"), "mod", 0),
        (33, kc_y("current-context", 0, "prod-eu-west-1"), None, 0),
        (34, kc_y("users"), None, 0),
        (35, kc_y("name", 0, "aws-prod", dash=True), None, 0),
        (36, kc_y("user", 2), None, 0),
        (37, kc_y("exec", 4), None, 0),
        (38, kc_y("apiVersion", 6, "client.authentication.k8s.io/v1beta1"), None, 0),
        (39, kc_y("command", 6, "aws"), None, 0),
        (40, flow_args, None, 0),
        (41, flow_env, None, 0),
        (42, kc_y("name", 0, "aws-staging", dash=True), None, 7),
        (50, kc_y("name", 0, "break-glass", dash=True, com="emergency access only, rotate after use"), None, 0),
        (51, kc_y("user", 2), None, 0),
        (52, masked, None, 0),
    ]

def kc_yaml_editor():
    out = []
    for n, text, mark, fold in kc_yaml_rows():
        cur = n == 22
        bar = C["accent"] if mark == "mod" else "transparent"
        chev = ic("cr", 11, C["dim"]) if fold else ""
        pill = (f'<span style="margin-left:10px;padding:0 6px;border-radius:3px;background:#353a45;color:var(--dim);font-family:\'IBM Plex Sans\';font-size:11px;line-height:16px">⋯ {fold} lines</span>' if fold else "")
        was = '<span style="margin-left:18px;color:var(--faint);font-family:\'IBM Plex Sans\';font-size:11.5px">was default</span>' if mark == "mod" else ""
        out.append(f'<div class="mono" style="display:flex;height:20px;align-items:center;font-size:12px;white-space:pre;{"background:#2f343e;" if cur else ""}">'
                   f'<span style="width:40px;text-align:right;color:{"var(--text)" if cur else "var(--faint)"}">{n}</span>'
                   f'<span style="width:20px;display:flex;justify-content:center">{chev}</span>'
                   f'<span style="width:3px;height:20px;background:{bar}"></span><span style="padding-left:12px">{text}</span>{pill}{was}</div>')
    return f'<div style="flex:1;min-width:0;overflow:hidden;padding-top:6px">{"".join(out)}</div>'

def kc_step(state, name, lines=(), last=False, body="", took=""):
    rail = "" if last else '<span style="flex:1;width:1px;background:var(--border);margin-top:3px"></span>'
    col = {"err": C["red"], "skip": C["faint"]}.get(state, C["text"])
    det = "".join(l if l.startswith("<") else f'<div class="mono" style="font-size:11px;line-height:16px;color:var(--dim);white-space:nowrap;overflow:hidden;text-overflow:ellipsis">{l}</div>' for l in lines)
    t = f'<span class="mono" style="font-size:11px;font-weight:400;color:var(--dim)">{took}</span>' if took else ""
    return (f'<div style="display:flex;gap:10px"><div style="display:flex;flex-direction:column;align-items:center;width:16px;flex-shrink:0">{kc_badge(state)}{rail}</div>'
            f'<div style="flex:1;min-width:0;padding-bottom:7px"><div style="display:flex;justify-content:space-between;align-items:baseline;font-size:12.5px;font-weight:600;line-height:16px;color:{col}">{name}{t}</div>{det}{body}</div></div>')

def kc_perm(granted, text):
    mark = ic("check", 11, C["green"], 2.5) if granted else ic("x", 11, C["faint"], 2.5)
    return f'<div class="mono" style="font-size:11px;line-height:16px;display:flex;gap:6px;align-items:center;color:{C["dim"] if granted else C["faint"]}">{mark}{text}</div>'

def kc_test_panel():
    steps = "".join([
        kc_step("ok", "DNS and TCP", ["3F9C…gr7.eu-west-1.eks.amazonaws.com → 3.121.4.18:443"], took="12 ms"),
        kc_step("ok", "TLS", ['TLS 1.3 · verified by the kubeconfig CA "kubernetes"', "server cert CN=kube-apiserver · valid until 2026-11-02"], took="24 ms"),
        kc_step("ok", "Exec plugin", ["aws eks get-token · token valid until 14:17 (15 min)"], took="0.9 s"),
        kc_step("ok", "API server", ["GET /version · v1.30.4-eks-a737599"], took="38 ms"),
        kc_step("ok", "Authentication", ["arn:aws:iam::123456789012:user/alex", "groups: system:authenticated"], took="41 ms"),
        kc_step("ok", "Permissions", [kc_perm(True, "list namespaces (14)"), kc_perm(True, "list pods in payments"), kc_perm(False, "cluster-admin (not required)")], took="96 ms"),
        kc_step("ok", "Latency", ["38 ms median of 3"], last=True, took="0.1 s"),
    ])
    ns = "".join(f'<span class="chip{" on" if n == "payments" else ""}">{n}</span>' for n in ["payments", "checkout", "ledger", "risk", "monitoring"])
    skipped = "".join(f'<span style="display:flex;align-items:center;gap:6px"><span class="mono">—</span>{t}</span>' for t in ["Exec plugin", "API server", "Authentication", "Permissions", "Latency"])
    tls_fail = (f'<div style="font-size:12px;line-height:17px;color:var(--muted);margin:2px 0 7px">Certificate signed by unknown authority: the server\'s CA isn\'t the one in this kubeconfig.</div>'
                f'<div style="display:flex;gap:6px"><button class="btn" style="height:24px">{ic("fingerprint",12)}Fetch the CA from the server…</button><button class="btn g" style="height:24px">Details</button></div>')
    return f'''<aside class="dock" style="width:420px">
<div style="display:flex;align-items:center;gap:10px;padding:8px 8px 8px 14px;border-bottom:1px solid var(--bv)">{ic("flask",15,C["accent"])}
<div style="flex:1;min-width:0"><div style="color:var(--text);font-weight:500;white-space:nowrap">Test connection · <span class="mono" style="font-size:12.5px">prod-eu-west-1</span></div><div style="font-size:11.5px;color:var(--dim)">from unsaved edits · 1.2 s</div></div>
<button class="ib" aria-label="Run again" title="Run again">{ic("refresh",13)}</button><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div style="padding:12px 14px 2px">{steps}</div>
<div class="card" style="margin:0 14px 12px;padding:9px 12px 10px;background:#2a2e36">
<div style="display:flex;align-items:center;margin-bottom:7px"><span class="dtitle" style="margin:0;flex:1">Namespaces · 14</span><span style="font-size:11.5px;color:var(--dim)">click one: Use as default</span></div>
<div style="display:flex;gap:4px;flex-wrap:nowrap;overflow:hidden">{ns}<span class="chip" style="color:var(--dim)">+9 more</span></div></div>
<div style="border-top:1px solid var(--bv);padding:10px 14px 0">
<div style="display:flex;align-items:center;gap:8px;margin-bottom:9px">{dot(C["yellow"])}<span class="mono" style="font-size:12.5px;font-weight:500">staging-eu-west-1</span><span style="font-size:11.5px;color:var(--red)">failed at TLS · 0.3 s</span><span style="flex:1"></span><a href="#" style="font-size:12px;text-decoration:none">Run again</a></div>
{kc_step("ok", "DNS and TCP", ["C04E…p9x.eu-west-1.eks.amazonaws.com → 18.194.7.33:443"], took="14 ms")}
{kc_step("err", "TLS", body=tls_fail, took="260 ms")}
{kc_step("skip", "Skipped", last=True, body=f'<div style="display:flex;flex-wrap:wrap;gap:2px 14px;margin-top:2px;font-size:12px;color:var(--faint)">{skipped}</div>')}
</div>
</aside>'''

def kubeconfig_yaml_screen():
    banner = f'''<div style="height:38px;flex-shrink:0;display:flex;align-items:center;gap:10px;padding:0 12px;background:#35322a;border-bottom:1px solid #5a4f33;font-size:12.5px">
{ic("alert",14,C["yellow"])}<span style="flex:1;min-width:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis"><b style="font-weight:600;color:var(--yellow)">eks-prod.yaml changed on disk</b> <span style="color:var(--muted)">(<span class="mono" style="font-size:12px">aws eks update-kubeconfig</span>, 14:02). Your 2 unsaved changes are kept.</span></span>
<button class="btn" style="height:24px">{ic("diff",12)}Show diff</button><button class="btn" style="height:24px">{ic("refresh",12)}Reload</button><button class="btn g" style="height:24px">Keep mine</button>
</div>'''
    content = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0;min-height:0">
{kc_toolbar(KC_PROD, "yaml")}
{banner}
<div style="flex:1;display:flex;min-height:0">{kc_yaml_editor()}{kc_test_panel()}</div>
</div>'''
    return page("Kubeconfig editor, YAML and test — Kubyl", kc_shell(kc_tabs(KC_PROD), content))

def kc_stepper(steps, current):
    out = []
    for i, name in enumerate(steps, 1):
        if i < current:
            mark = kc_badge("ok", 18)
            label = f'<span style="font-size:12px;color:var(--muted)">{name}</span>'
        elif i == current:
            mark = f'<span style="width:18px;height:18px;border-radius:50%;background:var(--accent);color:{KC_INK};font-size:11px;font-weight:600;display:flex;align-items:center;justify-content:center;flex-shrink:0">{i}</span>'
            label = f'<span style="font-size:12px;color:var(--text);font-weight:600">{name}</span>'
        else:
            mark = f'<span style="width:18px;height:18px;border-radius:50%;border:1px solid var(--border);box-sizing:border-box;color:var(--dim);font-size:11px;display:flex;align-items:center;justify-content:center;flex-shrink:0">{i}</span>'
            label = f'<span style="font-size:12px;color:var(--dim)">{name}</span>'
        if i > 1:
            out.append(f'<span style="flex:1;min-width:10px;height:1px;background:{"#a1c18166" if i <= current else "var(--border)"}"></span>')
        out.append(f'<span style="display:flex;align-items:center;gap:6px;white-space:nowrap">{mark}{label}</span>')
    return '<div style="display:flex;align-items:center;gap:8px;padding:11px 16px;border-bottom:1px solid var(--bv);background:#2b3039">' + "".join(out) + '</div>'

def kc_wizard_modal():
    label = lambda t: f'<div style="font-size:12px;color:var(--muted);margin-bottom:5px">{t}</div>'
    cursor = '<span style="display:inline-block;width:1px;height:15px;background:var(--accent);margin-left:-6px"></span>'
    ca = kc_seg([("System trust store", False, "shield"), ("File", False, "file"), ("Paste PEM", False, "copy"), ("Fetch from server", True, "download")], h=28, stretch=True)
    fp = "3F:9C:0A:7B:51:E2:C8:D4:19:6A:F0:23:8B:77:E5:0C<br>A1:5D:92:3E:C4:08:B6:71:2F:DA:64:9E:13:C7:58:B0"
    cert = f'''<div class="card" style="background:#2a2e36;overflow:hidden">
<div style="display:flex;align-items:center;gap:8px;padding:8px 10px 8px 12px;border-bottom:1px solid var(--bv);font-size:12.5px">{ic("shieldalert",14,C["yellow"])}<span>Fetched from <span class="mono" style="font-size:12px">https://127.0.0.1:52341</span></span><span style="color:var(--yellow)">· not trusted yet</span><span style="flex:1"></span><button class="ib" aria-label="Copy PEM" title="Copy PEM">{ic("copy",13)}</button></div>
<dl class="kv" style="margin:0;padding:10px 12px 12px;grid-template-columns:86px minmax(0,1fr)">
<dt>Subject</dt><dd class="mono" style="font-size:12px">CN=kubernetes</dd>
<dt>Issuer</dt><dd><span class="mono" style="font-size:12px">CN=kubernetes</span> <span style="color:var(--dim)">(self-signed)</span></dd>
<dt>Valid</dt><dd class="mono" style="font-size:12px">2026-09-24 → 2036-09-22</dd>
<dt>Source</dt><dd><span class="mono" style="font-size:12px">kube-public/cluster-info</span> <span style="color:var(--dim)">(anonymous)</span></dd>
<dt style="display:flex;gap:5px;align-items:flex-start">{ic("fingerprint",13)}SHA-256</dt><dd class="mono" style="font-size:12px;line-height:18px;white-space:normal;color:var(--text)">{fp}</dd>
</dl></div>'''
    return f'''<div style="position:absolute;inset:0;background:rgba(15,17,21,.55);display:flex;align-items:flex-start;justify-content:center;padding-top:70px">
<div role="dialog" aria-label="New kubeconfig" style="width:640px;background:#2f343e;border:1px solid var(--border);border-radius:10px;box-shadow:0 20px 60px rgba(0,0,0,.5);overflow:hidden">
<div style="display:flex;align-items:center;gap:10px;padding:14px 16px;border-bottom:1px solid var(--bv)">{ic("fileplus",16,C["accent"])}<b style="font-weight:600;flex:1">New kubeconfig</b><span style="font-size:12px;color:var(--dim)">Step 2 of 6</span><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
{kc_stepper(["Name", "Cluster", "Credentials", "Context", "Test", "Save"], 2)}
<div style="padding:16px;display:flex;flex-direction:column;gap:14px">
<div style="display:grid;grid-template-columns:200px minmax(0,1fr);gap:12px">
<div>{label("Cluster name")}<div class="inp" style="height:28px"><span class="mono" style="font-size:12.5px;color:var(--text)">kind-dev</span></div></div>
<div>{label("API server")}<div class="inp focus" style="height:28px"><span class="mono" style="font-size:12.5px;color:var(--text)">https://127.0.0.1:52341</span>{cursor}</div></div>
</div>
<div>{label("Certificate authority")}{ca}</div>
{cert}
<div style="font-size:11.5px;color:var(--dim);line-height:17px">Trust on first use: compare this fingerprint with the CA your admin gave you, or <span class="mono" style="font-size:11px;color:var(--muted)">kubectl config view --raw</span> on a machine you trust. No credentials are sent to this server until you confirm.</div>
{check(True, "I compared the fingerprint; trust this CA for kind-dev")}
<div style="display:flex;align-items:center;gap:8px;padding-top:12px;border-top:1px solid var(--bv);font-size:12.5px">{ic("cr",12,C["dim"])}<span>Advanced</span><span style="font-size:11.5px;color:var(--dim)">TLS server name · Proxy URL · Skip TLS verification <span style="color:var(--red)">(insecure)</span></span></div>
</div>
<div style="display:flex;align-items:center;gap:8px;padding:12px 16px;border-top:1px solid var(--bv)">
<span style="flex:1;min-width:0;display:flex;gap:6px;align-items:flex-start;font-size:11.5px;color:var(--dim);line-height:16px">{ic("lock",12)}<span style="white-space:nowrap">Saved to <span class="mono" style="font-size:11px">~/Library/Application Support/kubyl/</span><br><span class="mono" style="font-size:11px">kubeconfigs/kind-dev.yaml</span> (0600)</span></span>
<button class="btn g">Cancel</button><button class="btn">{ic("left",12)}Back</button><button class="btn p">Next: Credentials{ic("right",12,KC_INK)}</button>
</div>
</div></div>'''

def kubeconfig_wizard_screen():
    return clusters_screen(overlay=kc_wizard_modal(), title="New kubeconfig wizard — Kubyl")

def kc_diff():
    # (old line, new line, sign, html)
    y = kc_y
    rows = [
        (18, 18, " ", y("contexts")),
        (19, 19, " ", kc_com("local kind cluster, recreated every Monday")),
        (20, 20, " ", y("name", 0, "kind-dev", dash=True)),
        (21, 21, " ", y("context", 2)),
        (22, 22, " ", y("cluster", 4, "kind-dev")),
        (23, 23, " ", y("user", 4, "kind-dev")),
        (24, "", "-", "    namespace: default"),
        ("", 24, "+", "    namespace: payments"),
        (25, 25, " ", y("name", 0, "docker-desktop", dash=True)),
        (26, 26, " ", y("context", 2)),
    ]
    look = {"-": ("background:#3a2e31;color:#e7a9ad", "#e7a9ad"), "+": ("background:#2d3a2c;color:#bfd9a6", "#bfd9a6")}
    out = [f'<div style="padding:2px 12px;background:#2a2e36;color:var(--dim);border-bottom:1px solid var(--bv)">@@ contexts[kind-dev] @@ <span style="font-family:\'IBM Plex Sans\';font-size:11.5px">· lines 18–26</span></div>']
    for old, new, sign, text in rows:
        bg, _ = look.get(sign, ("", ""))
        out.append(f'<div style="display:flex;{bg}"><span style="width:34px;text-align:right;color:var(--faint)">{old}</span><span style="width:34px;text-align:right;color:var(--faint)">{new}</span>'
                   f'<span style="width:26px;text-align:center">{sign.strip()}</span><span>{text}</span></div>')
    return f'<div class="mono" style="border:1px solid var(--bv);border-radius:7px;background:#23272e;overflow:hidden;font-size:12px;line-height:20px;white-space:pre;padding-bottom:4px">{"".join(out)}</div>'

def kubeconfig_save_screen():
    save = f'''<div role="dialog" aria-label="Save ~/.kube/config" style="width:740px;flex-shrink:0;background:#2f343e;border:1px solid var(--border);border-radius:10px;box-shadow:0 20px 60px rgba(0,0,0,.5);overflow:hidden">
<div style="display:flex;align-items:center;gap:10px;padding:14px 16px;border-bottom:1px solid var(--bv)">{ic("save",16,C["accent"])}<b style="font-weight:600;flex:1">Save <span class="mono" style="font-weight:500;font-size:13px">~/.kube/config</span></b><span class="chip">{ic("file",11)}external file</span><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div style="padding:16px;display:flex;flex-direction:column;gap:12px">
<div style="display:flex;align-items:center;gap:8px;font-size:12.5px">{ic("ok",14,C["green"])}<span><b style="font-weight:500">1 change</b> <span style="color:var(--muted)">· comments and key order kept · 7 comments</span></span></div>
{kc_diff()}
<div style="display:flex;gap:8px;align-items:center;font-size:11.5px;color:var(--yellow)">{ic("alert",12,C["yellow"])}If a change can't be written in place, the preview lists the comments that would be removed.</div>
<dl class="kv" style="margin:0;padding-top:12px;border-top:1px solid var(--bv);grid-template-columns:70px minmax(0,1fr);row-gap:7px">
<dt>Backup</dt><dd class="mono" style="font-size:11.5px;white-space:normal">~/Library/Application Support/kubyl/kubeconfig-backups/config-20260926-140512.yaml <span style="font-family:'IBM Plex Sans';color:var(--dim)">(0600)</span></dd>
<dt>Write</dt><dd>atomic (temp file + rename) · mode 0600 kept</dd>
<dt>Checked</dt><dd style="display:flex;gap:6px;align-items:center">{ic("check",12,C["green"],2.5)}unchanged on disk since you opened it <span style="color:var(--dim)">(sha256 <span class="mono" style="font-size:11.5px">7c1e…</span>)</span></dd>
</dl>
</div>
<div style="display:flex;justify-content:flex-end;gap:8px;padding:12px 16px;border-top:1px solid var(--bv)"><button class="btn g">Cancel</button><button class="btn">Save as a Kubyl copy…</button><button class="btn p">{ic("save",12,KC_INK)}Save</button></div>
</div>'''
    kv = [("command", "aws"), ("args", "eks get-token --cluster-name staging --region eu-west-1"), ("env", "AWS_PROFILE=staging"),
          ("apiVersion", "client.authentication.k8s.io/v1beta1"), ("interactive", "Never")]
    words = lambda v: " ".join(f'<span style="white-space:nowrap">{w}</span>' for w in v.split(" "))  # wrap between arguments only
    box = "".join(f'<span style="color:var(--dim)">{k}</span><span style="color:var(--text)">{words(v)}</span>' for k, v in kv)
    consent = f'''<div role="dialog" aria-label="Run an exec plugin for staging-eu-west-1?" style="width:460px;flex-shrink:0;margin-top:230px;background:#2f343e;border:1px solid var(--border);border-radius:10px;box-shadow:0 20px 60px rgba(0,0,0,.5);overflow:hidden">
<div style="display:flex;align-items:center;gap:10px;padding:14px 16px;border-bottom:1px solid var(--bv)">{ic("terminal",16,C["yellow"])}<b style="font-weight:600;flex:1">Run an exec plugin for staging-eu-west-1?</b></div>
<div style="padding:16px;display:flex;flex-direction:column;gap:12px">
<div style="font-size:12.5px;color:var(--muted);line-height:19px">This command comes from unsaved edits. Kubyl runs it only after you agree; it can do anything your user can.</div>
<div class="mono" style="display:grid;grid-template-columns:88px minmax(0,1fr);gap:4px 10px;padding:10px 12px;border-radius:7px;background:#23272e;border:1px solid var(--bv);font-size:11.5px;line-height:17px">{box}</div>
<div style="font-size:11.5px;color:var(--dim)">Resolved with your login shell PATH: <span class="mono" style="font-size:11px;color:var(--muted)">/opt/homebrew/bin/aws</span></div>
</div>
<div style="display:flex;justify-content:flex-end;gap:8px;padding:12px 16px;border-top:1px solid var(--bv)"><button class="btn g">Cancel</button><button class="btn p">{ic("play",11,KC_INK)}Run and test</button></div>
</div>'''
    overlay = f'<div style="position:absolute;inset:0;background:rgba(15,17,21,.55);display:flex;align-items:flex-start;justify-content:center;gap:24px;padding-top:70px">{save}{consent}</div>'
    return page("Save kubeconfig and exec consent — Kubyl", kc_shell(kc_tabs(KC_HOME), kc_editor(KC_HOME), overlay))

# ---------- 12–15. Argo CD ----------
# Argo CD's own status colors mapped to theme tokens.
ARGO_SYNC = {"Synced": C["green"], "OutOfSync": C["yellow"], "Unknown": C["dim"]}
ARGO_HEALTH = {"Healthy": C["green"], "Progressing": C["accent"], "Degraded": C["red"], "Suspended": C["purple"],
               "Missing": C["yellow"], "Unknown": C["dim"]}

def apill(status, table=None):
    c = (table or {**ARGO_SYNC, **ARGO_HEALTH}).get(status, C["muted"])
    return f'<span class="pill">{dot(c)}<span style="color:{c}">{status}</span></span>'

def check(on, label="", sub=""):
    box = (f'<span style="width:14px;height:14px;border-radius:3px;flex-shrink:0;display:flex;align-items:center;justify-content:center;'
           f'{"background:var(--accent)" if on else "border:1px solid #5d636f;box-sizing:border-box"}">'
           + (f'<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="#1b1e24" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 6 9 17l-5-5"></path></svg>' if on else "") + '</span>')
    text = f'<div style="min-width:0"><div style="font-size:12.5px">{label}</div>' + (f'<div style="font-size:11.5px;color:var(--dim)">{sub}</div>' if sub else "") + '</div>'
    return f'<label style="display:flex;gap:8px;align-items:flex-start">{box}{text if label else ""}</label>'

def argo_sidebar(active):
    a = lambda n: n == active
    rows = [
        f'<div class="phead"><span style="flex:1;font-weight:500;color:var(--text)">Explorer</span><button class="ib" aria-label="Filter kinds">{ic("search",13)}</button><button class="ib" aria-label="Add kubeconfig">{ic("plus",14)}</button><button class="ib" aria-label="More">{ic("more",14)}</button></div>',
        f'<div class="sec">{ic("cr",11)}Favorites<span style="flex:1"></span><span style="font-weight:400;letter-spacing:0;text-transform:none;color:var(--faint)">4</span></div>',
        f'<div class="sec">{ic("cd",11)}Clusters</div>',
        root("prod-eu-west-1", "on", True, C["red"], prod=True),
        ti("Overview", 1, "gauge"),
        ti("Events", 1, "bell", "23", color=C["yellow"]),
        ti("Workloads", 1, open_=False),
        ti("Network", 1, open_=False),
        ti("Config &amp; Secrets", 1, open_=False),
        ti("Storage", 1, open_=False),
        ti("Access Control", 1, open_=False),
        ti("Cluster", 1, open_=False),
        ti("Administration", 1, open_=True),
        ti("Installed Operators", 2, "blocks", "7"),
        ti("OperatorHub", 2, "store"),
        ti("Cluster Updates", 2, "up"),
        ti("Argo CD", 2, open_=True, extra=f'<span style="font-size:11px;color:var(--dim)">v3.5.3</span>'),
        ti("Applications", 3, "layers", "10", on=a("Applications"),
           extra=f'<span style="margin-right:2px">{dot(C["yellow"])}</span>'),
        ti("ApplicationSets", 3, "fork", "2", on=a("ApplicationSets")),
        ti("Projects", 3, "kanban", "4", on=a("Projects")),
        ti("Custom Resources", 1, open_=True),
        ti("argoproj.io", 2, open_=False),
        ti("cert-manager.io", 2, open_=False),
        ti("monitoring.coreos.com", 2, open_=False),
        ti('<span style="color:var(--dim)">14 more API groups…</span>', 2),
        root("staging-eu-west-1", "on", color=C["yellow"]),
        root("prod-us-east-1", "on", color=C["red"], prod=True),
        root("gke-analytics", None, color=C["cyan"]),
    ]
    return '<aside class="side">' + "\n".join(rows) + '</aside>'

def argo_shell(active, tabbar, content, overlay="", right_extra=""):
    return f'''<div class="app">
{titlebar()}
<div class="body">
{argo_sidebar(active)}
<main class="main">
{tabbar}
{content}
</main>
</div>
{statusbar(right_extra=right_extra)}
{overlay}
</div>'''

# name, app namespace, project, sync, health, auto (prune, heal), source, target, synced, destination (label, link), last sync (age, result), op
ARGO_APPS = [
 ("checkout-api", "argocd", "payments", "Synced", "Healthy", (True, True, True), ("acme/payments-deploy", "apps/checkout-api"), "main", "3f9c2a1", ("in-cluster", "payments", True), ("12m", "Succeeded"), None),
 ("payment-gateway", "argocd", "payments", "OutOfSync", "Degraded", (True, False, False), ("acme/payments-deploy", "apps/payment-gateway"), "main", "8d41b07", ("in-cluster", "payments", True), ("47m", "Succeeded"), None),
 ("ledger-writer", "argocd", "payments", "Synced", "Progressing", (True, True, False), ("acme/payments-deploy", "apps/ledger-writer"), "main", "3f9c2a1", ("in-cluster", "payments", True), ("now", ""), "Syncing 12/40"),
 ("fraud-scorer", "argocd", "risk", "Synced", "Healthy", (True, True, True), ("charts.acme.io", "fraud-scorer 0.19.4"), "0.19.4", "0.19.4", ("in-cluster", "risk", True), ("6h", "Succeeded"), None),
 ("reports-ui", "team-reports", "reports", "Unknown", "Missing", (False, False, False), ("acme/reports", "deploy/ui"), "release-3", "—", ("in-cluster", "reports", True), ("3d", "Failed"), None),
 ("kube-prometheus-stack", "argocd", "platform", "OutOfSync", "Healthy", (True, True, False), ("2 sources", "prometheus-community · acme/platform"), "72.6.2", "72.6.2", ("in-cluster", "monitoring", True), ("2h", "Succeeded"), None),
 ("ingress-nginx", "argocd", "platform", "Synced", "Healthy", (True, True, True), ("kubernetes.github.io", "ingress-nginx 4.12.1"), "4.12.1", "4.12.1", ("prod-us-east-1", "ingress-nginx", True), ("1d", "Succeeded"), None),
 ("cert-manager", "argocd", "platform", "Synced", "Healthy", (True, True, True), ("charts.jetstack.io", "cert-manager v1.15.3"), "v1.15.3", "v1.15.3", ("in-cluster", "cert-manager", True), ("4d", "Succeeded"), None),
 ("settlement-batch", "argocd", "payments", "Synced", "Suspended", (False, False, False), ("acme/payments-deploy", "apps/settlement"), "main", "c21e9d0", ("in-cluster", "payments", True), ("9d", "Succeeded"), None),
 ("analytics-etl", "argocd", "data", "Synced", "Healthy", (True, False, True), ("acme/data-platform", "etl/overlays/prod"), "v2.8.0", "a7b0c44", ("eks-legacy", "etl", False), ("5d", "Succeeded"), None),
]
AC = "grid-template-columns: minmax(0,1.3fr) 70px 104px 104px 42px minmax(0,1.5fr) 64px minmax(0,1fr) 58px"

def argo_rows(selected):
    out = []
    for i, (n, ans, proj, sync, health, auto, (repo, path), target, rev, (dc, dns, link), (age, res), op) in enumerate(ARGO_APPS):
        auto_on, prune, heal = auto
        auto_cell = (f'<span style="display:flex;gap:3px;align-items:center;color:var(--green)" title="auto-sync{", prune" if prune else ""}{", self-heal" if heal else ""}">{ic("refresh",12,C["green"])}'
                     f'<span class="mono" style="font-size:10.5px;color:var(--dim)">{"P" if prune else ""}{"H" if heal else ""}</span></span>') if auto_on else '<span style="color:var(--faint);font-size:12px">manual</span>'
        ns = f'<span style="color:var(--dim)"> · {ans}</span>' if ans != "argocd" else ""
        sync_cell = f'<span class="chip" style="height:19px;color:#a8cdf3;background:#2d3b4d;gap:6px"><svg width="10" height="10" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9" fill="none" stroke="#3f5a78" stroke-width="4"></circle><path d="M12 3a9 9 0 0 1 9 9" fill="none" stroke="#74ade8" stroke-width="4" stroke-linecap="round"></path></svg>{op}</span>' if op else apill(sync, ARGO_SYNC)
        dest = (f'<a href="#" class="mono" style="font-size:12px;text-decoration:none">{dc}<span style="color:var(--dim)"> · </span>{dns}</a>' if link
                else f'<span class="mono" style="font-size:12px;color:var(--muted)">{dc} · {dns}</span>')
        res_icon = {"Succeeded": ic("ok", 12, C["green"]), "Failed": ic("err", 12, C["red"])}.get(res, "")
        out.append(f'''<div class="tr{" on" if i == selected else ""}" style="{AC};height:32px">
<span class="mono" style="font-size:12px">{n}{ns}</span><span style="color:var(--muted)">{proj}</span>{sync_cell}{apill(health, ARGO_HEALTH)}{auto_cell}
<span style="font-size:12px"><span class="mono" style="font-size:11.5px">{path}</span> <span style="color:var(--dim)">@ {target}</span></span>
<span class="mono" style="font-size:11.5px">{rev}</span>{dest}
<span style="display:flex;gap:5px;align-items:center;font-size:12px;color:var(--muted)">{res_icon}{age}</span></div>''')
    return "".join(out)

def argo_filters():
    def fchip(label, n, c, on=False):
        return f'<span class="chip{" on" if on else ""}">{dot(c)}{label}<span class="mono" style="font-size:11px;color:var(--dim)">{n}</span></span>'
    sync = "".join(fchip(s, n, ARGO_SYNC[s], s == "OutOfSync" and False) for s, n in [("Synced", 7), ("OutOfSync", 2), ("Unknown", 1)])
    health = "".join(fchip(s, n, ARGO_HEALTH[s]) for s, n in [("Healthy", 6), ("Progressing", 1), ("Degraded", 1), ("Suspended", 1), ("Missing", 1)])
    lab = lambda t: f'<span style="font-size:11px;font-weight:600;letter-spacing:.06em;text-transform:uppercase;color:var(--dim)">{t}</span>'
    return f'''<div style="height:36px;flex-shrink:0;display:flex;align-items:center;gap:6px;padding:0 12px;border-bottom:1px solid var(--bv);white-space:nowrap;overflow:hidden">
{sync}<span style="width:1px;height:16px;background:var(--border);margin:0 4px"></span>{health}<span style="flex:1"></span></div>'''

def argo_toolbar(mode_chip):
    return f'''<div class="tool">
<div class="crumb" style="white-space:nowrap">{ic("layers",14,C["accent"])}<b>Applications</b><span>·</span><span>10</span><span style="color:var(--yellow)">· 2 out of sync</span><span style="color:var(--red)">· 1 degraded</span></div>
<div style="flex:1"></div>
<button class="btn g" style="height:24px;padding:0 6px">Project{ic("cd",11)}</button><button class="btn g" style="height:24px;padding:0 6px">Destination{ic("cd",11)}</button>
<div class="inp" style="width:150px">{ic("filter",12)}Filter</div>
{mode_chip}
<button class="ib" aria-label="Open Argo CD UI" title="Open Argo CD UI">{ic("globe",14)}</button>
<span class="chip" style="color:var(--green)">{dot(C["green"])}live</span>
</div>'''

K8S_MODE = f'<span class="chip" title="Reads and patches the Application objects with your Kubernetes access">{ic("wheel",11)}Kubernetes mode</span>'
API_MODE = f'<span class="chip on">{ic("link",11)}API · alice</span>'

def argo_apps_screen():
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{argo_toolbar(K8S_MODE)}
{argo_filters()}
<div class="th" style="{AC}"><span>NAME {ic("cd",10)}</span><span>PROJECT</span><span>SYNC</span><span>HEALTH</span><span>AUTO</span><span>SOURCE @ TARGET</span><span>REVISION</span><span>DESTINATION</span><span>LAST</span></div>
<div style="flex:1;overflow:hidden">{argo_rows(1)}</div>
{hints([("↵","Open"),("s","Sync…"),("r","Refresh"),("⇧r","Hard refresh"),("h","History"),("e","Edit YAML"),("⌃d","Delete…"),("/","Filter")])}
</div>'''
    res = lambda kind, name, sync, health: f'<div style="display:flex;align-items:center;gap:8px;padding:4px 0;font-size:12px"><span style="color:var(--dim);width:78px">{kind}</span><span class="mono" style="flex:1;font-size:11.5px;overflow:hidden;text-overflow:ellipsis">{name}</span>{apill(sync, ARGO_SYNC)}</div>'
    toggle = lambda on: f'<span style="width:26px;height:15px;border-radius:8px;background:{C["accent"] if on else "#4a505c"};position:relative;display:inline-block;flex-shrink:0"><span style="position:absolute;top:2px;{"right" if on else "left"}:2px;width:11px;height:11px;border-radius:50%;background:#fff"></span></span>'
    pol = lambda t, on: f'<div style="display:flex;align-items:center;gap:8px;font-size:12px;padding:3px 0"><span style="flex:1;color:var(--muted)">{t}</span>{toggle(on)}</div>'
    dock = f'''<aside class="dock" style="width:300px">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Application details</span><button class="ib" aria-label="Pin">{ic("star",13)}</button><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec">
<div class="mono" style="font-size:12.5px;color:var(--text);margin-bottom:6px">payment-gateway</div>
<div style="display:flex;gap:10px;flex-wrap:wrap;align-items:center">{apill("OutOfSync", ARGO_SYNC)}{apill("Degraded", ARGO_HEALTH)}<span class="chip">payments</span></div>
<div style="display:flex;gap:6px;margin-top:10px"><button class="btn p" style="height:24px">{ic("refresh",12,"#1b1e24")}Sync…</button><button class="btn" style="height:24px">Refresh</button><button class="btn g" style="height:24px">{ic("history",12)}History</button><button class="btn g" style="height:24px">Open</button></div>
</div>
<div class="dsec"><p class="dtitle">Out of sync · 2 of 6 resources</p>
{res("Deployment","payment-gateway","OutOfSync","Degraded")}{res("ConfigMap","payment-gateway-env","OutOfSync","")}
<div style="font-size:11.5px;color:var(--dim);margin-top:6px;line-height:17px">The diff needs API mode. <a href="#">Sign in to Argo CD…</a></div></div>
<div class="dsec"><p class="dtitle">Source</p><dl class="kv" style="margin:0;grid-template-columns:78px minmax(0,1fr)"><dt>Repository</dt><dd><a href="#" style="text-decoration:none">github.com/acme/payments-deploy</a></dd><dt>Path</dt><dd class="mono" style="font-size:11.5px">apps/payment-gateway</dd><dt>Target</dt><dd class="mono" style="font-size:11.5px">main {ic("branch",11,C["dim"])} <a href="#" style="text-decoration:none">8d41b07</a></dd><dt>Destination</dt><dd><a href="#" style="text-decoration:none">in-cluster · payments</a></dd></dl></div>
<div class="dsec"><p class="dtitle">Sync policy</p>{pol("Auto-sync", True)}{pol("Prune", False)}{pol("Self-heal", False)}
<div style="display:flex;gap:4px;flex-wrap:wrap;margin-top:6px"><span class="chip mchip">CreateNamespace=true</span><span class="chip mchip">ServerSideApply=true</span></div></div>
<div class="dsec" style="border-bottom:0"><p class="dtitle">Last operation</p>
<div style="display:flex;gap:8px;font-size:12px;align-items:flex-start">{ic("ok",13,C["green"])}<div style="flex:1;line-height:18px"><div>Sync succeeded · 47m ago · by <span class="mono" style="font-size:11.5px">alice</span></div><div style="color:var(--dim)">revision 8d41b07 · 6 resources synced</div></div></div></div>
</aside>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    tb = tabs([("layers", "Applications", True), ("fork", "ApplicationSets", False), ("box", "Pods", False)])
    return page("Argo CD applications — Kubyl", argo_shell("Applications", tb, content))

def app_header(name, sync, health, mode_chip, extra_btn=""):
    return f'''<div style="display:flex;align-items:center;gap:10px;padding:12px 16px 10px;border-bottom:1px solid var(--bv)">
<span style="width:30px;height:30px;border-radius:7px;background:#bf956a22;border:1px solid #bf956a55;display:flex;align-items:center;justify-content:center">{ic("layers",16,C["orange"])}</span>
<div style="min-width:0"><div style="display:flex;align-items:center;gap:8px"><span class="mono" style="font-size:15px;font-weight:500">{name}</span><span class="chip">payments</span><span style="font-size:12px;color:var(--dim)">argocd namespace</span></div>
<div style="display:flex;gap:12px;align-items:center;margin-top:3px;font-size:12px">{apill(sync, ARGO_SYNC)}{apill(health, ARGO_HEALTH)}<span style="color:var(--green);display:flex;gap:4px;align-items:center">{ic("refresh",12,C["green"])}auto-sync · prune · self-heal</span></div></div>
<div style="flex:1"></div>
{mode_chip}
<button class="btn" style="height:26px">{ic("refresh",12)}Refresh{ic("cd",11)}</button>
<button class="btn p" style="height:26px">{ic("refresh",12,"#1b1e24")}Sync…</button>
{extra_btn}
<button class="btn g" style="height:26px">{ic("globe",13)}Argo CD UI</button>
<button class="ib" aria-label="More">{ic("more",14)}</button>
</div>'''

def app_subtabs(active, counts):
    items = [("Summary", ""), ("Resources", counts.get("Resources", "")), ("Diff", counts.get("Diff", "")), ("History", counts.get("History", "")), ("Events", ""), ("Controller logs", "")]
    return '<div style="display:flex;gap:2px;padding:0 12px;border-bottom:1px solid var(--bv);height:36px;align-items:stretch">' + "".join(
        f'<span style="display:flex;align-items:center;gap:6px;padding:0 10px;{"color:var(--text);box-shadow:inset 0 -2px 0 var(--accent)" if t == active else "color:var(--dim)"}">{t}' + (f'<span class="chip" style="height:17px">{c}</span>' if c else "") + '</span>'
        for t, c in items) + '</div>'

def argo_app_screen():
    TC = "grid-template-columns: minmax(0,1fr) 104px 110px minmax(0,0.55fr) 56px"
    guide = lambda d: "".join(f'<span style="width:16px;flex-shrink:0;align-self:stretch;border-left:1px solid #3e4450;margin-left:6px"></span>' for _ in range(d))
    def node(depth, icon, kind, name, sync, health, info, age, open_=None, on=False, live=False):
        chev = ic("cd", 11, C["dim"]) if open_ is True else ic("cr", 11, C["dim"]) if open_ is False else '<span style="width:11px;flex-shrink:0"></span>'
        tag = '<span class="chip" style="height:16px;font-size:10.5px;padding:0 5px">live</span>' if live else ""
        s = apill(sync, ARGO_SYNC) if sync else '<span style="color:var(--faint)">—</span>'
        h = apill(health, ARGO_HEALTH) if health else '<span style="color:var(--faint)">—</span>'
        return (f'<div class="tr{" on" if on else ""}" style="{TC};height:30px"><span style="display:flex;align-items:center;gap:6px;height:100%;min-width:0">{guide(depth)}{chev}{ic(icon,13,C["dim"])}'
                f'<span style="color:var(--dim);font-size:12px">{kind}</span><span class="mono" style="font-size:12px;overflow:hidden;text-overflow:ellipsis">{name}</span>{tag}</span>{s}{h}'
                f'<span style="font-size:12px;color:var(--muted)">{info}</span><span class="mono" style="font-size:11.5px;color:var(--muted)">{age}</span></div>')
    tree = "".join([
        node(0, "layers", "Application", "checkout-api", "Synced", "Healthy", "rev 3f9c2a1", "41d", True),
        node(1, "layers", "Deployment", "checkout-api", "Synced", "Healthy", "3/3 ready · rev 14", "41d", True),
        node(2, "copy", "ReplicaSet", "checkout-api-7d9f8c6b5", "", "Healthy", "3 pods", "3d", True, live=True),
        node(3, "box", "Pod", "checkout-api-7d9f8c6b5-x2kqp", "", "Healthy", "Running · 10.0.12.188", "3d", on=True, live=True),
        node(3, "box", "Pod", "checkout-api-7d9f8c6b5-m8fzt", "", "Healthy", "Running · 10.0.14.21", "3d", live=True),
        node(3, "box", "Pod", "checkout-api-7d9f8c6b5-qj4wn", "", "Healthy", "Running · 10.0.11.7", "3d", live=True),
        node(2, "copy", "ReplicaSet", "checkout-api-5c7b9d8f4", "", "Healthy", "0 pods · rev 13", "48d", False, live=True),
        node(1, "network", "Service", "checkout-api", "Synced", "Healthy", "ClusterIP 10.96.44.12", "41d"),
        node(1, "file", "ConfigMap", "checkout-api-config", "Synced", "", "4 keys", "41d"),
        node(1, "activity", "HPA", "checkout-api", "Synced", "Healthy", "3–10 · cpu 38%", "41d"),
        node(1, "shield", "PodDisruptionBudget", "checkout-api", "Synced", "", "minAvailable 2", "41d"),
        node(1, "user", "ServiceAccount", "checkout-api", "Synced", "", "", "41d"),
        node(1, "key", "ExternalSecret", "checkout-api-db", "Synced", "Healthy", "SecretSynced", "41d"),
        node(1, "file", "ServiceMonitor", "checkout-api", "Synced", "", "", "41d"),
    ])
    seg = lambda items: '<span style="display:flex;border:1px solid var(--border);border-radius:5px;overflow:hidden">' + "".join(f'<span style="display:flex;align-items:center;gap:5px;height:22px;padding:0 8px;font-size:12px;{"background:#2d3b4d;color:#a8cdf3" if on else "color:var(--dim)"}">{ic(i,12)}{t}</span>' for i, t, on in items) + '</span>'
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div style="height:38px;flex-shrink:0;display:flex;align-items:center;gap:8px;padding:0 12px;border-bottom:1px solid var(--bv)">
{seg([("tree","Tree",True),("list","List",False)])}
<span class="chip on">All 14</span><span class="chip">{dot(C["yellow"])}Out of sync 0</span><span class="chip">{dot(C["red"])}Unhealthy 0</span>
<span style="flex:1"></span><span style="font-size:11.5px;color:var(--dim);white-space:nowrap"><span class="chip" style="height:16px;font-size:10.5px;padding:0 5px">live</span> children from Kubyl's watches</span>
<div class="inp" style="width:170px;height:24px">{ic("filter",12)}Filter</div></div>
<div class="th" style="{TC}"><span>RESOURCE</span><span>SYNC</span><span>HEALTH</span><span>INFO</span><span>AGE</span></div>
<div style="flex:1;overflow:hidden">{tree}</div>
{hints([("↵","Open in Kubyl"),("l","Logs"),("e","Edit YAML"),("d","Diff"),("t","Tree / list"),("s","Sync…"),("r","Refresh")])}
</div>'''
    src = lambda i, repo, path, rev: f'<div style="display:flex;gap:8px;padding:5px 0;align-items:flex-start">{ic("branch",13,C["dim"])}<div style="min-width:0;flex:1;font-size:12px;line-height:18px"><a href="#" style="text-decoration:none">{repo}</a><div class="mono" style="font-size:11.5px;color:var(--muted)">{path} <span style="color:var(--dim)">@</span> {rev}</div></div></div>'
    toggle = lambda on: f'<span style="width:26px;height:15px;border-radius:8px;background:{C["accent"] if on else "#4a505c"};position:relative;display:inline-block;flex-shrink:0"><span style="position:absolute;top:2px;{"right" if on else "left"}:2px;width:11px;height:11px;border-radius:50%;background:#fff"></span></span>'
    pol = lambda t, on: f'<div style="display:flex;align-items:center;gap:8px;font-size:12px;padding:3px 0"><span style="flex:1;color:var(--muted)">{t}</span>{toggle(on)}</div>'
    res = lambda k, n: f'<div style="display:flex;gap:8px;font-size:12px;padding:2px 0">{ic("ok",12,C["green"])}<span style="color:var(--dim);width:74px">{k}</span><span class="mono" style="font-size:11.5px;flex:1;overflow:hidden;text-overflow:ellipsis">{n}</span><span style="color:var(--dim)">synced</span></div>'
    side = f'''<aside style="width:330px;flex-shrink:0;border-left:1px solid var(--border);background:var(--panel);overflow:hidden">
<div class="dsec"><p class="dtitle">Sources</p>{src(1,"github.com/acme/payments-deploy","apps/checkout-api","main")}
<dl class="kv" style="margin:6px 0 0;grid-template-columns:78px minmax(0,1fr)"><dt>Synced</dt><dd class="mono" style="font-size:11.5px"><a href="#" style="text-decoration:none">3f9c2a1</a> <span style="color:var(--dim);font-family:'IBM Plex Sans'">· bump image to 2.14.1</span></dd><dt>Destination</dt><dd><a href="#" style="text-decoration:none">in-cluster · payments</a></dd><dt>Type</dt><dd>Kustomize</dd></dl></div>
<div class="dsec"><p class="dtitle">Sync policy</p>{pol("Auto-sync", True)}{pol("Prune", True)}{pol("Self-heal", True)}
<div style="display:flex;gap:4px;flex-wrap:wrap;margin-top:6px"><span class="chip mchip">CreateNamespace=true</span><span class="chip mchip">PruneLast=true</span><span class="chip mchip">retry 5 · 5s×2</span></div></div>
<div class="dsec"><p class="dtitle">Conditions</p><div style="display:flex;gap:8px;font-size:12px;line-height:17px">{ic("alert",13,C["yellow"])}<div><b style="font-weight:500">SyncWarning</b><div style="color:var(--dim)">ServiceMonitor CRD version v1 is deprecated upstream</div></div></div></div>
<div class="dsec" style="border-bottom:0"><p class="dtitle">Last operation</p>
<div style="display:flex;gap:8px;font-size:12px;align-items:flex-start;margin-bottom:8px">{ic("ok",13,C["green"])}<div style="flex:1;line-height:18px"><div>Auto-sync succeeded · 12m ago</div><div style="color:var(--dim)">revision 3f9c2a1 · took 9s</div></div></div>
{res("Deployment","checkout-api")}{res("ConfigMap","checkout-api-config")}{res("Service","checkout-api")}
<div style="font-size:11.5px;color:var(--dim);margin-top:4px">and 6 more unchanged</div></div>
</aside>'''
    content = f'''<div style="flex:1;display:flex;flex-direction:column;min-height:0">{app_header("checkout-api", "Synced", "Healthy", API_MODE)}{app_subtabs("Resources", {"Resources": "14", "History": "12"})}
<div style="flex:1;display:flex;min-height:0">{center}{side}</div></div>'''
    tb = tabs([("layers", "Applications", False), ("layers", "checkout-api", True), ("box", "Pods", False)])
    return page("Argo CD application — Kubyl", argo_shell("Applications", tb, content, right_extra=f'<span style="color:var(--text)">{ic("link",12,C["accent"])}Argo CD API</span>'))

HISTORY = [
 (14, "8d41b07", "raise pool size to 40", "main", "47m", "alice", True),
 (13, "5b1e9c4", "payment-gateway 5.2.0", "main", "2d", "bob", False),
 (12, "e03d7a2", "add fraud check timeout", "main", "5d", "automated", False),
 (11, "91c4f58", "payment-gateway 5.1.3", "main", "9d", "automated", False),
 (10, "4a2e8b1", "rotate TLS secret name", "main", "12d", "automated", False),
 (9, "d7f60c3", "payment-gateway 5.1.2", "main", "20d", "carol", False),
]

def argo_history_screen():
    HC = "grid-template-columns: 50px 130px minmax(0,1.6fr) 100px 110px 120px"
    rows = "".join(f'''<div class="tr{" on" if i == 1 else ""}" style="{HC};height:34px">
<span class="mono" style="color:{C["accent"] if cur else C["dim"]}">#{n}</span>
<span style="display:flex;gap:8px;align-items:center;min-width:0">{ic("commit",13,C["dim"])}<a href="#" class="mono" style="font-size:12px;text-decoration:none">{sha}</a>{ic("ext",11,C["dim"])}</span>
<span style="font-size:12px"><span style="color:var(--dim)">acme/payments-deploy</span> <span class="mono" style="font-size:11.5px">apps/payment-gateway</span> <span style="color:var(--dim)">@ {br}</span></span>
<span class="mono" style="font-size:11.5px;color:var(--muted)">{when} ago</span><span style="font-size:12px;color:{C["dim"] if by == "automated" else C["text"]}">{by}</span>
<span>{'<span class="chip on" style="height:19px">current</span>' if cur else f'<button class="btn g" style="height:22px;padding:0 8px">{ic("rollback",12)}Roll back…</button>'}</span></div>''' for i, (n, sha, msg, br, when, by, cur) in enumerate(HISTORY))
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div style="height:38px;flex-shrink:0;display:flex;align-items:center;gap:8px;padding:0 12px;border-bottom:1px solid var(--bv);font-size:12px;color:var(--dim)">{ic("history",13)}12 deployments, newest first · revisions link to GitHub<span style="flex:1"></span><span>history limit 12 (spec.revisionHistoryLimit)</span></div>
<div class="th" style="{HC}"><span>ID</span><span>REVISION</span><span>SOURCE</span><span>DEPLOYED</span><span>BY</span><span></span></div>
<div style="flex:1;overflow:hidden">{rows}</div>
{hints([("b","Roll back…"),("↵","Open commit"),("c","Copy revision"),("s","Sync…")])}
</div>'''
    content = f'''<div style="flex:1;display:flex;flex-direction:column;min-height:0">{app_header("payment-gateway", "OutOfSync", "Degraded", K8S_MODE)}{app_subtabs("History", {"Resources": "6", "History": "12"})}
<div style="flex:1;display:flex;min-height:0">{center}</div></div>'''
    modal = f'''<div style="position:absolute;inset:0;background:rgba(15,17,21,.55);display:flex;align-items:flex-start;justify-content:center;padding-top:110px">
<div role="dialog" aria-label="Roll back payment-gateway" style="width:540px;background:#2f343e;border:1px solid var(--border);border-radius:10px;box-shadow:0 20px 60px rgba(0,0,0,.5);overflow:hidden">
<div style="display:flex;align-items:center;gap:10px;padding:14px 16px;border-bottom:1px solid var(--bv)">{ic("rollback",16,C["accent"])}<b style="font-weight:600;flex:1">Roll back payment-gateway</b><span class="prod" style="font-size:9.5px;padding:0 4px">PROD</span></div>
<div style="padding:16px;display:flex;flex-direction:column;gap:14px">
<div style="font-size:12.5px;color:var(--muted);line-height:19px">Deploys history entry <b class="mono" style="font-weight:500;color:var(--text)">#13</b> again: the revision and source it was synced with, 2 days ago by bob.</div>
<div class="card" style="padding:10px 12px;display:flex;flex-direction:column;gap:6px;background:#2a2e36">
<div style="display:flex;gap:10px;align-items:center;font-size:12px"><span style="color:var(--dim);width:70px">Revision</span><span class="mono">8d41b07</span><span style="color:var(--dim)">→</span><a href="#" class="mono" style="text-decoration:none">5b1e9c4</a><span style="flex:1"></span><a href="#" style="font-size:11.5px;text-decoration:none">compare on GitHub {ic("ext",11,C["accent"])}</a></div>
<div style="display:flex;gap:10px;align-items:center;font-size:12px"><span style="color:var(--dim);width:70px">Source</span><span>unchanged</span><span class="mono" style="font-size:11.5px;color:var(--dim)">acme/payments-deploy · apps/payment-gateway</span></div>
<div style="display:flex;gap:10px;align-items:center;font-size:12px"><span style="color:var(--dim);width:70px">Current</span><span>#14 · deployed 47m ago by alice</span></div>
</div>
<div style="display:flex;gap:10px;padding:10px 12px;border-radius:7px;background:#35322a;border:1px solid #5a4f33">{ic("alert",15,C["yellow"])}<div style="flex:1;display:flex;flex-direction:column;gap:8px"><div style="font-size:12.5px;line-height:18px"><b style="font-weight:600;color:var(--yellow)">Auto-sync is on.</b> Argo CD would sync back to <span class="mono" style="font-size:11.5px">main</span> right away, so rollback needs it off.</div>
{check(True, "Turn off auto-sync first", "Removes spec.syncPolicy.automated; turn it on again from the summary.")}</div></div>
<div style="display:flex;gap:18px">{check(False, "Prune", "delete resources that 5b1e9c4 doesn't have")}{check(False, "Dry run")}</div>
<div style="display:flex;flex-direction:column;gap:6px"><div style="font-size:12px;color:var(--muted)">This is a production cluster. Type <span class="mono" style="color:var(--text)">payment-gateway</span> to confirm.</div>
<div class="inp focus" style="height:28px"><span class="mono" style="font-size:12.5px;color:var(--text)">payment-gate</span><span style="display:inline-block;width:1px;height:15px;background:var(--accent);margin-left:-6px"></span></div></div>
<div style="font-size:11.5px;color:var(--dim);line-height:17px">{ic("wheel",12)} Kubernetes mode: writes <span class="mono">operation.sync</span> with the entry's revision and source, like <span class="mono">argocd app rollback --core</span>.</div>
</div>
<div style="display:flex;justify-content:flex-end;gap:8px;padding:12px 16px;border-top:1px solid var(--bv)"><button class="btn g">Cancel</button><button class="btn" style="border-color:#7a4448;color:var(--red);opacity:.55">{ic("rollback",12,C["red"])}Roll back to #13</button></div>
</div></div>'''
    tb = tabs([("layers", "Applications", False), ("layers", "payment-gateway", True)])
    return page("Argo CD history and rollback — Kubyl", argo_shell("Applications", tb, content, modal))

def argo_sync_screen():
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{argo_toolbar(API_MODE)}
{argo_filters()}
<div class="th" style="{AC}"><span>NAME {ic("cd",10)}</span><span>PROJECT</span><span>SYNC</span><span>HEALTH</span><span>AUTO</span><span>SOURCE @ TARGET</span><span>REVISION</span><span>DESTINATION</span><span>LAST</span></div>
<div style="flex:1;overflow:hidden">{argo_rows(5)}</div>
{hints([("↵","Open"),("s","Sync…"),("r","Refresh"),("⇧r","Hard refresh"),("h","History"),("e","Edit YAML"),("⌃d","Delete…"),("/","Filter")])}
</div>'''
    RC = "grid-template-columns: 22px 108px minmax(0,1fr) 92px"
    resources = [(True, "Deployment", "kube-prometheus-stack-operator", "OutOfSync"), (True, "ConfigMap", "kube-prometheus-stack-grafana", "OutOfSync"),
                 (False, "Service", "kube-prometheus-stack-prometheus", "Synced"), (False, "Prometheus", "kube-prometheus-stack-prometheus", "Synced"),
                 (False, "ServiceMonitor", "kube-prometheus-stack-kubelet", "Synced")]
    rrows = "".join(f'<div style="display:grid;{RC};align-items:center;height:26px;padding:0 10px;font-size:12px;border-top:1px solid #363c46">{check(on)}<span style="color:var(--dim)">{k}</span><span class="mono" style="font-size:11.5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{n}</span>{apill(s, ARGO_SYNC)}</div>' for on, k, n, s in resources)
    opts = [("Prune", "delete resources no longer in Git", True), ("Dry run", "", False), ("Apply only", "skip hooks (kubectl apply)", False),
            ("Force", "delete and re-create when apply fails", False), ("Replace", "kubectl replace/create", False), ("Server-side apply", "", True)]
    grid = "".join(check(on, t, s) for t, s, on in opts)
    modal = f'''<div style="position:absolute;inset:0;background:rgba(15,17,21,.55);display:flex;align-items:flex-start;justify-content:center;padding-top:80px">
<div role="dialog" aria-label="Sync kube-prometheus-stack" style="width:580px;background:#2f343e;border:1px solid var(--border);border-radius:10px;box-shadow:0 20px 60px rgba(0,0,0,.5);overflow:hidden">
<div style="display:flex;align-items:center;gap:10px;padding:14px 16px;border-bottom:1px solid var(--bv)">{ic("refresh",16,C["accent"])}<b style="font-weight:600;flex:1">Sync kube-prometheus-stack</b>{apill("OutOfSync", ARGO_SYNC)}</div>
<div style="padding:16px;display:flex;flex-direction:column;gap:14px">
<div style="display:flex;flex-direction:column;gap:6px"><div style="font-size:12px;color:var(--dim)">Revisions</div>
<div style="display:grid;grid-template-columns:minmax(0,1fr) 150px;gap:8px;align-items:center;font-size:12px">
<span style="display:flex;gap:6px;align-items:center;min-width:0">{ic("branch",12,C["dim"])}<span class="mono" style="font-size:11.5px;overflow:hidden;text-overflow:ellipsis">prometheus-community · kube-prometheus-stack</span></span><div class="inp" style="height:26px"><span class="mono" style="font-size:12px;color:var(--text)">72.6.2</span></div>
<span style="display:flex;gap:6px;align-items:center;min-width:0">{ic("branch",12,C["dim"])}<span class="mono" style="font-size:11.5px">acme/platform · values/monitoring</span></span><div class="inp focus" style="height:26px"><span class="mono" style="font-size:12px;color:var(--text)">main</span></div></div></div>
<div style="display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:10px 18px">{grid}</div>
<div style="display:flex;gap:6px;align-items:center;font-size:12px;color:var(--dim)">From the app:<span class="chip mchip">CreateNamespace=true</span><span class="chip mchip">ServerSideApply=true</span></div>
<div class="card" style="overflow:hidden;background:#2a2e36">
<div style="display:flex;align-items:center;gap:8px;height:30px;padding:0 10px;font-size:12px">{check(True)}<span>Selective sync</span><span style="color:var(--dim)">· 2 of 38 selected</span><span style="flex:1"></span><span class="chip on" style="height:18px">out of sync</span><span class="chip" style="height:18px">all</span></div>
{rrows}</div>
</div>
<div style="display:flex;align-items:center;gap:8px;padding:12px 16px;border-top:1px solid var(--bv)"><span style="font-size:11.5px;color:var(--dim);display:flex;gap:6px;align-items:center">{ic("link",12)}API mode · as alice</span><span style="flex:1"></span><button class="btn g">Cancel</button><button class="btn p">{ic("refresh",12,"#1b1e24")}Synchronize</button></div>
</div></div>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}</div>'
    tb = tabs([("layers", "Applications", True), ("fork", "ApplicationSets", False), ("box", "Pods", False)])
    return page("Argo CD sync — Kubyl", argo_shell("Applications", tb, content, modal, f'<span style="color:var(--text)">{ic("link",12,C["accent"])}Argo CD API</span>'))

# ---------- 16. Alerts (phase 14) ----------
SEV = {"critical": C["red"], "warning": C["yellow"], "info": C["accent"], "none": C["dim"]}

def sev_pill(s):
    return f'<span class="pill">{dot(SEV[s])}<span style="color:{SEV[s]}">{s}</span></span>'

def astate(s):
    """State cell: firing, pending (hollow dot), silenced/inhibited (dim), resolved (green)."""
    if s == "firing":
        return f'<span class="pill" style="color:var(--red)">{dot(C["red"])}firing</span>'
    if s == "pending":
        return f'<span class="pill" style="color:var(--yellow)"><span class="dot" style="border:1.5px solid {C["yellow"]};box-sizing:border-box"></span>pending</span>'
    if s == "silenced":
        return f'<span class="pill" style="color:var(--dim)">{ic("belloff",12)}silenced</span>'
    return f'<span class="pill" style="color:var(--dim)">{s}</span>'

def am_statusbar(tip=False):
    item = f'<span title="3 critical · 7 warning · 2 info firing" style="color:var(--text)">{ic("siren",12,C["red"])}<span style="color:var(--red)">12</span></span>'
    return statusbar(left_extra=item)

def alerts_shell(active, tabbar, content, overlay="", sidebar_html=None):
    return f'''<div class="app">
{titlebar()}
<div class="body">
{sidebar_html or sidebar(active, alerts=True)}
<main class="main">
{tabbar}
{content}
</main>
</div>
{am_statusbar()}
{overlay}
</div>'''

def alerts_header(cluster="prod-eu-west-1", prod=True, sources=True, active="Alerts", counts=("12", "5", "312"), compact=False):
    if compact:
        chips = f'<span class="chip" title="monitoring/alertmanager-operated · v0.28.1 · 2/2 peers · Rules: Prometheus">{ic("siren",11,C["green"])}Alertmanager · v0.28.1</span>' if sources else ""
    else:
        chips = (f'<span class="chip" title="v0.28.1 · cluster ready · 2/2 peers">{ic("siren",11,C["green"])}Alertmanager <span class="mono" style="font-size:11px">monitoring/alertmanager-operated</span> · v0.28.1 · 2/2 peers</span>'
             f'<span class="chip">{ic("listchecks",11,C["green"])}Rules: Prometheus</span>') if sources else ""
    ui = (f'<button class="ib" aria-label="Open Alertmanager UI" title="Open Alertmanager UI">{ic("globe",14)}</button>' if compact
          else f'<button class="btn g" style="height:26px">{ic("globe",13)}Open Alertmanager UI</button>') if sources else ""
    tabs_ = [("Alerts", "siren", counts[0]), ("Silences", "belloff", counts[1]), ("Rules", "listchecks", counts[2])]
    sub = "".join(f'<span style="display:flex;align-items:center;gap:6px;padding:0 10px;{"color:var(--text);box-shadow:inset 0 -2px 0 var(--accent)" if t == active else "color:var(--dim)"}">{ic(i,13,C["accent"] if t == active else C["dim"])}{t}' + (f'<span class="chip" style="height:17px">{c}</span>' if c else "") + '</span>' for t, i, c in tabs_)
    return f'''<div style="display:flex;align-items:center;gap:10px;padding:0 12px 0 16px;height:44px;flex-shrink:0;border-bottom:1px solid var(--bv)">
<span style="font-size:15px;font-weight:600;white-space:nowrap">{cluster}</span>{'<span class="prod">PROD</span>' if prod else ''}
{chips}
<div style="flex:1"></div>
{ui}
<button class="ib" aria-label="More">{ic("more",14)}</button>
</div>
<div style="display:flex;gap:2px;padding:0 8px;border-bottom:1px solid var(--bv);height:34px;align-items:stretch;flex-shrink:0">{sub}</div>'''

# severity, alert, state, since, summary, target (icon, text), namespace, receivers
ALERTS = [
 ("g", "KubePodCrashLooping", "critical", "2", "oldest 2h 14m", True),
 ("critical", "KubePodCrashLooping", "firing", "2h 14m", "Pod is crash looping.", ("box", "payment-gateway-5c8b7f9d4-hl2vp"), "payments", "pagerduty-payments", True),
 ("critical", "KubePodCrashLooping", "firing", "19m", "Pod is crash looping.", ("box", "currency-rates-28791455-q9w8e"), "payments", "pagerduty-payments", False),
 ("critical", "KubeNodeNotReady", "firing", "18m", "Node is not ready.", ("server", "ip-10-0-15-3"), "Cluster", "pagerduty-platform", False),
 ("warning", "KubeDeploymentReplicasMismatch", "firing", "47m", "Deployment has not matched the expected number of replicas.", ("layers", "payment-gateway"), "payments", "slack-payments", False),
 ("warning", "KubeStatefulSetReplicasMismatch", "firing", "31m", "StatefulSet has not matched the expected number of replicas.", ("db", "ledger-writer"), "payments", "slack-payments", False),
 ("warning", "KubePersistentVolumeFillingUp", "firing", "26m", "PersistentVolume is filling up.", ("drive", "data-ledger-writer-0"), "payments", "slack-payments", False),
 ("warning", "KubeJobFailed", "firing", "19m", "Job failed to complete.", ("play", "currency-rates-28791455"), "payments", "slack-payments", False),
 ("warning", "NodeFilesystemSpaceFillingUp", "pending", "12m", "Filesystem is predicted to run out of space within the next 24 hours.", ("server", "ip-10-0-13-5"), "Cluster", "slack-platform", False),
 ("warning", "TargetDown", "firing", "11m", "One or more targets are unreachable.", ("network", "webhook-relay"), "payments", "slack-payments", False),
 ("warning", "KubeHpaMaxedOut", "pending", "9m", "HPA is running at max replicas.", ("activity", "checkout-api"), "payments", "slack-payments", False),
 ("warning", "KubePodNotReady", "firing", "2m", "Pod has been in a non-ready state for more than 15 minutes.", ("box", "ledger-writer-2"), "payments", "slack-payments", False),
 ("g", "CPUThrottlingHigh", "info", "2", "oldest 6h", False),
]
AL_COLS = "grid-template-columns: 82px minmax(0,1.25fr) 76px 58px minmax(0,1.5fr) minmax(0,1.25fr)"

def alert_rows(selected=1):
    out = []
    for i, a in enumerate(ALERTS):
        if a[0] == "g":
            _, name, sev, n, oldest, open_ = a
            out.append(f'<div class="tr" style="{AL_COLS};height:28px;background:#2a2e36"><span style="grid-column:1/-1;display:flex;align-items:center;gap:8px;font-size:12px">{ic("cd" if open_ else "cr",12,C["dim"])}<span class="mono" style="font-weight:500">{name}</span><span class="chip" style="height:17px">×{n}</span>{sev_pill(sev)}<span style="color:var(--dim)">· {oldest}</span></span></div>')
            continue
        sev, name, state, since, summary, (icon, target), ns, rcv, _ = a
        nested = name == "KubePodCrashLooping"
        out.append(f'''<div class="tr{" on" if i == selected else ""}" style="{AL_COLS};height:30px">{sev_pill(sev)}
<span class="mono" style="font-size:12px;{"padding-left:14px" if nested else ""}">{name}</span>{astate(state)}
<span class="mono" style="font-size:11.5px;color:var(--muted)" title="since 13:58 local · 11:58 UTC">{since}</span>
<span style="font-size:12px;color:var(--muted)">{summary}</span>
<a href="#" class="mono" style="font-size:11.5px;text-decoration:none;display:flex;align-items:center;gap:5px;min-width:0">{ic(icon,12,C["accent"])}<span style="overflow:hidden;text-overflow:ellipsis">{target}</span></a></div>''')
    out.append(f'<div class="tr" style="{AL_COLS};height:30px;color:var(--dim)"><span style="grid-column:1/-1;display:flex;align-items:center;gap:8px;font-size:12px">{ic("belloff",13)}5 silenced · 1 inhibited <a href="#" style="text-decoration:none">Show</a></span></div>')
    return "".join(out)

def alerts_summary():
    return f'''<div style="display:flex;align-items:center;gap:14px;padding:0 14px;height:34px;flex-shrink:0;border-bottom:1px solid var(--bv);font-size:12.5px;white-space:nowrap">
<span><b style="font-weight:600;color:var(--red)">3 critical</b> <span style="color:var(--faint)">·</span> <b style="font-weight:600;color:var(--yellow)">7 warning</b> <span style="color:var(--faint)">·</span> <b style="font-weight:600;color:var(--accent)">2 info</b> <span style="color:var(--muted)">firing</span> <span style="color:var(--faint)">·</span> <span style="color:var(--yellow)">2 pending</span> <span style="color:var(--faint)">·</span> <span style="color:var(--dim)">5 silenced</span></span>
<span style="flex:1"></span>
<span style="display:flex;align-items:center;gap:5px;color:var(--muted)" title="Watchdog: Prometheus delivers alerts to this Alertmanager">{ic("activity",12,C["green"])}Heartbeat OK · 14s ago</span>
<a href="#" style="display:flex;align-items:center;gap:5px;color:var(--red);text-decoration:none">{ic("err",12,C["red"])}1 rule fails to evaluate</a>
<span style="color:var(--dim)">checked 6s ago</span>
</div>'''

def alerts_filters():
    fchip = lambda label, n, c, on=False: f'<span class="chip{" on" if on else ""}">{dot(c)}{label}<span class="mono" style="font-size:11px;color:var(--dim)">{n}</span></span>'
    sev = "".join(fchip(s.capitalize(), n, SEV[s]) for s, n in [("critical", 3), ("warning", 9), ("info", 2)])
    state = "".join(f'<span class="chip{" on" if on else ""}">{t}<span class="mono" style="font-size:11px;color:var(--dim)">{n}</span></span>' for t, n, on in [("Firing", 12, False), ("Pending", 2, False), ("Silenced", 5, False), ("Inhibited", 1, False)])
    sep = '<span style="width:1px;height:16px;background:var(--border);margin:0 2px"></span>'
    return f'''<div style="height:38px;flex-shrink:0;display:flex;align-items:center;gap:6px;padding:0 12px;border-bottom:1px solid var(--bv);white-space:nowrap;overflow:hidden">
<div class="inp" style="width:170px;height:26px">{ic("filter",12)}<span class="mono" style="font-size:11.5px;color:var(--faint);overflow:hidden;white-space:nowrap">alertname=~"Kube.*"</span></div>
{sev}{sep}{state}<span style="flex:1"></span>
<button class="btn g" style="height:24px;padding:0 6px">All namespaces{ic("cd",11)}</button>
<button class="btn g" style="height:24px;padding:0 6px">Group: alert name{ic("cd",11)}</button>
</div>'''

AL_HINTS = [("↵", "Details"), ("s", "Silence…"), ("a", "Acknowledge"), ("o", "Go to target"), ("l", "Logs"), ("r", "Runbook"), ("y", "Copy matchers"), ("/", "Filter")]

def label_chips(labels, dim=False):
    return "".join(f'<span class="chip mchip" style="{"color:var(--dim)" if dim else ""}">{k}={v}</span>' for k, v in labels)

def alerts_screen():
    head = f'<div class="th" style="{AL_COLS}"><span>SEVERITY {ic("cd",10)}</span><span>ALERT</span><span>STATE</span><span>SINCE</span><span>SUMMARY</span><span>TARGET</span></div>'
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{alerts_summary()}
{alerts_filters()}
{head}
<div style="flex:1;overflow:hidden">{alert_rows()}</div>
{hints(AL_HINTS)}
</div>'''
    tl = [("#3a3f4a", 30), (C["yellow"], 3), (C["red"], 8), ("#3a3f4a", 22), (C["yellow"], 2), (C["red"], 5), ("#3a3f4a", 20), (C["yellow"], 1), (C["red"], 9)]
    timeline = "".join(f'<span style="flex:{w};background:{c};height:10px"></span>' for c, w in tl)
    kv = lambda k, v: f'<dt>{k}</dt><dd>{v}</dd>'
    dock = f'''<aside class="dock" style="width:350px;overflow:hidden">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Alert</span><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec">
<div class="mono" style="font-size:13px;color:var(--text);margin-bottom:6px">KubePodCrashLooping</div>
<div style="display:flex;gap:12px;align-items:center">{sev_pill("critical")}{astate("firing")}<span style="font-size:12px;color:var(--dim)">fired 3 times in 24 h</span></div>
<div style="display:flex;gap:6px;margin-top:10px"><button class="btn p" style="height:24px">{ic("belloff",12,"#1b1e24")}Silence…</button><button class="btn" style="height:24px">Acknowledge</button><span style="flex:1"></span><button class="btn g" style="height:24px" title="Copy labels · Copy as amtool filter">{ic("copy",12)}Copy{ic("cd",11)}</button></div>
</div>
<div class="dsec"><dl class="kv" style="margin:0;grid-template-columns:96px minmax(0,1fr)">{kv("Firing since", '2h 14m <span style="color:var(--dim)">· 13:58 (11:58 UTC)</span>')}{kv("Last received", "12s ago")}</dl><div style="display:flex;gap:8px;align-items:center;margin-top:5px;font-size:12px"><span style="color:var(--dim);width:96px;flex-shrink:0">Receivers</span><span style="display:flex;gap:4px;flex-wrap:wrap"><span class="chip" style="height:18px">pagerduty-payments</span><span class="chip" style="height:18px">slack-payments</span></span></div></div>
<div class="dsec"><p class="dtitle">Summary</p>
<div style="font-size:12.5px;line-height:18px">Pod is crash looping.</div>
<div style="font-size:12px;line-height:18px;color:var(--muted);margin-top:6px">Pod payments/payment-gateway-5c8b7f9d4-hl2vp (gateway) is in waiting state (reason: "CrashLoopBackOff"). <a href="#" style="text-decoration:none">More</a></div></div>
<div class="dsec"><p class="dtitle">Target</p>
<div style="display:flex;align-items:center;gap:8px;font-size:12px">{ic("box",13,C["accent"])}<a href="#" class="mono" style="font-size:11.5px;text-decoration:none;flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis">pod/payment-gateway-5c8b7f9d4-hl2vp</a>{st("CrashLoopBackOff")}</div>
<div style="display:flex;gap:6px;margin-top:8px"><button class="btn g" style="height:22px;padding:0 8px">{ic("list",12)}Logs</button><button class="btn g" style="height:22px;padding:0 8px">Details</button></div></div>
<div class="dsec"><p class="dtitle">Labels</p><div style="display:flex;gap:4px;flex-wrap:wrap">{label_chips([("container","gateway"),("job","kube-state-metrics"),("namespace","payments"),("pod","payment-gateway-5c8b…-hl2vp"),("severity","critical")])}</div></div>
<div class="dsec"><p class="dtitle">Runbook</p><a href="#" class="mono" style="font-size:11px;text-decoration:none;word-break:break-all">https://runbooks.prometheus-operator.dev/runbooks/kubernetes/kubepodcrashlooping</a></div>
<div class="dsec"><p class="dtitle">Rule <span style="text-transform:none;letter-spacing:0;font-weight:400">· kubernetes-apps · <a href="#" style="text-decoration:none">Edit PrometheusRule</a></span></p>
<div class="mono" style="font-size:11px;line-height:16px;padding:6px 8px;border-radius:5px;background:#23272e;color:var(--muted)">max_over_time(kube_pod_container_status_waiting_reason{{reason="CrashLoopBackOff", job="kube-state-metrics"}}[5m]) &gt;= 1</div>
<div style="display:flex;gap:12px;font-size:11.5px;color:var(--dim);margin-top:6px"><span>for 15m</span><span>{ic("ok",11,C["green"])} healthy</span><span>evaluated 21s ago</span></div></div>
<div class="dsec" style="border-bottom:0"><p class="dtitle">Last 24 h</p><div style="display:flex;border-radius:3px;overflow:hidden">{timeline}</div>
<div style="display:flex;justify-content:space-between;font-size:11px;color:var(--dim);margin-top:4px"><span>24h ago</span><span style="color:var(--yellow)">■ pending</span><span style="color:var(--red)">■ firing</span><span>now</span></div></div>
</aside>'''
    content = f'<div style="flex:1;display:flex;flex-direction:column;min-height:0">{alerts_header()}<div style="flex:1;display:flex;min-height:0">{center}{dock}</div></div>'
    tb = tabs([("siren", "Alerts", True), ("box", "Pods", False), ("gauge", "Overview", False)])
    tip = f'''<div role="tooltip" style="position:absolute;left:230px;bottom:34px;width:330px;background:#353b45;border:1px solid var(--border);border-radius:7px;box-shadow:0 10px 30px rgba(0,0,0,.45);padding:9px 11px;font-size:12px;display:flex;flex-direction:column;gap:5px">
<div style="color:var(--dim)">prod-eu-west-1 · 12 firing</div>
<div style="display:flex;gap:7px;align-items:center">{dot(C["red"])}<span class="mono" style="font-size:11.5px">KubePodCrashLooping</span><span style="color:var(--dim);flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">payment-gateway-5c8…</span><span class="mono" style="font-size:11px;color:var(--dim)">2h 14m</span></div>
<div style="display:flex;gap:7px;align-items:center">{dot(C["red"])}<span class="mono" style="font-size:11.5px">KubePodCrashLooping</span><span style="color:var(--dim);flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">currency-rates-2879…</span><span class="mono" style="font-size:11px;color:var(--dim)">19m</span></div>
<div style="display:flex;gap:7px;align-items:center">{dot(C["red"])}<span class="mono" style="font-size:11.5px">KubeNodeNotReady</span><span style="color:var(--dim);flex:1">ip-10-0-15-3</span><span class="mono" style="font-size:11px;color:var(--dim)">18m</span></div>
<div style="color:var(--dim);font-size:11.5px">and 9 more · click to open Alerts</div></div>'''
    return page("Alerts — Kubyl", alerts_shell("Alerts", tb, content, tip))

def alerts_states_screen():
    ok_side = [
        f'<div class="phead"><span style="flex:1;font-weight:500;color:var(--text)">Explorer</span><button class="ib" aria-label="Filter kinds">{ic("search",13)}</button><button class="ib" aria-label="Add kubeconfig">{ic("plus",14)}</button><button class="ib" aria-label="More">{ic("more",14)}</button></div>',
        f'<div class="sec">{ic("cr",11)}Favorites<span style="flex:1"></span><span style="font-weight:400;letter-spacing:0;text-transform:none;color:var(--faint)">4</span></div>',
        f'<div class="sec">{ic("cd",11)}Clusters</div>',
        root("prod-eu-west-1", "on", False, C["red"], prod=True, marker=alert_marker(3)),
        root("staging-eu-west-1", "on", True, C["yellow"]),
        ti("Overview", 1, "gauge"),
        ti("Alerts", 1, "siren", on=True, extra=alert_badge("ok", None)),
        ti("Events", 1, "bell", "4", color=C["yellow"]),
        ti("Workloads", 1, open_=False), ti("Network", 1, open_=False), ti("Config &amp; Secrets", 1, open_=False),
        ti("Storage", 1, open_=False), ti("Access Control", 1, open_=False), ti("Cluster", 1, open_=False),
        ti("Administration", 1, open_=False), ti("Custom Resources", 1, open_=False),
        root("gke-analytics", None, color=C["cyan"]),
        root("platform-onprem", "key", color=C["purple"]),
        root("homelab-k3s", "on", True, C["green"]),
        ti("Overview", 1, "gauge"),
        ti("Events", 1, "bell", "0"),
        ti("Workloads", 1, open_=False), ti("Network", 1, open_=False),
    ]
    side = '<aside class="side">' + "\n".join(ok_side) + '</aside>'
    all_clear = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0;border-right:1px solid var(--border)">
{tabs([("siren", "Alerts · staging", True), ("box", "Pods", False)], tools=False)}
{alerts_header("staging-eu-west-1", False, True, counts=("0", "1", "298"), compact=True)}
<div style="flex:1;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:14px;padding:30px">
<span style="width:56px;height:56px;border-radius:50%;background:#a1c18122;border:1px solid #a1c18155;display:flex;align-items:center;justify-content:center">{ic("check",28,C["green"],2.5)}</span>
<div style="font-size:17px;font-weight:600">No alerts firing</div>
<div style="display:flex;flex-direction:column;gap:7px;font-size:12.5px;color:var(--muted);align-items:center">
<span style="display:flex;gap:6px;align-items:center">{ic("activity",13,C["green"])}Heartbeat OK · Watchdog received 14s ago</span>
<span>298 alerting rules evaluate · all healthy</span>
<span><a href="#" style="text-decoration:none">1 pending</a> <span style="color:var(--faint)">·</span> <a href="#" style="text-decoration:none">1 silenced</a></span>
<span style="color:var(--dim)">Last check 9s ago · Alertmanager monitoring/alertmanager-operated</span></div>
</div>
{hints([("s","New silence…"),("⇧r","Look again"),("/","Filter")])}
</div>'''
    tried = [("ok", "prometheus-operator", "no <span class=\"mono\" style=\"font-size:11.5px\">monitoring.coreos.com</span> CRDs, no Alertmanager objects"),
             ("ok", "Prometheus's Alertmanagers", "no Prometheus found (phase 07 looked in 10 namespaces)"),
             ("err", "Services", '<span class="mono" style="font-size:11.5px">monitoring/alertmanager</span> · 403: needs <span class="mono" style="font-size:11.5px">get services/proxy</span> in <span class="mono" style="font-size:11.5px">monitoring</span>')]
    trows = "".join(f'<div style="display:flex;gap:10px;padding:7px 0;border-bottom:1px solid var(--bv);font-size:12.5px;align-items:flex-start">{ic("err" if s == "err" else "minus",13,C["red"] if s == "err" else C["dim"])}<div style="flex:1;min-width:0"><div>{t}</div><div style="color:var(--dim);font-size:12px;margin-top:2px">{d}</div></div></div>' for s, t, d in tried)
    none = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{tabs([("siren", "Alerts · homelab-k3s", True)], tools=False)}
{alerts_header("homelab-k3s", False, False, counts=("", "", ""), compact=True)}
<div style="flex:1;display:flex;flex-direction:column;gap:14px;padding:30px 34px">
<div style="display:flex;gap:12px;align-items:center">{ic("belloff",22,C["dim"])}<div><div style="font-size:16px;font-weight:600">No Alertmanager found</div><div style="font-size:12.5px;color:var(--muted);margin-top:2px">Kubyl looked for one 2 min ago and looks again every 5 min.</div></div></div>
<div class="card" style="padding:6px 14px 4px"><p class="dtitle" style="margin:6px 0 2px">What was tried</p>{trows}
<div style="font-size:12px;color:var(--dim);padding:8px 0">Best candidate: <span class="mono" style="font-size:11.5px;color:var(--text)">monitoring/alertmanager</span> · ask for <span class="mono" style="font-size:11.5px">get services/proxy</span> in <span class="mono" style="font-size:11.5px">monitoring</span>.</div></div>
<div style="font-size:12.5px;color:var(--muted);line-height:19px">Alertmanager somewhere else? Name it in settings.json:</div>
<div class="mono" style="font-size:11.5px;line-height:17px;padding:8px 10px;border-radius:6px;background:#23272e;color:var(--muted)">"alerts": {{ "clusters": {{ "homelab-k3s": {{<br>&nbsp;&nbsp;"alertmanagers": [{{ "url": "https://alertmanager.example.com" }}] }} }} }}</div>
<div style="display:flex;gap:8px"><button class="btn p">{ic("gear",13,"#1b1e24")}Set Alertmanager…</button><button class="btn">{ic("refresh",13)}Look again</button></div>
</div>
</div>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{all_clear}{none}</div>'
    inner = f'''<div class="app">
{titlebar("staging-eu-west-1", "payments", False, "EKS · v1.30.4")}
<div class="body">{side}<main class="main" style="flex-direction:row">{content}</main></div>
{statusbar(left_extra=f'<span title="No alerts firing">{ic("siren",12,C["green"])}{ic("check",12,C["green"],2.5)}</span>')}
</div>'''
    return page("Alerts: all clear and no Alertmanager — Kubyl", inner)

# state, matchers, comment, created by, starts, ends, matches now
SILENCES = [
 ("active", [("alertname", "=", "KubeHpaMaxedOut"), ("namespace", "=", "payments")], "Black Friday load test until 18:00", "alice@example.com", "10:02", "ends in 3h 12m", "1"),
 ("active", [("alertname", "=~", "Kube(Pod|Container).*"), ("namespace", "=", "sandbox")], "Sandbox namespace is noisy, cleaning up", "bob@example.com", "yesterday", "ends in 5d", "4"),
 ("active", [("alertname", "=", "CPUThrottlingHigh"), ("container", "=", "istio-proxy")], "Known, tracked in PLAT-2291", "carol@example.com", "3 days ago", "ends in 4d 2h", "0"),
 ("pending", [("alertname", "=", "NodeFilesystemSpaceFillingUp"), ("instance", "=", "ip-10-0-13-5")], "Disk resize in the maintenance window", "alice@example.com", "in 2h", "ends in 6h", "1"),
 ("active", [("alertname", "=", "TargetDown"), ("service", "=", "webhook-relay")], "Acknowledged in Kubyl by dave@example.com", "dave@example.com", "12m ago", "ends in 48m", "1"),
]
SI_COLS = "grid-template-columns: 76px minmax(0,1.5fr) minmax(0,1.2fr) 130px 84px 104px 66px 104px"

def matcher_chips(ms):
    return "".join(f'<span class="chip mchip">{k}<span style="color:var(--accent)">{op}</span>{v}</span>' for k, op, v in ms)

def silences_screen():
    rows = []
    for i, (state, ms, comment, by, start, end, n) in enumerate(SILENCES):
        col = {"active": C["green"], "pending": C["yellow"]}.get(state, C["dim"])
        rows.append(f'''<div class="tr{" on" if i == 0 else ""}" style="{SI_COLS};height:34px"><span class="pill" style="color:{col}">{dot(col)}{state}</span>
<span style="display:flex;gap:4px;overflow:hidden">{matcher_chips(ms)}</span><span style="font-size:12px;color:var(--muted)">{comment}</span>
<span style="font-size:12px;color:var(--muted)">{by}</span><span class="mono" style="font-size:11.5px;color:var(--muted)">{start}</span><span class="mono" style="font-size:11.5px">{end}</span><span class="mono" style="font-size:11.5px;text-align:right;padding-right:14px">{n}</span>
<span style="display:flex;gap:2px"><button class="btn g" style="height:22px;padding:0 6px">+1h</button><button class="btn g" style="height:22px;padding:0 6px">+4h</button><button class="ib" aria-label="More" style="height:22px">{ic("more",13)}</button></span></div>''')
    expired = f'<div class="tr" style="{SI_COLS};height:28px;background:#2a2e36"><span style="grid-column:1/-1;display:flex;gap:8px;align-items:center;font-size:12px;color:var(--dim)">{ic("cr",12)}Expired in the last 24 h<span class="chip" style="height:17px">3</span></span></div>'
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div style="height:38px;flex-shrink:0;display:flex;align-items:center;gap:8px;padding:0 12px;border-bottom:1px solid var(--bv)">
<div class="inp" style="width:260px;height:26px">{ic("filter",12)}<span style="font-size:12px">Filter matchers, comments, creators</span></div>
<span class="chip on">Active 4</span><span class="chip">Pending 1</span><span class="chip">Expired 3</span><span style="flex:1"></span>
<button class="btn p" style="height:26px">{ic("plus",13,"#1b1e24")}New silence…</button></div>
<div class="th" style="{SI_COLS}"><span>STATE</span><span>MATCHERS</span><span>COMMENT</span><span>CREATED BY</span><span>STARTS</span><span>ENDS</span><span style="text-align:right;padding-right:14px">MATCHES</span><span></span></div>
<div style="flex:1;overflow:hidden">{"".join(rows)}{expired}</div>
{hints([("↵","Edit…"),("e","Extend +1h"),("⌃d","Expire…"),("n","New silence…"),("c","Copy as amtool"),("/","Filter")])}
</div>'''
    def mrow(on, name, op, value, locked=False, note=""):
        box = check(on)
        if locked:
            box = f'<span style="width:14px;display:flex;justify-content:center">{ic("lock",12,C["dim"])}</span>'
        return f'''<div style="display:grid;grid-template-columns:18px 150px 58px minmax(0,1fr) 20px;gap:8px;align-items:center;height:30px">{box}
<div class="inp" style="height:26px;{"opacity:.55" if not on else ""}"><span class="mono" style="font-size:12px;color:var(--text)">{name}</span></div>
<div class="inp" style="height:26px;justify-content:space-between;{"opacity:.55" if not on else ""}"><span class="mono" style="font-size:12px;color:var(--accent)">{op}</span>{ic("cd",11)}</div>
<div class="inp" style="height:26px;{"opacity:.55" if not on else ""}"><span class="mono" style="font-size:12px;color:var(--text);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{value}</span>{note}</div>
<span style="color:var(--faint);display:flex">{"" if locked else ic("x",12)}</span></div>'''
    dur = "".join(f'<span class="chip{" on" if d == "2h" else ""}" style="height:24px;padding:0 10px">{d}</span>' for d in ["1h", "2h", "4h", "1d", "1w", "Custom…"])
    modal = f'''<div style="position:absolute;inset:0;background:rgba(15,17,21,.55);display:flex;align-items:flex-start;justify-content:center;padding-top:62px">
<div role="dialog" aria-label="New silence" style="width:600px;background:#2f343e;border:1px solid var(--border);border-radius:10px;box-shadow:0 20px 60px rgba(0,0,0,.5);overflow:hidden">
<div style="display:flex;align-items:center;gap:10px;padding:14px 16px;border-bottom:1px solid var(--bv)">{ic("belloff",16,C["accent"])}<b style="font-weight:600;flex:1">New silence · prod-eu-west-1</b><span class="prod" style="font-size:9.5px;padding:0 4px">PROD</span></div>
<div style="padding:14px 16px;display:flex;flex-direction:column;gap:12px">
<div style="font-size:12px;color:var(--muted)">From <span class="mono" style="color:var(--text)">KubePodCrashLooping</span> on <span class="mono">pod/payment-gateway-5c8b7f9d4-hl2vp</span>. Ticked labels become matchers.</div>
<div>{mrow(True, "alertname", "=", "KubePodCrashLooping")}{mrow(True, "namespace", "=", "payments")}{mrow(True, "pod", "=~", "payment-gateway-.*")}{mrow(False, "container", "=", "gateway")}{mrow(False, "severity", "=", "critical")}{mrow(False, "job", "=", "kube-state-metrics")}
<a href="#" style="font-size:12px;text-decoration:none;display:inline-flex;gap:5px;align-items:center;margin-top:4px">{ic("plus",12,C["accent"])}Add matcher</a></div>
<div style="display:flex;flex-direction:column;gap:6px"><div style="font-size:12px;color:var(--dim)">Duration</div><div style="display:flex;gap:6px;align-items:center">{dur}<span style="flex:1"></span><span style="font-size:12px;color:var(--muted)">starts now · ends 16:12 (14:12 UTC)</span></div></div>
<div style="display:flex;flex-direction:column;gap:6px"><div style="font-size:12px;color:var(--dim)">Comment <span style="color:var(--red)">required</span></div><div class="inp focus" style="height:48px;align-items:flex-start;padding-top:6px"><span style="font-size:12.5px;color:var(--text)">Rolling back gateway 5.2.0, see INC-4411</span></div></div>
<div style="display:grid;grid-template-columns:80px minmax(0,1fr);gap:8px;align-items:center"><span style="font-size:12px;color:var(--dim)">Created by</span><div class="inp" style="height:26px"><span style="font-size:12.5px;color:var(--text)">alice@example.com</span><span style="flex:1"></span><span style="font-size:11.5px">from your sign-in</span></div></div>
<div style="display:flex;gap:10px;align-items:center;padding:9px 11px;border-radius:7px;background:#2a2e36;border:1px solid var(--bv);font-size:12.5px">{ic("eye",14,C["accent"])}<span>Matches <b style="font-weight:600">3 alerts</b> now: <span style="color:var(--red)">1 critical</span>, <span style="color:var(--yellow)">2 warning</span></span><span style="flex:1"></span><a href="#" style="font-size:12px;text-decoration:none">Show</a></div>
</div>
<div style="display:flex;align-items:center;gap:8px;padding:12px 16px;border-top:1px solid var(--bv)"><span style="font-size:11.5px;color:var(--dim)">Alertmanager monitoring/alertmanager-operated</span><span style="flex:1"></span><button class="btn g">Cancel</button><button class="btn p">Review…</button></div>
</div></div>'''
    content = f'<div style="flex:1;display:flex;flex-direction:column;min-height:0">{alerts_header(active="Silences")}<div style="flex:1;display:flex;min-height:0">{center}</div></div>'
    tb = tabs([("siren", "Alerts", True), ("box", "Pods", False), ("gauge", "Overview", False)])
    return page("Silences and the silence editor — Kubyl", alerts_shell("Alerts", tb, content, modal))

def silence_confirm_screen():
    head = f'<div class="th" style="{AL_COLS}"><span>SEVERITY {ic("cd",10)}</span><span>ALERT</span><span>STATE</span><span>SINCE</span><span>SUMMARY</span><span>TARGET</span></div>'
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{alerts_summary()}
{alerts_filters()}
{head}
<div style="flex:1;overflow:hidden">{alert_rows()}</div>
{hints(AL_HINTS)}
</div>'''
    row = lambda k, v: f'<div style="display:flex;gap:10px;align-items:flex-start;font-size:12.5px"><span style="color:var(--dim);width:84px;flex-shrink:0">{k}</span><div style="flex:1;min-width:0">{v}</div></div>'
    matched = "".join(f'<div style="display:flex;gap:8px;align-items:center;font-size:12px;padding:2px 0">{dot(SEV[s])}<span class="mono" style="font-size:11.5px">{n}</span><span style="color:var(--dim);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{t}</span></div>' for s, n, t in [("critical", "KubePodCrashLooping", "pod/payment-gateway-5c8b7f9d4-hl2vp"), ("warning", "KubeDeploymentReplicasMismatch", "deployment/payment-gateway"), ("warning", "KubePodNotReady", "pod/payment-gateway-5c8b7f9d4-7tgxs")])
    modal = f'''<div style="position:absolute;inset:0;background:rgba(15,17,21,.55);display:flex;align-items:flex-start;justify-content:center;padding-top:96px">
<div role="dialog" aria-label="Create silence on prod-eu-west-1" style="width:540px;background:#2f343e;border:1px solid var(--border);border-radius:10px;box-shadow:0 20px 60px rgba(0,0,0,.5);overflow:hidden">
<div style="display:flex;align-items:center;gap:10px;padding:14px 16px;border-bottom:1px solid var(--bv)">{ic("belloff",16,C["accent"])}<b style="font-weight:600;flex:1">Create silence on prod-eu-west-1</b><span class="prod" style="font-size:9.5px;padding:0 4px">PROD</span></div>
<div style="padding:16px;display:flex;flex-direction:column;gap:12px">
{row("Matchers", f'<div style="display:flex;gap:4px;flex-wrap:wrap">{matcher_chips([("alertname","=","KubePodCrashLooping"),("namespace","=","payments"),("pod","=~","payment-gateway-.*")])}</div>')}
{row("Duration", '2h · until 16:12 today (14:12 UTC)')}
{row("Comment", 'Rolling back gateway 5.2.0, see INC-4411')}
{row("Created by", 'alice@example.com')}
{row("Silences", f'<div><b style="font-weight:600">3 alerts</b> firing now{matched}</div>')}
<div style="display:flex;gap:10px;padding:10px 12px;border-radius:7px;background:#3a2a2d;border:1px solid #6a3a3f">{ic("alert",15,C["red"])}<div style="font-size:12.5px;line-height:18px"><b style="font-weight:600;color:var(--red)">This silences a critical alert</b> on a production cluster. Nobody is paged for it until the silence ends or is expired.</div></div>
<div style="display:flex;flex-direction:column;gap:6px"><div style="font-size:12px;color:var(--muted)">Type <span class="mono" style="color:var(--text)">prod-eu-west-1</span> to confirm.</div>
<div class="inp focus" style="height:28px"><span class="mono" style="font-size:12.5px;color:var(--text)">prod-eu-w</span><span style="display:inline-block;width:1px;height:15px;background:var(--accent);margin-left:-6px"></span></div></div>
</div>
<div style="display:flex;justify-content:flex-end;gap:8px;padding:12px 16px;border-top:1px solid var(--bv)"><button class="btn g">Back</button><button class="btn p" style="opacity:.55">{ic("belloff",12,"#1b1e24")}Create silence</button></div>
</div></div>'''
    content = f'<div style="flex:1;display:flex;flex-direction:column;min-height:0">{alerts_header()}<div style="flex:1;display:flex;min-height:0">{center}</div></div>'
    tb = tabs([("siren", "Alerts", True), ("box", "Pods", False), ("gauge", "Overview", False)])
    return page("Silence confirmation on production — Kubyl", alerts_shell("Alerts", tb, content, modal))

# state, name, health, for, last eval, took, expression
RULES = [
 ("g", "kubernetes-apps", "monitoring/kube-prometheus-stack-kubernetes-apps", "18 rules · 3 firing"),
 ("firing", "KubePodCrashLooping", "ok", "15m", "21s", "4ms", 'max_over_time(kube_pod_container_status_waiting_reason{reason="CrashLoopBackOff", job="kube-state-metrics"}[5m]) >= 1'),
 ("inactive", "KubePodNotReady", "ok", "15m", "21s", "6ms", 'sum by (namespace, pod, cluster) (max by (namespace, pod, cluster) (kube_pod_status_phase{job="kube-state-metrics", phase=~"Pending|Unknown|Failed"}) …'),
 ("firing", "KubeDeploymentReplicasMismatch", "ok", "15m", "21s", "3ms", '(kube_deployment_spec_replicas{job="kube-state-metrics"} > kube_deployment_status_replicas_available{job="kube-state-metrics"}) and …'),
 ("pending", "KubeHpaMaxedOut", "ok", "15m", "21s", "2ms", 'kube_horizontalpodautoscaler_status_current_replicas{job="kube-state-metrics"} == kube_horizontalpodautoscaler_spec_max_replicas …'),
 ("g", "payments.rules", "payments/payments-slo", "4 rules · 1 failing"),
 ("inactive", "CheckoutErrorBudgetBurn", "err", "5m", "14s", "1ms", 'sum(rate(http_requests_total{job="checkout-api",code=~"5.."}[5m])) / sum(rate(http_requests_total{job="checkout-api"}[5m])) > 0.02'),
 ("inactive", "LedgerLagHigh", "ok", "10m", "14s", "2ms", 'ledger_writer_replication_lag_seconds > 30'),
 ("g", "node-exporter", "monitoring/kube-prometheus-stack-node-exporter", "22 rules · 1 pending"),
 ("pending", "NodeFilesystemSpaceFillingUp", "ok", "1h", "9s", "11ms", '(node_filesystem_avail_bytes{job="node-exporter",fstype!=""} / node_filesystem_size_bytes{…} * 100 < 15 and predict_linear(…[6h], 24*60*60) < 0 …'),
 ("inactive", "NodeHighNumberConntrackEntriesUsed", "ok", "", "9s", "1ms", '(node_nf_conntrack_entries{job="node-exporter"} / node_nf_conntrack_entries_limit) > 0.75'),
]
RU_COLS = "grid-template-columns: 76px minmax(0,1.1fr) 70px 46px 96px minmax(0,2fr)"

def rules_screen():
    rows = []
    for i, r in enumerate(RULES):
        if r[0] == "g":
            _, group, obj, count = r
            rows.append(f'<div class="tr" style="{RU_COLS};height:28px;background:#2a2e36"><span style="grid-column:1/-1;display:flex;align-items:center;gap:8px;font-size:12px">{ic("cd",12,C["dim"])}<span style="font-weight:500">{group}</span><span style="color:var(--dim)">·</span><a href="#" class="mono" style="font-size:11.5px;text-decoration:none">{obj}</a><span style="color:var(--dim)">· {count}</span></span></div>')
            continue
        state, name, health, for_, last, took, expr = r
        scol = {"firing": C["red"], "pending": C["yellow"]}.get(state, C["green"])
        slabel = {"inactive": "OK"}.get(state, state)
        hcell = f'<span class="pill" style="color:var(--green)">{ic("ok",12,C["green"])}ok</span>' if health == "ok" else f'<span class="pill" style="color:var(--red)">{ic("err",12,C["red"])}error</span>'
        rows.append(f'''<div class="tr{" on" if name == "CheckoutErrorBudgetBurn" else ""}" style="{RU_COLS};height:30px"><span class="pill" style="color:{scol}">{dot(scol)}{slabel}</span>
<span class="mono" style="font-size:12px">{name}</span>{hcell}<span class="mono" style="font-size:11.5px;color:var(--muted)">{for_ or "—"}</span>
<span class="mono" style="font-size:11.5px;color:var(--muted)">{last} · {took}</span><span class="mono" style="font-size:11px;color:var(--dim)">{expr.replace("<", "&lt;").replace(">", "&gt;")}</span></div>''')
        if health == "err":
            rows.append(f'<div class="tr" style="{RU_COLS};height:26px;border-bottom:1px solid #2e333b"><span></span><span style="grid-column:2/-1;display:flex;gap:6px;align-items:center;font-size:12px;color:var(--red)">{ic("alert",12,C["red"])}<span class="mono" style="font-size:11.5px">vector contains metrics with the same labelset after applying alert labels</span></span></div>')
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div style="height:38px;flex-shrink:0;display:flex;align-items:center;gap:8px;padding:0 12px;border-bottom:1px solid var(--bv);white-space:nowrap">
<span style="font-size:12.5px"><b style="font-weight:600">312 rules</b> <span style="color:var(--faint)">·</span> <span style="color:var(--red)">4 firing</span> <span style="color:var(--faint)">·</span> <span style="color:var(--yellow)">2 pending</span> <span style="color:var(--faint)">·</span> <span style="color:var(--red)">1 failing</span></span>
<span style="flex:1"></span><span class="chip">Only firing, pending or failing</span>
<div class="inp" style="width:200px;height:26px">{ic("filter",12)}<span style="font-size:12px">Filter rules</span></div></div>
<div class="th" style="{RU_COLS}"><span>STATE</span><span>ALERT RULE</span><span>HEALTH</span><span>FOR</span><span>EVALUATED</span><span>EXPRESSION</span></div>
<div style="flex:1;overflow:hidden">{"".join(rows)}</div>
{hints([("↵","Details"),("e","Edit PrometheusRule"),("a","Show its alerts"),("y","Copy expression"),("/","Filter")])}
</div>'''
    dock = f'''<aside class="dock" style="width:330px">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Alerting rule</span><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec"><div class="mono" style="font-size:13px;margin-bottom:6px">CheckoutErrorBudgetBurn</div>
<div style="display:flex;gap:12px;align-items:center;font-size:12px"><span class="pill" style="color:var(--red)">{ic("err",12,C["red"])}fails to evaluate</span><span style="color:var(--dim)">group payments.rules</span></div></div>
<div class="dsec"><p class="dtitle">Last error</p><div class="mono" style="font-size:11.5px;line-height:17px;color:var(--red)">vector contains metrics with the same labelset after applying alert labels</div>
<div style="font-size:11.5px;color:var(--dim);margin-top:6px">last evaluation 14s ago · took 1ms</div></div>
<div class="dsec"><p class="dtitle">Expression</p><div class="mono" style="font-size:11px;line-height:16px;padding:6px 8px;border-radius:5px;background:#23272e;color:var(--muted)">sum(rate(http_requests_total{{job="checkout-api",code=~"5.."}}[5m]))<br>/ sum(rate(http_requests_total{{job="checkout-api"}}[5m]))<br>&gt; 0.02</div>
<div style="display:flex;gap:6px;margin-top:8px"><button class="btn g" style="height:22px;padding:0 8px">{ic("copy",12)}Copy</button><button class="btn g" style="height:22px;padding:0 8px">{ic("pencil",12)}Edit PrometheusRule</button></div></div>
<div class="dsec"><dl class="kv" style="margin:0;grid-template-columns:104px minmax(0,1fr)"><dt>for</dt><dd class="mono" style="font-size:11.5px">5m</dd><dt>keep_firing_for</dt><dd class="mono" style="font-size:11.5px">—</dd><dt>Defined in</dt><dd><a href="#" class="mono" style="font-size:11.5px;text-decoration:none">payments/payments-slo</a></dd></dl></div>
<div class="dsec"><p class="dtitle">Labels</p><div style="display:flex;gap:4px;flex-wrap:wrap">{label_chips([("severity","critical"),("team","payments")])}</div></div>
<div class="dsec" style="border-bottom:0"><p class="dtitle">Annotations</p><dl class="kv" style="margin:0;grid-template-columns:86px minmax(0,1fr)"><dt>summary</dt><dd style="white-space:normal">Checkout burns its error budget.</dd><dt>runbook_url</dt><dd><a href="#" style="text-decoration:none">https://runbooks.example.com/checkout-slo</a></dd></dl></div>
</aside>'''
    content = f'<div style="flex:1;display:flex;flex-direction:column;min-height:0">{alerts_header(active="Rules")}<div style="flex:1;display:flex;min-height:0">{center}{dock}</div></div>'
    tb = tabs([("siren", "Alerts", True), ("box", "Pods", False), ("gauge", "Overview", False)])
    return page("Alerting rules — Kubyl", alerts_shell("Alerts", tb, content))

# ---------- 17. Cluster status and ConfigMap data (phase 15) ----------
OCP_EU = "ocp.eu1.example.com"
OCP_US = "ocp.us1.example.com"
JANE = "jane.doe@example.com"

def polish_sidebar(tooltip=True):
    fav = lambda ns, cl, col, faint=False: (f'<div class="ti" style="padding-left:12px">{ic("star",13,C["yellow"],1.6,C["yellow"])}'
                                            f'<span class="n"><span style="color:{C["dim"] if faint else C["text"]}">{ns}</span> <span style="color:{C["faint"] if faint else C["dim"]}">· {cl}</span></span>{dot(col)}</div>')
    rows = [
        f'<div class="phead"><span style="flex:1;font-weight:500;color:var(--text)">Explorer</span><button class="ib" aria-label="Filter kinds">{ic("search",13)}</button><button class="ib" aria-label="Add kubeconfig">{ic("plus",14)}</button><button class="ib" aria-label="More">{ic("more",14)}</button></div>',
        f'<div class="sec">{ic("cd",11)}Favorites<span style="flex:1"></span><span style="font-weight:400;letter-spacing:0;text-transform:none;color:var(--faint)">4</span></div>',
        fav("payments", "prod-eu-west-1", C["red"]),
        fav("shop", f"{OCP_EU} · {JANE}", C["cyan"]),
        fav("checkout", "gke-analytics", C["purple"], True),
        fav("ops", f"{OCP_EU} · kube:admin", C["red"], True),
        '<div style="height:6px"></div>',
        f'<div class="sec">{ic("cd",11)}Clusters<span style="flex:1"></span><span title="Connected only" style="display:flex;text-transform:none;letter-spacing:0;font-weight:400;gap:4px;align-items:center;color:var(--faint)">{dot(C["green"])}4 of 9</span></div>',
        root("prod-eu-west-1", "on", color=C["red"], prod=True),
        root("staging-eu-west-1", "on", color=C["yellow"]),
        root(f"{OCP_EU} · {JANE}", "on", color=C["cyan"], on=tooltip),
        root(f"{OCP_EU} · kube:admin", None, color=C["red"], prod=True),
        root(f"{OCP_US} · {JANE}", "connecting", color=C["accent"]),
        root("0.0.0.0:55878 · system:admin", None, color=C["orange"]),
        root("platform-onprem", "key", color=C["purple"]),
        root("homelab-k3s", "err"),
        root("kind-dev", "on", True, C["green"]),
        ti("Overview", 1, "gauge"),
        ti("Events", 1, "bell", "2", color=C["yellow"]),
        ti("Workloads", 1, open_=True),
        ti("Pods", 2, "box", "14"),
        ti("Deployments", 2, "layers", "6"),
        ti("StatefulSets", 2, "db", "1"),
        ti("Network", 1, open_=False),
        ti("Config &amp; Secrets", 1, open_=True),
        ti("ConfigMaps", 2, "file", "9", on=not tooltip),
        ti("Secrets", 2, "key", "7"),
        ti("Storage", 1, open_=False),
    ]
    return '<aside class="side">' + "\n".join(rows) + '</aside>'

def group_tooltip():
    members = [("shop", "shop"), ("payments", "payments"), ("dev-alex", "dev-alex"), ("openshift-monitoring", "openshift-monitoring")]
    mrows = "".join(f'<div style="display:flex;gap:10px;font-size:11.5px"><span class="mono" style="flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--muted)">{c}/api-ocp-eu1-example-com:6443/{JANE}</span><span class="mono" style="color:var(--dim)">{n}</span></div>' for c, n in members)
    return f'''<div role="tooltip" style="position:absolute;left:250px;top:268px;width:430px;background:#353b45;border:1px solid var(--border);border-radius:7px;box-shadow:0 12px 34px rgba(0,0,0,.5);padding:10px 12px;display:flex;flex-direction:column;gap:7px;font-size:12px;z-index:5">
<div style="font-weight:600">{OCP_EU} · {JANE}</div>
<div style="display:flex;gap:6px;align-items:center;color:var(--muted)">{dot(C["green"])}Connected · 38 ms · v1.33.1</div>
<div style="color:var(--dim)">User <span class="mono" style="color:var(--text)">{JANE}</span> on <span class="mono">https://api.{OCP_EU}:6443</span></div>
<div style="border-top:1px solid var(--bv);padding-top:7px;color:var(--dim)">14 contexts in <span class="mono">~/.kube/config</span>, shown as one cluster:</div>
<div style="display:flex;justify-content:space-between;font-size:11px;color:var(--faint);text-transform:uppercase;letter-spacing:.05em"><span>Context</span><span>Namespace</span></div>
{mrows}
<div style="color:var(--dim);font-size:11.5px">and 10 more · <span style="color:var(--muted)">current context: shop</span></div>
</div>'''

def clusters_grouped_screen():
    CC = "grid-template-columns: 22px minmax(0,1.6fr) minmax(0,1.1fr) minmax(0,0.9fr) 136px"
    def grow(open_, color, label, n, server, auth, status, scol, on=False):
        return (f'<div class="tr{" on" if on else ""}" style="{CC};height:34px"><span>{dot(color)}</span>'
                f'<span style="display:flex;gap:7px;align-items:center;min-width:0">{ic("cd" if open_ else "cr",12,C["dim"])}<span style="font-weight:500;overflow:hidden;text-overflow:ellipsis">{label}</span><span class="chip" style="height:17px;flex-shrink:0">{n} contexts</span></span>'
                f'<span class="mono" style="color:var(--muted)">{server}</span><span style="color:var(--muted)">{auth}</span><span class="pill" style="color:{scol}">{dot(scol) if scol != C["dim"] else ""}{status}</span></div>')
    def member(ctx, ns):
        return (f'<div class="tr" style="{CC};height:28px"><span></span><span class="mono" style="font-size:11.5px;color:var(--muted);padding-left:26px;overflow:hidden;text-overflow:ellipsis">{ctx}</span>'
                f'<span class="mono" style="font-size:11.5px;color:var(--dim)">namespace {ns}</span><span></span><span></span></div>')
    def single(color, name, server, auth, status, scol):
        return (f'<div class="tr" style="{CC};height:34px"><span>{dot(color)}</span><span style="font-weight:500;padding-left:19px">{name}</span>'
                f'<span class="mono" style="color:var(--muted)">{server}</span><span style="color:var(--muted)">{auth}</span><span class="pill" style="color:{scol}">{dot(scol) if scol != C["dim"] else ""}{status}</span></div>')
    rows = "".join([
        grow(True, C["cyan"], f"{OCP_EU} · {JANE}", 14, f"https://api.{OCP_EU}:6443", "OpenShift · oc login", "Connected · 38ms", C["green"], True),
        f'<div class="tr" style="{CC};height:26px"><span></span><span style="grid-column:2/-1;display:flex;gap:6px;align-items:center;font-size:11.5px;color:var(--dim);padding-left:26px">{ic("info",12)}Shown as one cluster in the sidebar: same file, cluster and user; only the namespace differs. <a href="#" style="text-decoration:none">Show contexts separately</a></span></div>',
        member(f"shop/api-ocp-eu1-example-com:6443/{JANE}", "shop"),
        member(f"payments/api-ocp-eu1-example-com:6443/{JANE}", "payments"),
        member(f"dev-alex/api-ocp-eu1-example-com:6443/{JANE}", "dev-alex"),
        f'<div class="tr" style="{CC};height:26px"><span></span><span style="font-size:11.5px;color:var(--dim);padding-left:26px">and 11 more</span></div>',
        grow(False, C["red"], f"{OCP_EU} · kube:admin", 6, f"https://api.{OCP_EU}:6443", "OpenShift · oc login", "Not connected", C["dim"]),
        grow(False, C["accent"], f"{OCP_US} · {JANE}", 9, f"https://api.{OCP_US}:6443", "OpenShift · oc login", "Connecting…", C["yellow"]),
        grow(False, C["orange"], "0.0.0.0:55878 · system:admin", 5, "https://0.0.0.0:55878", "client certificate", "Not connected", C["dim"]),
        single(C["red"], "prod-eu-west-1", "https://3F9C…gr7.eu-west-1.eks.amazonaws.com", "exec · aws eks get-token", "Connected · 41ms", C["green"]),
        single(C["green"], "kind-dev", "https://127.0.0.1:52341", "client certificate", "Connected · 2ms", C["green"]),
    ])
    src = lambda path, sub, on=False, icon="file": f'<div style="display:flex;gap:10px;padding:10px 12px;border-radius:6px;{"background:var(--sel);outline:1px solid var(--accent);outline-offset:-1px" if on else ""}">{ic(icon,15,C["accent"] if on else C["dim"])}<div style="flex:1;min-width:0"><div class="mono" style="font-size:12px;color:var(--text)">{path}</div><div style="font-size:11.5px;color:var(--dim)">{sub}</div></div></div>'
    left = f'''<div style="width:300px;flex-shrink:0;border-right:1px solid var(--bv);padding:16px 12px;display:flex;flex-direction:column;gap:4px">
<div style="display:flex;align-items:center;margin:0 4px 8px"><h2 style="font-size:14px;font-weight:600;flex:1">Kubeconfig sources</h2><button class="btn" style="height:24px">{ic("plus",12)}Add</button></div>
{src("~/.kube/config","37 contexts · 5 clusters · 8 users · watched",True)}
{src("~/work/kube/eks-prod.yaml","2 contexts · watched")}
{src("~/work/kube/platform-onprem.yaml","1 context · OIDC")}
</div>'''
    toggle = lambda on: f'<span style="width:28px;height:16px;border-radius:8px;background:{C["accent"] if on else "#4a505c"};position:relative;display:inline-block;flex-shrink:0"><span style="position:absolute;top:2px;{"right" if on else "left"}:2px;width:12px;height:12px;border-radius:50%;background:#fff"></span></span>'
    right = f'''<div style="flex:1;min-width:0;padding:16px 18px;display:flex;flex-direction:column;gap:12px">
<div style="display:flex;align-items:center;gap:10px"><h2 style="font-size:14px;font-weight:600">Contexts</h2><span style="color:var(--dim);font-size:12px">37 in ~/.kube/config · 8 clusters in the sidebar</span><div style="flex:1"></div><div class="inp" style="width:200px">{ic("search",12)}Filter contexts</div></div>
<div class="card" style="overflow:hidden"><div class="th" style="{CC}"><span></span><span>CONTEXT</span><span>API SERVER</span><span>AUTH</span><span>STATUS</span></div>{rows}</div>
<div class="card" style="padding:12px 16px;display:flex;gap:12px;align-items:center"><div style="flex:1"><div style="font-size:12.5px">One entry per cluster and user</div><div style="font-size:11.5px;color:var(--dim)">Contexts that differ only in their namespace (<span class="mono">oc project</span>) share one sidebar row · <span class="mono">kubernetes.group_contexts</span></div></div>{toggle(True)}</div>
</div>'''
    content = f'<div style="flex:1;display:flex;min-height:0;min-width:0">{left}{right}</div>'
    tb = tabs([("gear", "Clusters &amp; kubeconfigs", True), ("box", "Pods", False)])
    inner = f'''<div class="app">
{titlebar(f"{OCP_EU} · {JANE}", "shop", False, "OpenShift · v1.33.1")}
<div class="body">{polish_sidebar(True)}<main class="main">{tb}{content}</main></div>
{statusbar()}
{group_tooltip()}
</div>'''
    return page("Cluster status dots and grouped contexts — Kubyl", inner)

def yk(k, ind=0): return f'{"&nbsp;" * ind}<span style="color:{C["red"]}">{k}</span><span style="color:var(--muted)">:</span>'
def ys(v): return f' <span style="color:{C["green"]}">{v}</span>'
def yn(v): return f' <span style="color:{C["orange"]}">{v}</span>'

def cm_block(key, fmt, size, lines, body, shown=None, total=None, open_=True, binary=False, empty=False):
    chip = f'<span class="chip" style="height:17px;font-size:10.5px;padding:0 5px;{"color:var(--purple)" if binary else ""}">{fmt}</span>'
    meta = f'<span style="font-size:11px;color:var(--dim);white-space:nowrap">{size}{" · " + lines if lines else ""}</span>'
    if binary:
        tools = f'<button class="btn g" style="height:20px;padding:0 6px;font-size:11.5px">{ic("copy",11)}base64</button><button class="btn g" style="height:20px;padding:0 6px;font-size:11.5px">{ic("download",11)}Save…</button>'
    else:
        tools = f'<button class="ib" aria-label="Wrap" style="width:20px;height:20px">{ic("wrap",12)}</button><button class="ib" aria-label="Copy {key}" style="width:20px;height:20px">{ic("copy",12)}</button>'
    head = (f'<div style="display:flex;align-items:center;gap:7px;height:28px;padding:0 6px 0 8px">{ic("cd" if open_ else "cr",11,C["dim"])}'
            f'<span class="mono" style="font-size:12px;color:var(--text);flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{key}</span>{chip}{meta}{tools}</div>')
    if not open_:
        return f'<div style="border:1px solid var(--bv);border-radius:6px;background:#2a2e36">{head}</div>'
    if empty:
        content = '<div style="padding:4px 10px 8px 26px;font-size:12px;color:var(--faint);font-style:italic">(empty)</div>'
    elif binary:
        content = '<div style="padding:4px 10px 8px 26px;font-size:12px;color:var(--dim)">Binary data (Java keystore). Copy it as base64 or save it to a file.</div>'
    else:
        more = f'<div style="padding:4px 0 2px;font-size:11.5px"><a href="#" style="text-decoration:none">Show all {total} lines</a></div>' if shown else ""
        content = f'<div style="padding:2px 10px 6px 26px"><div class="mono" style="font-size:11.5px;line-height:17px;white-space:pre;color:var(--text);overflow:hidden">{body}</div>{more}</div>'
    return f'<div style="border:1px solid var(--bv);border-radius:6px;background:#2a2e36">{head}{content}</div>'

def configmap_screen():
    CMC = "grid-template-columns: minmax(0,1fr) 60px 70px 56px"
    cms = [("checkout-api-config", 10, "18.4 KiB", "41d"), ("currency-codes", 1, "2.2 KiB", "90d"), ("fraud-scorer-model", 3, "812 KiB", "6h"), ("istio-ca-root-cert", 1, "1.1 KiB", "90d"),
           ("kube-root-ca.crt", 1, "1.1 KiB", "90d"), ("ledger-writer-scripts", 4, "6.0 KiB", "12d"), ("payment-gateway-env", 9, "640 B", "47m"), ("webhook-relay-routes", 2, "3.4 KiB", "2d")]
    rows = "".join(f'<div class="tr{" on" if i == 0 else ""}" style="{CMC}"><span class="mono">{n}</span><span class="mono">{k}</span><span class="mono" style="color:var(--muted)">{s}</span><span class="mono" style="color:var(--muted)">{a}</span></div>' for i, (n, k, s, a) in enumerate(cms))
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div class="tool"><div class="crumb">{ic("file",14,C["accent"])}<b>ConfigMaps</b><span>·</span><span>8 in payments</span></div><div style="flex:1"></div><div class="inp" style="width:170px">{ic("filter",12)}Filter</div><span class="chip" style="color:var(--green)">{dot(C["green"])}live</span></div>
<div class="th" style="{CMC}"><span>NAME</span><span>DATA</span><span>SIZE</span><span>AGE</span></div>
<div style="flex:1;overflow:hidden">{rows}</div>
{hints([("↵","Details"),("e","Edit YAML"),("y","Copy"),("⌃d","Delete"),("/","Filter")])}
</div>'''
    app_yaml = "<br>".join([yk("server"), yk("port", 2) + yn("8080"), yk("shutdown", 2) + ys("graceful"), yk("payments"), yk("gateway", 2),
                            yk("url", 4) + ys("http://payment-gateway.payments.svc:8080"), yk("timeout", 4) + ys("2s")])
    flags = "<br>".join(['<span style="color:var(--muted)">{</span>', f'&nbsp;&nbsp;<span style="color:{C["red"]}">"newCheckout"</span><span style="color:var(--muted)">:</span> <span style="color:{C["orange"]}">true</span><span style="color:var(--muted)">,</span>',
                         f'&nbsp;&nbsp;<span style="color:{C["red"]}">"applePay"</span><span style="color:var(--muted)">:</span> <span style="color:{C["orange"]}">false</span><span style="color:var(--muted)">,</span>', f'&nbsp;&nbsp;<span style="color:{C["red"]}">"rolloutPercent"</span><span style="color:var(--muted)">:</span> <span style="color:{C["orange"]}">25</span>'])
    props = "<br>".join(["status = warn", "appender.console.type = Console", "rootLogger.level = info"])
    menu = f'''<div role="menu" style="position:absolute;right:14px;top:34px;width:270px;background:#353b45;border:1px solid var(--border);border-radius:7px;box-shadow:0 12px 34px rgba(0,0,0,.5);padding:4px;font-size:12.5px;z-index:4">
<div style="padding:6px 10px;border-radius:4px;background:var(--sel)">Copy as YAML <span style="color:var(--dim)">(the data map)</span></div>
<div style="padding:6px 10px">Copy as .env <span style="color:var(--dim)">· skips 3 multi-line keys</span></div></div>'''
    used = [("layers", "Deployment", "checkout-api", "3 pods", "volume config → application.yaml, feature-flags.json"),
            ("layers", "Deployment", "checkout-worker", "2 pods", "envFrom"),
            ("play", "Job", "checkout-migrate-28791440", "1 pod", "env LOG_LEVEL")]
    urows = "".join(f'<div style="display:flex;gap:8px;align-items:flex-start;padding:4px 0;font-size:12px">{ic(i,13,C["dim"])}<div style="flex:1;min-width:0"><div><span style="color:var(--dim)">{k}</span> <a href="#" class="mono" style="font-size:11.5px;text-decoration:none">{n}</a> <span style="color:var(--dim)">· {p}</span></div><div class="mono" style="font-size:11px;color:var(--muted)">{how}</div></div></div>' for i, k, n, p, how in used)
    dock = f'''<aside class="dock" style="width:470px;position:relative">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">ConfigMap details</span><button class="ib" aria-label="Pin">{ic("star",13)}</button><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec"><div class="mono" style="font-size:12.5px;margin-bottom:6px">checkout-api-config</div>
<div style="display:flex;gap:6px;flex-wrap:wrap"><span class="chip">9 keys · 1 binary</span><span class="chip">18.4 KiB</span><span class="chip" style="color:var(--yellow)">{ic("lock",11,C["yellow"])}immutable</span></div></div>
<div class="dsec" style="position:relative;padding-bottom:10px"><div style="display:flex;align-items:center;gap:6px;margin-bottom:8px"><p class="dtitle" style="margin:0;flex:1">Data</p>
<button class="btn g" style="height:22px;padding:0 6px;font-size:12px">Expand all</button><button class="btn g" style="height:22px;padding:0 6px;font-size:12px">Collapse all</button><button class="btn" style="height:22px;padding:0 8px;font-size:12px">{ic("copy",12)}Copy all{ic("cd",11)}</button></div>
{menu}
<div class="inp" style="height:26px;margin-bottom:8px">{ic("filter",12)}<span style="font-size:12px">Filter 10 keys</span></div>
<div style="display:flex;flex-direction:column;gap:6px">
{cm_block("LOG_LEVEL", "text", "4 B", "1 line", "info")}
{cm_block("SENTRY_DSN", "text", "0 B", "", "", empty=True)}
{cm_block("application.yaml", "YAML", "2.1 KiB", "64 lines", app_yaml, shown=True, total=64)}
{cm_block("entrypoint.sh", "shell", "1.2 KiB", "38 lines", "", open_=False)}
{cm_block("feature-flags.json", "JSON", "96 B", "5 lines", "", open_=False)}
{cm_block("log4j2.properties", "properties", "84 B", "3 lines", "", open_=False)}
{cm_block("nginx.conf", "text", "3.9 KiB", "142 lines", "", open_=False)}
{cm_block("truststore.jks", "binary", "3.1 KiB", "", "", binary=True)}
</div></div>
<div class="dsec" style="border-bottom:0"><p class="dtitle">Used by <span style="text-transform:none;letter-spacing:0;font-weight:400">· 6 pods</span></p>{urows}</div>
</aside>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    tb = tabs([("file", "ConfigMaps", True), ("box", "Pods", False)])
    inner = f'''<div class="app">
{titlebar("kind-dev", "payments", False, "kind · v1.37.0")}
<div class="body">{polish_sidebar(False)}<main class="main">{tb}{content}</main></div>
{statusbar()}
</div>'''
    return page("ConfigMap data in the details — Kubyl", inner)

def contexts_palette_screen():
    item = lambda state, label, sub, right, on=False: (f'<div style="display:flex;align-items:center;gap:10px;height:40px;padding:0 12px;border-radius:5px;{"background:var(--sel)" if on else ""}">{ic("wheel",14,C["accent"] if on else C["dim"])}'
                                                       f'<div style="flex:1;min-width:0"><div style="color:var(--text);white-space:nowrap;overflow:hidden;text-overflow:ellipsis">{label}</div><div class="mono" style="font-size:11px;color:var(--dim);white-space:nowrap;overflow:hidden;text-overflow:ellipsis">{sub}</div></div>'
                                                       f'<span style="font-size:11.5px;color:var(--dim);white-space:nowrap">{right}</span>{status_slot(state)}</div>')
    grp = lambda t: f'<div style="padding:8px 12px 4px;font-size:11px;font-weight:600;letter-spacing:.06em;text-transform:uppercase;color:var(--dim)">{t}</div>'
    hl = lambda t: f'<span style="color:var(--accent)">{t}</span>'
    pal = f'''<div role="dialog" aria-label="Command palette" style="position:absolute;left:50%;top:60px;margin-left:-310px;width:620px;background:#2f343e;border:1px solid var(--border);border-radius:9px;box-shadow:0 24px 60px rgba(0,0,0,.55);overflow:hidden">
<div style="display:flex;align-items:center;gap:10px;height:44px;padding:0 14px;border-bottom:1px solid var(--bv)"><span class="mono" style="color:var(--accent);font-size:14px">@</span><span class="mono" style="font-size:14px;color:var(--text)">dev-al<span style="display:inline-block;width:1px;height:16px;background:var(--accent);vertical-align:-3px;margin-left:1px"></span></span><span style="flex:1"></span><span style="font-size:11.5px;color:var(--dim)">9 clusters · 4 connected</span></div>
<div style="padding:4px 6px 8px">
{grp("Clusters")}
{item("on", f"{OCP_EU} · {JANE}", f"context {hl('dev-al')}ex/api-ocp-eu1-example-com:6443/{JANE}", "opens in dev-alex", True)}
{item("connecting", f"{OCP_US} · {JANE}", f"context {hl('dev-al')}ex/api-ocp-us1-example-com:6443/{JANE}", "opens in dev-alex")}
{item("on", "kind-dev", f"~/.kube/config · {hl('dev')}", "payments")}
{grp("Recent")}
{item("on", "prod-eu-west-1", "~/work/kube/eks-prod.yaml", "payments")}
{item(None, f"{OCP_EU} · kube:admin", "~/.kube/config · 6 contexts", "openshift-monitoring")}
</div>
<div style="display:flex;gap:16px;padding:8px 14px;border-top:1px solid var(--bv);font-size:11.5px;color:var(--dim)"><span><span class="kbd">↵</span> switch</span><span style="flex:1"></span><span>{dot(C["green"])} connected</span><span><span class="kbd">esc</span></span></div>
</div>'''
    CMC = "grid-template-columns: minmax(0,1fr) 60px 70px 56px"
    rows = "".join(f'<div class="tr" style="{CMC}"><span class="mono">{n}</span><span class="mono">{k}</span><span class="mono" style="color:var(--muted)">{s}</span><span class="mono" style="color:var(--muted)">{a}</span></div>' for n, k, s, a in [("checkout-api-config", 12, "18.4 KiB", "41d"), ("currency-codes", 1, "2.2 KiB", "90d"), ("fraud-scorer-model", 3, "812 KiB", "6h")])
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div class="tool"><div class="crumb">{ic("file",14,C["accent"])}<b>ConfigMaps</b><span>·</span><span>3 in shop</span></div><div style="flex:1"></div></div>
<div class="th" style="{CMC}"><span>NAME</span><span>DATA</span><span>SIZE</span><span>AGE</span></div>
<div style="flex:1;overflow:hidden">{rows}</div></div>'''
    pem = "<br>".join(["-----BEGIN CERTIFICATE-----", "MIIDBTCCAe2gAwIBAgIIU0lTZXhhbXBsZTANBgkqhkiG9w0BAQsFADAV", "MRMwEQYDVQQDEwprdWJlcm5ldGVzMB4XDTI2MDkyNjA4MDAwMFoXDTM2", "MDkyNDA4MDAwMFowFTETMBEGA1UEAxMKa3ViZXJuZXRlczCCASIwDQYJ"])
    masked = lambda k: f'<div style="display:flex;align-items:center;gap:7px;height:28px;padding:0 6px 0 8px;border:1px solid var(--bv);border-radius:6px;background:#2a2e36">{ic("key",12,C["dim"])}<span class="mono" style="font-size:12px;flex:1">{k}</span><span class="mono" style="font-size:12px;color:var(--dim)">••••••••</span><button class="ib" aria-label="Reveal" style="width:20px;height:20px">{ic("eye",12)}</button><button class="ib" aria-label="Copy" style="width:20px;height:20px">{ic("copy",12)}</button></div>'
    revealed = cm_block("ca.crt", "PEM", "1.1 KiB", "19 lines", pem, shown=True, total=19).replace(f'aria-label="Wrap"', 'aria-label="Hide"').replace(ic("wrap",12), ic("eye",12,C["accent"]), 1)
    sdock = f'''<aside class="dock" style="width:420px">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Secret details</span><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec"><div class="mono" style="font-size:12.5px;margin-bottom:6px">shop-db</div><div style="display:flex;gap:6px"><span class="chip">Opaque</span><span class="chip">3 keys</span></div></div>
<div class="dsec"><p class="dtitle">Data</p><div style="display:flex;flex-direction:column;gap:6px">{masked("DB_PASSWORD")}{masked("DB_USER")}{revealed}</div></div>
<div class="dsec" style="border-bottom:0"><p class="dtitle">Used by <span style="text-transform:none;letter-spacing:0;font-weight:400">· 2 pods</span></p>
<div style="display:flex;gap:8px;align-items:flex-start;font-size:12px">{ic("layers",13,C["dim"])}<div><div><span style="color:var(--dim)">Deployment</span> <a href="#" class="mono" style="font-size:11.5px;text-decoration:none">shop-api</a> <span style="color:var(--dim)">· 2 pods</span></div><div class="mono" style="font-size:11px;color:var(--muted)">env DB_PASSWORD, DB_USER · volume db-ca → ca.crt</div></div></div></div>
</aside>'''
    inner = f'''<div class="app">
{titlebar(f"{OCP_EU} · {JANE}", "shop", False, "OpenShift · v1.33.1")}
<div class="body">{polish_sidebar(False)}<main class="main">{tabs([("file", "ConfigMaps", True)])}<div style="flex:1;display:flex;min-height:0">{center}{sdock}</div></main></div>
{statusbar()}
{pal}
</div>'''
    return page("@ contexts with status dots and aliases — Kubyl", inner)

# ---------- 18. Network flows (phase 16) ----------
VERDICT = {"forwarded": C["green"], "dropped": C["red"], "no reply": C["yellow"], "error": C["orange"], "audit": C["purple"]}

def verdict_pill(v):
    """Verdict cell: a dot and the word, colored; dropped rows also get a red tint."""
    c = VERDICT[v]
    return f'<span class="pill" style="color:{c}">{dot(c)}{v}</span>'

def ep(ns, name, kind="pod"):
    """An endpoint cell: namespace in dim, then the pod or workload; world and host endpoints get an icon."""
    if kind == "world":
        return f'<span style="display:flex;align-items:center;gap:5px;min-width:0">{ic("globe",12,C["dim"])}<span class="mono" style="font-size:11.5px;overflow:hidden;text-overflow:ellipsis">{name}</span></span>'
    if kind == "host":
        return f'<span style="display:flex;align-items:center;gap:5px;min-width:0">{ic("server",12,C["dim"])}<span class="mono" style="font-size:11.5px">{name}</span></span>'
    return f'<span class="mono" style="font-size:11.5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;display:block"><span style="color:var(--dim)">{ns}/</span>{name}</span>'

def flows_header(cluster="prod-eu-west-1", prod=True, backend=None, active="Flows", counts=("12,418", None), paused=False, rate="184 flows/s", window="15m", compact=False):
    """Cluster, backend indicator (tooltip: how it's reached), other backends, live state, time window."""
    backend = backend or (f'<span class="chip" style="height:22px" title="kube-system/hubble-relay · through a temporary port-forward (127.0.0.1:51234) · Relay v1.20.2 · 8,190 flows buffered">'
                          f'{ic("flows",11,C["green"])}Hubble Relay <span class="mono" style="font-size:11px">kube-system/hubble-relay</span> · v1.20.2 · 3/3 nodes</span>'
                          f'<span style="font-size:12px;color:var(--dim);white-space:nowrap" title="Also found: NetObserv (FlowCollector cluster). Switch in the menu or with netflow.clusters.&lt;cluster&gt;.backend">also: NetObserv</span>')
    live = (f'<span class="chip" style="color:var(--yellow)">{ic("pause",11,C["yellow"])}paused · 1,204 new</span>' if paused
            else f'<span class="chip" style="color:var(--green)">{dot(C["green"])}live · {rate}</span>')
    ranges = "".join(f'<span style="padding:3px 9px;{"background:#2d3b4d;color:#a8cdf3" if r == window else "color:var(--dim)"}">{r}</span>' for r in ["15m", "1h", "6h", "24h", "7d"])
    picker = f'<div style="display:flex;border:1px solid var(--border);border-radius:5px;overflow:hidden;font-size:12px;flex-shrink:0">{ranges}</div>'
    tabs_ = [("Flows", "list", counts[0]), ("Topology", "flows", counts[1])]
    sub = "".join(f'<span style="display:flex;align-items:center;gap:6px;padding:0 10px;{"color:var(--text);box-shadow:inset 0 -2px 0 var(--accent)" if t == active else "color:var(--dim)"}">{ic(i,13,C["accent"] if t == active else C["dim"])}{t}' + (f'<span class="chip" style="height:17px">{c}</span>' if c else "") + '</span>' for t, i, c in tabs_)
    return f'''<div style="display:flex;align-items:center;gap:10px;padding:0 12px 0 16px;height:44px;flex-shrink:0;border-bottom:1px solid var(--bv);white-space:nowrap;overflow:hidden">
<span style="font-size:15px;font-weight:600">{cluster}</span>{'<span class="prod">PROD</span>' if prod else ''}
{backend}
<div style="flex:1"></div>
{"" if compact else live}
<button class="ib" aria-label="{"Resume" if paused else "Pause"}">{ic("play" if paused else "pause",13)}</button>
{"" if compact else picker}
<button class="ib" aria-label="More">{ic("more",14)}</button>
</div>
<div style="display:flex;gap:2px;padding:0 8px;border-bottom:1px solid var(--bv);height:34px;align-items:stretch;flex-shrink:0">{sub}</div>'''

def fchip(text, server=True, removable=True):
    """A filter term as a chip. A bolt: the backend applied it server-side; none: applied in Kubyl."""
    mark = f'<span title="applied by Hubble Relay (server-side)" style="display:flex">{ic("zap",10,C["accent"])}</span>' if server else f'<span title="applied in Kubyl over the buffered flows" style="display:flex">{ic("filter",10,C["dim"])}</span>'
    x = f'<span style="color:var(--dim);display:flex">{ic("x",10)}</span>' if removable else ""
    return f'<span class="chip mchip" style="gap:5px">{mark}{text}{x}</span>'

def flows_filters(expr, chips, verdicts=((("All", "12,418"), True), (("Forwarded", "12,187"), False), (("Dropped", "231"), False), (("No reply", "0"), False)), focus=False, scope="Cluster"):
    vchips = "".join(f'<span class="chip{" on" if on else ""}">{dot(VERDICT[l.lower()]) if l.lower() in VERDICT else ""}{l}<span class="mono" style="font-size:11px;color:var(--dim)">{n}</span></span>' for (l, n), on in verdicts)
    return f'''<div style="display:flex;flex-direction:column;gap:6px;padding:7px 12px;border-bottom:1px solid var(--bv);flex-shrink:0">
<div style="display:flex;align-items:center;gap:6px;white-space:nowrap">
<div class="inp{" focus" if focus else ""}" style="flex:1;height:26px;min-width:0">{ic("filter",12)}<span class="mono" style="font-size:12px;color:{"var(--text)" if expr else "var(--faint)"};overflow:hidden;white-space:nowrap">{expr or "ns=payments verdict=dropped port=443, or any text"}</span>{'<span style="width:1px;height:14px;background:var(--accent)"></span>' if focus else ""}</div>
<button class="btn g" style="height:24px;padding:0 6px">{ic("folder",12)}{scope}{ic("cd",11)}</button>
</div>
<div style="display:flex;align-items:center;gap:5px;white-space:nowrap;overflow:hidden">{chips}<span style="flex:1"></span>{vchips}</div>
</div>'''

# time, dir, source, destination, proto/port (or L7), verdict, policy, bytes/pkts
FLOWS = [
 ("14:02:31.482", "in", ep("storefront", "shopper-6fd84cfbb4-8wr4x"), ep("payments", "ledger-api-5cd68f8d6c-qmjh7"), "TCP :80", "dropped", '<span style="color:var(--red)">isolated · no policy allows it</span>', "—"),
 ("14:02:31.479", "in", ep("storefront", "scraper-655c844475-v6zbq"), ep("storefront", "web-574ff6d9fd-6hd5l"), "TCP :80", "dropped", f'<span style="color:var(--red);display:flex;gap:5px;align-items:center">{ic("shield",11,C["red"])}denied by web-guard</span>', "—"),
 ("14:02:31.466", "in", ep("storefront", "shopper-6fd84cfbb4-8wr4x"), ep("storefront", "web-574ff6d9fd-8tmc5"), '<span class="mono" style="font-size:11.5px">HTTP GET /search?q=…&amp;token=…</span>', "forwarded", "web-guard", "—"),
 ("14:02:31.421", "in", ep("payments", "checkout-client-548847d7b8-22xmb"), ep("payments", "ledger-api-5cd68f8d6c-qmjh7"), "TCP :80", "forwarded", "ledger-api-isolation", "—"),
 ("14:02:31.409", "out", ep("payments", "checkout-client-548847d7b8-22xmb"), ep("kube-system", "coredns-559f6c778d-9ffn8"), '<span class="mono" style="font-size:11.5px">DNS ledger-api.payments.svc…</span>', "forwarded", '<span style="color:var(--dim)">—</span>', "—"),
 ("14:02:31.388", "in", ep("", "203.0.113.24", "world"), ep("payments", "checkout-api-6f6b95ccc5-gtqjf"), "TCP :8443", "forwarded", "checkout-public", "—"),
 ("14:02:31.371", "out", ep("payments", "payment-gateway-557f6f8557-fbfd4"), ep("", "api.bank.example.com", "world"), "TCP :443", "forwarded", "gateway-egress", "—"),
 ("14:02:31.350", "in", ep("storefront", "shopper-6fd84cfbb4-8wr4x"), ep("payments", "checkout-api-6f6b95ccc5-kddkk"), "TCP :80", "forwarded", '<span style="color:var(--dim)">—</span>', "—"),
 ("14:02:31.342", "in", ep("", "ip-10-0-12-41", "host"), ep("payments", "checkout-api-6f6b95ccc5-s5kk2"), "TCP :80", "forwarded", '<span style="color:var(--dim)">health check</span>', "—"),
 ("14:02:31.318", "in", ep("storefront", "shopper-6fd84cfbb4-8wr4x"), ep("payments", "ledger-api-5cd68f8d6c-qmjh7"), "TCP :80", "dropped", '<span style="color:var(--red)">isolated · no policy allows it</span>', "—"),
 ("14:02:31.305", "in", ep("storefront", "scraper-655c844475-v6zbq"), ep("storefront", "web-574ff6d9fd-8tmc5"), "TCP :80", "dropped", f'<span style="color:var(--red);display:flex;gap:5px;align-items:center">{ic("shield",11,C["red"])}denied by web-guard</span>', "—"),
 ("14:02:31.277", "out", ep("storefront", "shopper-6fd84cfbb4-8wr4x"), ep("kube-system", "coredns-559f6c778d-2k7vq"), '<span class="mono" style="font-size:11.5px">DNS checkout-api.payments…</span>', "forwarded", '<span style="color:var(--dim)">—</span>', "—"),
 ("14:02:31.240", "in", ep("payments", "checkout-events-6b4dc748-8lv8h"), ep("payments", "checkout-api-6f6b95ccc5-gtqjf"), "TCP :80", "forwarded", '<span style="color:var(--dim)">—</span>', "—"),
 ("14:02:31.221", "in", ep("storefront", "shopper-6fd84cfbb4-8wr4x"), ep("storefront", "web-574ff6d9fd-6hd5l"), '<span class="mono" style="font-size:11.5px">HTTP GET / 200</span>', "forwarded", "web-guard", "—"),
 ("14:02:31.198", "in", ep("payments", "checkout-client-548847d7b8-22xmb"), ep("payments", "checkout-api-6f6b95ccc5-kddkk"), "TCP :80", "forwarded", '<span style="color:var(--dim)">—</span>', "—"),
 ("14:02:31.180", "out", ep("monitoring", "prometheus-0"), ep("payments", "checkout-api-6f6b95ccc5-s5kk2"), "TCP :9090", "forwarded", "allow-scrapes", "—"),
 ("14:02:31.152", "in", ep("storefront", "shopper-6fd84cfbb4-8wr4x"), ep("payments", "ledger-api-5cd68f8d6c-qmjh7"), "TCP :80", "dropped", '<span style="color:var(--red)">isolated · no policy allows it</span>', "—"),
]
FL_COLS = "grid-template-columns: 96px 34px minmax(0,1.3fr) minmax(0,1.3fr) minmax(0,1.05fr) 88px minmax(0,1fr) 56px;column-gap:10px"

def flow_rows(rows, selected=1):
    out = []
    for i, (t, d, s, dst, proto, v, pol, b) in enumerate(rows):
        tint = "background:#d0727712;" if v == "dropped" and i != selected else ("background:#dec18410;" if v == "no reply" and i != selected else "")
        out.append(f'''<div class="tr{" on" if i == selected else ""}" style="{FL_COLS};height:28px;{tint}">
<span class="mono" style="font-size:11.5px;color:var(--muted)">{t}</span>
<span style="font-size:11.5px;color:var(--dim)">{d}</span>{s}{dst}
<span style="font-size:12px">{proto}</span>{verdict_pill(v)}
<span style="font-size:12px;overflow:hidden;text-overflow:ellipsis">{pol}</span>
<span class="mono" style="font-size:11.5px;color:var(--dim);text-align:right">{b}</span></div>''')
    return "".join(out)

FL_HEAD = f'<div class="th" style="{FL_COLS}"><span>TIME {ic("cd",10)}</span><span>DIR</span><span>SOURCE</span><span>DESTINATION</span><span>PROTOCOL · PORT</span><span>VERDICT</span><span>POLICY</span><span style="text-align:right">BYTES</span></div>'
FL_HINTS = [("↵", "Details"), ("space", "Pause"), ("c", "Filter to connection"), ("s", "Filter to source"), ("d", "Filter to destination"), ("t", "Topology"), ("o", "Open pod"), ("/", "Filter")]

def flows_shell(active_tab, content, overlay="", cluster="prod-eu-west-1", ns="payments", prod=True, meta="EKS · v1.30.4", side=None):
    sessions = f'<span title="Network flows · svc/hubble-relay → :51234 (temporary port-forward)">{ic("flows",12,C["green"])}1 forward</span>'
    inner = f'''<div class="app">
{titlebar(cluster, ns, prod, meta)}
<div class="body">
{side or sidebar("Flows", alerts=True, flows=True)}
<main class="main">
{tabs([("flows", "Network Flows", True), ("box", "Pods", False), ("gauge", "Overview", False)])}
{content}
</main>
</div>
{statusbar(left_extra=sessions, cluster=cluster, ns=ns)}
{overlay}
</div>'''
    return page(active_tab, inner)

def flow_detail():
    kv = lambda k, v: f'<dt>{k}</dt><dd>{v}</dd>'
    raw = [("event_type", "5 · policy-verdict"), ("traffic_direction", "INGRESS"), ("drop_reason_desc", "POLICY_DENY"), ("policy_match_type", "1 · L3"),
           ("ingress_denied_by", "storefront/web-guard (CiliumNetworkPolicy, rev 3)"), ("node_name", "ip-10-0-14-7"), ("source.identity", "8791"), ("destination.identity", "31844"),
           ("l4.TCP.flags", "SYN"), ("is_reply", "false"), ("uuid", "b0c9ec1d-1166-43cf-ae43-…")]
    raw_rows = "".join(f'<div style="display:grid;grid-template-columns:132px minmax(0,1fr);gap:8px"><span style="color:var(--dim)">{k}</span><span style="color:var(--text);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{v}</span></div>' for k, v in raw)
    side = lambda title, ns, pod, wl, ip, port, labels: f'''<div class="dsec"><p class="dtitle">{title}</p>
<div style="display:flex;align-items:center;gap:7px;font-size:12px;margin-bottom:6px">{ic("box",13,C["accent"])}<a href="#" class="mono" style="font-size:11.5px;text-decoration:none;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{ns}/{pod}</a></div>
<dl class="kv" style="margin:0;grid-template-columns:84px minmax(0,1fr)">{kv("Workload", f'<a href="#" style="text-decoration:none">Deployment {wl}</a>')}{kv("Address", f'<span class="mono" style="font-size:11.5px">{ip}{port}</span>')}</dl>
<div style="display:flex;gap:4px;flex-wrap:wrap;margin-top:7px">{"".join(f'<span class="chip mchip">{l}</span>' for l in labels)}</div></div>'''
    return f'''<aside class="dock" style="width:352px;overflow:hidden">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Flow</span><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec">
<div style="display:flex;gap:10px;align-items:center;margin-bottom:6px">{verdict_pill("dropped")}<span class="chip">TCP :80</span><span class="chip">ingress</span></div>
<div style="font-size:12px;color:var(--muted)">14:02:31.479 <span style="color:var(--dim)">· 12:02:31 UTC · policy verdict</span></div>
<div style="display:flex;gap:6px;margin-top:10px"><button class="btn" style="height:24px">{ic("filter",12)}This connection</button><button class="btn g" style="height:24px">{ic("copy",12)}Copy as hubble observe</button></div>
</div>
<div class="dsec"><p class="dtitle">Policy</p>
<div style="display:flex;gap:8px;align-items:flex-start;font-size:12.5px">{ic("shield",14,C["red"])}<div style="flex:1;min-width:0"><div>Denied by <a href="#" style="text-decoration:none">storefront/web-guard</a></div><div style="color:var(--dim);font-size:12px;margin-top:2px">CiliumNetworkPolicy · ingressDeny rule · revision 3</div></div></div></div>
{side("Source", "storefront", "scraper-655c844475-v6zbq", "scraper", "10.244.1.205", ":46224", ["app=scraper", "team=storefront"])}
{side("Destination", "storefront", "web-574ff6d9fd-6hd5l", "web", "10.244.1.117", ":80", ["app=web", "team=storefront"])}
<div class="dsec" style="border-bottom:0"><p class="dtitle">Hubble fields</p>
<div class="mono" style="display:flex;flex-direction:column;gap:4px;font-size:11px">{raw_rows}</div></div>
</aside>'''

def network_flows_screen():
    chips = fchip("ns=storefront") + fchip("dst.port=80") + fchip("policy=web-guard", False) + '<span style="font-size:11.5px;color:var(--dim);margin-left:4px">1,284 of 12,418 flows</span>'
    suggest = f'''<div style="position:absolute;left:12px;top:132px;width:300px;background:#353b45;border:1px solid var(--border);border-radius:7px;box-shadow:0 10px 30px rgba(0,0,0,.45);padding:4px;font-size:12px;z-index:3">
<div style="padding:4px 8px;color:var(--dim);font-size:11px">verdict · seen in the buffer</div>
{"".join(f'<div style="display:flex;align-items:center;gap:8px;padding:4px 8px;border-radius:4px;{"background:#2d3b4d" if i == 0 else ""}">{dot(VERDICT[v])}<span class="mono" style="flex:1">verdict={v.replace(" ", "-")}</span><span class="mono" style="color:var(--dim);font-size:11px">{n}</span></div>' for i, (v, n) in enumerate([("dropped", "231"), ("forwarded", "12,187")]))}
<div style="padding:4px 8px;color:var(--faint);font-size:11px;border-top:1px solid var(--bv);margin-top:3px">tab completes · src. dst. restrict a side</div></div>'''
    rows = [r for r in FLOWS]
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0;position:relative">
{flows_filters("ns=storefront dst.port=80 policy=web-guard verdict=dr", chips, focus=True)}
{FL_HEAD}
<div style="flex:1;overflow:hidden">{flow_rows(rows)}</div>
{hints(FL_HINTS)}
{suggest}
</div>'''
    content = f'<div style="flex:1;display:flex;flex-direction:column;min-height:0">{flows_header()}<div style="flex:1;display:flex;min-height:0">{center}{flow_detail()}</div></div>'
    return flows_shell("Network flows — Kubyl", content)

# ----- topology -----
TOPO_NS = [  # name, x, y, r, color, flows/s
 ("payments", 430, 290, 46, C["orange"], "96/s"), ("storefront", 220, 170, 36, C["accent"], "61/s"),
 ("kube-system", 660, 150, 30, C["purple"], "22/s"), ("monitoring", 680, 420, 24, C["cyan"], "8/s"),
 ("ingress-nginx", 190, 410, 26, "#c678dd", "14/s"), ("world", 430, 510, 22, C["dim"], "5/s"),
]
TOPO_EDGES = [  # a, b, width, kind, label
 ("storefront", "payments", 6, "forwarded", ""), ("storefront", "payments", 2.5, "dropped", "231 dropped"),
 ("payments", "kube-system", 3, "forwarded", ""), ("storefront", "kube-system", 2.5, "forwarded", ""),
 ("monitoring", "payments", 2, "forwarded", ""), ("ingress-nginx", "storefront", 4.5, "forwarded", ""),
 ("world", "ingress-nginx", 4, "forwarded", ""), ("payments", "world", 2, "forwarded", ""),
 ("monitoring", "kube-system", 1.5, "forwarded", ""),
]

def topo_svg(nodes, edges, w, h, selected=None, sel_edge=None, labels_inside=True):
    pos = {n: (x, y, r, c) for n, x, y, r, c, *_ in nodes}
    out = []
    for i, (a, b, width, kind, label) in enumerate(edges):
        x1, y1, r1, _ = pos[a]; x2, y2, r2, _ = pos[b]
        # Two edges between one pair bend apart.
        bend = 18 if kind == "dropped" else (0 if not any(e[0] == a and e[1] == b and e[3] == "dropped" for e in edges) else -10)
        mx, my = (x1 + x2) / 2, (y1 + y2) / 2
        dx, dy = x2 - x1, y2 - y1
        ln = math.hypot(dx, dy) or 1
        cx, cy = mx - dy / ln * bend, my + dx / ln * bend
        col = {"forwarded": "#6f8a5a", "dropped": C["red"], "no reply": C["yellow"]}[kind]
        is_sel = sel_edge == i
        dash = ' stroke-dasharray="6 4"' if kind == "dropped" else ""
        # Arrow head at the target's rim.
        ex, ey = x2 - (x2 - cx) / (math.hypot(x2 - cx, y2 - cy) or 1) * (r2 + 3), y2 - (y2 - cy) / (math.hypot(x2 - cx, y2 - cy) or 1) * (r2 + 3)
        ang = math.atan2(ey - cy, ex - cx)
        ah = 7 + width
        p1 = (ex - ah * math.cos(ang - 0.4), ey - ah * math.sin(ang - 0.4)); p2 = (ex - ah * math.cos(ang + 0.4), ey - ah * math.sin(ang + 0.4))
        glow = f'<path d="M{x1},{y1} Q{cx:.1f},{cy:.1f} {ex:.1f},{ey:.1f}" fill="none" stroke="{C["accent"]}" stroke-opacity=".35" stroke-width="{width + 7}"></path>' if is_sel else ""
        out.append(f'{glow}<path d="M{x1},{y1} Q{cx:.1f},{cy:.1f} {ex:.1f},{ey:.1f}" fill="none" stroke="{col}" stroke-width="{width}" stroke-opacity=".85"{dash}></path>'
                   f'<path d="M{ex:.1f},{ey:.1f} L{p1[0]:.1f},{p1[1]:.1f} L{p2[0]:.1f},{p2[1]:.1f} Z" fill="{col}"></path>')
        if label:
            out.append(f'<text x="{cx:.1f}" y="{cy - 6:.1f}" fill="{C["red"]}" font-size="11" font-family="IBM Plex Sans, system-ui, sans-serif" text-anchor="middle">{label}</text>')
    for n, x, y, r, c, *rest in nodes:
        is_sel = n == selected
        ring = f'<circle cx="{x}" cy="{y}" r="{r + 5}" fill="none" stroke="{C["accent"]}" stroke-width="2"></circle>' if is_sel else ""
        icon_node = n == "world"
        fill = "#3b414d" if icon_node else c
        out.append(f'{ring}<circle cx="{x}" cy="{y}" r="{r}" fill="{fill}" fill-opacity="{".9" if icon_node else ".22"}" stroke="{c}" stroke-width="2"></circle>')
        if rest and rest[0]:
            out.append(f'<text x="{x}" y="{y + 4}" fill="{C["text"]}" font-size="11" font-family="IBM Plex Mono, ui-monospace, monospace" text-anchor="middle">{rest[0]}</text>')
        out.append(f'<text x="{x}" y="{y + r + 16}" fill="{C["text"] if is_sel else C["muted"]}" font-size="12" font-family="IBM Plex Sans, system-ui, sans-serif" text-anchor="middle" font-weight="{600 if is_sel else 400}">{n}</text>')
    return f'<svg width="{w}" height="{h}" viewBox="0 0 {w} {h}" aria-label="Topology">{"".join(out)}</svg>'

def topo_toolbar(zoom="Namespaces", summary="6 namespaces · 9 edges"):
    seg = "".join(f'<span style="padding:3px 10px;{"background:#2d3b4d;color:#a8cdf3" if z == zoom else "color:var(--dim)"}">{z}</span>' for z in ["Namespaces", "Workloads"])
    legend = (f'<span class="pill" style="font-size:11.5px;color:var(--muted)"><span style="width:14px;height:3px;background:#6f8a5a;display:inline-block"></span>forwarded</span>'
              f'<span class="pill" style="font-size:11.5px;color:var(--muted)"><span style="width:14px;height:0;border-top:3px dashed {C["red"]};display:inline-block"></span>dropped</span>'
              f'<span class="pill" style="font-size:11.5px;color:var(--muted)"><span style="width:14px;height:3px;background:{C["yellow"]};display:inline-block"></span>no reply</span>'
              f'<span style="font-size:11.5px;color:var(--dim)">width: flows</span>')
    return f'''<div style="display:flex;align-items:center;gap:10px;padding:0 12px;height:38px;border-bottom:1px solid var(--bv);flex-shrink:0;white-space:nowrap">
<div style="display:flex;border:1px solid var(--border);border-radius:5px;overflow:hidden;font-size:12px">{seg}</div>
<span style="font-size:12px;color:var(--dim)">{summary}</span>
<div style="flex:1"></div>{legend}
<button class="btn g" style="height:24px;padding:0 6px" title="Fit to view">{ic("max",12)}Fit</button>
</div>'''

def topo_panel(title, icon, sub, blocks, buttons):
    return f'''<aside class="dock" style="width:320px;overflow:hidden">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">{title}</span><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec"><div style="display:flex;align-items:center;gap:8px;font-size:13px">{icon}<span class="mono" style="font-size:12.5px">{sub}</span></div>
<div style="display:flex;gap:6px;margin-top:10px">{buttons}</div></div>
{blocks}
</aside>'''

def peer_rows(rows):
    return "".join(f'<div style="display:grid;grid-template-columns:minmax(0,1fr) 62px 70px;gap:8px;font-size:12px;padding:3px 0"><span class="mono" style="font-size:11.5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{a}</span><span class="mono" style="font-size:11.5px;color:var(--muted);text-align:right">{n}</span><span style="text-align:right;color:{C["red"] if d else "var(--dim)"}">{d or "—"}</span></div>' for a, n, d in rows)

def network_topology_screen():
    svg = topo_svg(TOPO_NS, TOPO_EDGES, 840, 600, selected="payments")
    canvas = f'<div style="flex:1;position:relative;overflow:hidden;background:radial-gradient(circle at 1px 1px,#30353f 1px,transparent 0) 0 0/22px 22px">{svg}<div style="position:absolute;right:12px;bottom:10px;display:flex;flex-direction:column;gap:2px"><button class="ib" style="background:#2f343e;border:1px solid var(--border)" aria-label="Zoom in">{ic("plus",13)}</button><button class="ib" style="background:#2f343e;border:1px solid var(--border)" aria-label="Zoom out">{ic("minus",13)}</button></div></div>'
    blocks = (f'<div class="dsec"><p class="dtitle">Traffic · last 15m</p><dl class="kv" style="margin:0;grid-template-columns:96px minmax(0,1fr)"><dt>In</dt><dd>5,210 flows <span style="color:var(--dim)">· 4 peers</span></dd><dt>Out</dt><dd>3,164 flows <span style="color:var(--dim)">· 3 peers</span></dd><dt>Dropped</dt><dd style="color:var(--red)">231 <span style="color:var(--dim)">· isolated, no policy allows them</span></dd></dl></div>'
              f'<div class="dsec"><p class="dtitle">Peers <span style="text-transform:none;letter-spacing:0;font-weight:400">· flows · dropped</span></p>{peer_rows([("← storefront", "3,902", "231"), ("← monitoring", "610", ""), ("← world", "698", ""), ("→ kube-system", "2,480", ""), ("→ world", "684", "")])}</div>'
              f'<div class="dsec" style="border-bottom:0"><p class="dtitle">Workloads</p>{peer_rows([("checkout-api", "4,012", ""), ("ledger-api", "1,361", "231"), ("checkout-client", "1,420", ""), ("payment-gateway", "702", "")])}</div>')
    panel = topo_panel("Namespace", ic("folder", 14, C["orange"]), "payments", blocks,
                       f'<button class="btn" style="height:24px">{ic("list",12)}Show flows</button><button class="btn g" style="height:24px">{ic("flows",12)}Workloads</button>')
    center = f'<div style="flex:1;display:flex;flex-direction:column;min-width:0">{flows_filters("", fchip("ns=payments", True, False).replace("ns=payments", "scope: all namespaces"), focus=False)}{topo_toolbar()}{canvas}{hints([("click", "Select"), ("double-click", "Show flows"), ("drag", "Pan"), ("scroll", "Zoom"), ("w", "Workloads"), ("f", "Fit"), ("/", "Filter")])}</div>'
    content = f'<div style="flex:1;display:flex;flex-direction:column;min-height:0">{flows_header(active="Topology", counts=("12,418", "6 · 9"))}<div style="flex:1;display:flex;min-height:0">{center}{panel}</div></div>'
    return flows_shell("Network topology — Kubyl", content)

TOPO_WL = [  # name, x, y, r, color, label
 ("storefront/shopper", 150, 170, 26, C["accent"], ""), ("storefront/scraper", 120, 390, 18, C["accent"], ""),
 ("storefront/web", 320, 280, 30, C["accent"], ""), ("payments/checkout-api", 540, 170, 30, C["orange"], ""),
 ("payments/ledger-api", 540, 420, 24, C["orange"], ""), ("payments/checkout-client", 710, 300, 22, C["orange"], ""),
 ("kube-system/coredns", 760, 90, 22, C["purple"], ""), ("payments/checkout-events", 720, 470, 16, C["orange"], ""),
]
TOPO_WL_EDGES = [
 ("storefront/shopper", "storefront/web", 5, "forwarded", ""), ("storefront/scraper", "storefront/web", 3, "dropped", "denied by web-guard"),
 ("storefront/shopper", "payments/checkout-api", 3.5, "forwarded", ""), ("storefront/shopper", "payments/ledger-api", 3, "dropped", "isolated"),
 ("payments/checkout-client", "payments/ledger-api", 3, "forwarded", ""), ("payments/checkout-client", "payments/checkout-api", 3, "forwarded", ""),
 ("payments/checkout-client", "kube-system/coredns", 2, "forwarded", ""), ("storefront/shopper", "kube-system/coredns", 2, "forwarded", ""),
 ("payments/checkout-events", "payments/checkout-api", 1.5, "forwarded", ""),
]

def network_topology_workloads_screen():
    # Namespace hulls behind the workloads.
    hulls = (f'<rect x="70" y="120" width="310" height="330" rx="22" fill="{C["accent"]}" fill-opacity=".05" stroke="{C["accent"]}" stroke-opacity=".3" stroke-dasharray="4 4"></rect>'
             f'<text x="86" y="140" fill="{C["accent"]}" font-size="11.5" font-family="IBM Plex Sans, system-ui, sans-serif">storefront</text>'
             f'<rect x="480" y="120" width="300" height="400" rx="22" fill="{C["orange"]}" fill-opacity=".05" stroke="{C["orange"]}" stroke-opacity=".3" stroke-dasharray="4 4"></rect>'
             f'<text x="496" y="140" fill="{C["orange"]}" font-size="11.5" font-family="IBM Plex Sans, system-ui, sans-serif">payments</text>')
    svg = topo_svg(TOPO_WL, TOPO_WL_EDGES, 840, 580, sel_edge=1)
    svg = svg.replace('aria-label="Topology">', 'aria-label="Topology">' + hulls, 1)
    # Short labels: the workload without its namespace.
    for n, *_ in TOPO_WL:
        svg = svg.replace(f'>{n}</text>', f'>{n.split("/")[1]}</text>')
    canvas = f'<div style="flex:1;position:relative;overflow:hidden;background:radial-gradient(circle at 1px 1px,#30353f 1px,transparent 0) 0 0/22px 22px">{svg}</div>'
    blocks = (f'<div class="dsec"><dl class="kv" style="margin:0;grid-template-columns:96px minmax(0,1fr)"><dt>Flows</dt><dd>412 <span style="color:var(--dim)">· last 15m</span></dd><dt>Verdict</dt><dd>{verdict_pill("dropped")}</dd><dt>Port</dt><dd class="mono" style="font-size:11.5px">TCP :80</dd></dl></div>'
              f'<div class="dsec"><p class="dtitle">Policy</p><div style="display:flex;gap:8px;align-items:flex-start;font-size:12.5px">{ic("shield",14,C["red"])}<div>Denied by <a href="#" style="text-decoration:none">storefront/web-guard</a><div style="color:var(--dim);font-size:12px;margin-top:2px">CiliumNetworkPolicy · ingressDeny · all 412 flows</div></div></div></div>'
              f'<div class="dsec" style="border-bottom:0"><p class="dtitle">Pods</p>{peer_rows([("scraper-655c844475-v6zbq → web-…-6hd5l", "209", "209"), ("scraper-655c844475-v6zbq → web-…-8tmc5", "203", "203")])}</div>')
    panel = topo_panel("Connection", ic("right", 14, C["red"]), "scraper → web", blocks,
                       f'<button class="btn" style="height:24px">{ic("list",12)}Show flows</button><button class="btn g" style="height:24px">{ic("shield",12)}Open web-guard</button>')
    center = f'<div style="flex:1;display:flex;flex-direction:column;min-width:0">{flows_filters("ns=storefront,payments", fchip("ns=storefront,payments"), focus=False)}{topo_toolbar("Workloads", "8 workloads · 9 edges")}{canvas}{hints([("click", "Select"), ("double-click", "Show flows"), ("drag", "Pan"), ("scroll", "Zoom"), ("n", "Namespaces"), ("f", "Fit"), ("/", "Filter")])}</div>'
    content = f'<div style="flex:1;display:flex;flex-direction:column;min-height:0">{flows_header(active="Topology", counts=("12,418", "8 · 9"))}<div style="flex:1;display:flex;min-height:0">{center}{panel}</div></div>'
    return flows_shell("Network topology: workloads — Kubyl", content)

# ----- other backends: Calico Whisker, NetObserv without Loki -----
WH_COLS = "grid-template-columns: 84px minmax(0,1.2fr) minmax(0,1.2fr) 58px 80px minmax(0,1.1fr) 82px;column-gap:8px"

def network_backends_screen():
    wh_rows = [
        ("14:02:15–30", "storefront/scraper-655c844475-*", "storefront/web-574ff6d9fd-*", "TCP :80", "dropped", f'{ic("shield",11,C["red"])}<span>web-guard <span style="color:var(--dim)">· tier default</span></span>', "8 · 592 B"),
        ("14:02:15–30", "storefront/shopper-6fd84cfbb4-*", "payments/ledger-api-5cd68f8d6c-*", "TCP :80", "dropped", f'<span style="color:var(--red)">isolated by ledger-api-isolation</span>', "30 · 2.2 KB"),
        ("14:02:15–30", "storefront/shopper-6fd84cfbb4-*", "storefront/web-574ff6d9fd-*", "TCP :80", "forwarded", '<span>web-guard <span style="color:var(--dim)">· tier default</span></span>', "210 · 31 KB"),
        ("14:02:15–30", "payments/checkout-client-548847d7b8-*", "payments/ledger-api-5cd68f8d6c-*", "TCP :80", "forwarded", "ledger-api-isolation", "90 · 11 KB"),
        ("14:02:15–30", "storefront/scraper-655c844475-*", "kube-system/coredns-559f6c778d-*", "UDP :53", "forwarded", '<span style="color:var(--dim)">kns.kube-system (profile)</span>', "4 · 453 B"),
        ("14:02:00–15", "payments/checkout-client-548847d7b8-*", "payments/checkout-api-6f6b95ccc5-*", "TCP :80", "forwarded", '<span style="color:var(--dim)">kns.payments (profile)</span>', "88 · 10 KB"),
    ]
    rows = "".join(f'''<div class="tr" style="{WH_COLS};height:28px;{"background:#d0727712;" if v == "dropped" else ""}"><span class="mono" style="font-size:11.5px;color:var(--muted)">{t}</span>{ep(s.split("/")[0], s.split("/")[1])}{ep(d.split("/")[0], d.split("/")[1])}<span style="font-size:12px">{p}</span>{verdict_pill(v)}<span style="font-size:12px;display:flex;gap:5px;align-items:center;overflow:hidden;white-space:nowrap">{pol}</span><span class="mono" style="font-size:11.5px;color:var(--dim);text-align:right">{b}</span></div>''' for t, s, d, p, v, pol, b in wh_rows)
    wh_head = f'<div class="th" style="{WH_COLS}"><span>TIME</span><span>SOURCE</span><span>DESTINATION</span><span>PORT</span><span>VERDICT</span><span>POLICY</span><span style="text-align:right">PKTS · BYTES</span></div>'
    whisker_backend = (f'<span class="chip" style="height:22px">{ic("flows",11,C["green"])}Calico Whisker <span class="mono" style="font-size:11px">calico-system/whisker</span> · v3.32.2</span>'
                       f'<span style="font-size:12px;color:var(--dim)" title="Whisker reports 15-second aggregates per source and destination">15 s aggregates</span>')
    left = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0;border-right:1px solid var(--border)">
{tabs([("flows", "Network Flows · staging", True)], tools=False)}
{flows_header("staging-eu-west-1", False, whisker_backend, counts=("1,906", None), rate="12 records/s")}
{flows_filters("verdict=dropped", fchip("verdict=dropped").replace("Hubble Relay", "Whisker"), verdicts=((("All", "1,906"), False), (("Forwarded", "1,860"), False), (("Dropped", "46"), True)))}
{wh_head}<div style="flex:1;overflow:hidden">{rows}</div>
{hints([("↵", "Details"), ("space", "Pause"), ("t", "Topology"), ("/", "Filter")])}
</div>'''
    netobserv_backend = (f'<span class="chip" style="height:22px;color:var(--yellow)" title="FlowCollector cluster: spec.loki.enable is false">{ic("flows",11,C["yellow"])}NetObserv 2.0 · metrics only</span>')
    no_nodes = [("payments", 220, 200, 40, C["orange"], ""), ("storefront", 90, 110, 30, C["accent"], ""), ("kube-system", 350, 110, 26, C["purple"], ""), ("openshift-ingress", 90, 300, 26, "#c678dd", ""), ("world", 350, 300, 20, C["dim"], "")]
    no_edges = [("storefront", "payments", 5, "forwarded", ""), ("payments", "kube-system", 3, "forwarded", ""), ("openshift-ingress", "storefront", 4, "forwarded", ""), ("world", "openshift-ingress", 3, "forwarded", ""), ("storefront", "kube-system", 2, "forwarded", "")]
    right = f'''<div style="width:470px;flex-shrink:0;display:flex;flex-direction:column;min-width:0">
{tabs([("flows", "Network Flows · ocp-lab", True)], tools=False)}
{flows_header("ocp-lab.example.com", False, netobserv_backend, active="Topology", counts=(None, "5 · 5"), compact=True)}
<div style="display:flex;gap:10px;align-items:flex-start;padding:10px 14px;border-bottom:1px solid var(--bv);font-size:12.5px;background:#dec18410">{ic("info",14,C["yellow"])}<div style="flex:1;line-height:18px"><b style="font-weight:600">No single flows: NetObserv doesn't store them here.</b> <span style="color:var(--muted)">Its FlowCollector has Loki turned off, so the Flows tab stays empty. The topology comes from NetObserv's metrics (<span class="mono" style="font-size:11.5px">netobserv_workload_ingress_bytes_total</span>) through Prometheus.</span></div></div>
{topo_toolbar("Namespaces", "5 namespaces · bytes, 1h")}
<div style="flex:1;position:relative;overflow:hidden;background:radial-gradient(circle at 1px 1px,#30353f 1px,transparent 0) 0 0/22px 22px;padding:30px 20px">{topo_svg(no_nodes, no_edges, 430, 420)}</div>
{hints([("click", "Select"), ("w", "Workloads"), ("f", "Fit"), ("/", "Filter")])}
</div>'''
    content = f'<div style="flex:1;display:flex;min-height:0;min-width:0">{left}{right}</div>'
    inner = f'''<div class="app">
{titlebar("staging-eu-west-1", "storefront", False, "kind · v1.37.0")}
<div class="body">{sidebar("Flows", flows=True)}<main class="main" style="flex-direction:row">{content}</main></div>
{statusbar(cluster="staging-eu-west-1", ns="storefront")}
</div>'''
    return page("Network flows: Calico Whisker and NetObserv — Kubyl", inner)

# ----- states: nothing to read, forbidden -----
def network_states_screen():
    checked = [("minus", "Hubble Relay", 'no <span class="mono" style="font-size:11.5px">hubble-relay</span> Service in kube-system or cilium, none labelled <span class="mono" style="font-size:11.5px">k8s-app=hubble-relay</span>'),
               ("minus", "Calico Whisker", 'no <span class="mono" style="font-size:11.5px">calico-system/whisker</span> Service'),
               ("minus", "NetObserv", 'no <span class="mono" style="font-size:11.5px">FlowCollector</span> (flows.netobserv.io isn\'t served)')]
    crow = "".join(f'<div style="display:flex;gap:10px;padding:7px 0;border-bottom:1px solid var(--bv);font-size:12.5px;align-items:flex-start">{ic(i,13,C["dim"])}<div style="flex:1;min-width:0"><div>{t}</div><div style="color:var(--dim);font-size:12px;margin-top:2px">{d}</div></div></div>' for i, t, d in checked)
    card = lambda title, text, link: f'<div class="card" style="flex:1;padding:10px 12px;display:flex;flex-direction:column;gap:5px;min-width:0"><div style="font-weight:500;font-size:12.5px">{title}</div><div style="color:var(--muted);font-size:12px;line-height:17px;flex:1">{text}</div><a href="#" style="font-size:12px;text-decoration:none;display:flex;gap:5px;align-items:center">{ic("ext",11)}{link}</a></div>'
    none_ = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0;border-right:1px solid var(--border)">
{tabs([("flows", "Network Flows · homelab-k3s", True)], tools=False)}
<div style="display:flex;align-items:center;gap:10px;padding:0 16px;height:44px;border-bottom:1px solid var(--bv);flex-shrink:0"><span style="font-size:15px;font-weight:600">homelab-k3s</span><span class="chip" style="height:22px">{ic("flows",11,C["dim"])}no flow source</span><div style="flex:1"></div><button class="btn g">{ic("refresh",13)}Look again</button></div>
<div style="flex:1;display:flex;flex-direction:column;gap:14px;padding:26px 30px">
<div style="display:flex;gap:12px;align-items:center">{ic("flows",22,C["dim"])}<div><div style="font-size:16px;font-weight:600">No network flows for this cluster</div><div style="font-size:12.5px;color:var(--muted);margin-top:2px">It runs flannel, which records no flows. Kubyl reads them from one of these:</div></div></div>
<div style="display:flex;gap:10px">{card("NetObserv", "Works with any CNI, flannel included: an eBPF agent on each node. Single flows need its Loki; the topology works from its metrics.", "Install NetObserv")}{card("Cilium with Hubble", "Replaces the CNI. Hubble Relay names the policy behind each verdict and sees HTTP and DNS.", "Cilium on k3s")}{card("Calico 3.30+ with Whisker", "Replaces the CNI. Whisker's flow log shows the policy trace of each verdict.", "Calico Whisker")}</div>
<div class="card" style="padding:6px 14px 4px"><p class="dtitle" style="margin:6px 0 2px">What was checked · 1 min ago</p>{crow}</div>
<div style="font-size:12.5px;color:var(--muted);line-height:19px">Somewhere else? Name it in settings.json:</div>
<div class="mono" style="font-size:11.5px;line-height:17px;padding:8px 10px;border-radius:6px;background:#23272e;color:var(--muted)">"netflow": {{ "clusters": {{ "homelab-k3s": {{ "backend": "hubble",<br>&nbsp;&nbsp;"hubble": {{ "namespace": "cilium", "service": "relay" }} }} }} }}</div>
</div>
</div>'''
    forbidden = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{tabs([("flows", "Network Flows · prod-eu-west-1", True)], tools=False)}
<div style="display:flex;align-items:center;gap:10px;padding:0 16px;height:44px;border-bottom:1px solid var(--bv);flex-shrink:0"><span style="font-size:15px;font-weight:600">prod-eu-west-1</span><span class="prod">PROD</span><span class="chip" style="height:22px;color:var(--red)">{ic("flows",11,C["red"])}Hubble Relay · forbidden</span><div style="flex:1"></div><button class="btn g">{ic("refresh",13)}Try again</button></div>
<div style="flex:1;display:flex;flex-direction:column;gap:14px;padding:26px 30px">
<div style="display:flex;gap:12px;align-items:center">{ic("lock",22,C["red"])}<div><div style="font-size:16px;font-weight:600">Kubyl can't reach Hubble Relay</div><div style="font-size:12.5px;color:var(--muted);margin-top:2px">Relay speaks gRPC, which goes through a temporary port-forward on this machine.</div></div></div>
<div class="card" style="padding:12px 14px;display:flex;gap:10px;align-items:flex-start">{ic("err",14,C["red"])}<div style="font-size:12.5px;line-height:19px">Forbidden: you can't <span class="mono" style="font-size:11.5px">create pods/portforward</span> in <span class="mono" style="font-size:11.5px">kube-system</span>.<div style="color:var(--muted)">Ask for a role that can create pods/portforward on the hubble-relay pods in kube-system. Nothing else is needed: Kubyl never reads Secrets or exposes a port.</div></div></div>
<div style="font-size:12px;color:var(--dim)">Also found: NetObserv (FlowCollector cluster) · <a href="#" style="text-decoration:none">Use NetObserv instead</a></div>
</div>
</div>'''
    content = f'<div style="flex:1;display:flex;min-height:0;min-width:0">{none_}{forbidden}</div>'
    inner = f'''<div class="app">
{titlebar("homelab-k3s", "default", False, "k3s · v1.36.3")}
<div class="body">{sidebar("Flows", flows=True)}<main class="main" style="flex-direction:row">{content}</main></div>
{statusbar(cluster="homelab-k3s", ns="default")}
</div>'''
    return page("Network flows: no source, forbidden — Kubyl", inner)

# ---------- 19. Agents (phase 21) ----------
def ag_dock(body, composer=True, sub=None, width=420):
    head = f'<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Agent</span><button class="ib" aria-label="Threads">{ic("history",13)}</button><button class="ib" aria-label="New thread">{ic("plus",14)}</button><button class="ib" aria-label="Close thread">{ic("trash",13)}</button></div>'
    if sub is None:
        sub = f'<div style="display:flex;align-items:center;gap:6px;padding:6px 14px;border-bottom:1px solid var(--bv)">{dot(C["red"])}<span style="flex:1;font-size:12.5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">Why does payment-gateway crash?</span><span class="prod" style="font-size:9.5px;padding:0 4px">PROD</span><span style="font-size:11.5px;color:var(--faint)">working</span></div>'
    comp = ""
    if composer:
        comp = (f'<div style="flex-shrink:0;display:flex;flex-direction:column;gap:6px;padding:10px 12px;border-top:1px solid var(--bv)">'
                f'<div style="display:flex;gap:4px;flex-wrap:wrap"><span class="chip">{ic("link",11)}Pod payments/payment-gateway-5c8b7f9d4-hl2vp{ic("x",10)}</span></div>'
                '<div class="inp focus" style="height:52px;align-items:flex-start;padding-top:6px;color:var(--faint)">Ask about this cluster…  (@ mentions an object, Enter sends)</div>'
                f'<div style="display:flex;align-items:center;gap:6px"><span class="chip on">Ask</span><span class="chip">Plan</span><span style="flex:1;font-size:11.5px;color:var(--faint)">Claude · prod-eu-west-1</span><button class="btn" style="height:24px">{ic("square",11)}Stop</button></div>'
                '</div>')
    return f'<aside class="dock" style="width:{width}px">{head}{sub}<div style="flex:1;overflow:hidden;padding:12px 14px;display:flex;flex-direction:column;gap:10px">{body}</div>{comp}</aside>'

def ag_user(text, chip=None):
    c = f'<div style="display:flex;gap:4px"><span class="chip">{ic("link",11)}{chip}</span></div>' if chip else ""
    return f'<div style="display:flex;flex-direction:column;gap:6px;padding:8px 10px;border-radius:6px;background:var(--elev);border:1px solid var(--bv)">{c}<div style="font-size:13px">{text}</div></div>'

def ag_tool(icon, title, status="ok", body=""):
    sc = {"ok": ("check", C["green"]), "run": ("clock", C["yellow"]), "err": ("x", C["red"])}[status]
    return f'<div style="display:flex;flex-direction:column;gap:6px;padding:6px 10px;border-radius:6px;border:1px solid var(--bv)"><div style="display:flex;align-items:center;gap:6px">{ic(icon,13,C["muted"])}<span style="flex:1;font-size:12.5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{title}</span>{ic(sc[0],12,sc[1])}</div>{body}</div>'

def ag_mono(text):
    return f'<div class="mono" style="font-size:11.5px;line-height:17px;padding:6px 8px;border-radius:4px;background:var(--bg);border:1px solid var(--bv);color:var(--muted);white-space:pre">{text}</div>'

def ag_ask(icon, title, inner, buttons):
    return f'<div style="display:flex;flex-direction:column;gap:8px;padding:10px;border-radius:6px;border:1px solid var(--yellow);background:var(--elev)"><div style="display:flex;gap:6px;align-items:center">{ic(icon,13,C["yellow"])}<span style="font-size:12.5px">{title}</span></div>{inner}<div style="display:flex;gap:6px;flex-wrap:wrap">{"".join(buttons)}</div></div>'

def ag_pods_center():
    head = f'<div class="th" style="{POD_COLS}"><span>NAME {ic("cd",10)}</span><span>READY</span><span>STATUS</span><span style="text-align:right;padding-right:10px">RESTARTS</span><span>CPU</span><span>MEMORY</span><span>NODE</span><span>AGE</span></div>'
    rows, picked = [], False
    for (n, rd, s, r, cpu, cp, mem, mp, node, age) in PODS[:14]:
        on = s == "CrashLoopBackOff" and not picked
        picked = picked or on
        rows.append(f'<div class="tr{" on" if on else ""}" style="{POD_COLS}"><span class="mono">{n}</span><span class="mono">{rd}</span>{st(s)}<span class="mono" style="text-align:right;padding-right:10px">{r}</span><span class="mono">{f"{cpu}m" if cpu else "—"}</span><span class="mono">{mem if cpu else "—"}</span><span class="mono" style="color:var(--muted)">{node}</span><span class="mono" style="color:var(--muted)">{age}</span></div>')
    return f'<div style="flex:1;display:flex;flex-direction:column;min-width:0">{head}<div style="flex:1;overflow:hidden">{"".join(rows)}</div>{hints([("↵","Details"),("l","Logs"),("s","Shell"),("⇧a","Ask agent"),("y","YAML"),("/","Filter")])}</div>'

def ag_app(dock, right_extra=""):
    return ('<div class="app">' + titlebar() + '<div class="body">' + sidebar("Pods")
            + '<main class="main">' + tabs([("box", "Pods", True), ("list", "Logs · payment-gateway", False)])
            + f'<div style="flex:1;display:flex;min-height:0">{ag_pods_center()}{dock}</div></main></div>'
            + statusbar(right_extra=right_extra) + '</div>')

AG_WAITING = f'<span style="color:var(--yellow)">{ic("zap",12,C["yellow"])}Agent needs you</span>'

def agent_screen():
    plan = (f'<div style="display:flex;flex-direction:column;gap:3px;padding:8px;border-radius:6px;background:var(--elev)">'
            f'<div style="display:flex;gap:6px;align-items:center;font-size:11.5px;color:var(--muted)">{ic("listchecks",12)}Plan</div>'
            f'<div style="font-size:12px;color:var(--dim)">{ic("ok",11,C["green"])} Read the pod and its events</div>'
            f'<div style="font-size:12px">{ic("right",11,C["accent"])} Compare memory with the limit</div>'
            f'<div style="font-size:12px">{ic("square",11,C["faint"])} Propose a new limit</div></div>')
    warn = f'<div style="display:flex;gap:6px;font-size:12px;color:var(--red)">{ic("alert",12,C["red"])}Reads Secrets: their values would go to the agent&#39;s provider.</div>'
    body = "".join([
        ag_user("Why does this pod keep restarting?", "Pod payments/payment-gateway-5c8b7f9d4-hl2vp"),
        f'<div style="display:flex;gap:4px;align-items:center;font-size:11.5px;color:var(--dim)">{ic("right",11,C["faint"])}Thinking</div>',
        ag_tool("layers", "Kubyl · describe pod payments/payment-gateway-5c8b7f9d4-hl2vp"),
        ag_tool("layers", "Kubyl · logs payments/payment-gateway-5c8b7f9d4-hl2vp"),
        ag_tool("layers", "Kubyl · events payments"),
        '<div style="font-size:13px;line-height:19px">The <span class="mono" style="font-size:12px">gateway</span> container is <b>OOMKilled</b> 14 times in 3 hours: its limit is <span class="mono" style="font-size:12px">256Mi</span> and it peaks at about 300Mi while it warms the currency cache.</div>',
        plan,
        ag_tool("terminal", "Check the chart's values", "run", ag_mono("$ helm get values payments -n payments\n…")),
        ag_ask("terminal", "Run this command?",
               ag_mono("$ kubectl -n payments get secret gateway-db -o yaml") + warn
               + '<div style="font-size:11.5px;color:var(--dim)">in ~/Library/Caches/kubyl/agent/threads/…/work</div>',
               ['<button class="btn p" style="height:24px">Run</button>', '<button class="btn g" style="height:24px">Don&#39;t run</button>']),
    ])
    return page("Agent — Kubyl", ag_app(ag_dock(body), AG_WAITING))

def agent_states_screen():
    note = (f'<div style="display:flex;flex-direction:column;gap:6px;padding:10px;border-radius:6px;border:1px solid var(--border);background:var(--elev)">'
            f'<div style="display:flex;gap:6px;align-items:center">{ic("shield",13,C["accent"])}<span style="font-size:12.5px">What the agent sees</span></div>'
            '<div style="font-size:11.5px;color:var(--muted)">Your own agent (Claude, Codex, Gemini…) runs on this machine with its own sign-in. It reads one cluster through Kubyl&#39;s read-only tools, with your access. Kubyl masks Secret values, private keys and token-like text, and asks before every command and file change.</div>'
            '<div style="font-size:11.5px;color:var(--muted)">What it reads goes to the agent&#39;s provider, logs included. The agent isn&#39;t sandboxed: its own tools can still read files on this machine, such as ~/.kube/config.</div>'
            '<div><button class="btn" style="height:24px">Got it</button></div></div>')
    def agent(name, ok, sub, sel=False, copy=False):
        bg = "background:var(--sel);" if sel else ""
        cp = f'<button class="ib" aria-label="Copy">{ic("copy",12)}</button>' if copy else ""
        return f'<div style="display:flex;gap:8px;align-items:center;padding:5px 6px;border-radius:4px;{bg}">{dot(C["green"] if ok else C["faint"])}<div style="flex:1;min-width:0"><div style="font-size:12.5px">{name}</div><div class="mono" style="font-size:11px;color:var(--dim);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{sub}</div></div>{cp}</div>'
    sec = lambda t: f'<div class="dtitle" style="margin:6px 0 0">{t}</div>'
    stopped = (f'<div style="display:flex;flex-direction:column;gap:6px;padding:10px;border-radius:6px;border:1px solid var(--red)">'
               f'<div style="display:flex;gap:6px;font-size:12px;color:var(--red)">{ic("alert",12,C["red"])}The agent stopped: connect ECONNREFUSED</div>'
               f'<div style="display:flex;gap:6px"><button class="btn" style="height:24px">{ic("undo",12)}Retry</button><button class="btn g" style="height:24px">Agent output</button></div></div>')
    body = "".join([
        note, sec("Cluster"),
        f'<div style="display:flex;gap:6px;align-items:center"><span class="chip">{dot(C["red"])}prod-eu-west-1</span><span style="font-size:11.5px;color:var(--dim)">The agent reads it through Kubyl&#39;s tools, with your access.</span></div>',
        sec("Agent"),
        agent("Claude", True, "installed · /opt/homebrew/bin/claude-agent-acp", True),
        agent("Codex", True, "running · ~/.npm-global/bin/codex-acp"),
        agent("Gemini CLI", False, "npm install -g @google/gemini-cli", copy=True),
        agent("GitHub Copilot", False, "npm install -g @github/copilot", copy=True),
        agent("goose", False, "See block.github.io/goose for install options"),
        sec("Sign-in and stopped states"),
        ag_ask("key", "Claude needs a sign-in", '<div style="font-size:11.5px;color:var(--muted)">Run `claude` in a terminal once and sign in (Claude Code), then try again.</div>',
               ['<button class="btn" style="height:24px">Log in with Claude</button>', '<button class="btn g" style="height:24px">Retry</button>']),
        stopped,
    ])
    sub = '<div style="display:flex;align-items:center;gap:6px;padding:6px 14px;border-bottom:1px solid var(--bv);font-size:12px;color:var(--dim)">New thread</div>'
    return page("Agent: new thread and states — Kubyl", ag_app(ag_dock(body, sub=sub)))

def agent_questions_screen():
    field = lambda label, ctl, note="": f'<div style="display:flex;flex-direction:column;gap:3px"><div style="font-size:11.5px;color:var(--muted)">{label}</div>{ctl}{note}</div>'
    form = ag_ask("info", "Where should the fix go?",
        '<div style="font-size:12px">I can write the new limit into your GitOps repo. Which overlay?</div>'
        + field("Overlay *", '<div style="display:flex;gap:4px"><span class="chip">dev</span><span class="chip">staging</span><span class="chip on">prod</span></div>')
        + field("Memory limit *", '<div class="inp" style="width:140px">384M</div>', '<div style="font-size:11.5px;color:var(--red)">Doesn&#39;t match ^[0-9]+(Mi|Gi)$.</div>')
        + field("Open a merge request", '<div><span class="chip on">Yes</span></div>')
        + '<div style="font-size:11.5px;color:var(--dim)">Your answer goes to the agent. Don&#39;t enter passwords or tokens here.</div>',
        ['<button class="btn p" style="height:24px">Answer</button>', '<button class="btn g" style="height:24px">Decline</button>'])
    url = ag_ask("globe", "The agent asks you to open a page",
        '<div style="font-size:12px">Authorize GitLab access for the merge request.</div>' + ag_mono("https://gitlab.example.com/oauth/authorize?client_id=…")
        + '<div style="font-size:11.5px;color:var(--dim)">It opens in your browser. Check the address before you sign in anywhere.</div>',
        [f'<button class="btn p" style="height:24px">{ic("ext",12,"#1b1e24")}Open in browser</button>', '<button class="btn g" style="height:24px">Decline</button>'])
    line = lambda text, col, bg="": f'<div class="mono" style="font-size:11.5px;padding:0 6px;color:{col};{bg}">{text}</div>'
    diff = (line("  resources:", C["dim"]) + line("    limits:", C["dim"])
            + line("-      memory: 256Mi", C["red"], "background:rgba(208,114,119,.08)")
            + line("+      memory: 384Mi", C["green"], "background:rgba(161,193,129,.08)"))
    write = ag_ask("diff", "Change this file?",
        '<div style="font-size:11.5px;color:var(--dim)">~/gitops/apps/payments/overlays/prod/gateway.yaml</div>'
        + f'<div style="padding:4px 0;border-radius:4px;border:1px solid var(--bv);background:var(--bg)">{diff}</div>',
        ['<button class="btn p" style="height:24px">Write</button>', '<button class="btn g" style="height:24px">Don&#39;t write</button>'])
    body = ag_user("Fix it in the repo, please.") + form + url + write
    return page("Agent: questions, links and file changes — Kubyl", ag_app(ag_dock(body), AG_WAITING))

# ---------- 20. Helm: charts, install, upgrade, rollback, uninstall (phase 22) ----------
HELM_CHARTS = [
 ("kubyl-demo", "kubyl-dev", "0.2.0", "1.1.0", "A small chart for Kubyl's Helm views."),
 ("kubyl-hookfail", "kubyl-dev", "0.1.0", "0.1.0", "A chart whose post-install hook fails."),
 ("ingress-nginx", "ingress-nginx", "4.13.3", "1.13.3", "Ingress controller for Kubernetes using NGINX as a reverse proxy."),
 ("kube-prometheus-stack", "prometheus-community", "84.1.0", "v0.86.1", "Prometheus, Alertmanager, Grafana and the Prometheus Operator."),
 ("metrics-server", "metrics-server", "3.13.0", "0.8.0", "Metrics Server for the Kubernetes resource metrics API."),
 ("cert-manager", "jetstack", "v1.19.1", "v1.19.1", "A Helm chart for cert-manager."),
 ("external-secrets", "external-secrets", "0.20.4", "v0.20.4", "External secret management for Kubernetes."),
 ("argo-cd", "argo", "9.0.5", "v3.1.9", "A Helm chart for Argo CD, a declarative GitOps tool."),
 ("redis", "bitnami", "21.2.13", "8.2.1", "Redis(R) is an open source, advanced key-value store."),
]
HCC = "grid-template-columns: minmax(0,1fr) 120px 76px 80px minmax(0,1.4fr)"

def helm_charts_center(selected=0, query="", oci=False):
    rows = []
    if oci:
        rows.append(f'<div class="tr on" style="{HCC};height:34px"><span class="mono" style="display:flex;gap:6px;align-items:center">{ic("box",12,C["accent"])}kubyl-demo</span><span style="color:var(--muted)">OCI reference</span><span class="mono">0.2.0</span><span class="mono" style="color:var(--muted)">1.1.0</span><span class="mono" style="font-size:11.5px;color:var(--dim)">oci://localhost:5022/charts/kubyl-demo</span></div>')
    for i, (n, repo, v, av, d) in enumerate(HELM_CHARTS if not oci else []):
        rows.append(f'<div class="tr{" on" if i == selected else ""}" style="{HCC};height:34px"><span class="mono">{n}</span><span class="mono" style="color:var(--muted)">{repo}</span><span class="mono">{v}</span><span class="mono" style="color:var(--muted)">{av}</span><span style="color:var(--dim);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{d}</span></div>')
    repos = [("All repositories", "41", True), ("kubyl-dev", "2", False), ("argo", "6", False), ("bitnami", "112", False), ("ingress-nginx", "1", False), ("jetstack", "2", False), ("metrics-server", "1", False), ("prometheus-community", "48", False)]
    rrows = "".join(f'<div style="display:flex;align-items:center;height:24px;padding:0 8px;border-radius:5px;font-size:12.5px;{"background:var(--sel);color:var(--text)" if on else "color:var(--muted)"}"><span style="flex:1" class="{"" if i == 0 else "mono"}">{n}</span><span class="mono" style="font-size:11px;color:var(--dim)">{k}</span></div>' for i, (n, k, on) in enumerate(repos))
    q = query or "Search charts, or oci://…"
    return f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div class="tool"><div class="crumb">{ic("anchor",14,C["accent"])}<b>Charts</b><span>·</span><span>173 charts · 8 repositories</span></div><div style="flex:1"></div>
<div class="inp{" focus" if query else ""}" style="width:330px">{ic("search",12)}<span style="color:{"var(--text)" if query else "var(--dim)"}" class="{"mono" if query else ""}">{q}</span></div><button class="btn g">{ic("refresh",12)}Update</button><button class="btn">{ic("gear",12)}Repositories…</button></div>
<div style="flex:1;display:flex;min-height:0">
<div style="width:200px;flex-shrink:0;border-right:1px solid var(--bv);padding:12px 10px;display:flex;flex-direction:column;gap:6px"><p class="dtitle" style="padding-left:8px">Repositories</p>{rrows}
<div style="flex:1"></div><div style="font-size:11px;color:var(--dim);padding:0 8px;line-height:16px">From Helm's own repositories.yaml (helm env). Artifact Hub search: off (helm.artifact_hub).</div></div>
<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div class="th" style="{HCC}"><span>CHART</span><span>REPOSITORY</span><span>VERSION</span><span>APP</span><span>DESCRIPTION</span></div>
<div style="flex:1;overflow:hidden">{"".join(rows)}</div></div></div>
{hints([("↵","Details"),("i","Install…"),("/","Search"),("⇧u","Update repositories")])}
</div>'''

def helm_chart_details():
    readme = ('<div style="font-size:16px;font-weight:600;margin-bottom:6px">kubyl-demo 0.2.0</div>'
              '<div style="font-size:12.5px;color:var(--muted);line-height:18px;margin-bottom:8px">A small chart to try Kubyl\'s Helm views: install, upgrade, rollback and uninstall.</div>'
              '<div style="display:grid;grid-template-columns:110px 84px 1fr;font-size:12px;border-top:1px solid var(--bv)">'
              + "".join(f'<span class="mono" style="padding:4px 0;border-bottom:1px solid var(--bv)">{a}</span><span class="mono" style="padding:4px 0;border-bottom:1px solid var(--bv);color:var(--muted)">{b}</span><span style="padding:4px 0;border-bottom:1px solid var(--bv);color:var(--muted)">{c}</span>' for a, b, c in [("replicaCount", "1", "How many pods run"), ("greeting", "hello", "Shown in the ConfigMap"), ("auth.password", "change-me", "Rendered into a Secret")])
              + '</div>')
    return f'''<aside class="dock" style="width:390px">
<div class="phead" style="border-bottom:1px solid var(--bv);height:auto;padding:12px 12px 12px 14px;align-items:flex-start">{op_tile("KD",C["accent"],34)}<div style="flex:1;margin-left:6px;min-width:0"><div style="color:var(--text);font-weight:600;font-size:14px" class="mono">kubyl-demo</div><div style="font-size:11.5px;color:var(--dim)">kubyl-dev · A small chart for Kubyl's Helm views.</div></div><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec"><div style="display:flex;gap:8px;align-items:center"><button class="btn p">{ic("download",13,"#1b1e24")}Install…</button><button class="btn" style="width:150px;justify-content:space-between"><span class="mono" style="font-size:12px">0.2.0</span><span style="color:var(--dim)">latest</span>{ic("cd",12)}</button></div></div>
<div class="dsec"><dl class="kv" style="margin:0"><dt>App version</dt><dd class="mono" style="font-size:11.5px">1.1.0</dd><dt>Versions</dt><dd>0.2.0 · 0.1.0</dd><dt>Type</dt><dd>application</dd><dt>Home</dt><dd><a href="#">kubyl.dev</a></dd><dt>Maintainers</dt><dd>Kubyl</dd><dt>Keywords</dt><dd><span class="chip">kubyl</span> <span class="chip">demo</span></dd><dt>CRDs</dt><dd class="mono" style="font-size:11.5px">widgets.demo.kubyl.dev</dd><dt>Values schema</dt><dd style="color:var(--green)">yes</dd></dl></div>
<div style="display:flex;gap:2px;padding:0 8px;border-bottom:1px solid var(--bv);height:32px;align-items:stretch">{"".join(f'<span style="display:flex;align-items:center;padding:0 10px;{"color:var(--text);box-shadow:inset 0 -2px 0 var(--accent)" if t == "README" else "color:var(--dim)"}">{t}</span>' for t in ["README", "Default values", "CRDs"])}</div>
<div class="dsec" style="border-bottom:0">{readme}</div>
</aside>'''

def helm_tabs(on):
    return tabs([("anchor", "Charts", on == "charts"), ("blocks", "Helm Releases", on == "releases"), ("anchor", "kubyl-demo", on == "release")])

def helm_charts_screen():
    content = f'<div style="flex:1;display:flex;min-height:0">{helm_charts_center()}{helm_chart_details()}</div>'
    return page("Helm charts — Kubyl", op_shell("Helm", helm_tabs("charts"), content))

def helm_modal(width, title, lead, extra, body, footer, top=50):
    return f'''<div style="position:absolute;inset:0;background:rgba(15,17,21,.55);display:flex;align-items:flex-start;justify-content:center;padding-top:{top}px">
<div role="dialog" aria-label="{title}" style="width:{width}px;background:#2f343e;border:1px solid var(--border);border-radius:10px;box-shadow:0 20px 60px rgba(0,0,0,.5);overflow:hidden">
<div style="display:flex;align-items:center;gap:10px;padding:14px 16px;border-bottom:1px solid var(--bv)">{lead}<b style="font-weight:600">{title}</b><span style="flex:1"></span>{extra}</div>
{body}
<div style="display:flex;align-items:center;gap:8px;padding:12px 16px;border-top:1px solid var(--bv)">{footer}</div>
</div></div>'''

def helm_steps(current):
    out = []
    for i, s in enumerate(["Configure", "Preview", "Install"]):
        on = i == current
        done = i < current
        col = C["accent"] if on else (C["green"] if done else C["dim"])
        out.append(f'<span style="display:flex;gap:6px;align-items:center;font-size:12px;color:{col}"><span style="width:16px;height:16px;border-radius:50%;border:1px solid {col};display:flex;align-items:center;justify-content:center;font-size:10px">{i + 1}</span>{s}</span>')
    return '<span style="display:flex;gap:14px">' + f'<span style="color:var(--faint)">›</span>'.join(out) + '</span>'

def values_rows(lines, start=1, marks=None):
    marks = marks or {}
    return "".join(f'<div class="mono" style="display:flex;height:20px;align-items:center;font-size:12.5px;white-space:pre"><span style="width:38px;text-align:right;padding-right:10px;color:var(--faint)">{i + start}</span><span style="width:3px;height:20px;background:{marks.get(i, "transparent")}"></span><span style="padding-left:10px">{t}</span></div>' for i, t in enumerate(lines))

def helm_install_screen():
    N = C["orange"]
    com = lambda t: f'<span style="color:var(--dim);font-style:italic"># {t}</span>'
    L = [com("How many pods run."), yl("replicaCount", 0, "2", N), yl("image"), yl("repository", 2, "registry.k8s.io/pause"), yl("tag", 2, '"3.10"'), yl("service"), yl("port", 2, "80", N),
         com("Shown in the ConfigMap."), yl("greeting", 0, "hello from Kubyl"), yl("auth"), "  " + com("Rendered into a Secret."), yl("password", 2, "s3cret-pass"), yl("storage", 0, '<span style="text-decoration:underline wavy #e06c75">5Mi</span>')]
    editor = f'''<div style="border:1px solid var(--border);border-radius:6px;overflow:hidden;background:var(--bg)">
<div style="display:flex;gap:6px;align-items:center;padding:0 8px;height:30px;border-bottom:1px solid var(--bv);font-size:12px"><span class="chip on" style="height:20px">Chart defaults</span><span class="chip" style="height:20px">Override only</span><span style="flex:1"></span><span style="color:var(--dim);display:flex;gap:5px;align-items:center">{ic("ok",12,C["green"])}values.schema.json</span></div>
<div style="padding:4px 0;height:262px;overflow:hidden">{values_rows(L, marks={1: C["accent"], 8: C["accent"], 11: C["accent"]})}</div>
<div style="display:flex;gap:6px;align-items:center;padding:4px 10px;border-top:1px solid var(--bv);font-size:11.5px;color:var(--red)">{ic("err",12,C["red"])}storage: Unsupported value “5Mi”: supported values are “1Mi”, “2Mi”</div></div>'''
    opts = (f'<div style="display:grid;grid-template-columns:1fr 1fr;gap:8px 16px">{check(True, "Roll back on failure", "--atomic (Helm 4: --rollback-on-failure)")}{check(False, "Wait until ready", "--wait, up to the timeout")}'
            f'{check(False, "Skip CRDs", "1 CRD in crds/: widgets.demo.kubyl.dev")}<div style="display:flex;gap:8px;align-items:center;font-size:12.5px">Timeout<div class="inp" style="width:80px;height:24px"><span class="mono" style="font-size:12px;color:var(--text)">5m0s</span></div></div></div>')
    left = f'''<div style="width:380px;flex-shrink:0;padding:16px;display:flex;flex-direction:column;gap:12px;border-right:1px solid var(--bv)">
{form_row("Chart", '<span class="mono" style="font-size:12.5px">kubyl-dev/kubyl-demo</span>')}
{form_row("Version", f'<button class="btn" style="width:100%;justify-content:space-between"><span class="mono" style="font-size:12px">0.2.0</span><span style="color:var(--dim)">latest · app 1.1.0</span>{ic("cd",12)}</button>')}
{form_row("Release name", '<div class="inp focus" style="height:28px"><span class="mono" style="font-size:12.5px;color:var(--text)">web</span></div>')}
{form_row("Namespace", f'<div class="inp" style="height:28px;justify-content:space-between"><span class="mono" style="font-size:12.5px;color:var(--text)">shop</span>{ic("cd",12)}</div><div style="font-size:11.5px;color:var(--dim);margin-top:5px">{check(True, "Create it (--create-namespace)")}</div>')}
{form_row("Description", '<div class="inp" style="height:28px"><span style="color:var(--dim)">optional</span></div>')}
<div><p class="dtitle">Options</p>{opts}</div></div>'''
    body = f'<div style="display:flex;height:470px">{left}<div style="flex:1;min-width:0;padding:16px;display:flex;flex-direction:column;gap:8px"><p class="dtitle" style="margin:0">Values</p>{editor}<div style="font-size:11.5px;color:var(--dim);line-height:16px">Values go to helm on stdin, never into a file. They can hold passwords: Kubyl keeps them in this dialog only.</div></div></div>'
    footer = f'<span style="font-size:12px;color:var(--dim)">Into kind-kubyl-dev · helm 4.3.0</span><span style="flex:1"></span><button class="btn g">Cancel</button><button class="btn p" style="opacity:.55">{ic("eye",12,"#1b1e24")}Preview</button>'
    modal = helm_modal(1000, "Install kubyl-demo", op_tile("KD", C["accent"]), helm_steps(0), body, footer)
    content = f'<div style="flex:1;display:flex;min-height:0">{helm_charts_center()}</div>'
    return page("Install a chart — Kubyl", op_shell("Helm", helm_tabs("charts"), content, modal))

def helm_object_list(items, selected):
    tag = {"+": (C["green"], "added"), "~": (C["yellow"], "changed"), "-": (C["red"], "removed"), "=": (C["dim"], "")}
    out = []
    for i, (t, kind, name) in enumerate(items):
        col, word = tag[t]
        on = i == selected
        out.append(f'<div style="display:flex;gap:8px;align-items:center;padding:5px 8px;border-radius:5px;font-size:12px;{"background:var(--sel);outline:1px solid var(--accent);outline-offset:-1px" if on else ""}"><span class="mono" style="width:10px;color:{col};font-weight:600">{t if t != "=" else ""}</span><span style="color:var(--dim);width:86px;flex-shrink:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{kind}</span><span class="mono" style="flex:1;font-size:11.5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{name}</span><span style="font-size:11px;color:{col}">{word}</span></div>')
    return "".join(out)

def helm_install_preview_screen():
    groups = [("Deployment", ["web"]), ("Service", ["web"]), ("ConfigMap", ["web-settings"]), ("Secret", ["web-auth"]), ("PersistentVolumeClaim", ["web-data"])]
    glist = "".join(f'<div style="padding:4px 8px"><div style="font-size:11px;color:var(--dim);text-transform:uppercase;letter-spacing:.05em">{k} · {len(ns)}</div>' + "".join(f'<div class="mono" style="font-size:12px;padding:3px 0 3px 8px;{"background:var(--sel);border-radius:4px" if k == "Secret" else ""}">{n}</div>' for n in ns) + '</div>' for k, ns in groups)
    M = '<span style="color:var(--dim)">••••••••</span>'
    L = [f'<span style="color:var(--dim);font-style:italic"># Source: kubyl-demo/templates/secret.yaml</span>', yl("apiVersion", 0, "v1"), yl("kind", 0, "Secret"), yl("metadata"), yl("name", 2, "web-auth"), yl("namespace", 2, "shop"), yl("type", 0, "Opaque"), yl("stringData"), yl("password", 2, M)]
    right = f'''<div style="width:300px;flex-shrink:0;border-left:1px solid var(--bv);padding:12px 14px;display:flex;flex-direction:column;gap:12px">
<div><p class="dtitle">Hooks that run</p>{op_chg("zap",C["accent"],f'Job <span {MONO11}>web-migrate</span> · post-install, post-upgrade')}</div>
<div><p class="dtitle">CRDs (crds/)</p>{op_chg("file",C["yellow"],f'<span {MONO11}>widgets.demo.kubyl.dev</span> is installed once; Helm never upgrades or deletes it')}</div>
<div><p class="dtitle">Namespace</p>{op_chg("plus",C["green"],f'<span {MONO11}>shop</span> is created (--create-namespace)')}</div>
<div><p class="dtitle">Notes</p><div class="mono" style="font-size:11.5px;color:var(--muted);line-height:17px">kubyl-demo 0.2.0 is installed as web in shop.</div></div></div>'''
    body = f'''<div style="display:flex;align-items:center;gap:8px;padding:8px 16px;border-bottom:1px solid var(--bv);font-size:12.5px;background:#2b3a2f">{ic("ok",13,C["green"])}<span>Server-side dry run passed: 5 objects in 5 kinds, 1 hook, 1 CRD. Nothing was applied.</span></div>
<div style="display:flex;height:420px"><div style="width:270px;flex-shrink:0;border-right:1px solid var(--bv);padding:8px;overflow:hidden">{glist}</div>
<div style="flex:1;min-width:0;display:flex;flex-direction:column"><div style="display:flex;align-items:center;gap:8px;height:32px;padding:0 12px;border-bottom:1px solid var(--bv);font-size:12px"><span class="mono">Secret web-auth</span><span style="flex:1"></span><span style="display:flex;gap:5px;align-items:center;color:var(--dim)">{ic("lock",12)}Secret data is masked</span></div><div style="padding:6px 0">{values_rows(L)}</div></div>{right}</div>'''
    footer = f'<span style="font-size:12px;color:var(--muted)">Type <span class="mono" style="color:var(--text)">kind-kubyl-dev</span> to install on a production cluster</span><div class="inp focus" style="width:180px;height:28px"><span class="mono" style="font-size:12.5px;color:var(--text)">kind-kubyl</span></div><span style="flex:1"></span><button class="btn g">{ic("left",12)}Back</button><button class="btn p" style="opacity:.55">{ic("download",12,"#1b1e24")}Install</button>'
    modal = helm_modal(1000, "Install kubyl-demo as web", op_tile("KD", C["accent"]), helm_steps(1) + '<span class="prod" style="font-size:9.5px;padding:0 4px;margin-left:10px">PROD</span>', body, footer)
    content = f'<div style="flex:1;display:flex;min-height:0">{helm_charts_center()}</div>'
    return page("Install preview — Kubyl", op_shell("Helm", helm_tabs("charts"), content, modal))

def helm_release_header(status="deployed", buttons=True):
    btns = (f'<button class="btn p" style="height:26px">{ic("up",12,"#1b1e24")}Upgrade…</button><button class="btn" style="height:26px">{ic("rollback",12)}Roll back…</button><button class="btn d" style="height:26px">{ic("trash",12,C["red"])}Uninstall…</button>' if buttons else "")
    return (f'<div style="display:flex;align-items:center;gap:10px;padding:0 12px 0 16px;height:44px;flex-shrink:0;border-bottom:1px solid var(--bv)">{ic("anchor",15,C["accent"])}<span class="mono" style="font-size:14px;font-weight:600">web</span><span style="color:var(--dim)">shop</span>{helm_status(status)}'
            f'<span class="mono" style="font-size:12px;color:var(--muted)">kubyl-demo-0.1.0 · app 1.0.0 · revision</span><button class="btn" style="height:24px;padding:0 8px"><span class="mono" style="font-size:12px">3</span>{ic("cd",11)}</button><div style="flex:1"></div>{btns}<button class="btn g" style="height:26px">{ic("zap",12)}Ask agent</button><button class="btn g" style="height:26px;padding:0 6px">{ic("copy",12)}{ic("cd",11)}</button></div>')

def helm_release_bg(status="deployed", buttons=True):
    sub = "".join(f'<span style="display:flex;align-items:center;gap:6px;padding:0 10px;{"color:var(--text);box-shadow:inset 0 -2px 0 var(--accent)" if t == "History" else "color:var(--dim)"}">{t}</span>' for t in ["Values", "Manifest", "Notes", "History", "Resources"])
    return f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">{helm_release_header(status, buttons)}
<div style="display:flex;gap:2px;padding:0 8px;border-bottom:1px solid var(--bv);height:34px;align-items:stretch;flex-shrink:0">{sub}</div><div style="flex:1"></div>
{hints([("1–5","Tabs"),("u","Upgrade…"),("b","Roll back…"),("⌃d","Uninstall…"),("c","Copy helm command")])}</div>'''

def helm_upgrade_screen():
    items = [("~", "Deployment", "web"), ("~", "ConfigMap", "web-settings"), ("~", "Secret", "web-auth"), ("+", "Job", "web-migrate (hook)"), ("+", "PersistentVolumeClaim", "web-data"), ("=", "Service", "web")]
    lines = [("@", "   spec"), (" ", "   replicas:"), ("-", "   replicas: 1"), ("+", "   replicas: 2"), (" ", "   selector:"), (" ", "     matchLabels:"), (" ", "       app.kubernetes.io/instance: web"), ("@", "   spec.template.spec.containers[0]"), (" ", "       - name: app"), ("-", '         image: "registry.k8s.io/pause:3.9"'), ("+", '         image: "registry.k8s.io/pause:3.10"')]
    dl = "".join(diff_line(k, t) for k, t in lines)
    M = "••••••••"
    vlines = [(" ", "auth:"), (" ", f"  password: {M}"), ("-", f"greeting: {M}"), ("+", f"greeting: {M}"), ("-", f"replicaCount: {M}"), ("+", f"replicaCount: {M}"), ("+", f"storage: {M}")]
    vl = "".join(diff_line(k, t) for k, t in vlines)
    body = f'''<div style="display:flex;align-items:center;gap:8px;padding:8px 16px;border-bottom:1px solid var(--bv);font-size:12.5px;background:#35322a">{ic("alert",13,C["yellow"])}<span>The chart's CRD <span class="mono" style="font-size:11.5px">widgets.demo.kubyl.dev</span> (crds/) isn't applied by an upgrade: Helm only installs CRDs. Hook <span class="mono" style="font-size:11.5px">web-migrate</span> runs post-upgrade.</span></div>
<div style="display:flex;height:430px">
<div style="width:280px;flex-shrink:0;border-right:1px solid var(--bv);padding:8px;display:flex;flex-direction:column;gap:2px"><p class="dtitle" style="padding-left:6px">Objects · 3 changed · 2 added · 1 unchanged</p>{helm_object_list(items, 0)}
<div style="flex:1"></div><div style="font-size:11px;color:var(--dim);padding:0 6px;line-height:16px">Rendered by a server-side dry run of 0.2.0, compared with revision 3's manifest. Secret data changes are reported, not shown.</div></div>
<div style="flex:1;min-width:0;display:flex;flex-direction:column">
<div style="display:flex;align-items:center;gap:8px;height:34px;padding:0 12px;border-bottom:1px solid var(--bv);font-size:12px"><span class="chip on" style="height:20px">Manifest</span><span class="chip" style="height:20px">Values · 3 changes</span><span style="flex:1"></span><span class="mono">Deployment web</span><span style="color:var(--dim)">· revision 3 → new</span><span class="chip" style="height:18px">side by side</span></div>
<div style="flex:1;overflow:hidden;padding:4px 0">{dl}</div>
<div style="border-top:1px solid var(--bv);padding:6px 0"><div style="display:flex;gap:8px;align-items:center;padding:0 12px 4px;font-size:11.5px;color:var(--dim)">{ic("lock",12)}Values diff (masked)<span style="flex:1"></span><button class="btn g" style="height:22px">{ic("eye",11)}Reveal</button></div>{vl}</div></div></div>'''
    footer = f'<span style="font-size:12px;color:var(--dim)">kubyl-dev/kubyl-demo 0.1.0 → <span style="color:var(--green)">0.2.0</span> · values: edited (--reset-values) · --atomic</span><span style="flex:1"></span><button class="btn g">{ic("left",12)}Back</button><button class="btn p">{ic("up",12,"#1b1e24")}Upgrade</button>'
    modal = helm_modal(1080, "Upgrade web", ic("up", 16, C["accent"]), '<span style="display:flex;gap:14px">' + '<span style="font-size:12px;color:var(--green)">1 Configure</span><span style="color:var(--faint)">›</span><span style="font-size:12px;color:var(--accent)">2 Review</span><span style="color:var(--faint)">›</span><span style="font-size:12px;color:var(--dim)">3 Upgrade</span></span>', body, footer, top=36)
    return page("Upgrade review — Kubyl", op_shell("Helm", helm_tabs("release"), helm_release_bg(), modal))

def helm_rollback_screen():
    revs = [("3", "deployed", "Upgrade complete", "kubyl-demo-0.1.0", "2m", False), ("2", "superseded", "Upgrade complete", "kubyl-demo-0.1.0", "1h", True), ("1", "superseded", "Install complete", "kubyl-demo-0.1.0", "1h", False)]
    rl = "".join(f'<div style="display:grid;grid-template-columns:16px 26px 100px minmax(0,1fr) 40px;gap:8px;align-items:center;padding:7px 10px;border-radius:6px;font-size:12px;{"background:var(--sel);outline:1px solid var(--accent);outline-offset:-1px" if on else ""}"><span style="width:12px;height:12px;border-radius:50%;border:1px solid {C["accent"] if on else C["faint"]};display:flex;align-items:center;justify-content:center">{"<span style=\"width:6px;height:6px;border-radius:50%;background:var(--accent)\"></span>" if on else ""}</span><span class="mono">{r}</span>{helm_status(s)}<span style="color:var(--dim);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{d} · {c}</span><span class="mono" style="color:var(--dim)">{a}</span></div>' for r, s, d, c, a, on in revs)
    items = [("~", "ConfigMap", "web-settings"), ("=", "Deployment", "web"), ("=", "Secret", "web-auth"), ("=", "Service", "web")]
    lines = [("@", "   data"), ("-", '   greeting: "hi"'), ("+", '   greeting: "hello"')]
    dl = "".join(diff_line(k, t) for k, t in lines)
    opts = f'<div style="display:flex;flex-direction:column;gap:7px">{check(False, "Wait until ready")}{check(True, "Clean up on failure", "--cleanup-on-fail")}{check(False, "No hooks", "--no-hooks")}</div>'
    body = f'''<div style="display:flex;height:390px">
<div style="width:360px;flex-shrink:0;border-right:1px solid var(--bv);padding:12px;display:flex;flex-direction:column;gap:4px"><p class="dtitle" style="padding-left:4px">Roll back to</p>{rl}<div style="flex:1"></div><p class="dtitle" style="padding-left:4px">Options</p>{opts}</div>
<div style="flex:1;min-width:0;display:flex;flex-direction:column"><div style="display:flex;align-items:center;gap:8px;height:34px;padding:0 12px;border-bottom:1px solid var(--bv);font-size:12px"><span>revision 3 → 2</span><span style="color:var(--dim)">· 1 changed · 3 unchanged · values: 1 change</span></div>
<div style="display:flex;flex:1;min-height:0"><div style="width:230px;flex-shrink:0;border-right:1px solid var(--bv);padding:8px">{helm_object_list(items, 0)}</div><div style="flex:1;min-width:0;padding:4px 0">{dl}</div></div></div></div>'''
    footer = f'<span style="font-size:12px;color:var(--dim)">Creates revision 4 with revision 2\'s manifest and values</span><span style="flex:1"></span><button class="btn g">Cancel</button><button class="btn p">{ic("rollback",12,"#1b1e24")}Roll back to 2</button>'
    modal = helm_modal(900, "Roll back web", ic("rollback", 16, C["accent"]), '<span style="font-size:12px;color:var(--dim)">shop · kind-kubyl-dev</span>', body, footer, top=60)
    return page("Roll back a release — Kubyl", op_shell("Helm", helm_tabs("release"), helm_release_bg(), modal))

def helm_uninstall_screen():
    gone = [("Deployment", "web"), ("Service", "web"), ("ConfigMap", "web-settings"), ("Secret", "web-auth"), ("Secret", "sh.helm.release.v1.web.v1–v3 (history)")]
    gl = "".join(f'<div style="display:flex;gap:8px;align-items:center;font-size:12px;padding:4px 0;border-bottom:1px solid var(--bv)">{ic("minus",12,C["red"])}<span style="width:120px;color:var(--dim)">{k}</span><span class="mono" style="font-size:11.5px">{n}</span></div>' for k, n in gone)
    stay = [("PersistentVolumeClaim web-data", "helm.sh/resource-policy: keep"), ("CustomResourceDefinition widgets.demo.kubyl.dev", "installed from crds/: Helm never deletes it"), ("PersistentVolumeClaims data-db-*", "created by the StatefulSet, not by Helm")]
    sl = "".join(f'<div style="display:flex;gap:8px;align-items:flex-start;font-size:12px;padding:4px 0;border-bottom:1px solid var(--bv)">{ic("lock",12,C["yellow"])}<div><div class="mono" style="font-size:11.5px">{n}</div><div style="color:var(--dim);font-size:11.5px">{w}</div></div></div>' for n, w in stay)
    body = f'''<div style="padding:14px 16px;display:flex;flex-direction:column;gap:14px">
<div style="display:grid;grid-template-columns:1fr 1fr;gap:18px"><div><p class="dtitle">Deleted · 4 objects</p>{gl}</div><div><p class="dtitle">Stays</p>{sl}</div></div>
<div><p class="dtitle">Hooks</p>{op_chg("zap",C["accent"],f'Job <span {MONO11}>web-cleanup</span> runs pre-delete')}</div>
<div style="display:flex;gap:18px">{check(False, "Keep history", "--keep-history: rollback stays possible")}{check(False, "No hooks", "--no-hooks")}{check(False, "Wait", "until the objects are gone")}</div>
<div style="display:flex;flex-direction:column;gap:6px"><span style="font-size:12px;color:var(--muted)">Type <span class="mono" style="color:var(--text)">web</span> to uninstall on a production cluster</span><div class="inp focus" style="height:28px"><span class="mono" style="font-size:12.5px;color:var(--text)">we</span></div></div></div>'''
    footer = f'<span style="font-size:12px;color:var(--dim)">helm uninstall web -n shop</span><span style="flex:1"></span><button class="btn g">Cancel</button><button class="btn" style="background:#5c2b30;border-color:#7a3a40;color:#f2b8bd;opacity:.6">{ic("trash",12,"#f2b8bd")}Uninstall</button>'
    modal = helm_modal(760, "Uninstall web", ic("trash", 16, C["red"]), '<span class="prod" style="font-size:9.5px;padding:0 4px">PROD</span>', body, footer, top=80)
    return page("Uninstall a release — Kubyl", op_shell("Helm", helm_tabs("release"), helm_release_bg(), modal))

def helm_repos_screen():
    repos = [("kubyl-dev", "http://127.0.0.1:8879"), ("argo", "https://argoproj.github.io/argo-helm"), ("bitnami", "https://charts.bitnami.com/bitnami"), ("ingress-nginx", "https://kubernetes.github.io/ingress-nginx"), ("jetstack", "https://charts.jetstack.io"), ("prometheus-community", "https://prometheus-community.github.io/helm-charts")]
    rl = "".join(f'<div style="display:grid;grid-template-columns:150px minmax(0,1fr) 54px 28px;gap:8px;align-items:center;font-size:12px;padding:6px 4px;border-bottom:1px solid var(--bv)"><span class="mono">{n}</span><span class="mono" style="font-size:11.5px;color:var(--muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{u}</span><button class="btn g" style="height:22px">{ic("refresh",11)}</button><button class="btn g" style="height:22px;padding:0 4px">{ic("trash",11,C["red"])}</button></div>' for n, u in repos)
    inp = lambda v, ph=False, mono=True: f'<div class="inp" style="height:26px"><span class="{"mono" if mono else ""}" style="font-size:12px;color:{"var(--dim)" if ph else "var(--text)"}">{v}</span></div>'
    add = f'''<div style="display:flex;flex-direction:column;gap:8px">
{form_row("Name", inp("private"))}{form_row("URL", inp("https://charts.example.com/stable"))}
{form_row("User name", inp("optional", True, False))}{form_row("Password", inp("••••••••"))}
{form_row("CA file", inp("optional, PEM", True, False))}{form_row("Client cert", inp("optional: certificate and key files", True, False))}
<div style="display:flex;gap:8px;align-items:flex-start;padding:8px 10px;border-radius:6px;background:#35322a;font-size:11.5px;line-height:16px;color:var(--muted)">{ic("alert",13,C["yellow"])}<span>Helm stores a repository's user name and password in plain text in <span class="mono">~/Library/Preferences/helm/repositories.yaml</span>. Kubyl keeps no copy. Prefer tokens scoped to reading charts.</span></div>
<div style="display:flex;justify-content:flex-end"><button class="btn p">{ic("plus",12,"#1b1e24")}Add repository</button></div></div>'''
    oci = f'''<div style="display:flex;flex-direction:column;gap:8px">{form_row("Registry", inp("ghcr.io"))}{form_row("User name", inp("octocat"))}{form_row("Password", inp("••••••••"))}
<div style="font-size:11.5px;color:var(--dim);line-height:16px">helm registry login: the login goes to Docker's credential store (or Helm's registry config.json). Type oci:// references into the Charts search.</div>
<div style="display:flex;justify-content:flex-end"><button class="btn">{ic("key",12)}Log in</button></div></div>'''
    body = f'''<div style="display:flex;height:440px"><div style="flex:1;min-width:0;padding:12px 16px;border-right:1px solid var(--bv);display:flex;flex-direction:column"><div style="display:flex;align-items:center;margin-bottom:6px"><p class="dtitle" style="margin:0;flex:1">Repositories · 6</p><button class="btn g" style="height:24px">{ic("refresh",12)}Update all</button></div>{rl}
<div style="flex:1"></div><div style="font-size:11px;color:var(--dim)">The same list as helm repo list (HELM_REPOSITORY_CONFIG).</div></div>
<div style="width:400px;flex-shrink:0;padding:12px 16px;display:flex;flex-direction:column;gap:10px"><div style="display:flex;gap:6px"><span class="chip on" style="height:22px;padding:0 10px">HTTP repository</span><span class="chip" style="height:22px;padding:0 10px">OCI registry login</span></div>{add}</div></div>'''
    footer = '<span style="flex:1"></span><button class="btn g">Close</button>'
    modal = helm_modal(940, "Helm repositories", ic("gear", 16, C["accent"]), "", body, footer, top=60)
    content = f'<div style="flex:1;display:flex;min-height:0">{helm_charts_center()}</div>'
    return page("Helm repositories — Kubyl", op_shell("Helm", helm_tabs("charts"), content, modal))

def helm_missing_screen():
    missing = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0;border-right:1px solid var(--border)">
{tabs([("anchor", "Charts", True)], tools=False)}
<div style="flex:1;display:flex;flex-direction:column;gap:14px;padding:30px 34px">
<div style="display:flex;gap:12px;align-items:center">{ic("anchor",22,C["dim"])}<div><div style="font-size:16px;font-weight:600">helm isn't installed</div><div style="font-size:12.5px;color:var(--muted);margin-top:2px">Kubyl runs your <span class="mono" style="font-size:11.5px">helm</span> (3.13 or newer) for charts, installs and upgrades. It looked in your login shell's PATH.</div></div></div>
<div class="card" style="padding:10px 14px;display:flex;flex-direction:column;gap:8px;font-size:12.5px;color:var(--muted);line-height:19px">
<div style="display:flex;gap:8px;align-items:center"><div class="mono" style="flex:1;font-size:11.5px;padding:6px 8px;border-radius:5px;background:#23272e;color:var(--text)">brew install helm</div><button class="btn g" style="height:24px">{ic("copy",12)}</button></div>
<div>Or set <span class="mono" style="font-size:11.5px;color:var(--text)">helm.path</span> in settings.json. Helm releases stay visible without it (read-only), and their commands can be copied.</div></div>
<div style="display:flex;gap:8px"><button class="btn">{ic("refresh",13)}Check again</button><button class="btn g">{ic("ext",13)}helm.sh/docs/intro/install</button></div>
</div></div>'''
    stuck = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div style="display:flex;align-items:center;gap:10px;padding:0 12px 0 16px;height:44px;flex-shrink:0;border-bottom:1px solid var(--bv)">{ic("anchor",15,C["accent"])}<span class="mono" style="font-size:14px;font-weight:600">kubyl-stuck</span><span style="color:var(--dim)">kubyl-helm</span>{helm_status("pending-upgrade")}<div style="flex:1"></div><button class="btn" style="height:26px">{ic("rollback",12)}Roll back…</button></div>
<div style="display:flex;gap:10px;align-items:flex-start;padding:10px 14px;background:#35322a;border-bottom:1px solid #5a4f33;font-size:12.5px;line-height:18px">{ic("alert",14,C["yellow"])}<div style="flex:1"><b style="font-weight:600">The release is stuck in pending-upgrade</b><div style="color:var(--muted)">No helm is running on it: an upgrade was interrupted. Helm refuses new upgrades ("another operation is in progress") until you roll back to revision 1, the last deployed one.</div></div><button class="btn p" style="height:24px">{ic("rollback",12,"#1b1e24")}Roll back to 1…</button></div>
<div style="padding:12px 16px"><p class="dtitle">An upgrade in progress (the dialog closed)</p>
<div class="card" style="padding:10px 12px;display:flex;flex-direction:column;gap:6px"><div style="display:flex;gap:8px;align-items:center;font-size:12.5px">{ic("refresh",13,C["accent"])}<b style="font-weight:500">Upgrading web in shop</b><span style="color:var(--dim)">· 1m 12s · --wait 5m0s</span><span style="flex:1"></span><button class="btn g" style="height:22px">Cancel</button></div>
<div class="mono" style="font-size:11.5px;color:var(--dim);line-height:17px">Pulled: localhost:5022/charts/kubyl-demo:0.2.0<br>beginning wait for 6 resources with timeout of 5m0s<br>Deployment is not ready: shop/web. 1 out of 2 expected pods are ready</div></div></div>
</div>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{missing}{stuck}</div>'
    return page("helm isn't installed, a stuck release — Kubyl", op_shell("Helm", "", content))
# ---------- 21. Flux (phase 23) ----------
FLUX_STATE = {"Ready": C["green"], "Reconciling": C["accent"], "Failed": C["red"], "Stalled": C["orange"],
              "Suspended": C["purple"], "Unknown": C["dim"]}

def fpill(state, label=None):
    c = FLUX_STATE.get(state, C["muted"])
    return f'<span class="pill">{dot(c)}<span style="color:{c}">{label or state}</span></span>'

def flux_sidebar(active, workloads=False):
    a = lambda n: n == active
    rows = [
        f'<div class="phead"><span style="flex:1;font-weight:500;color:var(--text)">Explorer</span><button class="ib" aria-label="Filter kinds">{ic("search",13)}</button><button class="ib" aria-label="Add kubeconfig">{ic("plus",14)}</button><button class="ib" aria-label="More">{ic("more",14)}</button></div>',
        f'<div class="sec">{ic("cr",11)}Favorites<span style="flex:1"></span><span style="font-weight:400;letter-spacing:0;text-transform:none;color:var(--faint)">4</span></div>',
        f'<div class="sec">{ic("cd",11)}Clusters</div>',
        root("prod-eu-west-1", "on", True, C["red"], prod=True),
        ti("Overview", 1, "gauge"),
        ti("Events", 1, "bell", "23", color=C["yellow"]),
        ti("Workloads", 1, open_=workloads),
        *([ti("Pods", 2, "box", "17"), ti("Deployments", 2, "layers", "9", on=a("Deployments")), ti("StatefulSets", 2, "db", "2")] if workloads else []),
        ti("Network", 1, open_=False),
        ti("Config &amp; Secrets", 1, open_=False),
        ti("Storage", 1, open_=False),
        ti("Access Control", 1, open_=False),
        ti("Cluster", 1, open_=False),
        ti("Administration", 1, open_=True),
        ti("Helm Releases", 2, "anchor"),
        ti("Cluster Updates", 2, "up"),
        ti("Flux", 2, open_=True, extra=f'<span style="font-size:11px;color:var(--dim)">v2.9.6</span>'),
        ti("Overview", 3, "gauge", on=a("Overview")),
        ti("Kustomizations", 3, "layers", "6", on=a("Kustomizations"), extra=f'<span style="margin-right:2px">{dot(C["red"])}</span>'),
        ti("HelmReleases", 3, "anchor", "2", on=a("HelmReleases")),
        ti("Sources", 3, "branch", on=a("Sources")),
        ti("Image Automation", 3, "box", on=a("Images")),
        ti("Notifications", 3, "bell", on=a("Notifications")),
        ti("Custom Resources", 1, open_=True),
        ti("kustomize.toolkit.fluxcd.io", 2, open_=False),
        ti("source.toolkit.fluxcd.io", 2, open_=False),
        ti('<span style="color:var(--dim)">12 more API groups…</span>', 2),
        root("staging-eu-west-1", "on", color=C["yellow"]),
        root("gke-analytics", None, color=C["cyan"]),
    ]
    return '<aside class="side">' + "\n".join(rows) + '</aside>'

def flux_shell(active, tabbar, content, overlay="", workloads=False):
    return f'''<div class="app">
{titlebar()}
<div class="body">
{flux_sidebar(active, workloads)}
<main class="main">
{tabbar}
{content}
</main>
</div>
{statusbar(right_extra=f'<span>{ic("layers",12,C["accent"])}Flux v2.9.6</span>')}
{overlay}
</div>'''

def fsec(title, body, last=False):
    return f'<div class="dsec"{" style=\"border-bottom:0\"" if last else ""}><p class="dtitle">{title}</p>{body}</div>'

def fbox(color, icon, title, text):
    return (f'<div style="display:flex;gap:10px;padding:10px 12px;border-radius:7px;background:{color}1a;border:1px solid {color}59">{ic(icon,15,color)}'
            f'<div style="flex:1;min-width:0"><div style="font-size:12.5px;font-weight:600;color:{color}">{title}</div><div style="font-size:12px;color:var(--muted);line-height:17px">{text}</div></div></div>')

FLUX_KS = [
 # name, ns, state, message, source, revision, suspended, interval, last, age
 ("apps", "flux-demo", "Ready", "Applied revision: master@sha1:3e0ff8a", "GitRepository/podinfo", "master@sha1:3e0ff8a", False, "10m", "4m", "2d"),
 ("apps-late", "flux-demo", "Failed", "dependency 'flux-demo/broken' is not ready", "GitRepository/podinfo", "", False, "10m", "1m", "2d"),
 ("broken", "flux-demo", "Failed", "kustomization path not found: stat /tmp/kustomization-1709…/does-not-exist", "GitRepository/podinfo", "", False, "5m", "1m", "2d"),
 ("infra", "flux-demo", "Ready", "Applied revision: master@sha1:3e0ff8a", "GitRepository/podinfo", "master@sha1:3e0ff8a", False, "10m", "4m", "2d"),
 ("paused", "flux-demo", "Suspended", "Applied revision: master@sha1:3e0ff8a", "GitRepository/podinfo", "master@sha1:3e0ff8a", True, "10m", "6d", "8d"),
 ("podinfo", "flux-demo", "Reconciling", "Reconcile requested; waiting for the controller.", "GitRepository/podinfo", "master@sha1:3e0ff8a", False, "10m", "now", "2d"),
]
KC = "grid-template-columns: minmax(0,1fr) 84px minmax(0,1.6fr) minmax(0,1fr) 140px 58px 50px 44px"

def flux_toolbar(label, icon, counts, kinds=False):
    kind = f'<button class="btn g" style="height:24px;padding:0 6px">All kinds{ic("cd",11)}</button>' if kinds else ""
    return f'''<div class="tool">
<div class="crumb" style="white-space:nowrap">{ic(icon,14,C["accent"])}<b>{label}</b><span>·</span>{counts}</div>
<div style="flex:1"></div>{kind}
<button class="btn g" style="height:24px;padding:0 6px">All namespaces{ic("cd",11)}</button>
<div class="inp" style="width:150px">{ic("filter",12)}Filter</div>
<span class="chip" style="color:var(--green)">{dot(C["green"])}live</span>
</div>'''

def flux_chips(items, extra=""):
    chips = "".join(f'<span class="chip{" on" if on else ""}">{dot(FLUX_STATE[s])}{s}<span class="mono" style="font-size:11px;color:var(--dim)">{n}</span></span>' for s, n, on in items)
    return f'<div style="height:36px;flex-shrink:0;display:flex;align-items:center;gap:6px;padding:0 12px;border-bottom:1px solid var(--bv)">{chips}<span style="flex:1"></span>{extra}</div>'

def flux_ks_rows(selected=(1,)):
    out = []
    for i, (n, ns, s, msg, src, rev, susp, iv, last, age) in enumerate(FLUX_KS):
        pause = ic("pause", 11, C["purple"]) if susp else ""
        mc = "var(--muted)" if s in ("Failed", "Stalled") else "var(--dim)"
        out.append(f'''<div class="tr{" on" if i in selected else ""}" style="{KC};height:32px">
<span class="mono" style="font-size:12px;display:flex;gap:5px;align-items:center">{n}{pause}</span><span style="color:var(--muted)">{ns}</span>
<span style="display:flex;gap:8px;align-items:center;min-width:0">{fpill(s)}<span style="font-size:12px;color:{mc};overflow:hidden;text-overflow:ellipsis">{msg}</span></span>
<span class="mono" style="font-size:11.5px">{src}</span><span class="mono" style="font-size:11.5px">{rev}</span>
<span class="mono" style="font-size:11.5px;color:var(--muted)">{iv}</span><span class="mono" style="font-size:11.5px;color:var(--muted)">{last}</span><span class="mono" style="font-size:11.5px;color:var(--muted)">{age}</span></div>''')
    return "".join(out)

KS_HEAD = f'<div class="th" style="{KC}"><span>NAME {ic("cd",10)}</span><span>NAMESPACE</span><span>STATE</span><span>SOURCE</span><span>REVISION</span><span>INTERVAL</span><span>LAST</span><span>AGE</span></div>'
KS_HINTS = [("↵","Open"),("r","Reconcile"),("⇧r","With source"),("s","Suspend"),("u","Resume"),("l","Logs"),("e","Edit YAML"),("⌃d","Delete…"),("/","Filter")]
KS_COUNTS = '<span>6</span><span style="color:var(--red)">· 2 failing</span><span style="color:var(--purple)">· 1 suspended</span>'

def flux_overview_screen():
    tile = lambda icon, label, val, sub, pills: (f'<div class="card" style="flex:1;padding:12px;display:flex;flex-direction:column;gap:6px">'
        f'<div style="display:flex;gap:6px;align-items:center;font-size:12px;color:var(--dim)">{ic(icon,13)}{label}</div>'
        f'<div style="display:flex;align-items:flex-end;gap:6px"><span class="mono" style="font-size:22px">{val}</span><span style="font-size:12px;color:var(--dim);padding-bottom:3px">{sub}</span></div>'
        f'<div style="display:flex;gap:10px;font-size:11.5px">{pills}</div></div>')
    tiles = "".join([
        tile("layers", "Kustomizations", "3/6", "ready", fpill("Failed", "2 failing") + fpill("Suspended", "1 suspended")),
        tile("anchor", "HelmReleases", "1/2", "ready", fpill("Stalled", "1 failing")),
        tile("branch", "Sources", "5/6", "ready", fpill("Failed", "1 failing")),
        tile("box", "Image Automation", "2/3", "ready", fpill("Suspended", "1 suspended")),
        tile("bell", "Notifications", "3", "objects", fpill("Ready", "all good")),
    ])
    att = lambda icon, kind, name, why, col, msg: (f'<div style="padding:8px 12px;border-bottom:1px solid #2e333b;display:flex;flex-direction:column;gap:3px">'
        f'<div style="display:flex;gap:8px;align-items:center;font-size:12px">{ic(icon,13,C["dim"])}<span style="color:var(--dim)">{kind}</span><span class="mono">{name}</span><span style="flex:1"></span><span class="pill">{dot(col)}<span style="color:{col}">{why}</span></span></div>'
        f'<div style="padding-left:21px;font-size:12px;color:var(--muted);line-height:17px">{msg}</div></div>')
    attention = "".join([
        att("anchor", "HelmRelease", "data/redis", "Stalled", C["orange"], "Failed to install after 4 attempt(s): context deadline exceeded"),
        att("layers", "Kustomization", "flux-demo/broken", "Failed", C["red"], "kustomization path not found: stat /tmp/kustomization-1709002560/does-not-exist: no such file or directory"),
        att("branch", "GitRepository", "flux-system/infra", "Not fetched", C["red"], "failed to checkout and determine revision: authentication required"),
        att("layers", "Kustomization", "flux-demo/apps-late", "Waiting for flux-demo/broken", C["yellow"], "dependency 'flux-demo/broken' is not ready"),
        att("layers", "Kustomization", "flux-demo/paused", "Suspended for 6d", C["purple"], "Applied revision: master@sha1:3e0ff8a"),
    ])
    ev = lambda age, warn, kind, name, reason, msg: (f'<div style="display:flex;gap:8px;padding:6px 12px;border-bottom:1px solid #2e333b;font-size:12px;align-items:flex-start">'
        f'<span class="mono" style="width:30px;font-size:11.5px;color:var(--dim)">{age}</span>{ic("alert" if warn else "info",13,C["yellow"] if warn else C["dim"])}'
        f'<div style="flex:1;min-width:0"><div style="display:flex;gap:6px"><span style="color:var(--dim)">{kind}</span><span class="mono" style="font-size:11.5px">{name}</span><span style="color:{C["yellow"] if warn else C["muted"]}">{reason}</span></div>'
        f'<div style="color:var(--muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{msg}</div></div></div>')
    events = "".join([
        ev("12s", False, "Kustomization", "flux-demo/podinfo", "ReconciliationSucceeded", "Reconciliation finished in 170ms, next run in 10m0s"),
        ev("1m", True, "Kustomization", "flux-demo/broken", "ArtifactFailed", "kustomization path not found: stat /tmp/kustomization-1709…"),
        ev("1m", False, "Kustomization", "flux-demo/apps-late", "DependencyNotReady", "Dependencies do not meet ready condition, retrying in 30s"),
        ev("4m", False, "GitRepository", "flux-demo/podinfo", "NewArtifact", "stored artifact for commit 'Release v6.15.0'"),
        ev("9m", True, "HelmRelease", "data/redis", "InstallFailed", "Helm install failed for release data/redis with chart redis@19.6.4"),
        ev("14m", False, "HelmRelease", "flux-demo/podinfo-helm", "InstallSucceeded", "Helm install succeeded for release flux-demo/podinfo-helm.v1"),
        ev("21m", True, "GitRepository", "flux-system/infra", "GitOperationFailed", "failed to checkout and determine revision: authentication required"),
        ev("38m", False, "ImagePolicy", "flux-demo/podinfo", "Succeeded", "Latest image tag for ghcr.io/stefanprodan/podinfo resolved to 6.15.0"),
    ])
    ctrl = lambda n, v: f'<div style="display:flex;gap:10px;align-items:center;height:26px;padding:0 12px;border-bottom:1px solid #2e333b;font-size:12px">{dot(C["green"])}<span class="mono" style="width:220px">{n}</span><span class="mono" style="width:70px;color:var(--muted)">{v}</span><span style="color:var(--dim)">1/1 ready</span></div>'
    controllers = "".join(ctrl(n, v) for n, v in [("helm-controller", "v1.6.5"), ("kustomize-controller", "v1.9.6"), ("notification-controller", "v1.9.4"), ("source-controller", "v1.9.6")])
    head = lambda t, extra="": f'<div style="display:flex;align-items:center;gap:6px;height:32px;padding:0 12px;background:#2a2e36;border-bottom:1px solid var(--bv)"><span class="dtitle" style="margin:0">{t}</span><span style="flex:1"></span>{extra}</div>'
    content = f'''<div style="flex:1;display:flex;flex-direction:column;min-height:0">
<div class="tool">{ic("gauge",14,C["accent"])}<b style="font-weight:500">Flux overview</b><span class="chip mchip">v2.9.6</span><span style="font-size:12px;color:var(--dim)">controllers in flux-system</span><span style="flex:1"></span><span class="chip" style="color:var(--green)">{dot(C["green"])}live</span></div>
<div style="flex:1;overflow:hidden;display:flex;flex-direction:column;gap:12px;padding:14px 16px">
<div style="display:flex;gap:12px;align-items:center;padding:12px;border-radius:8px;background:#d072771a;border:1px solid #d0727759">{ic("err",20,C["red"])}<div><div style="font-size:14px;font-weight:600;color:var(--red)">Failing</div><div style="font-size:12.5px;color:var(--muted)">14 of 19 ready · 3 failing · 1 waiting · 1 suspended for more than a day</div></div><span style="flex:1"></span><span style="font-size:12px;color:var(--dim)">Controllers: 6 of 6 ready</span></div>
<div style="display:flex;gap:10px">{tiles}</div>
<div style="display:flex;gap:12px;min-height:0;align-items:flex-start">
<div class="card" style="flex:1;overflow:hidden;background:var(--bg)">{head("Needs attention · 5")}{attention}</div>
<div class="card" style="flex:1;overflow:hidden;background:var(--bg)">{head("Recent activity", f'<span class="chip">{dot(C["yellow"])}Warnings</span><button class="btn g" style="height:22px;padding:0 6px">All kinds{ic("cd",11)}</button><button class="btn g" style="height:22px;padding:0 6px">All namespaces{ic("cd",11)}</button>')}{events}</div>
</div>
<div class="card" style="overflow:hidden;background:var(--bg)">{head("Controllers")}{controllers}</div>
</div>
{hints([("↵","Open")])}
</div>'''
    tb = tabs([("gauge", "Flux", True), ("layers", "Kustomizations", False)])
    return page("Flux overview — Kubyl", flux_shell("Overview", tb, content))

def flux_dock(name, state, msg, revision):
    return f'''<aside class="dock" style="width:300px">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Kustomization details</span><button class="ib" aria-label="Pin">{ic("star",13)}</button><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec"><div class="mono" style="font-size:12.5px;margin-bottom:6px">{name}</div>
<div style="display:flex;gap:10px;align-items:center;font-size:12px">{fpill(state)}<span class="mono" style="font-size:11.5px">{revision}</span></div>
<div style="font-size:12px;color:var(--red);margin-top:6px;line-height:17px">{msg}</div>
<div style="display:flex;gap:6px;margin-top:10px"><button class="btn p" style="height:24px">{ic("refresh",12,"#1b1e24")}Reconcile</button><button class="btn" style="height:24px">{ic("pause",12)}Suspend</button><button class="btn g" style="height:24px">Open</button></div></div>
{fsec("Dependencies", f'<div style="font-size:12px;color:var(--yellow)">{ic("clock",12,C["yellow"])} Waiting for flux-demo/broken (Failed)</div>')}
{fsec("Details", '<dl class="kv" style="margin:0;grid-template-columns:84px minmax(0,1fr)"><dt>Source</dt><dd class="mono" style="font-size:11.5px">GitRepository/podinfo</dd><dt>Interval</dt><dd class="mono" style="font-size:11.5px">10m</dd><dt>Last change</dt><dd>1m ago</dd><dt>Applied</dt><dd>0 objects</dd></dl>', True)}
</aside>'''

def flux_kustomizations_screen():
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{flux_toolbar("Kustomizations", "layers", KS_COUNTS)}
{flux_chips([("Ready", 2, False), ("Reconciling", 1, False), ("Failed", 2, False), ("Suspended", 1, False)])}
{KS_HEAD}
<div style="flex:1;overflow:hidden">{flux_ks_rows((1,))}</div>
{hints(KS_HINTS)}
</div>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{flux_dock("apps-late", "Failed", "dependency \'flux-demo/broken\' is not ready", "")}</div>'
    tb = tabs([("gauge", "Flux", False), ("layers", "Kustomizations", True), ("branch", "Sources", False)])
    return page("Flux Kustomizations — Kubyl", flux_shell("Kustomizations", tb, content))

def flux_header(icon, name, kind, state, rev, every, extra=""):
    return f'''<div style="display:flex;align-items:center;gap:10px;padding:12px 16px 10px;border-bottom:1px solid var(--bv)">
<span style="width:30px;height:30px;border-radius:7px;background:#74ade822;border:1px solid #74ade855;display:flex;align-items:center;justify-content:center">{ic(icon,16,C["accent"])}</span>
<div style="min-width:0"><div style="display:flex;align-items:center;gap:8px"><span class="mono" style="font-size:15px;font-weight:500">{name}</span><span class="chip">flux-demo</span><span style="font-size:12px;color:var(--dim)">{kind}</span></div>
<div style="display:flex;gap:12px;align-items:center;margin-top:3px;font-size:12px">{fpill(state)}<span class="mono" style="font-size:11.5px;color:var(--muted)">{rev}</span><span style="color:var(--dim)">every {every}</span></div></div>
<div style="flex:1"></div>{extra}
<span style="display:flex"><button class="btn p" style="height:26px;border-radius:5px 0 0 5px">{ic("refresh",12,"#1b1e24")}Reconcile</button><button class="btn g" style="height:26px;padding:0 6px">{ic("cd",12)}</button></span>
<button class="btn" style="height:26px">{ic("pause",12)}Suspend</button>
<button class="btn g" style="height:26px">{ic("zap",13)}Ask agent</button>
<button class="ib" aria-label="More">{ic("more",14)}</button>
</div>'''

def flux_subtabs(active, items):
    return '<div style="display:flex;gap:2px;padding:0 12px;border-bottom:1px solid var(--bv);height:36px;align-items:stretch">' + "".join(
        f'<span style="display:flex;align-items:center;gap:6px;padding:0 10px;{"color:var(--text);box-shadow:inset 0 -2px 0 var(--accent)" if t == active else "color:var(--dim)"}">{t}' + (f'<span class="chip" style="height:17px">{c}</span>' if c else "") + '</span>'
        for t, c in items) + '</div>'

KS_TABS = [("Summary", ""), ("Inventory", "3"), ("History", "4"), ("Events", ""), ("Controller logs", "")]

def fcond(ok, t, reason, msg, age, col=None):
    col = col or (C["green"] if ok else C["red"])
    return (f'<div style="display:flex;gap:8px;font-size:12px;padding:3px 0;align-items:flex-start">{ic("ok" if ok else "err",13,col)}<div style="flex:1;min-width:0">'
            f'<div style="display:flex;gap:6px"><b style="font-weight:500">{t}</b><span style="color:var(--dim)">{reason}</span><span style="flex:1"></span><span class="mono" style="font-size:11px;color:var(--dim)">{age}</span></div>'
            f'<div style="color:var(--muted);line-height:17px">{msg}</div></div></div>')

def flux_kustomization_screen():
    left = f'''<div style="flex:1;min-width:0">
<div style="padding:12px 14px 0">{fbox(C["green"], "ok", "Ready", "Applied revision: master@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2")}</div>
{fsec("Conditions · 1", fcond(True, "Ready=True", "ReconciliationSucceeded", "Applied revision: master@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2", "4m"))}
{fsec("Dependencies · 1", f'<div style="display:flex;gap:8px;font-size:12px;align-items:center;padding:2px 0"><span style="width:70px;color:var(--dim)">depends on</span><a href="#" class="mono" style="text-decoration:none">flux-demo/infra</a><span style="flex:1"></span>{fpill("Ready")}</div><div style="display:flex;gap:8px;font-size:12px;align-items:center;padding:2px 0"><span style="width:70px;color:var(--dim)">needed by</span><a href="#" class="mono" style="text-decoration:none">flux-demo/frontend</a></div>')}
{fsec("Events · last 3", '<div style="font-size:12px;color:var(--muted);line-height:20px">4m · ReconciliationSucceeded · Reconciliation finished in 170ms, next run in 10m0s<br>2d · Progressing · Deployment/flux-chain/backend created<br>2d · DependencyNotReady · Dependencies do not meet ready condition</div>', True)}
</div>'''
    right = f'''<aside style="width:380px;flex-shrink:0;border-left:1px solid var(--border);background:var(--panel);overflow:hidden">
{fsec("Source", '<dl class="kv" style="margin:0;grid-template-columns:84px minmax(0,1fr)"><dt>Source</dt><dd><a href="#" class="mono" style="font-size:11.5px;text-decoration:none">GitRepository/podinfo</a></dd><dt>URL</dt><dd class="mono" style="font-size:11.5px">https://github.com/stefanprodan/podinfo</dd><dt>Applied</dt><dd><a href="#" class="mono" style="font-size:11.5px;text-decoration:none">master@sha1:3e0ff8a</a> ' + ic("ext",11,C["dim"]) + '</dd><dt>Source at</dt><dd class="mono" style="font-size:11.5px">master@sha1:3e0ff8a</dd></dl>')}
{fsec("Kustomization", '<dl class="kv" style="margin:0;grid-template-columns:84px minmax(0,1fr)"><dt>Path</dt><dd class="mono" style="font-size:11.5px">./deploy/webapp/backend</dd><dt>Prune</dt><dd>on: removes what leaves Git</dd><dt>Target ns</dt><dd class="mono" style="font-size:11.5px">flux-chain</dd><dt>Interval</dt><dd class="mono" style="font-size:11.5px">10m</dd><dt>Timeout</dt><dd class="mono" style="font-size:11.5px">5m</dd></dl>'
      + '<p class="dtitle" style="margin:12px 0 6px">Post-build substitutions</p><div style="font-size:12px;display:flex;flex-direction:column;gap:4px"><span class="mono" style="font-size:11.5px">${cluster_env} <span style="color:var(--dim)">=</span> dev</span>'
      + f'<span style="display:flex;gap:6px;align-items:center">{ic("lock",12,C["dim"])}<span style="color:var(--dim)">Secret</span><span class="mono" style="font-size:11.5px">podinfo-substitutions</span><span class="chip" style="height:17px">optional</span><span style="color:var(--faint)">values not shown</span></span></div>', True)}
</aside>'''
    content = f'''<div style="flex:1;display:flex;flex-direction:column;min-height:0">{flux_header("layers", "apps", "Kustomization · kustomize.toolkit.fluxcd.io/v1", "Ready", "master@sha1:3e0ff8a", "10m")}{flux_subtabs("Summary", KS_TABS)}
<div style="flex:1;display:flex;min-height:0">{left}{right}</div>
{hints([("r","Reconcile"),("⇧r","With source"),("s","Suspend"),("l","Logs"),("e","Edit YAML"),("⌃d","Delete…")])}</div>'''
    tb = tabs([("gauge", "Flux", False), ("layers", "Kustomizations", False), ("layers", "apps", True)])
    return page("Flux Kustomization — Kubyl", flux_shell("Kustomizations", tb, content))

def flux_inventory_screen():
    TC = "grid-template-columns: minmax(0,1fr) 150px minmax(0,0.6fr)"
    guide = lambda d: "".join('<span style="width:16px;flex-shrink:0;align-self:stretch;border-left:1px solid #3e4450;margin-left:6px"></span>' for _ in range(d))
    def node(d, kind, name, health, col, info, open_=None, live=False, on=False):
        chev = ic("cd", 11, C["dim"]) if open_ is True else ic("cr", 11, C["dim"]) if open_ is False else '<span style="width:11px"></span>'
        tag = '<span class="chip" style="height:16px;font-size:10.5px;padding:0 5px">live</span>' if live else ""
        h = f'<span class="pill">{dot(col)}<span style="color:{col}">{health}</span></span>' if health else '<span style="color:var(--dim)">present</span>'
        return (f'<div class="tr{" on" if on else ""}" style="{TC};height:30px"><span style="display:flex;align-items:center;gap:6px;height:100%">{guide(d)}{chev}'
                f'<span style="color:var(--dim);font-size:12px">{kind}</span><span class="mono" style="font-size:12px">{name}</span>{tag}</span>{h}<span style="font-size:12px;color:var(--muted)">{info}</span></div>')
    tree = "".join([
        node(0, "Deployment", "flux-podinfo/podinfo", "Healthy", C["green"], "2/2 ready", True),
        node(1, "ReplicaSet", "podinfo-6f8c7b9d4", "Healthy", C["green"], "2/2 ready", True, live=True),
        node(2, "Pod", "podinfo-6f8c7b9d4-x2kqp", "Running", C["green"], "1/1 ready · 10.244.0.31", live=True, on=True),
        node(2, "Pod", "podinfo-6f8c7b9d4-m8fzt", "Running", C["green"], "1/1 ready · 10.244.0.32", live=True),
        node(0, "Service", "flux-podinfo/podinfo", "Healthy", C["green"], "ClusterIP 10.96.181.7"),
        node(0, "HorizontalPodAutoscaler", "flux-podinfo/podinfo", "", None, ""),
    ])
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div style="height:36px;flex-shrink:0;display:flex;align-items:center;gap:8px;padding:0 12px;border-bottom:1px solid var(--bv);font-size:12px;color:var(--dim)">{ic("tree",13)}3 objects · 1 Deployment, 1 Service, 1 HorizontalPodAutoscaler<span style="flex:1"></span><span class="chip" style="height:16px;font-size:10.5px;padding:0 5px">live</span> children from Kubyl's watches</div>
<div class="th" style="{TC}"><span>OBJECT</span><span>HEALTH</span><span>INFO</span></div>
<div style="flex:1;overflow:hidden">{tree}</div>
{hints([("↵↵","Open in Kubyl"),("r","Reconcile"),("s","Suspend"),("l","Logs"),("e","Edit YAML"),("⌃d","Delete…")])}
</div>'''
    content = f'''<div style="flex:1;display:flex;flex-direction:column;min-height:0">{flux_header("layers", "podinfo", "Kustomization · kustomize.toolkit.fluxcd.io/v1", "Ready", "master@sha1:3e0ff8a", "10m")}{flux_subtabs("Inventory", KS_TABS)}
<div style="flex:1;display:flex;min-height:0">{center}</div></div>'''
    tb = tabs([("gauge", "Flux", False), ("layers", "Kustomizations", False), ("layers", "podinfo", True)])
    return page("Flux inventory — Kubyl", flux_shell("Kustomizations", tb, content))

def flux_helmrelease_screen():
    hist = lambda v, chart, app, status, col, when: f'<div style="display:grid;grid-template-columns:44px minmax(0,1fr) 70px 100px 44px 96px;align-items:center;height:28px;font-size:12px;border-top:1px solid #363c46"><span class="mono">v{v}</span><span class="mono" style="font-size:11.5px">{chart}</span><span class="mono" style="font-size:11.5px;color:var(--muted)">{app}</span><span class="pill">{dot(col)}<span style="color:{col}">{status}</span></span><span class="mono" style="font-size:11.5px;color:var(--muted)">{when}</span><a href="#" style="text-decoration:none">Helm release</a></div>'
    left = f'''<div style="flex:1;min-width:0">
<div style="padding:12px 14px 0">{fbox(C["green"], "ok", "Ready", "Helm upgrade succeeded for release flux-demo/podinfo-helm.v3 with chart podinfo@6.15.0")}</div>
{fsec("Conditions · 2", fcond(True, "Ready=True", "UpgradeSucceeded", "Helm upgrade succeeded for release flux-demo/podinfo-helm.v3 with chart podinfo@6.15.0", "3h") + fcond(True, "Released=True", "UpgradeSucceeded", "Helm upgrade succeeded for release flux-demo/podinfo-helm.v3", "3h"))}
{fsec("Releases · 3", hist(3, "podinfo 6.15.0", "6.15.0", "deployed", C["green"], "3h") + hist(2, "podinfo 6.14.1", "6.14.1", "superseded", C["dim"], "2d") + hist(1, "podinfo 6.14.0", "6.14.0", "superseded", C["dim"], "9d"), True)}
</div>'''
    right = f'''<aside style="width:380px;flex-shrink:0;border-left:1px solid var(--border);background:var(--panel);overflow:hidden">
{fsec("Chart", '<dl class="kv" style="margin:0;grid-template-columns:84px minmax(0,1fr)"><dt>Chart</dt><dd class="mono" style="font-size:11.5px">podinfo 6.x</dd><dt>Installed</dt><dd class="mono" style="font-size:11.5px">podinfo 6.15.0 (app 6.15.0)</dd><dt>From</dt><dd><a href="#" class="mono" style="font-size:11.5px;text-decoration:none">HelmRepository/podinfo</a></dd><dt>URL</dt><dd class="mono" style="font-size:11.5px">https://stefanprodan.github.io/podinfo</dd><dt>Release</dt><dd class="mono" style="font-size:11.5px">podinfo-helm</dd></dl>'
      + f'<div style="margin-top:10px"><button class="btn g" style="height:24px;padding:0 6px">{ic("anchor",12)}Open Helm release</button></div>')}
{fsec("Values", f'<div style="display:flex;flex-direction:column;gap:5px;font-size:12px"><span style="display:flex;gap:6px;align-items:center">{ic("file",12,C["dim"])}<span style="color:var(--dim)">ConfigMap</span><span class="mono" style="font-size:11.5px">podinfo-values</span></span><span style="display:flex;gap:6px;align-items:center">{ic("lock",12,C["dim"])}<span style="color:var(--dim)">Secret</span><span class="mono" style="font-size:11.5px">podinfo-secret-values</span><span style="color:var(--dim)">· values.yaml</span><span style="color:var(--faint)">values not shown</span></span><span style="display:flex;gap:6px"><span style="color:var(--dim)">inline</span><span class="mono" style="font-size:11.5px">resources</span></span></div>')}
{fsec("Remediation", '<dl class="kv" style="margin:0;grid-template-columns:84px minmax(0,1fr)"><dt>Install</dt><dd>3 retries</dd><dt>Upgrade</dt><dd>3 retries</dd><dt>Last failure</dt><dd>remediated</dd><dt>Failures</dt><dd>install 0 · upgrade 0 · total 0</dd></dl>', True)}
</aside>'''
    tabs_ = [("Summary", ""), ("Inventory", "2"), ("History", "3"), ("Events", ""), ("Controller logs", "")]
    content = f'''<div style="flex:1;display:flex;flex-direction:column;min-height:0">{flux_header("anchor", "podinfo-helm", "HelmRelease · helm.toolkit.fluxcd.io/v2", "Ready", "6.15.0", "10m")}{flux_subtabs("Summary", tabs_)}
<div style="flex:1;display:flex;min-height:0">{left}{right}</div>
{hints([("r","Reconcile"),("⇧r","With source"),("s","Suspend"),("l","Logs"),("e","Edit YAML"),("⌃d","Delete…")])}</div>'''
    tb = tabs([("gauge", "Flux", False), ("anchor", "HelmReleases", False), ("anchor", "podinfo-helm", True)])
    return page("Flux HelmRelease — Kubyl", flux_shell("HelmReleases", tb, content))

def flux_sources_screen():
    SC = "grid-template-columns: minmax(0,0.8fr) 118px 84px minmax(0,1.4fr) minmax(0,1.2fr) 150px 58px 50px"
    rows = [
        ("infra", "GitRepository", "flux-system", "Failed", "failed to checkout and determine revision: authentication required", "https://gitlab.example.com/platform/infra.git", "", "1m", "3m"),
        ("podinfo", "GitRepository", "flux-demo", "Ready", "stored artifact for revision 'master@sha1:3e0ff8a…'", "https://github.com/stefanprodan/podinfo", "master@sha1:3e0ff8a", "5m", "4m"),
        ("podinfo", "HelmRepository", "flux-demo", "Ready", "stored artifact: revision 'sha256:e7dc68a4…'", "https://stefanprodan.github.io/podinfo", "sha256:e7dc68a4", "30m", "4m"),
        ("flux-demo-podinfo-helm", "HelmChart", "flux-demo", "Ready", "pulled 'podinfo' chart with version '6.15.0'", "HelmRepository/podinfo", "6.15.0", "10m", "4m"),
        ("podinfo-manifests", "OCIRepository", "flux-demo", "Ready", "stored artifact for digest 'latest@sha256:87815bbd…'", "oci://ghcr.io/stefanprodan/manifests/podinfo", "latest@sha256:87815bbd", "30m", "4m"),
        ("backups", "Bucket", "flux-system", "Suspended", "stored artifact: revision 'sha256:1c4e…'", "minio.storage.svc:9000", "sha256:1c4e88a0", "1h", "3d"),
    ]
    out = "".join(f'''<div class="tr{" on" if i == 1 else ""}" style="{SC};height:32px"><span class="mono" style="font-size:12px">{n}</span><span style="color:var(--muted)">{k}</span><span style="color:var(--muted)">{ns}</span>
<span style="display:flex;gap:8px;align-items:center;min-width:0">{fpill(s)}<span style="font-size:12px;color:var(--dim);overflow:hidden;text-overflow:ellipsis">{m}</span></span><span class="mono" style="font-size:11.5px">{u}</span><span class="mono" style="font-size:11.5px">{r}</span><span class="mono" style="font-size:11.5px;color:var(--muted)">{iv}</span><span class="mono" style="font-size:11.5px;color:var(--muted)">{last}</span></div>''' for i, (n, k, ns, s, m, u, r, iv, last) in enumerate(rows))
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{flux_toolbar("Sources", "branch", '<span>6</span><span style="color:var(--red)">· 1 failing</span><span style="color:var(--purple)">· 1 suspended</span>', kinds=True)}
{flux_chips([("Ready", 4, False), ("Failed", 1, False), ("Suspended", 1, False)])}
<div class="th" style="{SC}"><span>NAME {ic("cd",10)}</span><span>KIND</span><span>NAMESPACE</span><span>STATE</span><span>SOURCE</span><span>REVISION</span><span>INTERVAL</span><span>LAST</span></div>
<div style="flex:1;overflow:hidden">{out}</div>
{hints([("↵","Open"),("r","Reconcile"),("s","Suspend"),("u","Resume"),("l","Logs"),("e","Edit YAML"),("⌃d","Delete…"),("/","Filter")])}
</div>'''
    dock = f'''<aside class="dock" style="width:300px">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">GitRepository details</span><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec"><div class="mono" style="font-size:12.5px;margin-bottom:6px">podinfo</div><div style="display:flex;gap:10px;font-size:12px">{fpill("Ready")}<span class="mono" style="font-size:11.5px">master@sha1:3e0ff8a</span></div>
<div style="display:flex;gap:6px;margin-top:10px"><button class="btn p" style="height:24px">{ic("refresh",12,"#1b1e24")}Reconcile</button><button class="btn" style="height:24px">{ic("pause",12)}Suspend</button><button class="btn g" style="height:24px">Open</button></div></div>
{fsec("Source", '<dl class="kv" style="margin:0;grid-template-columns:84px minmax(0,1fr)"><dt>URL</dt><dd class="mono" style="font-size:11.5px">https://github.com/stefanprodan/podinfo</dd><dt>Ref</dt><dd class="mono" style="font-size:11.5px">branch master</dd><dt>Digest</dt><dd class="mono" style="font-size:11.5px">sha256:903f57ee86…</dd><dt>Last fetch</dt><dd>4m ago</dd></dl>')}
{fsec("Used by · 6", '<div style="font-size:12px;line-height:22px"><span style="color:var(--dim)">Kustomization</span> <a href="#" class="mono" style="text-decoration:none">flux-demo/apps</a><br><span style="color:var(--dim)">Kustomization</span> <a href="#" class="mono" style="text-decoration:none">flux-demo/broken</a><br><span style="color:var(--dim)">ImageUpdateAutomation</span> <a href="#" class="mono" style="text-decoration:none">flux-demo/podinfo</a><br><span style="color:var(--faint)">and 3 more</span></div>', True)}
</aside>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    tb = tabs([("gauge", "Flux", False), ("branch", "Sources", True)])
    return page("Flux sources — Kubyl", flux_shell("Sources", tb, content))

def flux_managed_screen():
    DC = "grid-template-columns: minmax(0,1fr) 70px 80px 90px 150px 60px"
    rows = [("backend", "1/1", "1", "1", ("ks/apps", "ok", C["green"]), "2d"), ("checkout-api", "3/3", "3", "3", None, "41d"),
            ("payment-gateway", "1/2", "2", "1", ("hr/payment-gateway", "err", C["red"]), "9d"), ("podinfo", "2/2", "2", "2", ("ks/podinfo", "ok", C["green"]), "2d"),
            ("podinfo-helm", "1/1", "1", "1", ("hr/podinfo-helm", "ok", C["green"]), "2d"), ("paused-web", "1/1", "1", "1", ("ks/paused", "pause", C["purple"]), "8d")]
    def fcell(f):
        if not f:
            return '<span style="color:var(--faint)">—</span>'
        label, icon, col = f
        return f'<span class="chip" style="height:20px;gap:5px">{ic(icon,11,col)}<span class="mono" style="font-size:11px;color:var(--text)">{label}</span></span>'
    out = "".join(f'<div class="tr{" on" if i == 3 else ""}" style="{DC}"><span class="mono">{n}</span><span class="mono">{r}</span><span class="mono">{u}</span><span class="mono">{a}</span>{fcell(f)}<span class="mono" style="color:var(--muted)">{age}</span></div>' for i, (n, r, u, a, f, age) in enumerate(rows))
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
<div class="tool"><div class="crumb">{ic("layers",14,C["accent"])}<b>Deployments</b><span>·</span><span>6</span></div><div style="flex:1"></div><div class="inp" style="width:150px">{ic("filter",12)}Filter</div></div>
<div class="th" style="{DC}"><span>NAME {ic("cd",10)}</span><span>READY</span><span>UP-TO-DATE</span><span>AVAILABLE</span><span>FLUX</span><span>AGE</span></div>
<div style="flex:1;overflow:hidden">{out}</div>
{hints([("↵","Details"),("l","Logs"),("s","Shell"),("e","Edit YAML"),("⌃d","Delete…"),("/","Filter")])}
</div>'''
    dock = f'''<aside class="dock" style="width:320px">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Deployment details</span><button class="ib" aria-label="Close">{ic("x",13)}</button></div>
<div class="dsec"><div class="mono" style="font-size:12.5px;margin-bottom:6px">podinfo</div><div style="display:flex;gap:10px;font-size:12px">{st("Running")}<span style="color:var(--dim)">2/2 ready · flux-podinfo</span></div></div>
{fsec("Flux", f'<div style="font-size:12px;display:flex;gap:6px">Managed by Flux Kustomization <a href="#" style="text-decoration:none">flux-demo/podinfo</a></div><div style="display:flex;gap:10px;font-size:12px;margin-top:6px">{fpill("Ready")}<span class="mono" style="font-size:11.5px">master@sha1:3e0ff8a</span></div><div style="font-size:12px;color:var(--dim);margin-top:6px;line-height:17px">kustomize-controller reverts changes made here at its next reconcile (every 10m).</div>')}
{fsec("Pods · 2", '<div style="font-size:12px;line-height:22px"><span class="mono">podinfo-6f8c7b9d4-x2kqp</span> <span style="color:var(--green)">Running</span><br><span class="mono">podinfo-6f8c7b9d4-m8fzt</span> <span style="color:var(--green)">Running</span></div>', True)}
</aside>'''
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    yaml_note = ""
    tb = tabs([("layers", "Deployments", True), ("file", "podinfo.yaml", False)])
    return page("Managed by Flux — Kubyl", flux_shell("Deployments", tb, content + yaml_note, workloads=True))

def flux_dialog(title, icon, body, buttons, width=520):
    return f'''<div style="position:absolute;inset:0;background:rgba(15,17,21,.55);display:flex;align-items:flex-start;justify-content:center;padding-top:120px">
<div role="dialog" aria-label="{title}" style="width:{width}px;background:#2f343e;border:1px solid var(--border);border-radius:10px;box-shadow:0 20px 60px rgba(0,0,0,.5);overflow:hidden">
<div style="display:flex;align-items:center;gap:10px;padding:14px 16px;border-bottom:1px solid var(--bv)">{ic(icon,16,C["accent"])}<b style="font-weight:600;flex:1">{title}</b><span class="prod" style="font-size:9.5px;padding:0 4px">PROD</span></div>
<div style="padding:16px;display:flex;flex-direction:column;gap:12px">{body}</div>
<div style="display:flex;justify-content:flex-end;gap:8px;padding:12px 16px;border-top:1px solid var(--bv)">{buttons}</div></div></div>'''

def flux_actions_screen():
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{flux_toolbar("Kustomizations", "layers", KS_COUNTS)}
{flux_chips([("Ready", 2, False), ("Reconciling", 1, False), ("Failed", 2, False), ("Suspended", 1, False)], '<span style="font-size:12px;color:var(--muted)">2 selected</span>')}
{KS_HEAD}
<div style="flex:1;overflow:hidden">{flux_ks_rows((0, 3))}</div>
{hints(KS_HINTS)}
</div>'''
    body = ('<div style="font-size:12.5px;color:var(--muted);line-height:19px">Flux stops reconciling them until they\'re resumed: changes in Git aren\'t applied and drift isn\'t corrected.</div>'
            '<div class="card" style="padding:8px 12px;background:#2a2e36;display:flex;flex-direction:column;gap:4px;font-size:12px"><span class="mono">Kustomization flux-demo/apps</span><span class="mono">Kustomization flux-demo/infra</span></div>'
            f'<div style="font-size:11.5px;color:var(--dim)">{ic("wheel",12)} Patches <span class="mono">spec.suspend: true</span> with your Kubernetes access, like <span class="mono">flux suspend ks</span>.</div>')
    modal = flux_dialog("Suspend 2 objects?", "pause", body, f'<button class="btn g">Cancel</button><button class="btn" style="border-color:#7a4448;color:var(--red)">{ic("pause",12,C["red"])}Suspend</button>', 480)
    content = f'<div style="flex:1;display:flex;min-height:0">{center}</div>'
    tb = tabs([("gauge", "Flux", False), ("layers", "Kustomizations", True)])
    return page("Flux suspend — Kubyl", flux_shell("Kustomizations", tb, content, modal))

def flux_delete_screen():
    center = f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{flux_toolbar("Kustomizations", "layers", KS_COUNTS)}
{flux_chips([("Ready", 2, False), ("Reconciling", 1, False), ("Failed", 2, False), ("Suspended", 1, False)])}
{KS_HEAD}
<div style="flex:1;overflow:hidden">{flux_ks_rows((5,))}</div>
{hints(KS_HINTS)}
</div>'''
    body = ('<div style="display:flex;gap:10px;padding:10px 12px;border-radius:7px;background:#3a2b2e;border:1px solid #6b3c41">' + ic("alert",15,C["red"]) +
            '<div style="font-size:12.5px;line-height:18px">Prune is on: kustomize-controller deletes everything it applied (1 Deployment, 1 Service, 1 HorizontalPodAutoscaler).</div></div>'
            '<div class="card" style="padding:8px 12px;background:#2a2e36;display:flex;flex-direction:column;gap:4px;font-size:12px;color:var(--muted)"><span class="mono">prunes Deployment flux-podinfo/podinfo</span><span class="mono">prunes Service flux-podinfo/podinfo</span><span class="mono">prunes HorizontalPodAutoscaler flux-podinfo/podinfo</span></div>'
            '<div style="display:flex;flex-direction:column;gap:6px"><div style="font-size:12px;color:var(--muted)">This is a production cluster. Type <span class="mono" style="color:var(--text)">podinfo</span> to confirm.</div>'
            '<div class="inp focus" style="height:28px"><span class="mono" style="font-size:12.5px;color:var(--text)">podin</span><span style="display:inline-block;width:1px;height:15px;background:var(--accent);margin-left:-6px"></span></div></div>')
    modal = flux_dialog("Delete Kustomization flux-demo/podinfo?", "trash", body, f'<button class="btn g">Cancel</button><button class="btn" style="border-color:#7a4448;color:var(--red);opacity:.55">{ic("trash",12,C["red"])}Delete</button>')
    content = f'<div style="flex:1;display:flex;min-height:0">{center}</div>'
    tb = tabs([("gauge", "Flux", False), ("layers", "Kustomizations", True)])
    return page("Flux delete — Kubyl", flux_shell("Kustomizations", tb, content, modal))


# ---------- 22 · Resource views (phase 24) ----------
RV_CLUSTER = "kind-dev"

def rv_sidebar(active, open_groups):
    """kind-dev with the groups phase 24 adds to (only kinds the cluster serves show)."""
    groups = [
        ("Workloads", [("Pods", "box", "21"), ("Deployments", "layers", "4"), ("StatefulSets", "db", "1"), ("Jobs", "play", "2"), ("VerticalPodAutoscalers", "activity", "1")]),
        ("Network", [("Services", "network", "6"), ("Endpoints", "commit", "6"), ("EndpointSlices", "commit", "6"), ("Ingresses", "globe", "1"), ("NetworkPolicies", "shield", "2"),
                     ("Gateways", "globe", "1"), ("GatewayClasses", "globe", "1"), ("HTTPRoutes", "right", "1"), ("GRPCRoutes", "right", "1"), ("ListenerSets", "list", "0"), ("ReferenceGrants", "link", "1")]),
        ("Config &amp; Secrets", []), ("Storage", []),
        ("Device Resources", [("ResourceClaims", "cpu", "2"), ("ResourceClaimTemplates", "fileplus", "1"), ("DeviceClasses", "blocks", "1"), ("ResourceSlices", "server", "1"), ("DeviceTaintRules", "alert", "0")]),
        ("Access Control", []),
        ("Cluster", [("Nodes", "server", "2"), ("Namespaces", "folder", "24"), ("PriorityClasses", "up", "2"), ("RuntimeClasses", "cpu", "1"), ("Leases", "lock", "9"),
                     ("MutatingWebhooks", "zap", "1"), ("ValidatingWebhooks", "ok", "2"), ("ValidatingAdmissionPolicies", "shield", "2"), ("ValidatingAdmissionPolicyBindings", "link", "2"),
                     ("MutatingAdmissionPolicies", "shield", "1"), ("MutatingAdmissionPolicyBindings", "link", "1")]),
        ("Administration", []), ("Custom Resources", []),
    ]
    rows = [
        f'<div class="phead"><span style="flex:1;font-weight:500;color:var(--text)">Explorer</span><button class="ib" aria-label="Filter kinds">{ic("search",13)}</button><button class="ib" aria-label="Add kubeconfig">{ic("plus",14)}</button><button class="ib" aria-label="More">{ic("more",14)}</button></div>',
        f'<div class="sec">{ic("cr",11)}Favorites<span style="flex:1"></span><span style="font-weight:400;letter-spacing:0;text-transform:none;color:var(--faint)">4</span></div>',
        f'<div class="sec">{ic("cd",11)}Clusters</div>',
        root(RV_CLUSTER, "on", True, C["green"]),
        ti("Overview", 1, "gauge"),
        ti("Events", 1, "bell", "3", color=C["yellow"]),
    ]
    for g, kinds in groups:
        is_open = g in open_groups
        rows.append(ti(g, 1, open_=is_open))
        if is_open:
            rows += [ti(n, 2, icon, cnt, on=n == active) for n, icon, cnt in kinds]
    rows += [root("prod-eu-west-1", "on", color=C["red"], prod=True), root("staging-eu-west-1", "on", color=C["yellow"])]
    return '<aside class="side">' + "\n".join(rows) + '</aside>'

def rv_app(title, active, open_groups, tb, content, ns="kubyl-views", overlay=""):
    inner = f'''<div class="app">
{titlebar(RV_CLUSTER, ns, False, "kind · v1.37.0")}
<div class="body">{rv_sidebar(active, open_groups)}<main class="main">{tb}{content}</main></div>
{statusbar(cluster=RV_CLUSTER, ns=ns)}{overlay}
</div>'''
    return page(title, inner)

def rv_list(icon, title, sub, cols, head, rows, keys, extra_tool=""):
    toolbar = f'''<div class="tool">
<div class="crumb">{ic(icon,14,C["accent"])}<b>{title}</b><span>·</span><span>{sub}</span></div>
<div style="flex:1"></div>{extra_tool}
<div class="inp" style="width:180px">{ic("filter",12)}Filter</div>
<button class="btn g" aria-label="Columns">{ic("sliders",13)}</button>
<span class="chip" style="color:var(--green)">{dot(C["green"])}live</span>
</div>'''
    hd = f'<div class="th" style="{cols}">' + "".join(f"<span>{h}</span>" for h in head) + "</div>"
    body = "".join(f'<div class="tr{" on" if i == 0 else ""}" style="{cols}">{"".join(r)}</div>' for i, r in enumerate(rows))
    return f'''<div style="flex:1;display:flex;flex-direction:column;min-width:0">
{toolbar}{hd}<div style="flex:1;overflow:hidden">{body}</div>
{hints(keys)}
</div>'''

def rv_dock(title, width, sections):
    return (f'<aside class="dock" style="width:{width}px"><div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">{title}</span>'
            f'<button class="ib" aria-label="Pin">{ic("star",13)}</button><button class="ib" aria-label="Close">{ic("x",13)}</button></div>'
            + "".join(sections) + "</aside>")

def rv_head(name, chips, ns=None):
    nsl = f'<div style="font-size:11.5px;color:var(--dim);margin-bottom:6px">namespace {ns}</div>' if ns else ""
    return f'<div class="dsec"><div class="mono" style="font-size:12.5px;margin-bottom:4px">{name}</div>{nsl}<div style="display:flex;gap:6px;flex-wrap:wrap">{chips}</div></div>'

def rv_sec(title, body, last=False):
    return f'<div class="dsec"{" style=border-bottom:0" if last else ""}><p class="dtitle">{title}</p>{body}</div>'

def m(t, c="var(--text)", s=11.5): return f'<span class="mono" style="font-size:{s}px;color:{c}">{t}</span>'
def lk(t, s=11.5): return f'<a href="#" class="mono" style="font-size:{s}px;text-decoration:none">{t}</a>'
def tpill(label, col): return f'<span class="pill" style="font-size:12px">{dot(col)}<span style="color:{col}">{label}</span></span>'
def chip_ok(label): return f'<span class="chip" style="color:var(--green)">{ic("check",11,C["green"],2.5)}{label}</span>'
def cel(lines): return ('<div class="mono" style="font-size:11.5px;line-height:17px;white-space:pre-wrap;color:var(--text);background:#2a2e36;border:1px solid var(--bv);border-radius:5px;padding:5px 8px">'
                        + "<br>".join(lines) + "</div>")
def card(inner): return f'<div style="border:1px solid var(--bv);border-radius:6px;background:#2a2e36;padding:7px 10px;display:flex;flex-direction:column;gap:3px;font-size:12px">{inner}</div>'

def dra_claims_screen():
    cols = "grid-template-columns: minmax(0,1fr) 196px 150px 110px 54px"
    rows = [
        [m("shared-gpu", s=12), tpill("Allocated, reserved by 1", C["green"]), m("gpu.example.com", "var(--muted)"), f'<span style="color:var(--green)">2 healthy</span>', m("14m", "var(--muted)")],
        [m("gpu-job-0-gpu-nf4ln", s=12), tpill("Allocated, reserved by 1", C["green"]), m("gpu.example.com", "var(--muted)"), f'<span style="color:var(--green)">1 healthy</span>', m("14m", "var(--muted)")],
        [m("train-a100-x4", s=12), tpill("Pending", C["yellow"]), m("gpu.example.com", "var(--muted)"), m("—", "var(--faint)"), m("2m", "var(--muted)")],
        [m("nic-sriov-7", s=12), tpill("Allocated", C["accent"]), m("net.example.com", "var(--muted)"), f'<span style="color:var(--red)">1 unhealthy</span>', m("3d", "var(--muted)")],
    ]
    center = rv_list("cpu", "ResourceClaims", "4 in kubyl-views", cols, ["NAME", "STATE", "DEVICE CLASSES", "DEVICE HEALTH", "AGE"], rows,
                     [("↵", "Details"), ("d", "Describe"), ("e", "Edit YAML"), ("⌃d", "Delete"), ("/", "Filter")])
    results = "".join(
        f'<div style="display:flex;align-items:center;gap:7px;padding:3px 0;font-size:12px">{ic("cpu",13,C["green"])}{m(dev)}<span style="color:var(--dim)">in</span>{lk(slice_)}<span style="flex:1"></span><span style="color:var(--green);font-size:11.5px">Healthy</span></div>'
        for dev, slice_ in [("gpu-0", "kubyl-dev-worker-dvvbf"), ("gpu-1", "kubyl-dev-worker-dvvbf")])
    dock = rv_dock("ResourceClaim details", 380, [
        rv_head("shared-gpu", tpill("Allocated, reserved by 1", C["green"]) + '<span class="chip">age 14m</span>', "kubyl-views"),
        rv_sec("Requests · 1", card(f'<div style="display:flex;gap:6px;align-items:center"><b style="font-weight:500">gpus</b><span class="chip" style="height:18px">exactly 2</span><span style="flex:1"></span>{lk("gpu.example.com")}</div>'
                                   '<div style="color:var(--dim);font-size:11.5px">ExactCount · no selectors · no admin access</div>')),
        rv_sec("Allocation", '<div style="display:flex;flex-direction:column;gap:4px">'
               + rkv("Node", lk("kubyl-dev-worker"), 70) + rkv("Driver", m("gpu.example.com"), 70) + rkv("Pool", m("kubyl-dev-worker"), 70) + rkv("Allocated", "14m ago", 70)
               + '</div><div style="font-size:11.5px;color:var(--dim);margin:8px 0 2px">Devices · 2</div>' + results),
        rv_sec("Reserved for · 1", f'<div style="display:flex;align-items:center;gap:7px;font-size:12px">{dot(C["green"])}<span style="color:var(--dim)">Pod</span>{lk("gpu-shared")}<span style="flex:1"></span><span style="color:var(--green)">Running</span></div>'),
        rv_sec("Device status", '<div style="display:flex;flex-direction:column;gap:6px">' + "".join(
            card(f'<div style="display:flex;gap:6px;align-items:center">{m(d)}<span style="flex:1"></span><span class="pill">{ic("ok",12,C["green"])}Ready</span></div>'
                 f'<div style="display:flex;gap:4px;flex-wrap:wrap"><span class="chip mchip">model=LATEST-GPU-MODEL</span><span class="chip mchip">driverVersion=1.0.0</span></div>') for d in ["gpu-0", "gpu-1"]) + '</div>', last=True),
    ])
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    tb = tabs([("cpu", "ResourceClaims", True), ("box", "Pods", False)])
    return rv_app("Device resources — Kubyl", "ResourceClaims", ("Device Resources",), tb, content)

def dra_devices_screen():
    cols = "grid-template-columns: minmax(0,1fr) 130px 130px 130px 70px 80px 54px"
    rows = [[m("kubyl-dev-worker-dvvbf", s=12), m("gpu.example.com", "var(--muted)"), m("kubyl-dev-worker", "var(--muted)"), lk("kubyl-dev-worker", 12), m("8"), m("3"), m("14m", "var(--muted)")],
            [m("kubyl-dev-worker-net-q7d2k", s=12), m("net.example.com", "var(--muted)"), m("kubyl-dev-worker", "var(--muted)"), lk("kubyl-dev-worker", 12), m("4"), m("0", "var(--dim)"), m("3d", "var(--muted)")],
            [m("fabric-pool-0", s=12), m("fabric.example.com", "var(--muted)"), m("fabric", "var(--muted)"), m("all nodes", "var(--dim)"), m("16"), m("2"), m("3d", "var(--muted)")]]
    center = rv_list("server", "ResourceSlices", "3", cols, ["NAME", "DRIVER", "POOL", "NODE", "DEVICES", "ALLOCATED", "AGE"], rows,
                     [("↵", "Details"), ("d", "Describe"), ("e", "Edit YAML"), ("/", "Filter")])
    def device(name, claim, attrs, cap, taint=None):
        c = (f'<span style="color:var(--dim)">claim</span>{lk(claim)}' if claim else '<span style="color:var(--dim)">free</span>')
        t = f'<div style="font-size:11.5px;color:var(--yellow)">{ic("alert",11,C["yellow"])} taint {taint}</div>' if taint else ""
        return card(f'<div style="display:flex;gap:6px;align-items:center">{ic("cpu",13,C["green"] if claim else C["dim"])}{m(name, s=12)}<span style="flex:1"></span>{c}</div>'
                    f'<div style="display:flex;gap:4px;flex-wrap:wrap">{"".join(f"<span class=\"chip mchip\">{a}</span>" for a in attrs)}</div>'
                    f'<div style="font-size:11.5px;color:var(--dim)">capacity {cap}</div>{t}')
    dock = rv_dock("ResourceSlice details", 400, [
        rv_head("kubyl-dev-worker-dvvbf", '<span class="chip mchip">gpu.example.com</span><span class="chip">3 of 8 allocated</span><span class="chip">age 14m</span>'),
        rv_sec("Slice", '<div style="display:flex;flex-direction:column;gap:4px">' + rkv("Driver", m("gpu.example.com"), 70) + rkv("Pool", m("kubyl-dev-worker") + ' <span style="color:var(--dim)">· generation 1 · 1 slice</span>', 70) + rkv("Node", lk("kubyl-dev-worker"), 70) + '</div>'),
        rv_sec("Devices · 8", '<div style="display:flex;flex-direction:column;gap:6px">'
               + device("gpu-0", "kubyl-views/shared-gpu", ["model=LATEST-GPU-MODEL", "index=0", "driverVersion=1.0.0"], "compute 100 · memory 80Gi")
               + device("gpu-1", "kubyl-views/shared-gpu", ["model=LATEST-GPU-MODEL", "index=1", "driverVersion=1.0.0"], "compute 100 · memory 80Gi")
               + device("gpu-2", None, ["model=LATEST-GPU-MODEL", "index=2", "driverVersion=1.0.0"], "compute 100 · memory 80Gi", "maintenance=planned:NoSchedule")
               + '<div style="font-size:11.5px;color:var(--dim)">… 5 more</div></div>', last=True),
    ])
    # The pod's and the node's sections, as they appear in their details (inset).
    pod_card = f'''<div style="position:absolute;left:300px;bottom:44px;width:420px;background:var(--panel);border:1px solid var(--border);border-radius:8px;box-shadow:0 12px 34px rgba(0,0,0,.45);overflow:hidden">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Pod details · gpu-shared</span></div>
{rv_sec("Resource claims · 1", card(f'<div style="display:flex;gap:6px;align-items:center"><b style="font-weight:500">gpus</b><span style="color:var(--dim)">→</span>{lk("shared-gpu")}<span style="flex:1"></span>{tpill("Allocated, reserved", C["green"])}</div>'
        f'<div style="font-size:11.5px;color:var(--dim)">container ctr · devices {m("gpu-0", "var(--muted)")}, {m("gpu-1", "var(--muted)")} · {lk("gpu.example.com")} · <span style="color:var(--green)">Healthy</span></div>'), last=True)}
</div>'''
    node_card = f'''<div style="position:absolute;left:740px;bottom:44px;width:400px;background:var(--panel);border:1px solid var(--border);border-radius:8px;box-shadow:0 12px 34px rgba(0,0,0,.45);overflow:hidden">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Node details · kubyl-dev-worker</span></div>
{rv_sec("Devices · 12 · 3 allocated", '<div style="display:flex;flex-direction:column;gap:3px;font-size:12px">'
        + f'<div style="display:flex;gap:6px;align-items:center">{lk("kubyl-dev-worker-dvvbf")}<span style="color:var(--dim)">gpu.example.com · pool kubyl-dev-worker · 8 devices</span></div>'
        + "".join(f'<div style="display:flex;gap:7px;align-items:center;padding-left:12px">{ic("cpu",12,C["green"] if c else C["dim"])}{m(d)}<span style="flex:1"></span>{lk(c) if c else "<span style=color:var(--dim)>free</span>"}</div>' for d, c in [("gpu-0", "kubyl-views/shared-gpu"), ("gpu-1", "kubyl-views/shared-gpu"), ("gpu-3", "kubyl-views/gpu-job-0-gpu-nf4ln")])
        + f'<div style="display:flex;gap:6px;align-items:center;margin-top:4px">{lk("kubyl-dev-worker-net-q7d2k")}<span style="color:var(--dim)">net.example.com · 4 devices, all free</span></div></div>', last=True)}
</div>'''
    content = f'<div style="flex:1;display:flex;min-height:0;position:relative">{center}{dock}</div>'
    tb = tabs([("server", "ResourceSlices", True), ("cpu", "ResourceClaims", False)])
    inner = f'''<div class="app">
{titlebar(RV_CLUSTER, "kubyl-views", False, "kind · v1.37.0")}
<div class="body">{rv_sidebar("ResourceSlices", ("Device Resources",))}<main class="main">{tb}{content}</main></div>
{statusbar(cluster=RV_CLUSTER, ns="kubyl-views")}
{pod_card}{node_card}
</div>'''
    return page("Resource slices, a pod's claims and a node's devices — Kubyl", inner)

def admission_screen():
    cols = "grid-template-columns: minmax(0,1fr) 90px 80px 80px 54px"
    rows = [[m("kubyl-replica-limit", s=12), m("2"), m("Fail"), lk("1", 12), m("12m", "var(--muted)")],
            [m("safe-upgrades.gateway.networking.k8s.io", s=12), m("2"), m("Fail"), lk("1", 12), m("12m", "var(--muted)")],
            [m("require-team-label", s=12), m("1"), m("Ignore"), m("0", "var(--yellow)"), m("6d", "var(--muted)")]]
    center = rv_list("shield", "ValidatingAdmissionPolicies", "3", cols, ["NAME", "VALIDATIONS", "FAILURE", "BINDINGS", "AGE"], rows,
                     [("↵", "Details"), ("d", "Describe"), ("e", "Edit YAML"), ("⌃d", "Delete"), ("/", "Filter")])
    val = lambda expr, msg: card(cel(expr) + f'<div style="font-size:11.5px;color:var(--dim)">{msg}</div>')
    binding = card(f'<div style="display:flex;gap:6px;align-items:center">{ic("link",12,C["dim"])}{lk("kubyl-replica-limit-views")}<span style="flex:1"></span><span class="chip" style="height:18px;color:var(--red)">Deny</span><span class="chip" style="height:18px">Audit</span></div>'
                   f'<div style="font-size:11.5px;color:var(--dim)">params {lk("ConfigMap kubyl-views/replica-limit", 11)} · missing params: Deny</div>'
                   f'<div style="font-size:11.5px;color:var(--dim)">namespaces {m("kubyl.dev/views=true", "var(--muted)", 11)} · 1 matches: {lk("kubyl-views", 11)}</div>')
    dock = rv_dock("ValidatingAdmissionPolicy details", 470, [
        rv_head("kubyl-replica-limit", '<span class="chip">failure Fail</span><span class="chip">2 validations</span><span class="chip">params ConfigMap</span><span class="chip">age 12m</span>'),
        rv_sec("Match constraints", '<div style="display:flex;flex-direction:column;gap:4px">' + rkv("Resources", m("apps/v1 deployments, statefulsets"), 90) + rkv("Operations", "CREATE, UPDATE", 90) + rkv("Match policy", "Equivalent", 90) + "</div>"),
        rv_sec("Match conditions · 1", card(f'<b style="font-weight:500">not-a-system-user</b>' + cel(["!request.userInfo.username.startsWith('system:')"]))),
        rv_sec("Variables · 2", '<div style="display:flex;flex-direction:column;gap:4px;font-size:12px">'
               + rkv(m("replicas"), cel(["has(object.spec.replicas) ? object.spec.replicas : 1"]), 70) + rkv(m("limit"), cel(["int(params.data.maxReplicas)"]), 70) + "</div>"),
        rv_sec("Validations · 2", '<div style="display:flex;flex-direction:column;gap:6px">'
               + val(["variables.replicas &lt;= variables.limit"], "message: 'replicas must be at most ' + string(variables.limit) · reason Invalid")
               + val(["object.metadata.name.size() &lt;= 40"], "names are at most 40 characters") + "</div>"),
        rv_sec("Audit annotations · 1", rkv(m("replicas"), cel(["string(variables.replicas)"]), 70)),
        rv_sec("Bindings · 1", binding, last=True),
    ])
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    tb = tabs([("shield", "ValidatingAdmissionPolicies", True), ("ok", "ValidatingWebhooks", False)])
    return rv_app("Admission policies — Kubyl", "ValidatingAdmissionPolicies", ("Cluster",), tb, content)

def gateway_screen():
    cols = "grid-template-columns: minmax(0,1fr) 96px 130px 120px 90px 70px 54px"
    rows = [[m("web-gateway", s=12), m("kubyl-fake", "var(--muted)"), m("172.18.0.240", "var(--muted)"), tpill("Programmed", C["green"]), m("2"), m("3"), m("12m", "var(--muted)")],
            [m("internal", s=12), m("cilium", "var(--muted)"), m("&lt;pending&gt;", "var(--dim)"), tpill("Pending", C["yellow"]), m("1"), m("0", "var(--dim)"), m("2d", "var(--muted)")]]
    center = rv_list("globe", "Gateways", "2 in kubyl-views", cols, ["NAME", "CLASS", "ADDRESSES", "PROGRAMMED", "LISTENERS", "ROUTES", "AGE"], rows,
                     [("↵", "Details"), ("d", "Describe"), ("e", "Edit YAML"), ("⌃d", "Delete"), ("/", "Filter")])
    listener = lambda name, proto, host, routes: card(
        f'<div style="display:flex;gap:6px;align-items:center"><b style="font-weight:500">{name}</b><span class="chip mchip" style="height:18px">{proto}</span>{m(host, "var(--muted)")}<span style="flex:1"></span>'
        f'<span class="pill">{ic("ok",12,C["green"])}Programmed</span></div><div style="font-size:11.5px;color:var(--dim)">{routes}</div>')
    route = lambda kind, name, via: f'<div style="display:flex;align-items:center;gap:7px;padding:3px 0;font-size:12px">{ic("ok",12,C["green"])}<span style="color:var(--dim)">{kind}</span>{lk(name)}<span style="flex:1"></span><span style="color:var(--dim);font-size:11.5px">{via}</span></div>'
    dock = rv_dock("Gateway details", 400, [
        rv_head("web-gateway", tpill("Accepted", C["green"]) + tpill("Programmed", C["green"]) + '<span class="chip">age 12m</span>', "kubyl-views"),
        rv_sec("Gateway", '<div style="display:flex;flex-direction:column;gap:4px">' + rkv("Class", lk("kubyl-fake") + ' <span style="color:var(--dim)">· kubyl.dev/fake-gateway-controller</span>', 70) + rkv("Addresses", m("172.18.0.240") + ' <span style="color:var(--dim)">IPAddress</span>', 70) + "</div>"),
        rv_sec("Listeners · 2", '<div style="display:flex;flex-direction:column;gap:6px">' + listener("http", "HTTP :80", "any host", "1 route attached · routes from the same namespace")
               + listener("https", "HTTPS :443", "*.shop.example.com", "2 routes attached · routes from all namespaces · TLS Terminate · Secret shop-tls") + "</div>"),
        rv_sec("Attached routes · 2", route("HTTPRoute", "shop", "http, https") + route("GRPCRoute", "checkout", "https"), last=True),
    ])
    svc_card = f'''<div style="position:absolute;left:300px;bottom:44px;width:440px;background:var(--panel);border:1px solid var(--border);border-radius:8px;box-shadow:0 12px 34px rgba(0,0,0,.45);overflow:hidden">
<div class="phead" style="border-bottom:1px solid var(--bv)"><span style="flex:1;color:var(--text);font-weight:500">Service details · web</span></div>
{rv_sec("Routes · 1", f'<div style="display:flex;flex-direction:column;gap:3px;font-size:12px"><div style="display:flex;gap:7px;align-items:center">{ic("ok",12,C["green"])}<span style="color:var(--dim)">HTTPRoute</span>{lk("shop")}<span style="flex:1"></span><span style="color:var(--dim);font-size:11.5px">port 80 · weight 90</span></div>'
        f'<div style="font-size:11.5px;color:var(--dim);padding-left:19px">shop.example.com, www.shop.example.com · via {lk("web-gateway", 11)}</div></div>')}
{rv_sec("EndpointSlices · 1", f'<div style="display:flex;gap:7px;align-items:center;font-size:12px">{lk("web-x8f2k")}<span style="color:var(--dim)">IPv4 · 2/2 ready · http 80/TCP</span></div>', last=True)}
</div>'''
    content = f'<div style="flex:1;display:flex;min-height:0;position:relative">{center}{dock}</div>'
    tb = tabs([("globe", "Gateways", True), ("right", "HTTPRoutes", False), ("network", "Services", False)])
    inner = f'''<div class="app">
{titlebar(RV_CLUSTER, "kubyl-views", False, "kind · v1.37.0")}
<div class="body">{rv_sidebar("Gateways", ("Network",))}<main class="main">{tb}{content}</main></div>
{statusbar(cluster=RV_CLUSTER, ns="kubyl-views")}
{svc_card}
</div>'''
    return page("Gateway API — Kubyl", inner)

def vpa_screen():
    cols = "grid-template-columns: minmax(0,1fr) 170px 100px 210px 54px"
    rows = [[m("web", s=12), lk("Deployment/web", 12), m("Off", "var(--dim)"), m("nginx 80m / 96Mi"), m("12m", "var(--muted)")],
            [m("ledger-writer", s=12), lk("StatefulSet/ledger-writer", 12), m("InPlaceOrRecreate"), m("app 410m / 1.2Gi"), m("9d", "var(--muted)")],
            [m("batch-scorer", s=12), lk("CronJob/batch-scorer", 12), m("Initial"), m("—", "var(--faint)"), m("1m", "var(--muted)")]]
    center = rv_list("activity", "VerticalPodAutoscalers", "3 in kubyl-views", cols, ["NAME", "TARGET", "UPDATE MODE", "RECOMMENDATION (CPU / MEMORY)", "AGE"], rows,
                     [("↵", "Details"), ("d", "Describe"), ("e", "Edit YAML"), ("⌃d", "Delete"), ("/", "Filter")])
    grid = "display:grid;grid-template-columns:70px repeat(4,minmax(0,1fr));gap:4px 8px;font-size:12px;align-items:center"
    def rec(container, rows_, note=""):
        cells = "".join(f'<span style="color:var(--dim)">{r}</span>' + "".join(m(v, col) for v, col in vals) for r, vals in rows_)
        return card(f'<div style="display:flex;gap:6px;align-items:center"><b style="font-weight:500">{container}</b><span style="flex:1"></span><span style="font-size:11.5px;color:var(--dim)">{note}</span></div>'
                    f'<div style="{grid}"><span></span>' + "".join(f'<span style="color:var(--dim);font-size:11px;text-transform:uppercase;letter-spacing:.05em">{h}</span>' for h in ["Requests", "Lower", "Target", "Upper"]) + cells + "</div>")
    dock = rv_dock("VerticalPodAutoscaler details", 430, [
        rv_head("web", tpill("RecommendationProvided", C["green"]) + '<span class="chip">mode Off</span><span class="chip">age 12m</span>', "kubyl-views"),
        rv_sec("Target", '<div style="display:flex;flex-direction:column;gap:4px">' + rkv("Workload", f'<span style="color:var(--dim)">Deployment</span> {lk("web")}', 80) + rkv("Update mode", "Off <span style=color:var(--dim)>· recommendations only</span>", 80) + "</div>"),
        rv_sec("Recommendations · 1 of 2 containers", '<div style="display:flex;flex-direction:column;gap:6px">'
               + rec("nginx", [("CPU", [("50m", "var(--muted)"), ("25m", "var(--muted)"), ("80m", C["accent"]), ("320m", "var(--muted)")]),
                               ("Memory", [("64Mi", "var(--muted)"), ("48Mi", "var(--muted)"), ("96Mi", C["accent"]), ("256Mi", "var(--muted)")])], "target above requests")
               + card('<div style="display:flex;gap:6px;align-items:center"><b style="font-weight:500">metrics</b><span style="flex:1"></span><span style="font-size:11.5px;color:var(--dim)">mode Off · not scaled</span></div>')
               + "</div>"),
        rv_sec("Conditions", f'<span class="pill" style="font-size:12px">{ic("ok",12,C["green"])}RecommendationProvided</span>', last=True),
    ])
    content = f'<div style="flex:1;display:flex;min-height:0">{center}{dock}</div>'
    tb = tabs([("activity", "VerticalPodAutoscalers", True), ("layers", "Deployments", False)])
    return rv_app("VerticalPodAutoscalers — Kubyl", "VerticalPodAutoscalers", ("Workloads",), tb, content)


# ---------- 23 · Lens parity (phase 25) ----------
def menu_item(label, checked=None, icon=None, kbd=None, dim=False, on=False):
    lead = ic("check", 12, C["accent"], 2.5) if checked else ('<span style="width:12px"></span>' if checked is False else (ic(icon, 12, "currentColor") if icon else ""))
    k = f'<span class="kbd" style="margin-left:auto">{kbd}</span>' if kbd else ""
    bg = "background:var(--sel);" if on else ""
    col = "var(--faint)" if dim else "var(--text)"
    return f'<div style="display:flex;align-items:center;gap:8px;height:26px;padding:0 10px;border-radius:4px;color:{col};{bg}">{lead}<span>{label}</span>{k}</div>'

def menu_box(items, left, top, width=220):
    return (f'<div style="position:absolute;left:{left}px;top:{top}px;width:{width}px;padding:4px;border-radius:7px;background:var(--elev);border:1px solid var(--border);'
            f'box-shadow:0 8px 24px rgba(0,0,0,.45);font-size:12.5px;z-index:5">' + "".join(items) + "</div>")

def menu_sep(): return '<div style="height:1px;background:var(--bv);margin:4px 2px"></div>'
def menu_label(t): return f'<div style="padding:4px 10px;font-size:11px;font-weight:600;letter-spacing:.06em;text-transform:uppercase;color:var(--dim)">{t}</div>'

def toast(kind, text, left=None, bottom=44, right=16, buttons=""):
    col = {"ok": C["green"], "err": C["red"], "info": C["accent"]}[kind]
    icon = {"ok": "ok", "err": "alert", "info": "info"}[kind]
    return (f'<div style="position:absolute;right:{right}px;bottom:{bottom}px;width:380px;padding:10px 12px;border-radius:7px;background:var(--elev);border:1px solid var(--border);'
            f'box-shadow:0 8px 24px rgba(0,0,0,.45);display:flex;gap:9px;align-items:flex-start;font-size:12.5px;z-index:6">{ic(icon, 14, col)}'
            f'<div style="flex:1;line-height:18px">{text}</div>{buttons}</div>')

def csv_screen():
    cols = "grid-template-columns: minmax(0,1fr) 60px 120px 70px 80px 90px 150px 54px"
    pods = [("api-7d9c5b6f4-2xk8q", "1/1", "Running", "0", "184m", "212Mi", "node-a", "3d"),
            ("api-7d9c5b6f4-9wm4z", "1/1", "Running", "0", "171m", "208Mi", "node-b", "3d"),
            ("checkout-5b7d8f9c6-4hq2n", "0/1", "CrashLoopBackOff", "12", "—", "—", "node-b", "41m"),
            ("ledger-0", "1/1", "Running", "0", "402m", "1.1Gi", "node-c", "9d"),
            ("web-6c8f7d55b-m2xvq", "2/2", "Running", "1", "36m", "96Mi", "node-a", "5h")]
    def tone(st): return C["green"] if st == "Running" else C["red"]
    rows = [[m(n, s=12), m(r), tpill(st, tone(st)), m(rs), m(cpu), m(mem), m(node, "var(--muted)"), m(age, "var(--muted)")] for n, r, st, rs, cpu, mem, node, age in pods]
    center = rv_list("box", "Pods", "5 in shop", cols, ["NAME", "READY", "STATUS", "RESTARTS", "CPU", "MEMORY", "NODE", "AGE"], rows,
                     [("↵", "Details"), ("l", "Logs"), ("s", "Shell"), ("e", "Edit YAML"), ("/", "Filter"), (":", "Kinds")])
    menu = menu_box([menu_label("Columns"), menu_item("Ready", True), menu_item("Status", True), menu_item("Restarts", True), menu_item("CPU", True), menu_item("Memory", True),
                     menu_item("Node", True), menu_item("Age", True), menu_item("IP", False), menu_sep(), menu_item("Wide (-o wide)", False), menu_sep(),
                     menu_item("Export CSV…", icon="download", on=True)], 1130, 74)
    t = toast("ok", 'Saved <b style="font-weight:500">5 rows</b> to <span class="mono" style="font-size:11.5px">~/Downloads/pods-shop-20261009-130509.csv</span>')
    note = ('<div style="position:absolute;left:300px;bottom:44px;width:420px;padding:9px 12px;border-radius:7px;background:var(--elev);border:1px solid var(--border);font-size:12px;line-height:18px;color:var(--muted);z-index:4">'
            f'{ic("info",13,C["accent"])} The file holds what the table shows: <b style="font-weight:500;color:var(--text)">visible columns</b> in the current <b style="font-weight:500;color:var(--text)">sort and filter</b>, related columns included. '
            'Secret lists export key counts only. Cells that start with <span class="mono">= + - @</span> get a leading <span class="mono">\'</span> so spreadsheets never run them.</div>')
    content = f'<div style="flex:1;display:flex;min-height:0">{center}</div>'
    tb = tabs([("box", "Pods", True), ("layers", "Deployments", False)])
    return rv_app("Export CSV — Kubyl", "Pods", ("Workloads",), tb, content, ns="shop", overlay=note + menu + t)


SCREENS = [
 ("Main.dc.html", "1 · Pods (k9s-style table + details)", pods_screen),
 ("Routes.dc.html", "1 · OpenShift Routes under Network, with details", routes_screen),
 ("Logs.dc.html", "2 · Live logs, exec shell, port-forwards", logs_screen),
 ("Yaml.dc.html", "3 · YAML editor with CRD schema", yaml_screen),
 ("Overview.dc.html", "4 · Overview, Prometheus metrics, events", overview_screen),
 ("Clusters.dc.html", "5 · Kubeconfigs, contexts, OIDC sign-in", clusters_screen),
 ("Palette.dc.html", "6 · Command palette (:resources, @contexts)", palette_screen),
 ("Operators.dc.html", "7 · Installed operators: pending upgrade, provided APIs", operators_screen),
 ("OperatorHub.dc.html", "7 · OperatorHub: catalog with search, categories, provider, capability level", operatorhub_screen),
 ("OperatorInstall.dc.html", "7 · Install an operator: channel, install mode, approval, namespace", install_screen),
 ("OperatorUpgrade.dc.html", "7 · Approve an upgrade: CRD schema diff, RBAC changes, compatibility", upgrade_screen),
 ("OperatorCreate.dc.html", "7 · Provided APIs: Create from alm-examples", create_screen),
 ("InstallPlans.dc.html", "7 · Install plans (pending first)", plans_screen),
 ("Subscriptions.dc.html", "7 · Subscriptions", subscriptions_screen),
 ("HelmReleases.dc.html", "7 · Helm releases", helm_screen),
 ("HelmRelease.dc.html", "7 · Helm release: values (masked), manifest, notes, history", helm_release_screen),
 ("OlmStates.dc.html", "7 · OLM not installed, and OLM v1 extensions", olm_states_screen),
 ("Updates.dc.html", "8 · Cluster updates (OpenShift): cluster operators, pools and update history", updates_screen),
 ("UpdatesChannel.dc.html", "8 · OpenShift: version card and channel selector", updates_channel_screen),
 ("UpdatesGraph.dc.html", "8 · Update graph: recommended, conditional and blocked", updates_graph_screen),
 ("UpdatesPreflight.dc.html", "8 · Pre-flight checks", updates_preflight_screen),
 ("UpdatesProgress.dc.html", "8 · Progress during an update", updates_progress_screen),
 ("UpdatesConfirm.dc.html", "8 · Update confirmation", updates_confirm_screen),
 ("UpdatesEks.dc.html", "8 · Amazon EKS: control plane, node groups, add-ons", updates_eks_screen),
 ("UpdatesK3s.dc.html", "8 · k3s: system-upgrade-controller plans", updates_k3s_screen),
 ("UpdatesSelfManaged.dc.html", "8 · Self-managed / read-only fallback", updates_selfmanaged_screen),
 ("UpdatesCredentials.dc.html", "8 · Credentials missing / provider not available", updates_credentials_screen),
 ("Files.dc.html", "9 · Pod file browser — drag & drop upload/download", files_screen),
 ("Webview.dc.html", "10 · Service web view over a temporary port-forward", webview_screen),
 ("RouteWebview.dc.html", "10 · Route web view and Open in browser", route_webview_screen),
 ("Kubeconfig.dc.html", "11 · Kubeconfig editor: contexts, clusters, users (form)", kubeconfig_screen),
 ("KubeconfigYaml.dc.html", "11 · Kubeconfig editor: YAML tab and Test connection", kubeconfig_yaml_screen),
 ("KubeconfigWizard.dc.html", "11 · New kubeconfig wizard (CA fetched from the server)", kubeconfig_wizard_screen),
 ("KubeconfigSave.dc.html", "11 · Save preview and exec plugin consent", kubeconfig_save_screen),
 ("ArgoApps.dc.html", "12 · Argo CD applications (sync, health, filters)", argo_apps_screen),
 ("ArgoApp.dc.html", "13 · Argo CD application: resource tree and summary", argo_app_screen),
 ("ArgoHistory.dc.html", "14 · Argo CD application: history and rollback", argo_history_screen),
 ("ArgoSync.dc.html", "15 · Argo CD sync dialog (options, selective sync)", argo_sync_screen),
 ("Alerts.dc.html", "16 · Alerts: firing alerts, details, sidebar badge and status bar", alerts_screen),
 ("AlertsStates.dc.html", "16 · Alerts: all clear, and no Alertmanager found", alerts_states_screen),
 ("Silences.dc.html", "16 · Silences and the silence editor", silences_screen),
 ("SilenceConfirm.dc.html", "16 · Silence summary on a production cluster", silence_confirm_screen),
 ("AlertRules.dc.html", "16 · Alerting rules and rule health", rules_screen),
 ("ClusterStatus.dc.html", "17 · Connection dots, one row per cluster and user", clusters_grouped_screen),
 ("ConfigMapData.dc.html", "17 · ConfigMap data in the details", configmap_screen),
 ("ContextsPalette.dc.html", "17 · @ contexts with status dots and aliases; a revealed Secret", contexts_palette_screen),
 ("NetworkFlows.dc.html", "18 · Network flows: live table, filter chips and completion, flow details, backend indicator", network_flows_screen),
 ("NetworkTopology.dc.html", "18 · Topology at namespace zoom, a namespace selected", network_topology_screen),
 ("NetworkTopologyWorkloads.dc.html", "18 · Topology at workload zoom, a denied connection selected", network_topology_workloads_screen),
 ("NetworkBackends.dc.html", "18 · Calico Whisker (aggregated records, policy trace) and NetObserv without Loki (metrics only)", network_backends_screen),
 ("NetworkStates.dc.html", "18 · No flow source (what to install for the CNI) and a forbidden port-forward", network_states_screen),
 ("Agent.dc.html", "19 · Agent panel: a thread with Kubyl tool calls, a plan and a command to approve", agent_screen),
 ("AgentStates.dc.html", "19 · Agent: new thread, agent picker, first-run note, sign-in and stopped states", agent_states_screen),
 ("AgentQuestions.dc.html", "19 · Agent questions: a form, a link to open, a file change", agent_questions_screen),
 ("HelmCharts.dc.html", "20 · Helm charts: repositories, search, a chart's details (versions, README, default values)", helm_charts_screen),
 ("HelmInstall.dc.html", "20 · Install a chart: name, namespace, version, values editor with the chart's schema, options", helm_install_screen),
 ("HelmInstallPreview.dc.html", "20 · Install preview: the dry run's objects by kind (Secrets masked), hooks, CRDs; typed name on PROD", helm_install_preview_screen),
 ("HelmUpgrade.dc.html", "20 · Upgrade review: per-object manifest diff, values diff (masked), CRDs Helm won't upgrade", helm_upgrade_screen),
 ("HelmRollback.dc.html", "20 · Roll back: pick a revision, what changes", helm_rollback_screen),
 ("HelmUninstall.dc.html", "20 · Uninstall: what gets deleted, what stays", helm_uninstall_screen),
 ("HelmRepos.dc.html", "20 · Repositories and OCI registry login", helm_repos_screen),
 ("HelmMissing.dc.html", "20 · helm isn't installed; a release stuck in pending-upgrade; an upgrade running", helm_missing_screen),
 ("FluxOverview.dc.html", "21 · Flux overview: health, counts, needs attention, recent activity, controllers", flux_overview_screen),
 ("FluxKustomizations.dc.html", "21 · Flux Kustomizations with the details dock (waiting for a dependency)", flux_kustomizations_screen),
 ("FluxKustomization.dc.html", "21 · Kustomization: conditions, source and revision, settings, dependencies", flux_kustomization_screen),
 ("FluxInventory.dc.html", "21 · Kustomization inventory: applied objects and their children from Kubyl's caches", flux_inventory_screen),
 ("FluxHelmRelease.dc.html", "21 · HelmRelease: chart, values sources, remediation, releases, link to the Helm release", flux_helmrelease_screen),
 ("FluxSources.dc.html", "21 · Flux sources (all source kinds) with what uses them", flux_sources_screen),
 ("FluxManaged.dc.html", "21 · Managed by Flux in an object's details; the Flux column in Deployments", flux_managed_screen),
 ("FluxSuspend.dc.html", "21 · Multi-select and the suspend confirmation", flux_actions_screen),
 ("FluxDelete.dc.html", "21 · Delete on PROD: what prune removes, typed confirmation", flux_delete_screen),
 ("DeviceResources.dc.html", "22 · Resource views: Device Resources, a claim's requests, allocation and reserved-for pods", dra_claims_screen),
 ("ResourceSlices.dc.html", "22 · Resource views: a slice's devices, a pod's Resource Claims and a node's Devices", dra_devices_screen),
 ("AdmissionPolicies.dc.html", "22 · Resource views: admission policies under Cluster, CEL validations and bindings", admission_screen),
 ("GatewayApi.dc.html", "22 · Resource views: a Gateway's listeners and attached routes, a Service's Routes", gateway_screen),
 ("VerticalPodAutoscalers.dc.html", "22 · Resource views: VerticalPodAutoscalers with recommendations", vpa_screen),
 ("CsvExport.dc.html", "23 · Lens parity: Export CSV from any table (the Columns menu, a toast)", csv_screen),
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
