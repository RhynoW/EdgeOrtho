"""同一張影像同時重送的端到端測試：兩個請求同時到達時，只能處理一次——一個回 200（processed），另一個回 409（正在處理）或 200（duplicate）；
之後再送一定是 200 duplicate。用法：python scripts/sim/test_ingest_dedupe.py --base http://127.0.0.1:8097 --key <uploader 或 admin 金鑰> --frame <某張假航照.jpg>"""
import argparse, json, sys, threading, urllib.error, urllib.request


def post(base, key, data, session):
    r = urllib.request.Request(f"{base}/api/v1/ingest/frame?session_id={session}", data=data, method="POST", headers={"X-API-Key": key})
    try:
        with urllib.request.urlopen(r, timeout=120) as f: return f.status, json.loads(f.read())
    except urllib.error.HTTPError as e: return e.code, json.loads(e.read() or b"{}")


def main():
    ap = argparse.ArgumentParser(); ap.add_argument("--base", required=True); ap.add_argument("--key", required=True); ap.add_argument("--frame", required=True)
    a = ap.parse_args(); data = open(a.frame, "rb").read(); out = []
    ths = [threading.Thread(target=lambda i=i: out.append(post(a.base, a.key, data, f"dup-{i}"))) for i in range(6)]
    [t.start() for t in ths]; [t.join() for t in ths]
    ok = True
    def chk(name, cond, extra=""):
        nonlocal ok; ok &= bool(cond); print(("PASS " if cond else "FAIL ") + name, extra)
    codes = sorted(s for s, _ in out); proc = [j for s, j in out if s == 200 and j.get("status") == "processed"]
    chk("6 個同時重送：恰好 1 個真的處理（processed）", len(proc) == 1, codes)
    chk("其餘不是 409（正在處理）就是 200 duplicate，沒有第二次處理，也沒有 5xx", all(s == 409 or (s == 200 and j.get("status") in ("processed", "duplicate")) for s, j in out) and not any(s >= 500 for s, _ in out), codes)
    s, j = post(a.base, a.key, data, "dup-after"); chk("處理完成後再送 → 200 duplicate", s == 200 and j["status"] == "duplicate", (s, j.get("status")))
    sys.exit(0 if ok else 1)


main()
