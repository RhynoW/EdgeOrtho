"""EdgeOrtho: build a static demo site (for a Hugging Face static Space or any web server).

Ingests one or more synthetic-frame folders into a fresh server (EPSG:3857 z21 + 4 pyramid levels), then copies the
resulting tile files and the detections GeoJSON next to the static viewer page. No server is needed to view the result.

Usage: python export_static_demo.py --sim <river_sim> --sim <bridge_sim> --out <site dir>
Requires the same things as run_demo.py (server built with --features ingest).
"""
import argparse, json, os, shutil, subprocess, sys, tempfile, time, urllib.request
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import run_demo as R   # build_config(), post(), constants


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sim", action="append", required=True, help="synthetic frames folder (repeatable)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--port", type=int, default=8099)
    ap.add_argument("--workers", type=int, default=4)
    a = ap.parse_args()

    tmp = tempfile.mkdtemp(prefix="edgeortho_static_")
    man0 = json.load(open(os.path.join(a.sim[0], "manifest.json"), encoding="utf8"))
    cfg = R.build_config(tmp, a.port, man0["gsd_per_agl"])
    exe = os.path.join(R.ROOT, "target", "release", "wmts_server.exe" if os.name == "nt" else "wmts_server")
    env = dict(os.environ, WMTS_CONFIG=cfg, WMTS_JWT_SECRET="demo_secret_demo_secret_demo_secret_12", WMTS_APIKEY_GIS_CLIENT=R.KEY)
    srv = subprocess.Popen([exe], cwd=R.ROOT, env=env, stdout=open(os.path.join(tmp, "server.log"), "w"), stderr=subprocess.STDOUT)
    try:
        for _ in range(60):
            time.sleep(0.5)
            try:
                if urllib.request.urlopen(f"http://127.0.0.1:{a.port}/healthz", timeout=2).status == 200: break
            except Exception: pass
        else:
            sys.exit("server did not start: " + os.path.join(tmp, "server.log"))
        base = f"http://127.0.0.1:{a.port}"
        for si, sim in enumerate(a.sim):
            man = json.load(open(os.path.join(sim, "manifest.json"), encoding="utf8"))
            if abs(man["gsd_per_agl"] - man0["gsd_per_agl"]) > 1e-12:
                sys.exit("all sims must share gsd_per_agl")
            frames = man["frames"]
            def one(f, sim=sim, si=si):
                data = open(os.path.join(sim, "frames", f["file"]), "rb").read()
                for _ in range(10):
                    s, j, dt = R.post(f"{base}/api/v1/ingest/frame?session_id=static-{si}", data)
                    if s in (503, 409): time.sleep(2); continue
                    return s
                return 0
            t0 = time.time()
            with ThreadPoolExecutor(a.workers) as ex:
                res = list(ex.map(one, frames))
            print(f"{os.path.basename(sim)}: {sum(1 for s in res if s == 200)}/{len(frames)} ok in {time.time()-t0:.1f}s", flush=True)
        src = os.path.join(tmp, "tiles")
        dst = os.path.join(a.out, "tiles")
        if os.path.exists(dst): shutil.rmtree(dst)
        shutil.copytree(src, dst)
        n = sum(len(fs) for _, _, fs in os.walk(dst)); sz = sum(os.path.getsize(os.path.join(r, f)) for r, _, fs in os.walk(dst) for f in fs)
        print(f"exported {n} tile files, {sz/1e6:.1f} MB -> {dst}")
        for root, dirs, files in os.walk(dst):
            rel = os.path.relpath(root, dst)
            if rel.count(os.sep) <= 2 and files == [] and dirs:
                print("  dir:", rel)
    finally:
        srv.kill(); srv.wait()
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
