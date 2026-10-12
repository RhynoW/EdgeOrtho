"""外部可用率探測：從伺服器「外面」每隔幾秒對 /readyz 取樣，並與伺服器自己量的月報表逐分鐘比對。

為什麼需要它：伺服器自己量的可用率（GET /api/v1/availability）量不到「機器關機、網路斷線、反向代理或 DNS 故障」這類
外部才看得到的中斷（內部只會呈現成沒有紀錄的分鐘，方向對但分不出原因），所以正式驗收（規範：月可用率 ≥ 99.5%）
要有一個獨立在外的探測當第二份證據。把這支腳本放在另一台機器（或至少另一個行程）持續執行。

用法：
  # 1) 持續探測（Ctrl-C 結束；記錄用附加模式，重啟探測器不會洗掉舊紀錄）
  python scripts/probe_readyz.py probe --base https://wmts.example.com --out probe.jsonl [--interval 10] [--duration 3600]
  # 2) 與伺服器內部報表逐分鐘比對（需要 API Key；期間預設為記錄涵蓋的範圍，不含頭尾不完整的分鐘）
  python scripts/probe_readyz.py compare --base https://wmts.example.com --key <API key> --log probe.jsonl [--tz 8] [--tolerance 1]

判定：外部一分鐘「可用」＝該分鐘至少有一次探測，且全部成功（HTTP 200 且 status=ok）；該分鐘沒有任何探測＝「未知」，不列入比較。
比較結果：agree（一致）、外部停機但內部可用（內部盲點：網路、代理、DNS）、外部可用但內部停機（內部誤報：例如重啟的空窗）。
不一致分鐘數超過 --tolerance 時結束代碼為 1。
"""
import argparse, json, os, sys, time, urllib.error, urllib.request
from datetime import datetime, timedelta, timezone


def probe(a):
    end = time.time() + a.duration if a.duration else None
    n = ok_n = 0
    with open(a.out, "a", encoding="utf8") as f:
        try:
            while end is None or time.time() < end:
                t0 = time.time(); status = 0; ok = False
                try:
                    with urllib.request.urlopen(a.base.rstrip("/") + "/readyz", timeout=5) as r:
                        status = r.status; ok = status == 200 and json.loads(r.read()).get("status") == "ok"
                except urllib.error.HTTPError as e: status = e.code
                except Exception: status = 0
                f.write(json.dumps({"t": round(t0, 3), "ok": ok, "status": status, "ms": round((time.time() - t0) * 1000, 1)}) + "\n"); f.flush()
                n += 1; ok_n += ok
                time.sleep(max(0, a.interval - (time.time() - t0)))
        except KeyboardInterrupt:
            pass
    print(f"探測 {n} 次，成功 {ok_n} 次 → {a.out}")


def fetch_internal(a, from_min, to_min):
    tz = a.tz
    fmt = lambda m: (datetime(1970, 1, 1, tzinfo=timezone.utc) + timedelta(minutes=m + tz * 60)).strftime("%Y-%m-%dT%H:%M")
    url = f"{a.base.rstrip('/')}/api/v1/availability?from={fmt(from_min)}&to={fmt(to_min)}&tz={tz}&detail=true&strict=true"
    with urllib.request.urlopen(urllib.request.Request(url, headers={"X-API-Key": a.key}), timeout=30) as r:
        return json.load(r)


def compare(a):
    rows = [json.loads(l) for l in open(a.log, encoding="utf8") if l.strip()]
    if len(rows) < 3: sys.exit("探測紀錄太少")
    by = {}
    for r in rows: by.setdefault(int(r["t"] // 60), []).append(r["ok"])
    ext = {m: all(v) for m, v in by.items()}
    lo, hi = min(by) + 1, max(by)                      # 去掉頭尾不完整的分鐘
    if hi - lo < 1: sys.exit("探測涵蓋不到一個完整分鐘")
    j = fetch_internal(a, lo, hi)
    internal = {m["minute"]: m["state"] for m in (j.get("minutes") or [])}
    agree = ext_down_int_up = ext_up_int_down = unknown = 0; diffs = []
    for m in range(lo, hi):
        if m not in by: unknown += 1; continue
        if m not in internal: unknown += 1; continue          # 內部尚未有紀錄（早於內部第一筆）
        e, i = ext[m], internal[m] == "up"
        if e == i: agree += 1
        elif not e and i: ext_down_int_up += 1; diffs.append((m, "外部停機但內部可用（內部盲點）", internal[m]))
        else: ext_up_int_down += 1; diffs.append((m, "外部可用但內部停機（內部誤報）", internal[m]))
    judged = agree + ext_down_int_up + ext_up_int_down
    ext_pct = 100 * sum(1 for m in range(lo, hi) if m in internal and ext.get(m)) / max(judged, 1)
    int_pct = 100 * sum(1 for m in range(lo, hi) if m in by and internal.get(m) == "up") / max(judged, 1)
    ts = lambda m: (datetime(1970, 1, 1, tzinfo=timezone.utc) + timedelta(minutes=m + a.tz * 60)).strftime("%m-%d %H:%M")
    print(f"比較期間 {ts(lo)} ～ {ts(hi)}（本地時間 UTC{a.tz:+d}），共判定 {judged} 分鐘，未知 {unknown} 分鐘")
    print(f"一致 {agree}；外部停機但內部可用 {ext_down_int_up}；外部可用但內部停機 {ext_up_int_down}")
    print(f"可用率：外部 {ext_pct:.2f}%｜內部 {int_pct:.2f}%（目標 {j['target_pct']}%）")
    for m, why, st in diffs[:20]: print(f"  {ts(m)}  {why}（內部狀態 {st}）")
    sys.exit(0 if len(diffs) <= a.tolerance else 1)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("probe"); p.add_argument("--base", required=True); p.add_argument("--out", required=True)
    p.add_argument("--interval", type=float, default=10.0); p.add_argument("--duration", type=float, default=0)
    c = sub.add_parser("compare"); c.add_argument("--base", required=True); c.add_argument("--key", required=True); c.add_argument("--log", required=True)
    c.add_argument("--tz", type=int, default=8); c.add_argument("--tolerance", type=int, default=1)
    a = ap.parse_args()
    {"probe": probe, "compare": compare}[a.cmd](a)


main()
