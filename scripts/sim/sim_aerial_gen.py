"""近即時正射模擬器：假航照產生器。

用國土測繪中心(NLSC)的 PHOTO3826 正射影像當「地面真值」，依割草機式航線（前後重疊 80%、左右重疊 70%）
切出一張張假的無人機影像，寫入 EXIF（GPS 經緯度、高度、拍攝時間）與 DJI 風格 XMP（機身／雲台姿態、RTK 旗標），
並輸出 manifest.json（含「真實姿態」與「紀錄姿態」兩組，供之後量測直接地理定位的誤差）。

為什麼這樣做：台灣沒有找到可直接匿名下載、授權明確、帶完整姿態的原始航照（見 docs/ortho-simulation.md），
所以先用已知地面真值合成；它能驗證「上傳→增量鑲嵌→圖磚更新」的管線與延遲，**不能**驗證真實相機的鏡頭畸變與空三精度。

限制（請務必理解）：
- 地面真值是 NLSC 的第 13 層（約 0.265 m/像素），所以假影像的 GSD 固定約 0.265 m，**不是**規範的 5 cm。
  影像尺寸預設縮為 1024×768（約 271×203 m 涵蓋），讓 5 cm 的重疊率與涵蓋公式仍可套用。
- 平坦地面、針孔相機、無鏡頭畸變、無地形起伏；姿態只用位移近似（不做透視變形）。
- NLSC 影像的授權條款我們尚未確認，**只存本機**（--out 請指向版本庫外的資料夾），不要提交或上傳。

用法：
  python scripts/sim/sim_aerial_gen.py --out <資料夾> [--center 120.31,23.13] [--size 900x600] [--seed 1] [--rtk|--no-rtk]
"""
import argparse, io, json, math, os, random, sys, time, urllib.request
from datetime import datetime, timedelta, timezone

import numpy as np
import pyproj
from PIL import Image
from PIL.TiffImagePlugin import IFDRational as R

# NLSC EPSG:3826 官方格網（見 config/layers.toml，已用 scripts/verify_tms_nlsc.py 對照）
ORIGIN = (50000.0, 3000000.0)
LEVEL, RES = 13, 944.9423660750896 * 0.00028      # 0.26458 m/像素
TILE = 256
TILE_URL = "https://wmts.nlsc.gov.tw/97/wmts/PHOTO3826/default/default028mm/{z}/{r}/{c}"
W, H = 1024, 768                                    # 假影像尺寸（像素）
T_WGS = pyproj.Transformer.from_crs(3826, 4326, always_xy=True)


def fetch_tile(r, c, cache):
    fn = os.path.join(cache, f"{LEVEL}_{r}_{c}.jpg")
    if not os.path.exists(fn):
        req = urllib.request.Request(TILE_URL.format(z=LEVEL, r=r, c=c), headers={"User-Agent": "Mozilla/5.0 (sim-aerial)"})
        for k in range(3):
            try: data = urllib.request.urlopen(req, timeout=30).read(); break
            except Exception:
                if k == 2: return None
                time.sleep(1.5)
        open(fn, "wb").write(data); time.sleep(0.08)    # 禮貌：不要連續灌請求
    return Image.open(fn).convert("RGB")


def ground_truth(e0, n0, e1, n1, cache):
    """回傳 (影像 ndarray, 左上角 E, 左上角 N)，涵蓋 [e0,e1]×[n0,n1]（TM2 公尺），解析度 RES。"""
    span = TILE * RES
    c0, c1 = int((e0 - ORIGIN[0]) // span), int((e1 - ORIGIN[0]) // span)
    r0, r1 = int((ORIGIN[1] - n1) // span), int((ORIGIN[1] - n0) // span)
    os.makedirs(cache, exist_ok=True)
    mos = Image.new("RGB", ((c1 - c0 + 1) * TILE, (r1 - r0 + 1) * TILE))
    miss = 0
    for r in range(r0, r1 + 1):
        for c in range(c0, c1 + 1):
            t = fetch_tile(r, c, cache)
            if t is None: miss += 1; continue
            mos.paste(t, ((c - c0) * TILE, (r - r0) * TILE))
    if miss: print(f"警告：{miss} 塊圖磚取不到（留黑）", file=sys.stderr)
    return np.asarray(mos), ORIGIN[0] + c0 * span, ORIGIN[1] - r0 * span


def plan_mission(cx, cy, w, h, fwd=0.80, side=0.70):
    """割草機式航線：沿東西向飛，影像上方＝機頭方向。回傳 [(E, N, 航向 yaw 度, 航線編號)]。"""
    along, across = H * RES, W * RES                   # 影像高度＝沿航線方向涵蓋；寬度＝橫向涵蓋
    step, spacing = along * (1 - fwd), across * (1 - side)
    nl = max(1, int(h // spacing) + 1)
    out = []
    for i in range(nl):
        n = cy - h / 2 + i * spacing
        k = max(1, int(w // step) + 1)
        xs = [cx - w / 2 + j * step for j in range(k)]
        if i % 2: xs.reverse()
        yaw = 90.0 if i % 2 == 0 else 270.0            # 東向 / 西向
        out += [(x, n, yaw, i) for x in xs]
    return out, step, spacing


def render(gt, gt_e0, gt_n0, e, n, yaw_deg, rng):
    """以真實姿態把地面真值轉成假影像（仿射：旋轉＋平移，無透視）。"""
    th = math.radians(yaw_deg); cs, sn = math.cos(th), math.sin(th)
    cx, cy = W / 2, H / 2
    X0, Y0 = (e - gt_e0) / RES, (gt_n0 - n) / RES
    a, b, c = cs, -sn, X0 - cx * cs + cy * sn
    d, ee, f = sn, cs, Y0 - cx * sn - cy * cs
    im = Image.fromarray(gt).transform((W, H), Image.AFFINE, (a, b, c, d, ee, f), resample=Image.BILINEAR)
    arr = np.asarray(im).astype(np.float32)
    arr = arr * rng.uniform(0.95, 1.05) + rng.normal(0, 1.5, arr.shape)      # 曝光差異＋感光雜訊
    return Image.fromarray(np.clip(arr, 0, 255).astype(np.uint8))


def dms(v):
    v = abs(v); d = int(v); m = int((v - d) * 60); s = (v - d - m / 60) * 3600
    return (R(d, 1), R(m, 1), R(int(round(s * 10000)), 10000))


def xmp_packet(rec):
    return ('<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF '
            'xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" '
            'xmlns:drone-dji="http://www.dji.com/drone-dji/1.0/" '
            f'drone-dji:AbsoluteAltitude="{rec["alt_asl"]:+.3f}" drone-dji:RelativeAltitude="{rec["alt_agl"]:+.3f}" '
            f'drone-dji:GimbalYawDegree="{rec["yaw"]:+.2f}" drone-dji:GimbalPitchDegree="{rec["pitch"]:+.2f}" drone-dji:GimbalRollDegree="{rec["roll"]:+.2f}" '
            f'drone-dji:FlightYawDegree="{rec["yaw"]:+.2f}" drone-dji:FlightPitchDegree="{rec["pitch"] + 90:+.2f}" drone-dji:FlightRollDegree="{rec["roll"]:+.2f}" '
            f'drone-dji:RtkFlag="{rec["rtk_flag"]}" drone-dji:SimulatedData="1"/></rdf:RDF></x:xmpmeta><?xpacket end="w"?>').encode("utf-8")


def write_jpeg(path, im, rec, when):
    ex = Image.Exif()
    ex[0x010F] = "SimDrone"; ex[0x0110] = "SIM-RTK-16MP"
    ex.get_ifd(0x8769)[36867] = when.strftime("%Y:%m:%d %H:%M:%S")            # DateTimeOriginal
    g = ex.get_ifd(0x8825)
    g[1] = "N" if rec["lat"] >= 0 else "S"; g[2] = dms(rec["lat"])
    g[3] = "E" if rec["lon"] >= 0 else "W"; g[4] = dms(rec["lon"])
    g[5] = 0; g[6] = R(int(round(rec["alt_asl"] * 1000)), 1000)                # 高度（海平面以上）
    g[16] = "T"; g[17] = R(int(round(rec["yaw"] % 360 * 100)), 100)            # GPSImgDirection（真北）
    im.save(path, "JPEG", quality=90, exif=ex.tobytes(), xmp=xmp_packet(rec))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", required=True, help="輸出資料夾（請放在版本庫外）")
    ap.add_argument("--center", default="120.31,23.13", help="經度,緯度（預設台南善化一帶）")
    ap.add_argument("--size", default="900x600", help="任務範圍 東西x南北，公尺")
    ap.add_argument("--agl", type=float, default=100.0, help="相對起飛點高度（公尺）")
    ap.add_argument("--terrain", type=float, default=15.0, help="地面海拔（公尺），平坦假設")
    ap.add_argument("--speed", type=float, default=10.0, help="飛行速度 m/s，決定拍攝時間間隔")
    ap.add_argument("--seed", type=int, default=1)
    g = ap.add_mutually_exclusive_group()
    g.add_argument("--rtk", dest="rtk", action="store_true", default=True, help="RTK 固定解（定位雜訊 σ≈2 cm，預設）")
    g.add_argument("--no-rtk", dest="rtk", action="store_false", help="一般 GNSS（σ≈1.5 m）")
    a = ap.parse_args()

    rng = np.random.default_rng(a.seed); random.seed(a.seed)
    lon, lat = (float(v) for v in a.center.split(",")); mw, mh = (float(v) for v in a.size.lower().split("x"))
    cx, cy = pyproj.Transformer.from_crs(4326, 3826, always_xy=True).transform(lon, lat)
    pts, step, spacing = plan_mission(cx, cy, mw, mh)
    margin = max(W, H) * RES * 0.75 + 30
    print(f"任務：{len(pts)} 張、{len(set(p[3] for p in pts))} 條航線；拍攝間距 {step:.1f} m、航線間距 {spacing:.1f} m；GSD {RES:.3f} m")
    gt, ge0, gn0 = ground_truth(cx - mw / 2 - margin, cy - mh / 2 - margin, cx + mw / 2 + margin, cy + mh / 2 + margin, os.path.join(a.out, "_gt_cache"))
    print(f"地面真值：{gt.shape[1]}×{gt.shape[0]} 像素")

    os.makedirs(os.path.join(a.out, "frames"), exist_ok=True)
    t0 = datetime(2026, 10, 10, 2, 0, 0, tzinfo=timezone.utc); t = 0.0; prev = None; frames = []
    sig_pos = 0.02 if a.rtk else 1.5
    for i, (e, n, yaw, line) in enumerate(pts):
        if prev is not None: t += math.hypot(e - prev[0], n - prev[1]) / a.speed + (6.0 if prev[2] != line else 0)   # 換線轉彎 6 秒
        prev = (e, n, line)
        # 真實姿態：風造成俯仰／橫滾偏差（σ≈1°），footprint 因此偏離機下點 agl·tan(傾角)
        pitch_dev, roll = float(rng.normal(0, 1.0)), float(rng.normal(0, 1.0))
        yaw_true = yaw + float(rng.normal(0, 1.5))
        th = math.radians(yaw_true); up = (math.sin(th), math.cos(th)); right = (math.cos(th), -math.sin(th))
        fe = e + a.agl * (math.tan(math.radians(roll)) * right[0] + math.tan(math.radians(pitch_dev)) * up[0])
        fn = n + a.agl * (math.tan(math.radians(roll)) * right[1] + math.tan(math.radians(pitch_dev)) * up[1])
        im = render(gt, ge0, gn0, fe, fn, yaw_true, rng)
        # 紀錄姿態：位置加定位雜訊；姿態感測器雜訊 σ≈0.1°（雲台角度）
        re_, rn_ = e + float(rng.normal(0, sig_pos)), n + float(rng.normal(0, sig_pos))
        rlon, rlat = T_WGS.transform(re_, rn_)
        rec = dict(lat=rlat, lon=rlon, alt_asl=a.terrain + a.agl + float(rng.normal(0, sig_pos if a.rtk else 3.0)), alt_agl=a.agl,
                   yaw=yaw_true + float(rng.normal(0, 0.1)), pitch=-90 + pitch_dev + float(rng.normal(0, 0.1)), roll=roll + float(rng.normal(0, 0.1)),
                   rtk_flag=50 if a.rtk else 0)
        when = t0 + timedelta(seconds=t)
        fn_ = f"SIM_{i:04d}.jpg"; write_jpeg(os.path.join(a.out, "frames", fn_), im, rec, when)
        frames.append(dict(file=fn_, t_s=round(t, 2), line=line, time=when.isoformat(),
                           true=dict(e=e, n=n, footprint_e=fe, footprint_n=fn, yaw=yaw_true, pitch_dev=pitch_dev, roll=roll),
                           recorded=dict(rec, e=re_, n=rn_)))
    man = dict(generator="scripts/sim/sim_aerial_gen.py", gt_source="NLSC PHOTO3826 level 13 (local use only)", crs="EPSG:3826",
               gsd_m=RES, image_px=[W, H], overlap=dict(forward=0.8, side=0.7), agl_m=a.agl, rtk=a.rtk, seed=a.seed, gt_origin=[ge0, gn0],
               gt_px=[gt.shape[1], gt.shape[0]], frames=frames)
    json.dump(man, open(os.path.join(a.out, "manifest.json"), "w", encoding="utf8"), ensure_ascii=False, indent=1)
    print(f"完成：{len(frames)} 張 → {os.path.join(a.out, 'frames')}；總任務時間 {t/60:.1f} 分鐘")


if __name__ == "__main__":
    main()
