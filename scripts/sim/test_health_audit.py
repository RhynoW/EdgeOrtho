"""稽核（401／403／429）、健康檢查與可用率報表的 HTTP 端到端測試。

前置：用「API Key 限流很低」的設定啟動伺服器（預設每分鐘 30 次），例如以 config/layers.demo.toml 為基礎改：
  bind = "127.0.0.1:8098"、allowed_origins／allowed_referers／public_base_url 改成 http://127.0.0.1:8098、
  api_key_rate_limit_per_minute = 30、db_path 指到乾淨的暫存資料庫。
用法：python scripts/sim/test_health_audit.py --base http://127.0.0.1:8098 --key <API key> [--limit 30] [--wait-minutes 2.3]
  --wait-minutes：等待可用率取樣累積的時間（每分鐘寫一筆；0 略過可用率的數值檢查，只檢查格式）。
"""
import argparse, json, sys, time, urllib.error, urllib.request

BAD_KEY = "NOT-A-REAL-KEY-7f3a9c"
BAD_JWT = "not.a.jwt-ABCDEF123456"


def req(base, path, key=None, headers=None, method="GET"):
    h = dict(headers or {})
    if key: h["X-API-Key"] = key
    r = urllib.request.Request(base + path, headers=h, method=method)
    try:
        with urllib.request.urlopen(r, timeout=30) as f: return f.status, f.read()
    except urllib.error.HTTPError as e: return e.code, e.read()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True); ap.add_argument("--key", required=True)
    ap.add_argument("--limit", type=int, default=30); ap.add_argument("--wait-minutes", type=float, default=2.3)
    a = ap.parse_args(); base, key = a.base.rstrip("/"), a.key
    ok = True
    def chk(name, cond, extra=""):
        nonlocal ok; ok &= bool(cond); print(("PASS " if cond else "FAIL ") + name, extra)
    def audit(**q):
        qs = "&".join(f"{k}={v}" for k, v in q.items())
        s, b = req(base, "/api/v1/audit?limit=200&" + qs, key); assert s == 200, (s, b[:200]); return json.loads(b)["entries"]
    cap = "/wmts/1.0.0/WMTSCapabilities.xml"

    # --- 健康檢查 ---
    s, b = req(base, "/healthz"); chk("/healthz 200 ok", s == 200 and b == b"ok", (s, b))
    s, b = req(base, "/readyz"); j = json.loads(b)
    chk("/readyz（免驗證）200 且資料庫正常", s == 200 and j["status"] == "ok" and j["checks"]["database"] is True, j)
    chk("/readyz 不洩漏路徑與設定", all(x not in b.decode() for x in ("tiles/", "\\", ".sqlite", "gpkg", "config")), b.decode()[:160])

    # --- 稽核：各種被拒絕的存取（每種原因各一筆，節流以 IP＋結果:原因 為單位）---
    s, _ = req(base, cap); chk("沒有憑證 → 401", s == 401, s)
    s, _ = req(base, cap, BAD_KEY); chk("未知 API Key → 401", s == 401, s)
    s, _ = req(base, cap, headers={"Authorization": "Bearer " + BAD_JWT}); chk("無效 Bearer → 401", s == 401, s)
    s, _ = req(base, cap, key, headers={"Origin": "http://evil.example"}); chk("不在白名單的 Origin → 403", s == 403, s)
    time.sleep(1.0)   # 稽核是非同步寫入
    e401, e403 = audit(outcome="unauthorized"), audit(outcome="forbidden")
    reasons = {x["detail"].split(":")[0] for x in e401}
    chk("稽核含 no_credentials／unknown_api_key／invalid_bearer 三種 401", {"no_credentials", "unknown_api_key", "invalid_bearer"} <= reasons, reasons)
    chk("稽核含 403（origin_or_referer）且記下被拒的 Origin", any("evil.example" in x["detail"] for x in e403), [x["detail"] for x in e403][:2])
    chk("稽核欄位：actor＝anonymous、有來源 IP、resource 含方法與路徑", all(x["actor"] == "anonymous" and x["ip"] and x["resource"].startswith("GET /wmts") for x in e401), e401[0])
    dump = json.dumps(audit(), ensure_ascii=False)
    chk("稽核紀錄不含任何密鑰值（壞 API Key、壞 JWT、正確的 Key）", BAD_KEY not in dump and BAD_JWT not in dump and key not in dump)

    # --- 節流：同一 IP 大量壞請求不會灌爆稽核表，且次數不漏 ---
    n_before = len(audit(outcome="unauthorized"))
    for _ in range(60): req(base, cap, BAD_KEY)
    time.sleep(1.0)
    n_mid = len(audit(outcome="unauthorized"))
    chk("60 次壞請求只多寫 ≤ 2 筆（節流）", n_mid - n_before <= 2, (n_before, n_mid))
    time.sleep(10.5)
    req(base, cap, BAD_KEY); time.sleep(1.0)
    latest = [x for x in audit(outcome="unauthorized") if x["detail"].startswith("unknown_api_key")][0]
    sup = int(latest["detail"].split("suppressed=")[1]) if "suppressed=" in latest["detail"] else 0
    chk("視窗過後的下一筆帶出被略過的次數（≥ 50）", sup >= 50, latest["detail"])

    # --- 限流 → 429 並記錄 ---
    codes = []
    for _ in range(a.limit + 20):
        s, _ = req(base, cap, key); codes.append(s)
        if s == 429: break
    chk(f"超過每分鐘 {a.limit} 次 → 429", 429 in codes, f"{len(codes)} 次內出現 429")
    time.sleep(9.0)   # 讓限流額度回補一些（約每 2 秒回補 1 次），之後查稽核的請求才不會又被 429
    e429 = audit(outcome="rate_limited")
    chk("限流寫入稽核（actor＝apikey:<label>，不含密鑰）", e429 and e429[0]["actor"].startswith("apikey:") and key not in json.dumps(e429), e429[0] if e429 else None)

    # --- 可用率報表 ---
    s, _ = req(base, "/api/v1/availability?month=2026-13", key); chk("月份格式錯誤 → 400", s == 400, s)
    s, b = req(base, "/api/v1/availability", None); chk("可用率報表需要驗證", s == 401, s)
    if a.wait_minutes > 0:
        print(f"等待 {a.wait_minutes} 分鐘讓可用率取樣累積…", flush=True); time.sleep(a.wait_minutes * 60)
    time.sleep(0.5)
    # 限流視窗：等過一分鐘後才不會被 429（取樣等待已經超過）
    s, b = req(base, "/api/v1/availability", key); j = json.loads(b) if s == 200 else {}
    chk("可用率報表 200，含目標、定義與事件清單", s == 200 and j.get("target_pct") == 99.5 and "definition" in j and "incidents" in j, list(j)[:8])
    if a.wait_minutes > 0:
        chk("已有取樣：total_minutes ≥ 1、up_minutes ≥ 1", j.get("total_minutes", 0) >= 1 and j.get("up_minutes", 0) >= 1, {k: j.get(k) for k in ("measured_from", "total_minutes", "up_minutes", "down_minutes", "availability_pct")})
        chk("可用率 ≥ 99.5%（meets_target）", j.get("meets_target") is True, j.get("availability_pct"))
        print("報表節錄：", json.dumps({k: j[k] for k in ("month", "total_minutes", "up_minutes", "down_minutes", "availability_pct", "allowed_downtime_minutes", "meets_target")}, ensure_ascii=False))
    sys.exit(0 if ok else 1)


main()
