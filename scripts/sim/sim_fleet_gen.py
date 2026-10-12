"""為「多架無人機同時回傳」模擬產生每架機各自的任務影像：N 個互不重疊的任務區（約 1 km 間隔的格點），各自一組假航照。

每架機呼叫一次 sim_aerial_gen.py（地面真值同樣取自 NLSC，只存本機），平行執行以縮短時間。
輸出：<out>/drone_00 … drone_NN，每個資料夾有 frames/ 與 manifest.json。

用法：python scripts/sim/sim_fleet_gen.py --out <版本庫外的資料夾> [--drones 22] [--size 450x300] [--workers 6]
"""
import argparse, concurrent.futures as cf, os, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))


def site(i, cols=6, lon0=120.280, lat0=23.100, dlon=0.012, dlat=0.015):
    """第 i 架的任務中心：cols 欄的格點（東西 ≈ 1.2 km、南北 ≈ 1.6 km 間隔），任務區 450×300 m 遠小於間隔，不會重疊。"""
    return lon0 + (i % cols) * dlon, lat0 + (i // cols) * dlat


def one(a):
    i, out, size = a
    lon, lat = site(i)
    d = os.path.join(out, f"drone_{i:02d}")
    if os.path.exists(os.path.join(d, "manifest.json")):
        return i, "已存在", 0
    t = time.time()
    r = subprocess.run([sys.executable, "-I", os.path.join(HERE, "sim_aerial_gen.py"), "--out", d, "--center", f"{lon:.5f},{lat:.5f}", "--size", size, "--seed", str(100 + i)],
                       capture_output=True, text=True, encoding="utf8", errors="replace")
    return i, ("OK" if r.returncode == 0 else "失敗：" + r.stderr[-200:]), time.time() - t


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True); ap.add_argument("--drones", type=int, default=22)
    ap.add_argument("--size", default="450x300"); ap.add_argument("--workers", type=int, default=6)
    a = ap.parse_args(); os.makedirs(a.out, exist_ok=True)
    t0 = time.time(); bad = 0
    with cf.ThreadPoolExecutor(a.workers) as ex:
        for i, st, dt in ex.map(one, [(i, a.out, a.size) for i in range(a.drones)]):
            print(f"drone_{i:02d} 中心 {site(i)[0]:.4f},{site(i)[1]:.4f}  {st}  {dt:.0f}s", flush=True); bad += st.startswith("失敗")
    n = sum(len(os.listdir(os.path.join(a.out, d, "frames"))) for d in os.listdir(a.out) if d.startswith("drone_") and os.path.isdir(os.path.join(a.out, d, "frames")))
    print(f"完成：{a.drones} 架、共 {n} 張，耗時 {time.time()-t0:.0f}s，失敗 {bad}")
    sys.exit(1 if bad else 0)


main()
