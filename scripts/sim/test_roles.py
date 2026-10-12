"""角色分權（client／uploader／admin）的 HTTP 端到端測試：每個角色 × 每個端點的狀態碼矩陣、JWT role 聲明、拒絕被稽核。

前置：以含三把金鑰的設定啟動（config/layers.demo.toml 已有）：
  WMTS_APIKEY_GIS_CLIENT=<admin 金鑰> WMTS_APIKEY_UPLOADER=<uploader 金鑰> WMTS_APIKEY_CLIENT_ONLY=<client 金鑰>，並需要 `--features ingest` 的建置。
用法：python scripts/sim/test_roles.py --base http://127.0.0.1:8098 --admin-key A --uploader-key U --client-key C --jwt-secret S [--jwt-issuer wmts-gis]
"""
import argparse, base64, hashlib, hmac, json, sys, time, urllib.error, urllib.request


def b64(b): return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def jwt(secret, issuer, role="__none__", sub="tester", exp_in=600, tamper=False):
    claims = {"sub": sub, "iss": issuer, "iat": int(time.time()), "exp": int(time.time()) + exp_in}
    if role != "__none__": claims["role"] = role
    msg = b64(json.dumps({"alg": "HS256", "typ": "JWT"}).encode()) + "." + b64(json.dumps(claims).encode())
    sig = hmac.new((secret + ("x" if tamper else "")).encode(), msg.encode(), hashlib.sha256).digest()
    return msg + "." + b64(sig)


def call(base, method, path, key=None, bearer=None, body=None):
    h = {}
    if key: h["X-API-Key"] = key
    if bearer: h["Authorization"] = "Bearer " + bearer
    r = urllib.request.Request(base + path, data=body, method=method, headers=h)
    try:
        with urllib.request.urlopen(r, timeout=30) as f: return f.status, f.read()
    except urllib.error.HTTPError as e: return e.code, e.read()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True); ap.add_argument("--admin-key", required=True); ap.add_argument("--uploader-key", required=True); ap.add_argument("--client-key", required=True)
    ap.add_argument("--jwt-secret", required=True); ap.add_argument("--jwt-issuer", default="wmts-gis")
    a = ap.parse_args(); base = a.base.rstrip("/"); ok = True
    def chk(name, cond, extra=""):
        nonlocal ok; ok &= bool(cond); print(("PASS " if cond else "FAIL ") + name, extra)

    # (方法, 路徑, 內容, 各角色預期)：none / client / uploader / admin。POST 影像用垃圾內容：過了權限檢查會是 400（不是 JPEG），權限不足是 403。
    cap = "/wmts/1.0.0/WMTSCapabilities.xml"
    matrix = [
        ("GET", cap, None, (401, 200, 200, 200)),
        ("GET", "/api/v1/tracks", None, (401, 200, 200, 200)),
        ("GET", "/api/v1/ingest/stats", None, (401, 403, 200, 200)),
        ("POST", "/api/v1/ingest/frame?session_id=roles", b"garbage", (401, 403, 400, 400)),
        ("GET", "/api/v1/audit", None, (401, 403, 403, 200)),
        ("GET", "/api/v1/audit/verify", None, (401, 403, 403, 200)),
        ("GET", "/api/v1/audit/export", None, (401, 403, 403, 200)),
        ("GET", "/api/v1/availability", None, (401, 403, 403, 200)),
        ("GET", "/api/v1/ingest/audit", None, (401, 403, 403, 200)),
    ]
    keys = [None, a.client_key, a.uploader_key, a.admin_key]
    names = ["未驗證", "client", "uploader", "admin"]
    bad = []
    for m, p, body, want in matrix:
        for k, n, w in zip(keys, names, want):
            s, _ = call(base, m, p, k, body=body)
            if s != w: bad.append((n, m, p, s, w))
            time.sleep(0.02)
    chk(f"角色 × 端點矩陣（{len(matrix)} 個端點 × 4 種身分）狀態碼全部符合", not bad, bad[:6])

    # JWT 的 role 聲明
    J = lambda **kw: jwt(a.jwt_secret, a.jwt_issuer, **kw)
    s_admin, _ = call(base, "GET", "/api/v1/audit", bearer=J(role="admin")); chk("JWT role=admin 可讀稽核", s_admin == 200, s_admin)
    s_none, _ = call(base, "GET", "/api/v1/audit", bearer=J()); chk("JWT 沒有 role → 視為 client，讀稽核 403", s_none == 403, s_none)
    s_root, _ = call(base, "GET", "/api/v1/audit", bearer=J(role="root")); chk("JWT role 未知值（root）不升權 → 403", s_root == 403, s_root)
    s_up, _ = call(base, "POST", "/api/v1/ingest/frame", bearer=J(role="uploader"), body=b"garbage"); chk("JWT role=uploader 可上傳（過權限檢查，內容不是 JPEG → 400）", s_up == 400, s_up)
    s_forged, _ = call(base, "GET", "/api/v1/audit", bearer=J(role="admin", tamper=True)); chk("簽章被竄改的 admin JWT → 401", s_forged == 401, s_forged)

    # 拒絕被稽核：誰、什麼角色、需要什麼角色
    time.sleep(1.2)
    s, b = call(base, "GET", "/api/v1/audit?outcome=forbidden&limit=100", a.admin_key); ents = json.loads(b)["entries"]
    role_rows = [e for e in ents if e["detail"].startswith("insufficient_role")]
    actors = {e["actor"] for e in role_rows}
    chk("權限不足被稽核（forbidden:insufficient_role），含身分與角色", any("僅讀取" in x for x in actors) and any("影像上傳" in x for x in actors) and any(x.startswith("jwt:") for x in actors), actors)
    chk("稽核 detail 說明角色與所需角色", any("role=client needs=admin" in e["detail"] for e in role_rows) and any("role=uploader needs=admin" in e["detail"] for e in role_rows), [e["detail"] for e in role_rows][:3])
    chk("稽核不含任何金鑰與 JWT", all(k not in json.dumps(ents) for k in (a.admin_key, a.uploader_key, a.client_key)))
    sys.exit(0 if ok else 1)


main()
