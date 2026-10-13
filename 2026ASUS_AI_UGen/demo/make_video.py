"""EdgeOrtho: assemble a draft of the Stage I demo video (English narration, burned-in captions, SRT).

Everything is produced locally and automatically:
  * narration  : Windows SAPI voice (System.Speech), one WAV per scene
  * scenes 1-3 : title / need / architecture cards drawn with PIL
  * scene 4    : headless-Edge screen capture of the viewer while synthetic frames are ingested (mosaic grows)
  * scene 5    : fly-in on the candidates (map.flyTo captured frame by frame), detections off/on
  * scene 6    : real console output of scripts/sim/test_audit_chain.py (tamper test) against the same server
  * scene 7    : bar chart drawn from the measured/estimated numbers (no imagery)
  * scene 8    : Stage II card and end card with attribution
The always-on label (synthetic replay / precomputed detections / desktop GPU / no UGen300 measurements) is burned into every frame.

Usage: python make_video.py --sim <river_sim dir> --out <output dir> [--voice "Microsoft Zira Desktop"] [--fps 15]
Needs: ffmpeg on PATH, Edge, `pip install websockets pillow`, server built with --features ingest.
"""
import argparse, asyncio, base64, io, json, os, subprocess, sys, tempfile, textwrap, threading, time, urllib.request, wave
from concurrent.futures import ThreadPoolExecutor

import websockets
from PIL import Image, ImageDraw, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import run_demo as R  # build_config(), post(), constants

W, H = 1920, 1080
NAVY, BLUE, SILVER, WHITE, AMBER, GREEN, RED = (11, 31, 58), (0, 151, 224), (201, 209, 219), (255, 255, 255), (245, 166, 35), (18, 160, 107), (185, 58, 50)
LABEL = "Synthetic flight replay · Precomputed detections · Desktop GPU prototype · No UGen300 measurements yet"
FONTS = r"C:\Windows\Fonts"


def font(size, bold=False):
    return ImageFont.truetype(os.path.join(FONTS, "arialbd.ttf" if bold else "arial.ttf"), size)


# ----------------------------------------------------------------------------- narration (subtitle text / spoken text)
SCENES = [
    ("problem", "River patrols are flown by drone, but the results usually arrive after the flight, as files, apart from the map.",
     "River patrols are flown by drone, but the results usually arrive after the flight, as files, apart from the map."),
    ("need", "Public agencies also need standard WMTS delivery, role-based access, an audit trail, and availability reporting. EdgeOrtho brings these together.",
     "Public agencies also need standard W M T S delivery, role-based access, an audit trail, and availability reporting. Edge Ortho brings these together."),
    ("idea", "Each frame is georeferenced and merged into a tile pyramid within seconds. In stage two, a detector running on an edge host with an ASUS UGen300 will flag objects of interest at capture time.",
     "Each frame is georeferenced and merged into a tile pyramid within seconds. In stage two, a detector running on an edge host with an ASUS U Gen three hundred will flag objects of interest at capture time."),
    ("live", "This is a replay of a simulated flight over the Touchian River in Hsinchu, built from real orthophotography at 3.6 centimetres per pixel. As frames arrive, the map grows tile by tile.",
     "This is a replay of a simulated flight over the Touchian River in Hsinchu, built from real orthophotography at three point six centimetres per pixel. As frames arrive, the map grows tile by tile."),
    ("cand", "The detections you see now were computed ahead of time on a desktop GPU. Objects around the channel and embankment appear as candidates, for human review. They are not verified findings, and the labels are still noisy.",
     "The detections you see now were computed ahead of time on a desktop G P U. Objects around the channel and embankment appear as candidates, for human review. They are not verified findings, and the labels are still noisy."),
    ("gov", "Access is role-based. Denied requests, uploads and exports go into a hash-chained log. Change one row, and verification fails.",
     "Access is role-based. Denied requests, uploads and exports go into a hash-chained log. Change one row, and verification fails."),
    ("scale", "In a simulation with 22 drones uploading at once, tile-level locking cut the median capture-to-visible delay from 79 seconds to 4.4. For five-centimetre imagery, our capacity model estimates one frame every three to four seconds per drone. That is an estimate, not a measurement.",
     "In a simulation with twenty-two drones uploading at once, tile-level locking cut the median capture-to-visible delay from seventy-nine seconds to four point four. For five-centimetre imagery, our capacity model estimates one frame every three to four seconds per drone. That is an estimate, not a measurement."),
    ("stage2", "Stage two measures what is still missing: detector quality on a labelled set, and latency and throughput on real hardware. EdgeOrtho.",
     "Stage two measures what is still missing: detector quality on a labelled set, and latency and throughput on real hardware. Edge Ortho."),
]


# ----------------------------------------------------------------------------- drawing helpers
def canvas(bg=NAVY):
    return Image.new("RGB", (W, H), bg)


def wrap(draw, text, fnt, maxw):
    words, lines, cur = text.split(), [], ""
    for w in words:
        t = (cur + " " + w).strip()
        if draw.textlength(t, font=fnt) <= maxw: cur = t
        else: lines.append(cur); cur = w
    if cur: lines.append(cur)
    return lines


def pill(d, x, y, text, fill, fg=WHITE, size=26):
    f = font(size, True); w = int(d.textlength(text, font=f)) + 36
    d.rounded_rectangle((x, y, x + w, y + size + 22), radius=(size + 22) // 2, fill=fill)
    d.text((x + 18, y + 9), text, font=f, fill=fg)
    return w


def overlay(img, caption=None):
    """Always-on label bottom-left and (optional) caption in the lower third."""
    img = img.copy(); d = ImageDraw.Draw(img, "RGBA")
    d.rectangle((0, H - 44, W, H), fill=(11, 31, 58, 235))
    d.text((24, H - 36), LABEL, font=font(22, True), fill=(255, 255, 255, 255))
    if caption:
        f = font(36); lines = wrap(d, caption, f, W - 240)
        hh = len(lines) * 46 + 28
        y0 = H - 44 - hh - 20
        d.rounded_rectangle((100, y0, W - 100, y0 + hh), radius=14, fill=(11, 31, 58, 215))
        for i, ln in enumerate(lines):
            d.text((130, y0 + 14 + i * 46), ln, font=f, fill=(255, 255, 255, 255))
    return img


def card_title():
    im = canvas(); d = ImageDraw.Draw(im)
    d.text((120, 250), "EdgeOrtho", font=font(150, True), fill=WHITE)
    d.text((124, 440), "River patrol, mapped as it is flown", font=font(54), fill=SILVER)
    d.text((124, 560), "Results arrive after the flight, as files, apart from the map.", font=font(40), fill=BLUE)
    pill(d, 124, 700, "WORKPLACE AI", BLUE); pill(d, 400, 700, "BATTLEFIELD LIGHTNING · UGen300 / HAILO-10H", (27, 58, 99))
    d.text((124, 800), "2026 ASUS UGen AI League Innovation Hackathon · Stage I · Team Sat_Dashboard", font=font(30), fill=SILVER)
    return im


def card_need():
    im = canvas(); d = ImageDraw.Draw(im)
    d.text((120, 120), "What public agencies ask for", font=font(60, True), fill=WHITE)
    items = [("Immediate", "tiles within seconds of upload"), ("Governed", "role-based access · audit trail · availability report"), ("Standard", "OGC WMTS, EPSG:3857 and EPSG:3826")]
    for i, (a, b) in enumerate(items):
        y = 320 + i * 190
        d.rounded_rectangle((120, y, W - 120, y + 150), radius=20, fill=(16, 41, 75))
        d.text((170, y + 28), a, font=font(60, True), fill=BLUE)
        d.text((620, y + 46), b, font=font(44), fill=WHITE)
    return im


def card_arch(stage2=True):
    im = canvas(); d = ImageDraw.Draw(im)
    d.text((120, 100), "From drone frame to governed, review-ready map", font=font(56, True), fill=WHITE)
    boxes = [("Drone", "JPEG + EXIF/XMP pose", False), ("Edge host + UGen300", "decode · tile · infer · georeference", True),
             ("Server", "auth · role · ingest · mosaic · pyramid · WMTS", False), ("Viewer", "live mosaic · candidates · review", False)]
    bw, gap, x0, y0 = 380, 70, 110, 330
    for i, (a, b, planned) in enumerate(boxes):
        x = x0 + i * (bw + gap)
        d.rounded_rectangle((x, y0, x + bw, y0 + 300), radius=20, fill=(16, 41, 75), outline=(BLUE if planned else (54, 82, 122)), width=4)
        d.text((x + 24, y0 + 30), a, font=font(40, True), fill=WHITE)
        for j, ln in enumerate(wrap(d, b, font(30), bw - 48)): d.text((x + 24, y0 + 110 + j * 40), ln, font=font(30), fill=SILVER)
        if planned: pill(d, x + 24, y0 + 230, "PLANNED · STAGE II", AMBER, size=22)
        else: pill(d, x + 24, y0 + 230, "BUILT" if i else "INPUT", GREEN if i else (54, 82, 122), size=22)
        if i < 3: d.polygon([(x + bw + 12, y0 + 140), (x + bw + 58, y0 + 160), (x + bw + 12, y0 + 180)], fill=BLUE)
    d.text((120, 760), "Detections travel as compact records alongside each frame; candidates are for human review.", font=font(38), fill=SILVER)
    return im


def card_terminal(lines, title="scripts/sim/test_audit_chain.py"):
    im = canvas((18, 18, 24)); d = ImageDraw.Draw(im)
    d.rounded_rectangle((100, 90, W - 100, H - 150), radius=18, fill=(28, 30, 38))
    d.text((140, 110), "$ python " + title, font=font(34, True), fill=(120, 200, 255))
    d.text((140, 152), "Audit chain: verify, export, then one database row is altered and verification must fail (console output)", font=font(24), fill=(170, 170, 180))
    f = ImageFont.truetype(os.path.join(FONTS, "msjh.ttc"), 28)
    y = 215
    for ln in lines[-16:]:
        col = (110, 230, 150) if "PASS" in ln else ((255, 120, 110) if "FAIL" in ln else (220, 220, 225))
        d.text((140, y), ln[:92], font=f, fill=col); y += 40
    return im


def card_scale():
    im = canvas(); d = ImageDraw.Draw(im)
    d.text((120, 100), "22 simulated drones uploading at once", font=font(60, True), fill=WHITE)
    d.text((120, 190), "median capture → tile visible", font=font(36), fill=SILVER)
    for i, (lab, val, col) in enumerate([("single global lock", 79.0, RED), ("tile-level locks", 4.4, GREEN)]):
        y = 300 + i * 170
        d.text((120, y + 30), lab, font=font(38), fill=WHITE)
        bw = int(val / 79.0 * 1100)
        d.rounded_rectangle((620, y, 620 + max(bw, 14), y + 100), radius=12, fill=col)
        d.text((620 + max(bw, 14) + 24, y + 24), f"{val:g} s", font=font(54, True), fill=WHITE)
    d.rounded_rectangle((120, 700, W - 120, 840), radius=18, fill=(40, 30, 10))
    d.text((160, 720), "Simulation · one machine · 0.265 m GSD", font=font(34), fill=(255, 213, 138))
    d.text((160, 775), "5 cm imagery: about one frame every 3.7 s per drone — an estimate, not a measurement", font=font(34), fill=(255, 213, 138))
    return im


def card_stage2():
    im = canvas(); d = ImageDraw.Draw(im)
    d.text((120, 100), "Stage II: measure what is still missing", font=font(60, True), fill=WHITE)
    rows = [("10/26 – 11/6", "Receive UGen300 · pin toolchain · reference model on the real host"),
            ("11/6 – 11/20", "Labelled river set · train detector · held-out evaluation"),
            ("11/20 – 12/4", "Compile · sidecar upload · candidates API · long run"),
            ("12/4 – 12/10", "Finals package, reproducible from the repository")]
    for i, (a, b) in enumerate(rows):
        y = 270 + i * 130
        d.text((120, y), a, font=font(42, True), fill=BLUE); d.text((560, y + 2), b, font=font(40), fill=WHITE)
    d.text((120, 850), "EdgeOrtho · Team Sat_Dashboard · github.com/RhynoW/EdgeOrtho", font=font(40, True), fill=SILVER)
    return im


def card_end():
    im = canvas(); d = ImageDraw.Draw(im)
    d.text((120, 230), "EdgeOrtho", font=font(130, True), fill=WHITE)
    d.text((124, 400), "On-device AI turns drone imagery into immediate, auditable river-patrol map intelligence", font=font(40), fill=SILVER)
    attr = ["Imagery: Water Resources Agency MOEA, Hsinchu City Environmental Protection Bureau,",
            "Taiwan Ecological Society, FeiMao Spatial Information (CC BY 4.0, via OpenAerialMap).",
            "Base map © OpenStreetMap contributors. Detector: OWLv2 (Apache-2.0)."]
    for i, ln in enumerate(attr): d.text((124, 640 + i * 46), ln, font=font(30), fill=SILVER)
    d.text((124, 560), "Team Sat_Dashboard · 2026 ASUS UGen AI League · Stage I", font=font(34, True), fill=BLUE)
    return im


# ----------------------------------------------------------------------------- narration audio
def tts(texts, voice, outdir, rate):
    ps = os.path.join(outdir, "tts.ps1")
    open(ps, "w", encoding="utf8").write(
        "param($textFile,$out,$voice,$rate)\nAdd-Type -AssemblyName System.Speech\n$s=New-Object System.Speech.Synthesis.SpeechSynthesizer\n"
        "$s.SelectVoice($voice)\n$s.Rate=[int]$rate\n$s.SetOutputToWaveFile($out)\n$s.Speak([IO.File]::ReadAllText($textFile))\n$s.Dispose()\n")
    wavs = []
    for i, t in enumerate(texts):
        tf = os.path.join(outdir, f"n{i}.txt"); open(tf, "w", encoding="utf8").write(t)
        wv = os.path.join(outdir, f"n{i}.wav")
        subprocess.run(["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", ps, tf, wv, voice, str(rate)], check=True, capture_output=True)
        wavs.append(wv)
    return wavs


def wav_info(p):
    with wave.open(p) as w: return w.getframerate(), w.getnchannels(), w.getsampwidth(), w.getnframes() / w.getframerate()


# ----------------------------------------------------------------------------- screen capture (scenes 4-6)
async def capture_live(a, tmp, durations):
    """Returns (live_frames, cand_frames, terminal_lines). Frames are PIL images."""
    man = json.load(open(os.path.join(a.sim, "manifest.json"), encoding="utf8"))
    cfg = R.build_config(tmp, a.port, man["gsd_per_agl"])
    exe = os.path.join(R.ROOT, "target", "release", "wmts_server.exe" if os.name == "nt" else "wmts_server")
    env = dict(os.environ, WMTS_CONFIG=cfg, WMTS_JWT_SECRET="demo_secret_demo_secret_demo_secret_12", WMTS_APIKEY_GIS_CLIENT=R.KEY)
    srv = subprocess.Popen([exe], cwd=R.ROOT, env=env, stdout=open(os.path.join(tmp, "server.log"), "w"), stderr=subprocess.STDOUT)
    viewer = subprocess.Popen([sys.executable, "-m", "http.server", str(R.VIEW_PORT), "--bind", "127.0.0.1", "--directory", HERE], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    edge = subprocess.Popen([R.EDGE, "--headless=new", f"--remote-debugging-port={R.CDP}", f"--user-data-dir={tempfile.mkdtemp(prefix='vid_edge_')}", "--no-first-run", "--disable-extensions", "about:blank"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    live, cand, term = [], [], []
    try:
        for _ in range(60):
            time.sleep(0.5)
            try:
                if urllib.request.urlopen(f"http://127.0.0.1:{a.port}/healthz", timeout=2).status == 200: break
            except Exception: pass
        else: sys.exit("server did not start")
        tab = None
        for _ in range(40):
            try: tab = [t for t in json.load(urllib.request.urlopen(f"http://127.0.0.1:{R.CDP}/json")) if t["type"] == "page"][0]; break
            except Exception: time.sleep(.5)
        async with websockets.connect(tab["webSocketDebuggerUrl"], max_size=None) as ws:
            n, pend = 0, {}
            async def rd():
                async for raw in ws:
                    r = json.loads(raw)
                    if "id" in r and r["id"] in pend: pend.pop(r["id"]).set_result(r)
            asyncio.create_task(rd())
            async def call(m, **p):
                nonlocal n; n += 1; f = asyncio.get_event_loop().create_future(); pend[n] = f
                await ws.send(json.dumps({"id": n, "method": m, "params": p})); return await f
            async def ev(js): return (await call("Runtime.evaluate", expression=js, returnByValue=True, awaitPromise=True))["result"]["result"].get("value")
            async def shot():
                r = await call("Page.captureScreenshot", format="jpeg", quality=82)
                return Image.open(io.BytesIO(base64.b64decode(r["result"]["data"]))).convert("RGB")
            await call("Page.enable"); await call("Runtime.enable")
            await call("Emulation.setDeviceMetricsOverride", width=1280, height=int((H - 44) / 1.5), deviceScaleFactor=1.5, mobile=False)
            url = (f"http://127.0.0.1:{R.VIEW_PORT}/viewer.html?base=http://127.0.0.1:{a.port}&key={R.KEY}&layer=orthophoto_live&det={a.det}&live=0.8&lat=24.8255&lng=120.9832&z=19")
            await call("Page.navigate", url=url); await asyncio.sleep(3)
            await ev("document.getElementById('t-det').click(); 0")        # detections off while the mosaic grows
            # ---- scene 4: paced ingest while capturing
            frames = man["frames"]
            base = f"http://127.0.0.1:{a.port}"
            stop = threading.Event()
            def feeder():
                def one(f):
                    data = open(os.path.join(a.sim, "frames", f["file"]), "rb").read()
                    for _ in range(10):
                        s, j, dt = R.post(f"{base}/api/v1/ingest/frame?session_id=video", data)
                        if s in (503, 409): time.sleep(2); continue
                        return s
                    return 0
                with ThreadPoolExecutor(4) as ex:
                    for f in frames:
                        ex.submit(one, f); time.sleep(a.ingest_gap)
                stop.set()
            th = threading.Thread(target=feeder, daemon=True); th.start()
            t0 = time.time()
            while not stop.is_set() and time.time() - t0 < 90:
                live.append(await shot()); await asyncio.sleep(0.15)
            th.join(timeout=60); await asyncio.sleep(4.5)
            for _ in range(6): live.append(await shot())
            # ---- scene 5: fly-in on a candidate, then detections on/off
            await ev("document.getElementById('t-det').click(); 0"); await asyncio.sleep(1.5)
            for _ in range(10): cand.append(await shot()); await asyncio.sleep(0.1)
            target = await ev("(()=>{let best=null; detLayer.eachLayer(l=>{ if(!best) best=l; }); return best? [best.getBounds().getCenter().lat, best.getBounds().getCenter().lng]:null})()")
            if target:
                await ev(f"map.flyTo([{target[0]},{target[1]}], 21, {{duration: 7}}); 0")
                tt = time.time()
                while time.time() - tt < 7.5:
                    cand.append(await shot()); await asyncio.sleep(0.05)
                for _ in range(12): cand.append(await shot()); await asyncio.sleep(0.1)
                await ev("document.getElementById('t-det').click(); 0"); await asyncio.sleep(1.5)
                for _ in range(14): cand.append(await shot()); await asyncio.sleep(0.1)
                await ev("document.getElementById('t-det').click(); 0"); await asyncio.sleep(1.5)
                for _ in range(14): cand.append(await shot()); await asyncio.sleep(0.1)
        # ---- scene 6: real console output of the audit-chain tamper test against this server
        db = os.path.join(tmp, "demo.sqlite3")
        r = subprocess.run([sys.executable, "-X", "utf8", os.path.join(R.ROOT, "scripts", "sim", "test_audit_chain.py"), "--base", base, "--key", R.KEY, "--db", db],
                           capture_output=True, text=True, encoding="utf8", timeout=180)
        term = [l for l in (r.stdout + r.stderr).splitlines() if l.strip()]
    finally:
        for p in (edge, viewer, srv):
            try: p.kill()
            except Exception: pass
    return live, cand, term


def kokoro_wavs(a):
    """Neural narration through demo/kokoro_tts.py, run in the environment that has kokoro-onnx installed."""
    if not (a.kokoro_python and a.kokoro_models): sys.exit("--tts kokoro needs --kokoro-python and --kokoro-models")
    tj = os.path.join(a.out, "kokoro_texts.json")
    json.dump([s[2] for s in SCENES], open(tj, "w", encoding="utf8"), ensure_ascii=False)
    r = subprocess.run([a.kokoro_python, os.path.join(HERE, "kokoro_tts.py"), "--models", a.kokoro_models, "--texts", tj, "--out", a.out,
                        "--voice", a.kokoro_voice, "--speed", str(a.kokoro_speed)], capture_output=True, text=True)
    if r.returncode: sys.exit("kokoro failed: " + (r.stderr or r.stdout)[-400:])
    return [os.path.join(a.out, f"n{i}.wav") for i in range(len(SCENES))]


def human_wavs(folder, outdir):
    """Normalise the user's scene_1..scene_8 recordings to mono 44.1 kHz 16-bit WAV, trim leading/trailing silence."""
    out = []
    for i in range(1, len(SCENES) + 1):
        src = None
        for ext in ("wav", "m4a", "mp3", "aac", "flac", "ogg", "wma", "mp4"):
            c = os.path.join(folder, f"scene_{i}.{ext}")
            if os.path.exists(c): src = c; break
        if not src: sys.exit(f"missing recording: scene_{i}.(wav|m4a|mp3|aac|flac) in {folder}")
        dst = os.path.join(outdir, f"n{i - 1}.wav")
        af = "silenceremove=start_periods=1:start_threshold=-50dB,areverse,silenceremove=start_periods=1:start_threshold=-50dB,areverse,loudnorm=I=-16:TP=-1.5:LRA=11"
        r = subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", src, "-af", af, "-ac", "1", "-ar", "44100", "-sample_fmt", "s16", dst], capture_output=True, text=True)
        if r.returncode: sys.exit(f"ffmpeg failed on {src}: {r.stderr[:300]}")
        out.append(dst)
    return out


def save_capture(outdir, live, cand, term):
    d = os.path.join(outdir, "_capture"); os.makedirs(d, exist_ok=True)
    for name, frames in (("live", live), ("cand", cand)):
        for i, im in enumerate(frames): im.save(os.path.join(d, f"{name}_{i:04d}.jpg"), "JPEG", quality=90)
    json.dump(term, open(os.path.join(d, "term.json"), "w", encoding="utf8"), ensure_ascii=False)
    return live, cand, term


def load_capture(outdir):
    d = os.path.join(outdir, "_capture")
    if not os.path.isdir(d): sys.exit("no saved capture in " + d + " (run once without --reuse-capture)")
    def frames(prefix):
        fs = sorted(f for f in os.listdir(d) if f.startswith(prefix + "_") and f.endswith(".jpg"))
        return [Image.open(os.path.join(d, f)).convert("RGB") for f in fs]
    return frames("live"), frames("cand"), json.load(open(os.path.join(d, "term.json"), encoding="utf8"))


def sample(frames, n):
    if not frames: return []
    return [frames[min(len(frames) - 1, int(i * len(frames) / max(1, n)))] for i in range(n)]


def srt_time(t):
    h, m = int(t // 3600), int(t % 3600 // 60); s = t % 60
    return f"{h:02d}:{m:02d}:{int(s):02d},{int((s % 1) * 1000):03d}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sim", required=True); ap.add_argument("--out", required=True)
    ap.add_argument("--det", default="output/detections_river.geojson")
    ap.add_argument("--voice", default="Microsoft Zira Desktop"); ap.add_argument("--rate", type=int, default=0)
    ap.add_argument("--fps", type=int, default=15); ap.add_argument("--port", type=int, default=8098)
    ap.add_argument("--ingest-gap", type=float, default=0.11, help="seconds between frame submissions in the live scene")
    ap.add_argument("--narration-dir", default=None,
                    help="folder with your own recordings scene_1 ... scene_8 (wav, m4a, mp3, aac, flac); replaces the synthetic voice")
    ap.add_argument("--tts", choices=["sapi", "kokoro"], default="sapi", help="synthetic voice engine: Windows SAPI (default) or Kokoro-82M neural voice")
    ap.add_argument("--kokoro-python", default=None, help="python.exe of an environment with kokoro-onnx and soundfile installed")
    ap.add_argument("--kokoro-models", default=None, help="folder with kokoro-v1.0.int8.onnx (or kokoro-v1.0.onnx) and voices-v1.0.bin")
    ap.add_argument("--kokoro-voice", default="af_heart"); ap.add_argument("--kokoro-speed", type=float, default=1.0)
    ap.add_argument("--reuse-capture", action="store_true", help="reuse the screen capture saved in <out>/_capture (skips the 5-minute capture)")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    tmp = tempfile.mkdtemp(prefix="edgeortho_video_")
    try:
        print("1/4 narration ...", flush=True)
        wavs = (human_wavs(a.narration_dir, a.out) if a.narration_dir else
                kokoro_wavs(a) if a.tts == "kokoro" else tts([s[2] for s in SCENES], a.voice, a.out, a.rate))
        infos = [wav_info(w) for w in wavs]
        rate, ch, sw = infos[0][0], infos[0][1], infos[0][2]
        gap = 0.6
        MIN = {"problem": 10, "need": 11, "idea": 14, "live": 26, "cand": 32, "gov": 14, "scale": 20, "stage2": 18}
        durs = [round(max(i[3] + gap + 0.4, MIN[k]), 2) for (k, _, _), i in zip(SCENES, infos)]
        print("   narration seconds:", [round(i[3], 1) for i in infos], "total scenes", round(sum(durs), 1), flush=True)
        print("2/4 screen capture (server + headless Edge) ...", flush=True)
        live, cand, term = load_capture(a.out) if a.reuse_capture else save_capture(a.out, *asyncio.run(capture_live(a, tmp, durs)))
        print(f"   captured live {len(live)}, candidates {len(cand)}, console lines {len(term)}", flush=True)
        # per-scene frame generators
        fps = a.fps
        stills = {"problem": card_title(), "need": card_need(), "idea": card_arch(), "scale": card_scale(), "stage2": card_stage2()}
        term_img = card_terminal(term) if term else card_terminal(["(console output unavailable)"])
        scene_frames = []
        for (key, cap, _), dur in zip(SCENES, durs):
            n = int(round(dur * fps))
            if key == "live": src = sample(live, n)
            elif key == "cand": src = sample(cand, n)
            elif key == "gov": src = [term_img] * n
            else: src = [stills[key]] * n
            if key == "stage2": src = src[: int(n * 0.72)] + [card_end()] * (n - int(n * 0.72))
            scene_frames.append((cap, src))
        # audio track: narration at scene start + padding
        print("3/4 audio ...", flush=True)
        ap_wav = os.path.join(a.out, "narration.wav")
        with wave.open(ap_wav, "wb") as out:
            out.setnchannels(ch); out.setsampwidth(sw); out.setframerate(rate)
            for w, dur in zip(wavs, durs):
                with wave.open(w) as r:
                    data = r.readframes(r.getnframes())
                out.writeframes(b"\x00" * (int(rate * 0.2) * ch * sw))
                out.writeframes(data)
                used = 0.2 + r.getnframes() / rate
                out.writeframes(b"\x00" * (int(rate * max(0.0, dur - used)) * ch * sw))
        # SRT
        t = 0.0; srt = []
        for i, ((key, cap, _), dur, info) in enumerate(zip(SCENES, durs, infos), 1):
            srt.append(f"{i}\n{srt_time(t + 0.2)} --> {srt_time(t + 0.2 + info[3])}\n{cap}\n")
            t += dur
        open(os.path.join(a.out, "EdgeOrtho_demo_draft.srt"), "w", encoding="utf8").write("\n".join(srt))
        print("4/4 encoding ...", flush=True)
        mp4 = os.path.join(a.out, "EdgeOrtho_demo_draft.mp4")
        cmd = ["ffmpeg", "-y", "-loglevel", "error", "-f", "image2pipe", "-framerate", str(fps), "-vcodec", "mjpeg", "-i", "-", "-i", ap_wav,
               "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "20", "-preset", "medium", "-c:a", "aac", "-b:a", "160k", "-shortest", mp4]
        pr = subprocess.Popen(cmd, stdin=subprocess.PIPE)
        for cap, src in scene_frames:
            for im in src:
                if im.size != (W, H):
                    c = Image.new("RGB", (W, H), (0, 0, 0)); c.paste(im.resize((W, min(H, im.height))), (0, 0)); im = c
                buf = io.BytesIO(); overlay(im, cap).save(buf, "JPEG", quality=88); pr.stdin.write(buf.getvalue())
        pr.stdin.close(); pr.wait()
        total = sum(len(s) for _, s in scene_frames) / fps
        print(f"done: {mp4} ({total:.1f} s)")
    finally:
        import shutil; shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
