"""稽核雜湊鏈、驗證與匯出的 HTTP 端到端測試（含「直接改資料庫檔案」的竄改偵測）。

前置：啟動伺服器（任何設定皆可，但要用乾淨的資料庫），並把資料庫路徑帶給 --db（腳本會用 sqlite3 直接改它來模擬竄改）。
用法：python scripts/sim/test_audit_chain.py --base http://127.0.0.1:8098 --key <API key> --db <伺服器的 sqlite 檔>
"""
import argparse, hashlib, json, sqlite3, struct, sys, time, urllib.error, urllib.request


def req(base, path, key=None, headers=None):
    h = dict(headers or {})
    if key: h["X-API-Key"] = key
    try:
        with urllib.request.urlopen(urllib.request.Request(base + path, headers=h), timeout=30) as f: return f.status, f.read(), dict(f.headers)
    except urllib.error.HTTPError as e: return e.code, e.read(), dict(e.headers)


def row_hash(prev, *fields):
    h = hashlib.sha256()
    for f in (prev,) + fields:
        b = (f or "").encode(); h.update(struct.pack("<I", len(b))); h.update(b)
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True); ap.add_argument("--key", required=True); ap.add_argument("--db", required=True)
    a = ap.parse_args(); base, key = a.base.rstrip("/"), a.key
    ok = True
    def chk(name, cond, extra=""):
        nonlocal ok; ok &= bool(cond); print(("PASS " if cond else "FAIL ") + name, extra)
    def js(path, h=None):
        s, b, _ = req(base, path, key, h); return s, json.loads(b) if b[:1] in b"{[" else b

    # 產生幾筆不同原因的稽核（節流單位是 IP＋結果:原因，所以各一筆）
    req(base, "/wmts/1.0.0/WMTSCapabilities.xml"); req(base, "/wmts/1.0.0/WMTSCapabilities.xml", "bad-key-xyz")
    req(base, "/wmts/1.0.0/WMTSCapabilities.xml", headers={"Authorization": "Bearer abc.def"}); req(base, "/wmts/1.0.0/WMTSCapabilities.xml", key, {"Origin": "http://evil.example"})
    time.sleep(1.2)

    s, v = js("/api/v1/audit/verify")
    chk("雜湊鏈完整", s == 200 and v["ok"] is True and v["rows_checked"] >= 4 and v["head_hash"], {k: v.get(k) for k in ("ok", "rows_checked", "unchained_rows", "head_id")})
    head_id, head_hash = v["head_id"], v["head_hash"]

    s, raw, hd = req(base, "/api/v1/audit/export", key)
    lines = [json.loads(l) for l in raw.decode("utf8").strip().split("\n")]
    rows, man = lines[:-1], lines[-1]["manifest"]
    chk("匯出為 JSON Lines，最後一行是清單", s == 200 and "ndjson" in hd.get("Content-Type", hd.get("content-type", "")) and man["rows"] == len(rows), (s, man["rows"], len(rows)))
    chk("清單含鏈頭與鏈狀態", man["chain"]["ok"] is True and man["chain"]["head_hash"] and man["range"]["from"] is None, man["chain"])
    # 匯出的資料自己也能獨立驗證（不需信任伺服器）：用 Python 重算雜湊
    prev, good = "", True
    for r in rows:
        if r["hash"] is None: continue
        good &= r["prev_hash"] == prev and r["hash"] == row_hash(prev, r["ts"], r["actor"], r["ip"], r["action"], r["resource"], r["outcome"], r["detail"]); prev = r["hash"]
    chk("匯出檔以獨立程式（Python）重算雜湊鏈：全部吻合", good and len(rows) >= 4)
    s, e = js("/api/v1/audit?action=audit_export")
    chk("匯出動作本身被稽核（誰、範圍、鏈狀態）", e["entries"] and e["entries"][0]["actor"].startswith("apikey:") and "chain_ok=true" in e["entries"][0]["detail"], e["entries"][0] if e["entries"] else None)
    s, raw2, _ = req(base, "/api/v1/audit/export?outcome=forbidden", key)
    sub = [json.loads(l) for l in raw2.decode("utf8").strip().split("\n")]
    chk("匯出可依結果過濾", len(sub) >= 2 and all(r["outcome"] == "forbidden" for r in sub[:-1]), len(sub) - 1)

    # 外部錨點
    s, v2 = js(f"/api/v1/audit/verify?expected_id={head_id}&expected_hash={head_hash}")
    chk("以外部保存的鏈頭驗證：anchor_match=true", v2["ok"] and v2["anchor_match"] is True, v2.get("anchor_match"))
    s, _ = js(f"/api/v1/audit/verify?expected_id={head_id}"); chk("只給 expected_id 不給 hash → 400", s == 400, s)

    # 竄改：直接改資料庫檔案（伺服器仍在執行）
    con = sqlite3.connect(a.db); mid = rows[1]["id"]
    con.execute("UPDATE audit_log SET outcome='ok', detail='nothing happened' WHERE id=?", (mid,)); con.commit()
    s, v3 = js("/api/v1/audit/verify")
    chk("改動內容 → 在該筆斷鏈", v3["ok"] is False and v3["first_bad_id"] == mid and "修改" in (v3["reason"] or ""), {k: v3.get(k) for k in ("ok", "first_bad_id", "reason")})
    # 攻擊者把整條鏈從該筆起重算（資料庫自己看是完整的），只有外部錨點抓得到
    allrows = con.execute("SELECT id, ts, actor, ip, action, resource, outcome, detail FROM audit_log WHERE id >= ? ORDER BY id", (mid,)).fetchall()
    prev = con.execute("SELECT hash FROM audit_log WHERE id < ? AND hash IS NOT NULL ORDER BY id DESC LIMIT 1", (mid,)).fetchone()
    prev = prev[0] if prev else ""
    for r in allrows:
        h = row_hash(prev, *r[1:]); con.execute("UPDATE audit_log SET prev_hash=?, hash=? WHERE id=?", (prev, h, r[0])); prev = h
    con.commit(); con.close()
    s, v4 = js("/api/v1/audit/verify")
    chk("重算整條鏈後，單看資料庫發現不了（這正是需要外部錨點的原因）", v4["ok"] is True, v4["ok"])
    s, v5 = js(f"/api/v1/audit/verify?expected_id={head_id}&expected_hash={head_hash}")
    chk("帶外部錨點驗證 → 抓到重算過的鏈", v5["ok"] is False and v5["anchor_match"] is False, {k: v5.get(k) for k in ("ok", "anchor_match", "reason")})
    sys.exit(0 if ok else 1)


main()
