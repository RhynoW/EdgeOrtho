"""對照 config/layers*.toml 的 TWD97_TM2_NLSC 格網與國土測繪中心(NLSC)官方 WMTS 服務。

用法：python scripts/verify_tms_nlsc.py [設定檔 ...]      （預設 config/layers.toml、config/layers.local.toml）
來源：https://wmts.nlsc.gov.tw/97/wmts（EPSG:3826，另有 EPSG:3825）。注意 https://wmts.nlsc.gov.tw/wmts 只有 EPSG:3857。
比對項目：原點、層數、每層比例尺分母（resolution / 0.00028）、MatrixWidth、MatrixHeight。全部一致才回傳 0。
"""
import os, sys, tomllib, urllib.request
import xml.etree.ElementTree as ET

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
URL = "https://wmts.nlsc.gov.tw/97/wmts?SERVICE=WMTS&REQUEST=GetCapabilities"
NS = {"w": "http://www.opengis.net/wmts/1.0", "o": "http://www.opengis.net/ows/1.1"}


def nlsc_grid():
    raw = urllib.request.urlopen(urllib.request.Request(URL, headers={"User-Agent": "Mozilla/5.0"}), timeout=60).read()
    for t in ET.fromstring(raw).findall(".//w:Contents/w:TileMatrixSet", NS):
        if t.find("o:SupportedCRS", NS).text.endswith("::3826"):
            ms = [(float(m.find("w:ScaleDenominator", NS).text), tuple(float(v) for v in m.find("w:TopLeftCorner", NS).text.split()),
                   int(m.find("w:MatrixWidth", NS).text), int(m.find("w:MatrixHeight", NS).text)) for m in t.findall("w:TileMatrix", NS)]
            return ms
    raise SystemExit("官方服務沒有 EPSG:3826 格網")


def main():
    files = sys.argv[1:] or ["config/layers.toml", "config/layers.local.toml"]
    ref = nlsc_grid(); bad = 0
    print(f"NLSC：{len(ref)} 層，原點 {ref[0][1]}")
    for f in files:
        cfg = tomllib.load(open(os.path.join(ROOT, f), "rb"))
        t = next(x for x in cfg["tilematrixsets"] if x["identifier"] == "TWD97_TM2_NLSC")
        errs = []
        if tuple(t["top_left_corner"]) != ref[0][1]: errs.append(f"原點 {t['top_left_corner']} ≠ {ref[0][1]}")
        if len(t["levels"]) != len(ref): errs.append(f"層數 {len(t['levels'])} ≠ {len(ref)}")
        for i, ((res, w, h), (sd, _, rw, rh)) in enumerate(zip(t["levels"], ref)):
            if abs(res / 0.00028 - sd) / sd > 1e-9 or (int(w), int(h)) != (rw, rh): errs.append(f"第 {i} 層 {res/0.00028:.6g},{int(w)}x{int(h)} ≠ {sd:.6g},{rw}x{rh}")
        print(("OK   " if not errs else "FAIL ") + f, *errs[:5])
        bad += bool(errs)
    sys.exit(1 if bad else 0)


main()
