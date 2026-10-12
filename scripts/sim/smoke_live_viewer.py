"""近即時圖層的瀏覽器 smoke test：無頭 Edge 開內建檢視器，按「切到近即時圖層」，同時重播假航照，
確認地圖上已載入的圖磚隨時間增加（近即時更新有效）、金字塔的低縮放級別（z15）也有圖、無 JS 例外。

前置：伺服器要用 `cargo build --release --features ingest` 的版本啟動，且 tiles/orthophoto_live 為空、
資料庫是乾淨的（否則重播的影像會被判為重複而不產生圖磚）。建議用臨時設定檔，db_path 指向暫存位置。
用法：python scripts/sim/smoke_live_viewer.py --dir <sim 輸出資料夾> --key <API key> [--base http://127.0.0.1:8080] [--shots <截圖資料夾>]
需要 pip install websockets；Edge 路徑可用環境變數 EDGE_PATH 覆寫。
"""
import argparse, asyncio, json, os, subprocess, sys, tempfile, time, urllib.request

import websockets

EDGE = os.environ.get("EDGE_PATH", r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe")
CDP = 9352
HERE = os.path.dirname(os.path.abspath(__file__))


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", required=True); ap.add_argument("--key", required=True)
    ap.add_argument("--base", default="http://127.0.0.1:8080"); ap.add_argument("--shots", default=None)
    ap.add_argument("--speed", type=float, default=25.0, help="重播倍速（任務 485 s ÷ 25 ≈ 20 s）")
    ap.add_argument("--mode", choices=["http", "cargo"], default="http",
                    help="http＝用 sim_aerial_replay.py 經 ingest 端點上傳；cargo＝改由 cargo 測試輔助 stream_frames_to_tiles 直接寫圖磚（伺服器只負責提供圖磚，"
                         "用在無法讓伺服器程序處理影像的環境）。cargo 模式需要 --tile-dir（與伺服器的 tile_dir 同一個資料夾）")
    ap.add_argument("--tile-dir", default=None); ap.add_argument("--limit", type=int, default=45, help="cargo 模式要鑲嵌的張數"); ap.add_argument("--interval-ms", type=int, default=400)
    a = ap.parse_args()
    results = []
    def chk(name, cond, extra=""):
        results.append(bool(cond)); print(("PASS " if cond else "FAIL ") + name, extra)

    prof = tempfile.mkdtemp(prefix="live_edge_")
    edge = subprocess.Popen([EDGE, "--headless=new", f"--remote-debugging-port={CDP}", f"--user-data-dir={prof}", "--disable-extensions", "--no-first-run", "about:blank"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    replay = None
    try:
        tab = None
        for _ in range(40):
            try: tab = [t for t in json.load(urllib.request.urlopen(f"http://127.0.0.1:{CDP}/json")) if t["type"] == "page"][0]; break
            except Exception: time.sleep(.5)
        async with websockets.connect(tab["webSocketDebuggerUrl"], max_size=None) as ws:
            n, errs, pend = 0, [], {}
            async def rd():
                async for raw in ws:
                    r = json.loads(raw)
                    if r.get("method") == "Runtime.exceptionThrown": errs.append(json.dumps(r["params"]["exceptionDetails"], ensure_ascii=False)[:300])
                    if "id" in r and r["id"] in pend: pend.pop(r["id"]).set_result(r)
            asyncio.create_task(rd())
            async def call(m, **p):
                nonlocal n; n += 1; f = asyncio.get_event_loop().create_future(); pend[n] = f
                await ws.send(json.dumps({"id": n, "method": m, "params": p})); return await f
            async def ev(js):
                r = await call("Runtime.evaluate", expression=js, returnByValue=True, awaitPromise=True)
                if "exceptionDetails" in r["result"]: print("EVAL ERR", json.dumps(r["result"]["exceptionDetails"], ensure_ascii=False)[:300])
                return r["result"]["result"].get("value")
            async def shot(name):
                if not a.shots: return
                os.makedirs(a.shots, exist_ok=True)
                import base64
                r = await call("Page.captureScreenshot", format="png")
                open(os.path.join(a.shots, name), "wb").write(base64.b64decode(r["result"]["data"]))
            count = lambda: ev("(()=>{const l=wmtsLayer; if(!l||!l.getContainer()) return -1; return [...l.getContainer().querySelectorAll('img')].filter(i=>i.naturalWidth>0).length})()")

            await call("Page.enable"); await call("Runtime.enable")
            await call("Emulation.setDeviceMetricsOverride", width=1280, height=800, deviceScaleFactor=1, mobile=False)
            await call("Page.navigate", url=a.base.rstrip("/") + "/viewer/"); await asyncio.sleep(3)
            await ev(f"document.getElementById('cfg-base').value={json.dumps(a.base)}; document.getElementById('cfg-apikey').value={json.dumps(a.key)}; 0")
            await ev("document.getElementById('btn-live-preset').click(); 0"); await asyncio.sleep(2)
            chk("預設按鈕：圖層與 TMS 已切換", await ev("document.getElementById('cfg-layer').value==='orthophoto_live' && document.getElementById('cfg-tms').value==='GoogleMapsCompatible'"))
            chk("近即時更新已開啟", await ev("document.getElementById('wmts-live-checkbox').checked"))
            z = await ev("map.getZoom()"); c = await ev("[map.getCenter().lat, map.getCenter().lng]")
            chk("地圖移到 orthophoto_live 範圍中心（縮放 16）", z == 16 and abs(c[0] - 23.13) < 0.01 and abs(c[1] - 120.31) < 0.01, (z, c))
            await ev("map.setView([23.13, 120.31], 17, {animate:false}); 0"); await asyncio.sleep(2)   # z17 視窗涵蓋整個任務範圍，圖磚數隨航線增加
            c0 = await count(); await shot("live_0_empty.png"); print("開始時已載入圖磚：", c0)

            if a.mode == "http":
                replay = subprocess.Popen([sys.executable, os.path.join(HERE, "sim_aerial_replay.py"), "--dir", a.dir, "--key", a.key, "--base", a.base, "--speed", str(a.speed)],
                                          stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf8")
            else:
                root = os.path.dirname(os.path.dirname(HERE))
                env = dict(os.environ, FRAMES_DIR=os.path.join(a.dir, "frames"), TILE_DIR=a.tile_dir, INTERVAL_MS=str(a.interval_ms), LIMIT=str(a.limit))
                replay = subprocess.Popen(["cargo", "test", "--release", "--features", "ingest", "stream_frames_to_tiles", "--", "--ignored", "--nocapture"],
                                          cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf8")
            series = []
            t0 = time.time()
            while replay.poll() is None and time.time() - t0 < 280:
                await asyncio.sleep(4); series.append(await count()); print(f"  t={time.time()-t0:4.0f}s 已載入圖磚 {series[-1]}")
                if len(series) == 2: await shot("live_1_mid.png")
            await asyncio.sleep(5)    # 讓最後一輪更新（3 秒一次）完成
            series.append(await count()); await shot("live_2_end.png")
            out = replay.stdout.read() if replay.stdout else ""
            print("重播輸出（節錄）：", " | ".join(l for l in out.splitlines() if l.startswith(("PASS", "FAIL", "frame ", "test result")))[-400:])
            chk("重播成功結束", replay.returncode == 0, replay.returncode)
            grew = [b - a for a, b in zip(series, series[1:])]
            chk("地圖上的圖磚隨重播增加（近即時更新有效）", series[-1] >= c0 + 8 and min(grew) >= -1, series)
            # 縮小到 z15：金字塔的粗級別也要有圖
            await ev("map.setView([23.13, 120.31], 15, {animate:false}); 0"); await asyncio.sleep(4); c15 = await count(); await shot("live_3_z15.png")
            chk("縮放 15（金字塔）有圖磚", c15 >= 1, c15)
            await ev("map.setView([23.13, 120.31], 19, {animate:false}); 0"); await asyncio.sleep(4); c19 = await count(); await shot("live_4_z19.png")
            chk("縮放 19（原生級別）有圖磚", c19 >= 1, c19)
            chk("沒有 JS 例外", not errs, errs[:2])
    finally:
        if replay and replay.poll() is None: replay.kill()
        edge.kill()
    print(f"RESULT {sum(results)}/{len(results)}")
    sys.exit(0 if all(results) else 1)


asyncio.run(main())
