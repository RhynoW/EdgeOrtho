"""多架無人機同時回傳的模擬：N 個並行上傳者，依各自任務的真實拍攝節奏產生影像、先進先出上傳（像真實的存儲轉發），
量測「拍攝 → 圖磚可見」的延遲、伺服器排隊時間、積壓量，並同時探測圖磚服務與 /readyz 在壓力下的反應。

每架機：拍攝時刻 = manifest 的 t_s ÷ speed；上傳者一次只送一張（送完才送下一張）；影像若比上傳快，就在機上排隊（積壓）。
指標「拍攝到可見」= 該張上傳完成的時刻 − 它的拍攝時刻（含機上排隊＋網路＋伺服器排隊＋處理）。這才是「近即時」的實際體驗。

用法（先用 sim_fleet_gen.py 產生各機影像）：
  # 自動啟動全新的伺服器（臨時資料庫與圖磚資料夾）並跑一種情境：
  python scripts/sim/sim_fleet.py --fleet <fleet 資料夾> --spawn --drones 22 --speed 1
  # 找出撐得住的機數：每個機數各自用全新伺服器重跑（因為重送同一批影像會被判為重複）：
  python scripts/sim/sim_fleet.py --fleet <fleet 資料夾> --spawn --sweep 4,8,12,16,22 --speed 1 --duration 90
  # 連到已啟動的伺服器（資料庫要是乾淨的）：
  python scripts/sim/sim_fleet.py --fleet <fleet 資料夾> --base http://127.0.0.1:8097 --key <uploader 或 admin 金鑰>
"""
import argparse, bisect, json, os, re, shutil, statistics, subprocess, sys, tempfile, threading, time, urllib.error, urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def pct(v, p):
    s = sorted(v); return s[min(len(s) - 1, int(round((len(s) - 1) * p)))] if s else float("nan")


def http(method, url, key=None, data=None, timeout=30):
    h = {"X-API-Key": key} if key else {}
    r = urllib.request.Request(url, data=data, method=method, headers=h); t = time.time()
    try:
        with urllib.request.urlopen(r, timeout=timeout) as f: return f.status, f.read(), (time.time() - t) * 1000
    except urllib.error.HTTPError as e: return e.code, e.read(), (time.time() - t) * 1000
    except Exception as e: return 0, str(e).encode(), (time.time() - t) * 1000


class Fleet:
    def __init__(self, a, drones):
        self.a, self.drones = a, drones
        self.t0 = None; self.stop = threading.Event(); self.lock = threading.Lock()
        self.rec = []          # 完成的上傳：dict
        self.captured = 0      # 上傳者已「取用」的張數（含機上排隊後才輪到的）；真正的拍攝張數看 captured_by()
        self.all_caps = []     # 全部影像的拍攝時刻（秒，已除以 speed），排序
        self.inflight = 0
        self.probes = []       # (t, kind, ms, status)
        self.timeouts = 0; self.retries = 0; self.failed = 0; self.busy = 0; self.conflict = 0

    def captured_by(self, now):
        """到 now 為止實際拍攝的張數（不論是否已上傳）。"""
        return bisect.bisect_right(self.all_caps, now)

    def drone(self, i, manifest, frames_dir):
        a = self.a
        fr = manifest["frames"]
        for k, f in enumerate(fr):
            cap = f["t_s"] / a.speed
            # 拍攝時刻到了才算「已拍攝」；若上傳者還在忙，這張就在機上排隊
            now = time.time() - self.t0
            if cap > now:
                if self.stop.wait(cap - now): return
            if self.stop.is_set(): return
            with self.lock: self.captured += 1
            data = open(os.path.join(frames_dir, f["file"]), "rb").read()
            start = time.time() - self.t0; tries = 0; code = 0; body = b""; ms = 0
            with self.lock: self.inflight += 1
            while not self.stop.is_set():
                code, body, ms = http("POST", f"{a.base}/api/v1/ingest/frame?session_id=drone-{i:02d}", a.key, data, a.timeout)
                if code == 200: break
                if code in (503, 409):
                    # 伺服器忙或這張正在處理：照 Retry-After（伺服器固定回 2 秒）退避後重送，不算失敗、不消耗重試額度
                    with self.lock: self.busy += (code == 503); self.conflict += (code == 409)
                    if self.stop.wait(2.0): break
                    continue
                tries += 1
                with self.lock:
                    self.timeouts += (code == 0); self.retries += 1
                if tries > a.retries: break
                time.sleep(min(2.0 * tries, 10))
            with self.lock: self.inflight -= 1
            done = time.time() - self.t0
            if code == 200:
                j = json.loads(body)
                with self.lock: self.rec.append(dict(d=i, cap=cap, start=start, done=done, age=done - cap, ms=ms, q=j.get("queue_ms", 0), p=j.get("process_ms", 0), tiles=j.get("tiles_written", 0), dup=j.get("status") == "duplicate", tries=tries))
            else:
                with self.lock: self.failed += 1

    def prober(self):
        a = self.a; n = 0
        while not self.stop.wait(1.0):
            t = time.time() - self.t0
            s, _, ms = http("GET", f"{a.base}/readyz", None, None, 10); self.probes.append((t, "readyz", ms, s))
            # 讀一塊已存在的圖磚（若尚無資料就 404，仍量回應時間）
            s, _, ms = http("GET", f"{a.base}/wmts/1.0.0/orthophoto_live/default/GoogleMapsCompatible/17/56881/{109342 + (n % 3)}.png", a.key, None, 10); self.probes.append((t, "tile", ms, s)); n += 1

    def run(self):
        a = self.a
        manifests = []
        for i in self.drones:
            d = os.path.join(a.fleet, f"drone_{i:02d}")
            manifests.append((i, json.load(open(os.path.join(d, "manifest.json"), encoding="utf8")), os.path.join(d, "frames")))
        total = sum(len(m["frames"]) for _, m, _ in manifests)
        self.all_caps = sorted(f["t_s"] / a.speed for _, m, _ in manifests for f in m["frames"])
        self.t0 = time.time()
        th = [threading.Thread(target=self.drone, args=(i, m, fd), daemon=True) for i, m, fd in manifests]
        pt = threading.Thread(target=self.prober, daemon=True)
        pt.start(); [t.start() for t in th]
        end = a.duration
        series = []; last = 0
        while time.time() - self.t0 < end and any(t.is_alive() for t in th):
            time.sleep(10)
            now = time.time() - self.t0
            with self.lock:
                recent = [r for r in self.rec if now - 10 <= r["done"] < now]
                backlog = self.captured_by(now) - len(self.rec) - self.failed
                row = (now, len(recent) / 10, pct([r["age"] for r in recent], .5), pct([r["age"] for r in recent], .95), pct([r["q"] for r in recent], .5), backlog, self.inflight)
            series.append(row)
            print(f"  t={row[0]:4.0f}s  完成 {row[1]:4.1f} 張/s  拍攝→可見 p50 {row[2]:6.1f}s p95 {row[3]:6.1f}s  伺服器排隊 p50 {row[4]/1000:5.1f}s  機上積壓 {row[5]:3d} 張（正在上傳 {row[6]:2d} 架）", flush=True)
        cap_end = time.time() - self.t0
        drain = None
        if a.drain and any(t.is_alive() for t in th):
            t1 = time.time()
            while any(t.is_alive() for t in th) and time.time() - t1 < a.drain_max: time.sleep(2)
            drain = time.time() - t1
        self.stop.set(); [t.join(2) for t in th]; pt.join(2)
        return dict(total=total, series=series, cap_end=cap_end, drain=drain)


def report(f, res, label):
    rec = f.rec; done = [r for r in rec if not r["dup"]]
    age = [r["age"] for r in done]; q = [r["q"] / 1000 for r in done]; p = [r["p"] for r in done]
    wall = max((r["done"] for r in done), default=0)
    print(f"\n=== {label} ===")
    print(f"機數 {len(f.drones)}｜任務共 {res['total']} 張｜截止時已拍攝 {f.captured_by(res['cap_end'])}｜成功上傳 {len(done)}（{100*len(done)/max(f.captured_by(res['cap_end']),1):.0f}% 的已拍攝）｜失敗 {f.failed}｜逾時 {f.timeouts}｜重試 {f.retries}｜伺服器忙(503) {f.busy}｜處理中(409) {f.conflict}")
    if done:
        print(f"拍攝→圖磚可見（秒）：p50 {pct(age,.5):.1f}｜p95 {pct(age,.95):.1f}｜最大 {max(age):.1f}")
        print(f"伺服器排隊（秒，queue_ms）：p50 {pct(q,.5):.1f}｜p95 {pct(q,.95):.1f}｜最大 {max(q):.1f}；單張處理（ms）：p50 {pct(p,.5):.0f}｜p95 {pct(p,.95):.0f}")
        thr = len(done) / max(wall, 1)
        print(f"有效吞吐量：{thr:.2f} 張/s（{len(done)} 張 / {wall:.0f}s）；拍攝（到達）速率 {f.captured_by(res['cap_end']) / max(res['cap_end'],1):.2f} 張/s")
        per = [sum(1 for r in done if r["d"] == d) for d in f.drones]
        ages = [statistics.mean([r["age"] for r in done if r["d"] == d] or [0]) for d in f.drones]
        print(f"公平性：每架完成張數 最少 {min(per)}、最多 {max(per)}；各機平均延遲 最低 {min(ages):.1f}s、最高 {max(ages):.1f}s")
    backlog = f.captured_by(res['cap_end']) - len(rec) - f.failed
    backlog = max(backlog, 0)
    print(f"截止時機上積壓：{backlog} 張" + (f"；之後排空耗時 {res['drain']:.0f}s" if res.get("drain") is not None else ""))
    for kind, nm in (("readyz", "/readyz 回應"), ("tile", "圖磚 GetTile 回應")):
        v = [ms for _, k, ms, s in f.probes if k == kind and s in (200, 404)]; bad = sum(1 for _, k, ms, s in f.probes if k == kind and s not in (200, 404))
        if v: print(f"壓力下 {nm}（ms）：p50 {pct(v,.5):.0f}｜p95 {pct(v,.95):.0f}｜最大 {max(v):.0f}｜失敗 {bad}/{len(v)+bad}")
    return dict(label=label, drones=len(f.drones), done=len(done), captured=f.captured_by(res['cap_end']), age_p50=pct(age, .5) if age else None, age_p95=pct(age, .95) if age else None,
                age_max=max(age) if age else None, q_p95=pct(q, .95) if q else None, backlog=backlog, timeouts=f.timeouts, busy=f.busy, thr=len(done) / max(wall, 1))


def spawn_server(port, tmp, concurrency=None, max_queue=None):
    """以臨時設定啟動全新的伺服器：乾淨的資料庫與圖磚資料夾；金鑰用 admin 角色（能上傳也能讀統計）。"""
    cfg = open(os.path.join(ROOT, "config", "layers.demo.toml"), encoding="utf8").read()
    W = lambda p: p.replace("\\", "/")
    cfg = re.sub(r'(?m)^db_path = .*$', f'db_path = "{W(os.path.join(tmp, "fleet.sqlite3"))}"', cfg)
    cfg = cfg.replace("127.0.0.1:8080", f"127.0.0.1:{port}").replace("http://localhost:8080", f"http://127.0.0.1:{port}")
    cfg = cfg.replace('tile_dir = "tiles/orthophoto_live"', f'tile_dir = "{W(os.path.join(tmp, "tiles"))}"')
    extra = "".join(f"{k} = {v}\n" for k, v in (("concurrency", concurrency), ("max_queue", max_queue)) if v is not None)
    if extra: cfg = cfg.replace("[ingest]\n", "[ingest]\n" + extra, 1)
    cfgp = os.path.join(tmp, "layers.toml"); open(cfgp, "w", encoding="utf8").write(cfg)
    env = dict(os.environ, WMTS_CONFIG=cfgp, WMTS_JWT_SECRET="fleet_secret_fleet_secret_fleet_12", WMTS_APIKEY_GIS_CLIENT="fleetkey")
    exe = os.path.join(ROOT, "target", "release", "wmts_server.exe" if os.name == "nt" else "wmts_server")
    p = subprocess.Popen([exe], cwd=ROOT, env=env, stdout=open(os.path.join(tmp, "server.log"), "w"), stderr=subprocess.STDOUT)
    for _ in range(40):
        time.sleep(0.5)
        if http("GET", f"http://127.0.0.1:{port}/healthz")[0] == 200: return p, f"http://127.0.0.1:{port}", "fleetkey"
    p.kill(); sys.exit("伺服器沒有啟動（看 " + os.path.join(tmp, "server.log") + "）")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--fleet", required=True); ap.add_argument("--base", default=None); ap.add_argument("--key", default=None)
    ap.add_argument("--spawn", action="store_true", help="自動啟動全新的伺服器（臨時資料庫與圖磚資料夾）")
    ap.add_argument("--drones", type=int, default=22); ap.add_argument("--sweep", default=None, help="逗號分隔的機數清單，每個各用全新伺服器重跑")
    ap.add_argument("--speed", type=float, default=1.0, help="拍攝節奏倍速（1＝真實時間）"); ap.add_argument("--duration", type=float, default=200, help="最長執行秒數（拍攝階段）")
    ap.add_argument("--timeout", type=float, default=30, help="單次上傳的逾時秒數（機上上傳者的設定）"); ap.add_argument("--retries", type=int, default=3)
    ap.add_argument("--drain", action="store_true", help="拍攝結束後再等積壓排空"); ap.add_argument("--drain-max", type=float, default=600)
    ap.add_argument("--ingest-concurrency", type=int, default=None, help="--spawn 時覆寫 [ingest].concurrency（預設用設定檔／內建預設 4）")
    ap.add_argument("--ingest-max-queue", type=int, default=None, help="--spawn 時覆寫 [ingest].max_queue（預設 64）")
    ap.add_argument("--port", type=int, default=8097); ap.add_argument("--out", default=None, help="把結果摘要寫成 JSON")
    a = ap.parse_args()
    counts = [int(x) for x in a.sweep.split(",")] if a.sweep else [a.drones]
    results = []
    for n in counts:
        tmp = tempfile.mkdtemp(prefix="fleet_"); proc = None
        try:
            if a.spawn: proc, a.base, a.key = spawn_server(a.port, tmp, a.ingest_concurrency, a.ingest_max_queue)
            elif not (a.base and a.key): sys.exit("需要 --spawn，或同時給 --base 與 --key")
            print(f"\n>>> {n} 架，拍攝節奏 ×{a.speed}，最長 {a.duration:.0f}s，逾時 {a.timeout:.0f}s", flush=True)
            f = Fleet(a, list(range(n))); res = f.run()
            results.append(report(f, res, f"{n} 架同時回傳"))
            if proc:
                s, b, _ = http("GET", f"{a.base}/api/v1/ingest/stats", a.key)
                if s == 200: print("伺服器自報統計：", json.dumps({k: json.loads(b)[k] for k in ("frames", "tiles_written", "process_ms", "admission")}, ensure_ascii=False))
        finally:
            if proc: proc.kill(); proc.wait(); shutil.rmtree(tmp, ignore_errors=True)
    if len(results) > 1:
        print("\n=== 機數掃描摘要 ===\n機數 | 完成/已拍攝 | 吞吐 張/s | 拍攝→可見 p50 / p95 / 最大 (s) | 伺服器排隊 p95 (s) | 截止積壓 | 逾時")
        for r in results:
            print(f"{r['drones']:>4} | {r['done']:>4}/{r['captured']:<4} | {r['thr']:>7.2f} | {r['age_p50']:>6.1f} / {r['age_p95']:>6.1f} / {r['age_max']:>6.1f} | {r['q_p95']:>6.1f} | {r['backlog']:>4} | {r['timeouts']}")
    if a.out: json.dump(results, open(a.out, "w", encoding="utf8"), ensure_ascii=False, indent=1)


if __name__ == "__main__":
    main()
