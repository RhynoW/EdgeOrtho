"""近即時正射模擬：重播假航照到 ingest 端點，量測延遲與鑲嵌品質，並測試冪等與拒絕情境。

前置：
  1. python scripts/sim/sim_aerial_gen.py --out <資料夾>               產生假航照
  2. cargo build --release --features ingest                           編譯含 ingest 的伺服器
  3. 啟動：WMTS_CONFIG=config/layers.demo.toml WMTS_JWT_SECRET=... WMTS_APIKEY_GIS_CLIENT=<key> target/release/wmts_server
用法：
  python scripts/sim/sim_aerial_replay.py --dir <資料夾> --key <key> [--base http://127.0.0.1:8080] [--speed 0] [--limit N]
  --speed 0＝盡快送；N＝以 N 倍真實時間依 manifest 的拍攝間隔送（模擬航線進行中）。

量測：
  - 每張的 HTTP 往返（total_ms）、伺服器處理（process_ms）；圖磚寫入後立刻用 GetTile 讀回，確認「上傳完成即可見」。
  - 品質：TM2 第 13 層圖磚與 NLSC 同格網的地面真值逐像素比對（灰階平均差、相位相關估計的位移＝定位誤差）。
"""
import argparse, io, json, math, os, statistics, sys, time, urllib.error, urllib.request

import cv2
import numpy as np
import pyproj
from PIL import Image

ORIGIN = (50000.0, 3000000.0)
RES13 = 944.9423660750896 * 0.00028
SPAN13 = 256 * RES13


def req(method, url, key, data=None, headers=None):
    h = {"X-API-Key": key} if key else {}
    h.update(headers or {})
    r = urllib.request.Request(url, data=data, method=method, headers=h)
    t = time.time()
    try:
        with urllib.request.urlopen(r, timeout=120) as f:
            return f.status, f.read(), (time.time() - t) * 1000, dict(f.headers)
    except urllib.error.HTTPError as e:
        return e.code, e.read(), (time.time() - t) * 1000, dict(e.headers)


def pct(v, p):
    s = sorted(v); return s[min(len(s) - 1, int(round((len(s) - 1) * p)))] if s else 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", required=True); ap.add_argument("--key", required=True)
    ap.add_argument("--base", default="http://127.0.0.1:8080"); ap.add_argument("--speed", type=float, default=0)
    ap.add_argument("--limit", type=int, default=0); ap.add_argument("--session", default="sim-1")
    a = ap.parse_args()
    man = json.load(open(os.path.join(a.dir, "manifest.json"), encoding="utf8")); frames = man["frames"][: a.limit or None]
    base, key = a.base.rstrip("/"), a.key
    ok = True
    def check(name, cond, extra=""):
        nonlocal ok; ok &= bool(cond); print(("PASS " if cond else "FAIL ") + name, extra)

    # --- 拒絕與驗證情境（驗證失敗用小 body：伺服器在讀完大 body 前就回 401 並關線，大 body 的客戶端在 Windows 會收到連線重置）---
    f0 = open(os.path.join(a.dir, "frames", frames[0]["file"]), "rb").read()
    s, _, _, _ = req("POST", f"{base}/api/v1/ingest/frame", None, b"x"); check("沒有 API Key → 401", s == 401, s)
    s, _, _, _ = req("POST", f"{base}/api/v1/ingest/frame", "wrong-key", b"x"); check("錯誤 API Key → 401", s == 401, s)
    s, b, _, _ = req("POST", f"{base}/api/v1/ingest/frame", key, b"this is not a jpeg"); check("非 JPEG → 400", s == 400, (s, b[:80]))
    buf = io.BytesIO(); Image.new("RGB", (64, 48)).save(buf, "JPEG")
    s, b, _, _ = req("POST", f"{base}/api/v1/ingest/frame", key, buf.getvalue()); check("JPEG 但沒有 GPS/XMP → 400", s == 400, (s, b[:100]))
    s, b, _, _ = req("POST", f"{base}/api/v1/ingest/frame?session_id=bad/../id", key, f0); check("session_id 含路徑字元 → 400", s == 400, (s, b[:80]))

    # --- 重播 ---
    t_start = time.time(); tot, proc, tiles, vis = [], [], [], []
    to_tm2 = pyproj.Transformer.from_crs(4326, 3826, always_xy=True)
    for k, f in enumerate(frames):
        if a.speed > 0:
            wait = f["t_s"] / a.speed - (time.time() - t_start)
            if wait > 0: time.sleep(wait)
        data = open(os.path.join(a.dir, "frames", f["file"]), "rb").read()
        s, b, ms, _ = req("POST", f"{base}/api/v1/ingest/frame?session_id={a.session}", key, data)
        if s != 200: print("FAIL frame", f["file"], s, b[:200]); ok = False; continue
        j = json.loads(b); tot.append(ms); proc.append(j["process_ms"]); tiles.append(j["tiles_written"])
        # 上傳完成後立刻讀回 footprint 中心所在的 TM2 第 13 層圖磚
        e, n = j["footprint_center_tm2"]; col, row = int((e - ORIGIN[0]) // SPAN13), int((ORIGIN[1] - n) // SPAN13)
        s2, _, ms2, hd = req("GET", f"{base}/wmts/1.0.0/orthophoto_live/default/TWD97_TM2_NLSC/13/{row}/{col}.png", key)
        vis.append(ms + ms2 if s2 == 200 else float("nan"))
        if k == 0: first_cc = hd.get("Cache-Control", hd.get("cache-control"))
        if k % 20 == 0: print(f"  [{k+1}/{len(frames)}] {f['file']} tiles={j['tiles_written']} process={j['process_ms']}ms total={ms:.0f}ms")
    wall = time.time() - t_start
    print(f"\n重播 {len(frames)} 張，牆鐘 {wall:.1f}s（任務時間 {frames[-1]['t_s']:.0f}s，speed={a.speed or '最大'}）")
    check("所有圖磚在上傳完成後立即可讀", not any(math.isnan(v) for v in vis))
    print(f"處理時間 ms：p50 {pct(proc,.5)}｜p95 {pct(proc,.95)}｜max {max(proc)}；HTTP 往返 ms：p50 {pct(tot,.5):.0f}｜p95 {pct(tot,.95):.0f}｜max {max(tot):.0f}")
    print(f"上傳→圖磚可見 ms：p50 {pct([v for v in vis if v==v],.5):.0f}｜p95 {pct([v for v in vis if v==v],.95):.0f}；每張寫入圖磚數 平均 {statistics.mean(tiles):.1f}（兩個格網合計）")
    print(f"吞吐量：{len(frames)/wall:.2f} 張/s；{len(frames)/wall*86400:,.0f} 張/日（規範月 2,000 張 ≈ {2000/30:.0f} 張/日）")
    check("Cache-Control 已縮短（max-age <= 5）", first_cc and int(first_cc.split("max-age=")[1]) <= 5, first_cc)

    # --- 冪等 ---
    s, b, _, _ = req("POST", f"{base}/api/v1/ingest/frame?session_id=another", key, f0); j = json.loads(b)
    check("同一張重送 → duplicate，不重複鑲嵌", s == 200 and j["status"] == "duplicate", j.get("status"))

    # --- 統計與稽核 ---
    s, b, _, _ = req("GET", f"{base}/api/v1/ingest/stats", key); st = json.loads(b); print("stats:", json.dumps(st, ensure_ascii=False))
    check("stats 張數與重播一致", st["frames"] >= len(frames) and st["duplicates"] >= 1, (st["frames"], st["duplicates"]))
    s, b, _, _ = req("GET", f"{base}/api/v1/ingest/audit?limit=200", key); au = json.loads(b)["entries"]
    outcomes = {o: sum(1 for x in au if x["outcome"] == o) for o in ("ok", "duplicate", "rejected")}
    check("稽核紀錄含 ok/duplicate/rejected 與身分", outcomes["ok"] >= 1 and outcomes["duplicate"] >= 1 and outcomes["rejected"] >= 3 and au[0]["actor"].startswith("apikey:"), outcomes)

    # --- 品質：TM2 第 13 層與同格網的地面真值逐塊比對 ---
    gt_dir = os.path.join(a.dir, "_gt_cache"); live = os.path.join(os.path.dirname(os.path.abspath(a.dir)), "..")
    tile_root = None
    for cand in (os.environ.get("LIVE_TILES"), "tiles/orthophoto_live"):
        if cand and os.path.isdir(os.path.join(cand, "TWD97_TM2_NLSC", "13")): tile_root = cand; break
    if not tile_root: print("找不到 tiles/orthophoto_live（用環境變數 LIVE_TILES 指定），略過品質比對"); sys.exit(0 if ok else 1)
    diffs, shifts = [], []
    d13 = os.path.join(tile_root, "TWD97_TM2_NLSC", "13")
    for c in os.listdir(d13):
        for rf in os.listdir(os.path.join(d13, c)):
            r = int(rf.split(".")[0]); gtp = os.path.join(gt_dir, f"13_{r}_{c}.jpg")
            if not os.path.exists(gtp): continue
            t = np.asarray(Image.open(os.path.join(d13, c, rf)).convert("RGBA")); g = np.asarray(Image.open(gtp).convert("L"), np.float32)
            m = (t[..., 3] == 255); m = cv2.erode(m.astype(np.uint8), np.ones((9, 9), np.uint8)).astype(bool)
            if m.mean() < 0.9: continue                                  # 只比對完整覆蓋的圖磚（不含邊緣）
            tg = np.asarray(Image.fromarray(t[..., :3]).convert("L"), np.float32)
            diffs.append(float(np.abs(tg - g)[m].mean()))
            (dx, dy), _ = cv2.phaseCorrelate(g, tg, cv2.createHanningWindow((256, 256), cv2.CV_32F)); shifts.append(math.hypot(dx, dy) * RES13)
    if diffs:
        print(f"完整覆蓋圖磚 {len(diffs)} 塊：與地面真值灰階平均差 {statistics.mean(diffs):.2f}（0–255）；相位相關位移 平均 {statistics.mean(shifts):.2f} m、P95 {pct(shifts,.95):.2f} m")
        check("鑲嵌位置誤差 < 1 m（RTK）", statistics.mean(shifts) < 1.0)
    sys.exit(0 if ok else 1)


main()
