"""EdgeOrtho Stage I demo, step 4: ingest the synthetic frames into a fresh server (EPSG:3857 z21 pyramid), then open
viewer.html in headless Edge and take screenshots (mosaic + detection candidates).

Requires: target/release/wmts_server(.exe) built with `--features ingest`, `pip install websockets`, Edge.
Usage: python run_demo.py --sim <jinya_sim dir> --shots <screenshot dir> [--port 8098] [--workers 4]
Everything runs in one process because background servers do not survive the end of a tool command.
"""
import argparse, base64, asyncio, json, os, re, shutil, subprocess, sys, tempfile, threading, time, urllib.request
from concurrent.futures import ThreadPoolExecutor

import websockets

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
EDGE = os.environ.get("EDGE_PATH", r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe")
CDP, VIEW_PORT, KEY = 9371, 8123, "demokey"


def build_config(tmp, port, gsd_per_agl):
    cfg = open(os.path.join(ROOT, "config", "layers.demo.toml"), encoding="utf8").read()
    W = lambda p: p.replace("\\", "/")
    cfg = re.sub(r'(?m)^db_path = .*$', f'db_path = "{W(os.path.join(tmp, "demo.sqlite3"))}"', cfg)
    cfg = cfg.replace("127.0.0.1:8080", f"127.0.0.1:{port}").replace("http://localhost:8080", f"http://127.0.0.1:{port}")
    cfg = re.sub(r'(?m)^allowed_origins = .*$', f'allowed_origins = ["http://127.0.0.1:{port}", "http://127.0.0.1:{VIEW_PORT}"]', cfg)
    cfg = re.sub(r'(?m)^allowed_referers = .*$', f'allowed_referers = ["http://127.0.0.1:{port}", "http://127.0.0.1:{VIEW_PORT}"]', cfg)
    cfg = cfg.replace('tile_dir = "tiles/orthophoto_live"', f'tile_dir = "{W(os.path.join(tmp, "tiles"))}"\n', 1)
    cfg = cfg.replace("bbox_wgs84 = [120.25, 23.08, 120.37, 23.18]", "bbox_wgs84 = [120.97, 24.82, 120.99, 24.835]", 1)
    cfg = re.sub(r'(?m)^gsd_per_agl = .*$', f"gsd_per_agl = {gsd_per_agl}", cfg, count=1)
    cfg = re.sub(r'(?s)targets = \[.*?\n\]', 'targets = [\n  { tms = "GoogleMapsCompatible", zoom = 21, pyramid = 4 },\n]', cfg, count=1)
    p = os.path.join(tmp, "layers.toml"); open(p, "w", encoding="utf8").write(cfg)
    return p


def post(url, data):
    rq = urllib.request.Request(url, data=data, method="POST", headers={"X-API-Key": KEY, "Content-Type": "image/jpeg"})
    t = time.time()
    try:
        with urllib.request.urlopen(rq, timeout=120) as r:
            return r.status, json.loads(r.read()), time.time() - t
    except urllib.error.HTTPError as e:
        return e.code, {}, time.time() - t


async def shots(args, geo):
    prof = tempfile.mkdtemp(prefix="demo_edge_")
    edge = subprocess.Popen([EDGE, "--headless=new", f"--remote-debugging-port={CDP}", f"--user-data-dir={prof}", "--no-first-run", "--disable-extensions", "about:blank"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        tab = None
        for _ in range(40):
            try: tab = [t for t in json.load(urllib.request.urlopen(f"http://127.0.0.1:{CDP}/json")) if t["type"] == "page"][0]; break
            except Exception: time.sleep(.5)
        async with websockets.connect(tab["webSocketDebuggerUrl"], max_size=None) as ws:
            n, pend, errs = 0, {}, []
            async def rd():
                async for raw in ws:
                    r = json.loads(raw)
                    if r.get("method") == "Runtime.exceptionThrown": errs.append(str(r["params"]["exceptionDetails"])[:200])
                    if "id" in r and r["id"] in pend: pend.pop(r["id"]).set_result(r)
            asyncio.create_task(rd())
            async def call(m, **p):
                nonlocal n; n += 1; f = asyncio.get_event_loop().create_future(); pend[n] = f
                await ws.send(json.dumps({"id": n, "method": m, "params": p})); return await f
            async def ev(js):
                return (await call("Runtime.evaluate", expression=js, returnByValue=True, awaitPromise=True))["result"]["result"].get("value")
            await call("Page.enable"); await call("Runtime.enable")
            await call("Emulation.setDeviceMetricsOverride", width=1600, height=900, deviceScaleFactor=1, mobile=False)
            base = f"http://127.0.0.1:{VIEW_PORT}/viewer.html?base=http://127.0.0.1:{args.port}&key={KEY}&layer=orthophoto_live&det={args.det}"
            best = max(geo["features"], key=lambda f: f["properties"]["frames"])
            lon, lat = best["geometry"]["coordinates"]
            views = [("overview", base), ("closeup_z20", base + f"&lat={lat}&lng={lon}&z=20"), ("closeup_z21", base + f"&lat={lat}&lng={lon}&z=21")]
            os.makedirs(args.shots, exist_ok=True)
            for name, url in views:
                await call("Page.navigate", url=url); await asyncio.sleep(9)
                tiles = await ev("document.getElementById('s-t').textContent")
                r = await call("Page.captureScreenshot", format="png")
                open(os.path.join(args.shots, f"{args.prefix}_{name}.png"), "wb").write(base64.b64decode(r["result"]["data"]))
                print(f"shot {name}: mosaic tiles loaded {tiles}", flush=True)
            # detection layer off (to compare mosaic only)
            await ev("document.getElementById('t-det').click(); 0"); await asyncio.sleep(1)
            r = await call("Page.captureScreenshot", format="png")
            open(os.path.join(args.shots, f"{args.prefix}_closeup_z21_no_detections.png"), "wb").write(base64.b64decode(r["result"]["data"]))
            print("js errors:", errs[:2])
    finally:
        edge.kill()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sim", required=True); ap.add_argument("--shots", default=None)
    ap.add_argument("--port", type=int, default=8098); ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--det", default="output/detections.geojson", help="detections GeoJSON, relative to the demo folder")
    ap.add_argument("--prefix", default="viewer", help="screenshot file name prefix")
    ap.add_argument("--watch", action="store_true", help="recording mode: open a visible Edge window on the live viewer BEFORE ingesting, upload at --pace, then keep everything up for --hold seconds (no screenshots)")
    ap.add_argument("--pace", type=float, default=4.0, help="--watch: frames per second to upload")
    ap.add_argument("--hold", type=float, default=60.0, help="--watch: seconds to keep the server and viewer up after ingest")
    args = ap.parse_args()
    man = json.load(open(os.path.join(args.sim, "manifest.json"), encoding="utf8"))
    frames = man["frames"][: args.limit or None]
    tmp = tempfile.mkdtemp(prefix="edgeortho_")
    cfgp = build_config(tmp, args.port, man["gsd_per_agl"])
    exe = os.path.join(ROOT, "target", "release", "wmts_server.exe" if os.name == "nt" else "wmts_server")
    env = dict(os.environ, WMTS_CONFIG=cfgp, WMTS_JWT_SECRET="demo_secret_demo_secret_demo_secret_12", WMTS_APIKEY_GIS_CLIENT=KEY)
    srv = subprocess.Popen([exe], cwd=ROOT, env=env, stdout=open(os.path.join(tmp, "server.log"), "w"), stderr=subprocess.STDOUT)
    viewer = None
    try:
        for _ in range(60):
            time.sleep(0.5)
            try:
                if urllib.request.urlopen(f"http://127.0.0.1:{args.port}/healthz", timeout=2).status == 200: break
            except Exception: pass
        else:
            sys.exit("server did not start; see " + os.path.join(tmp, "server.log"))
        base = f"http://127.0.0.1:{args.port}"
        if args.watch:
            viewer = subprocess.Popen([sys.executable, "-m", "http.server", str(VIEW_PORT), "--bind", "127.0.0.1", "--directory", HERE],
                                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            time.sleep(1.0)
            url = (f"http://127.0.0.1:{VIEW_PORT}/viewer.html?base={base}&key={KEY}&layer=orthophoto_live&det={args.det}&live=2")
            edge = subprocess.Popen([EDGE, "--new-window", f"--user-data-dir={tempfile.mkdtemp(prefix='watch_edge_')}", "--no-first-run", "--window-size=1600,900", url],
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            print("Edge window opened; start your screen recorder now. Uploading starts in 8 s...", flush=True); time.sleep(8)
        print(f"ingesting {len(frames)} frames (gsd_per_agl={man['gsd_per_agl']}, target EPSG:3857 z21, workers {args.workers})", flush=True)
        t0 = time.time(); lat, res = [], []
        def one(f):
            data = open(os.path.join(args.sim, "frames", f["file"]), "rb").read()
            if args.watch: time.sleep(1.0 / args.pace * args.workers)
            for _ in range(10):
                s, j, dt = post(f"{base}/api/v1/ingest/frame?session_id=river-1", data)
                if s in (503, 409): time.sleep(2); continue
                return s, j, dt
            return 0, {}, 0
        with ThreadPoolExecutor(args.workers) as ex:
            for s, j, dt in ex.map(one, frames):
                res.append((s, j.get("status")));  lat.append(dt)
        wall = time.time() - t0
        ok = sum(1 for s, st in res if s == 200)
        lat.sort()
        print(f"ingest done: {ok}/{len(frames)} ok in {wall:.1f}s ({len(frames)/wall:.2f} frames/s); request latency p50 {lat[len(lat)//2]*1000:.0f} ms, p95 {lat[int(len(lat)*.95)]*1000:.0f} ms", flush=True)
        rq = urllib.request.Request(f"{base}/api/v1/ingest/stats", headers={"X-API-Key": KEY})
        st = json.load(urllib.request.urlopen(rq))
        print("server stats:", json.dumps({k: st.get(k) for k in ("frames", "tiles_written", "process_ms", "admission")}, ensure_ascii=False), flush=True)
        tiles = sum(len(fs) for _, _, fs in os.walk(os.path.join(tmp, "tiles")))
        print("tile files on disk:", tiles, flush=True)

        if args.watch:
            print(f"holding for {args.hold:.0f} s (viewer stays up; close the window or wait)...", flush=True); time.sleep(args.hold)
            return
        viewer = subprocess.Popen([sys.executable, "-m", "http.server", str(VIEW_PORT), "--bind", "127.0.0.1", "--directory", HERE],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1.5)
        geo = json.load(open(os.path.join(HERE, args.det), encoding="utf8"))
        asyncio.run(shots(args, geo))
    finally:
        if viewer: viewer.kill()
        if args.watch:
            try: edge.kill()
            except Exception: pass
        srv.kill(); srv.wait()
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
