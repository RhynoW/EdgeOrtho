"""機隊模擬＋內建檢視器截圖：啟動全新伺服器，22 架（或 --drones 指定）依真實節奏回傳，同時用無頭 Edge 開檢視器、按「切到近即時圖層」，
在指定的秒數截圖，讓你看到地圖上各任務區的影像怎麼隨時間（以及積壓時）長出來。

用法：python scripts/sim/sim_fleet_shots.py --fleet <fleet 資料夾> --shots <截圖資料夾> [--drones 22] [--at 40,100,160,215 | --every 6 --until 215]
      --ingest-concurrency N   伺服器的處理並行度（1＝等同改善前的序列化處理，用來做前後對比）
      --headed --hold 600      開一個看得到的 Edge 視窗讓你即時觀看（跑完後保留 --hold 秒）；不截圖時可把 --at 設成很大的數字
每次截圖也會把當下的拍攝／已上傳張數記進 <截圖資料夾>/stats.json，之後可用 make_fleet_gif.py 組成前後對比動畫。
需要 pip install websockets；Edge 路徑可用環境變數 EDGE_PATH 覆寫。
"""
import argparse, asyncio, base64, json, os, shutil, subprocess, sys, tempfile, threading, time, urllib.request
from types import SimpleNamespace

import websockets

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sim_fleet as F

EDGE = os.environ.get("EDGE_PATH", r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe")
CDP = 9361


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--fleet", required=True); ap.add_argument("--shots", required=True); ap.add_argument("--drones", type=int, default=22)
    ap.add_argument("--speed", type=float, default=1.0); ap.add_argument("--at", default="40,100,160,215"); ap.add_argument("--port", type=int, default=8097)
    ap.add_argument("--every", type=float, default=None); ap.add_argument("--until", type=float, default=215)
    ap.add_argument("--ingest-concurrency", type=int, default=None); ap.add_argument("--headed", action="store_true"); ap.add_argument("--hold", type=float, default=0)
    a = ap.parse_args(); os.makedirs(a.shots, exist_ok=True)
    at = [float(x) for x in a.at.split(",")] if not a.every else [a.every * k for k in range(1, int(a.until // a.every) + 1)]
    tmp = tempfile.mkdtemp(prefix="fleetshot_"); proc, base, key = F.spawn_server(a.port, tmp, a.ingest_concurrency)
    edge = subprocess.Popen([EDGE] + ([] if a.headed else ["--headless=new"]) + ["--window-size=2000,1200", f"--remote-debugging-port={CDP}", f"--user-data-dir={os.path.join(tmp, 'edge')}", "--disable-extensions", "--no-first-run", "about:blank"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        ns = SimpleNamespace(fleet=a.fleet, base=base, key=key, speed=a.speed, duration=max(at) + 5, timeout=30, retries=3, drain=False, drain_max=0)
        fleet = F.Fleet(ns, list(range(a.drones)))
        box = {}
        th = threading.Thread(target=lambda: box.update(res=fleet.run()), daemon=True)
        tab = None
        for _ in range(40):
            try: tab = [t for t in json.load(urllib.request.urlopen(f"http://127.0.0.1:{CDP}/json")) if t["type"] == "page"][0]; break
            except Exception: time.sleep(.5)
        async with websockets.connect(tab["webSocketDebuggerUrl"], max_size=None) as ws:
            n, pend = 0, {}
            async def rd():
                async for raw in ws:
                    r = json.loads(raw)
                    if "id" in r and r["id"] in pend: pend[r["id"]].set_result(r)
            asyncio.create_task(rd())
            async def call(m, **p):
                nonlocal n; n += 1; f = asyncio.get_event_loop().create_future(); pend[n] = f
                await ws.send(json.dumps({"id": n, "method": m, "params": p})); return await f
            ev = lambda js: call("Runtime.evaluate", expression=js, returnByValue=True, awaitPromise=True)
            await call("Page.enable"); await call("Runtime.enable")
            if not a.headed: await call("Emulation.setDeviceMetricsOverride", width=2400, height=1400, deviceScaleFactor=1, mobile=False)
            await call("Page.navigate", url=base + "/viewer/"); await asyncio.sleep(3)
            await ev(f"document.getElementById('cfg-base').value={json.dumps(base)}; document.getElementById('cfg-apikey').value={json.dumps(key)}; 0")
            await ev("document.getElementById('btn-live-preset').click(); 0"); await asyncio.sleep(2)
            # 機隊任務區涵蓋約 6×5 km：縮放 15（金字塔最粗的一級）才看得到全部
            await ev("map.setView([23.1225, 120.31], 15, {animate:false}); 0")
            th.start(); t0 = time.time(); stats = []
            for t in at:
                await asyncio.sleep(max(0, t - (time.time() - t0)))
                with fleet.lock: done = len(fleet.rec); cap = fleet.captured_by(time.time() - fleet.t0)
                r = await call("Page.captureScreenshot", format="png")
                name = f"fleet_{a.drones}_t{int(t):03d}.png"; open(os.path.join(a.shots, name), "wb").write(base64.b64decode(r["result"]["data"]))
                stats.append(dict(t=t, file=name, captured=cap, done=done)); json.dump(stats, open(os.path.join(a.shots, "stats.json"), "w"))
                print(f"t={t:4.0f}s 截圖 {name}｜已拍攝 {cap} 張、已上傳 {done} 張（機上積壓 {cap-done}）", flush=True)
        th.join(timeout=10)
        if a.hold: print(f"視窗保留 {a.hold:.0f} 秒供觀看…", flush=True); time.sleep(a.hold)
    finally:
        edge.kill(); proc.kill(); proc.wait(); shutil.rmtree(tmp, ignore_errors=True)


asyncio.run(main())
