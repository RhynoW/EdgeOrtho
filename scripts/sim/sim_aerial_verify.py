"""驗證假航照產生器：(1) 從 JPEG 讀回 EXIF GPS／高度／時間與 DJI XMP 姿態，和 manifest 的「紀錄姿態」比對；
(2) 只用「讀回的」姿態做直接地理定位鑲嵌，和地面真值比較，量化定位誤差（公尺）與影像差異。

用法：python scripts/sim/sim_aerial_verify.py <sim 輸出資料夾>
"""
import json, math, os, re, sys

import cv2
import numpy as np
import pyproj
from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sim_aerial_gen as G

T_TM2 = pyproj.Transformer.from_crs(4326, 3826, always_xy=True)


def read_pose(path):
    im = Image.open(path); ex = im.getexif(); g = ex.get_ifd(0x8825)
    ll = lambda v: float(v[0]) + float(v[1]) / 60 + float(v[2]) / 3600
    lat = ll(g[2]) * (1 if g[1] == "N" else -1); lon = ll(g[4]) * (1 if g[3] == "E" else -1)
    xmp = im.info.get("xmp", b"").decode("utf8", "replace")
    x = {k: float(v) for k, v in re.findall(r'drone-dji:(\w+)="([+\-0-9.]+)"', xmp)}
    return dict(lat=lat, lon=lon, alt=float(g[6]), time=ex.get_ifd(0x8769).get(36867), **x), im


def main():
    root = sys.argv[1]; man = json.load(open(os.path.join(root, "manifest.json"), encoding="utf8"))
    fr = man["frames"]; ge0, gn0 = man["gt_origin"]; gw, gh = man["gt_px"]
    errs_exif, errs_att, foot_err = [], [], []
    canvas = np.zeros((gh, gw), np.float32); wsum = np.zeros((gh, gw), np.float32); perf = np.zeros_like(canvas); wp = np.zeros_like(canvas)
    for f in fr:
        p, im = read_pose(os.path.join(root, "frames", f["file"])); rec = f["recorded"]
        e, n = T_TM2.transform(p["lon"], p["lat"])
        errs_exif.append(math.hypot(e - rec["e"], n - rec["n"]))                                    # EXIF 寫入/讀回的位置誤差（度分秒量化）
        errs_att.append(max(abs(p["GimbalYawDegree"] - rec["yaw"]), abs(p["GimbalPitchDegree"] - rec["pitch"]), abs(p["GimbalRollDegree"] - rec["roll"])))
        # 直接地理定位：以「讀回姿態」推算 footprint 中心 = 機下點 + agl·tan(傾角)（與產生器的幾何相同；偏航用讀回的 yaw）
        th = math.radians(p["GimbalYawDegree"]); up = (math.sin(th), math.cos(th)); right = (math.cos(th), -math.sin(th))
        rr, pp = math.tan(math.radians(p["GimbalRollDegree"])), math.tan(math.radians(p["GimbalPitchDegree"] + 90))
        agl = p["RelativeAltitude"]
        est_e, est_n = e + agl * (rr * right[0] + pp * up[0]), n + agl * (rr * right[1] + pp * up[1])
        foot_err.append(math.hypot(est_e - f["true"]["footprint_e"], est_n - f["true"]["footprint_n"]))
        gray = np.asarray(im.convert("L"), np.float32)
        for (ce, cn, yaw, cv, wv) in ((est_e, est_n, p["GimbalYawDegree"], canvas, wsum), (f["true"]["footprint_e"], f["true"]["footprint_n"], f["true"]["yaw"], perf, wp)):
            t = math.radians(yaw); cs, sn = math.cos(t), math.sin(t); X0, Y0 = (ce - ge0) / G.RES, (gn0 - cn) / G.RES
            # 影像 (u,v) → 鑲嵌 (x,y)：x = X0 + (u-cx)cs + (cy-v)sn ； y = Y0 + (u-cx)sn - (cy-v)cs
            cx, cy = G.W / 2, G.H / 2
            M = np.float32([[cs, -sn, X0 - cx * cs + cy * sn], [sn, cs, Y0 - cx * sn - cy * cs]])
            warped = cv2.warpAffine(gray, M, (gw, gh), flags=cv2.INTER_LINEAR)
            mask = cv2.warpAffine(np.ones_like(gray), M, (gw, gh), flags=cv2.INTER_NEAREST)
            cv[:] += warped * mask; wv[:] += mask
    ok = wsum > 0
    est = np.where(ok, canvas / np.maximum(wsum, 1), 0); per = np.where(wp > 0, perf / np.maximum(wp, 1), 0)
    gt = np.asarray(Image.fromarray(G.ground_truth(ge0 + 1, gn0 - gh * G.RES + 1, ge0 + gw * G.RES - 1, gn0 - 1, os.path.join(root, "_gt_cache"))[0]).convert("L"), np.float32)
    h = min(gt.shape[0], gh); w = min(gt.shape[1], gw)
    both = ok[:h, :w] & (wp[:h, :w] > 0)
    d_est = float(np.abs(est[:h, :w] - gt[:h, :w])[both].mean()); d_per = float(np.abs(per[:h, :w] - gt[:h, :w])[both].mean())
    print(f"影像 {len(fr)} 張；涵蓋 {ok.mean()*100:.0f}% 的真值範圍；平均每像素被 {wsum[ok].mean():.1f} 張覆蓋")
    print(f"EXIF 位置讀回誤差  最大 {max(errs_exif):.4f} m（度分秒量化）｜姿態讀回最大差 {max(errs_att):.3f}°")
    fe = np.array(foot_err); print(f"直接地理定位誤差（footprint 中心，公尺） 平均 {fe.mean():.2f}｜P95 {np.percentile(fe,95):.2f}｜最大 {fe.max():.2f}（RTK={man['rtk']}，agl={man['agl_m']} m）")
    print(f"與地面真值的平均灰階差：紀錄姿態 {d_est:.2f}｜真實姿態（下限，只含雜訊與重採樣）{d_per:.2f}（0–255）")
    out = np.stack([np.clip(est, 0, 255)] * 3, -1).astype(np.uint8); Image.fromarray(out).save(os.path.join(root, "mosaic_recorded_pose.png"))
    sys.exit(0 if max(errs_exif) < 0.01 and max(errs_att) < 0.02 and d_est < d_per + 12 else 1)


main()
