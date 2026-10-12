"""EdgeOrtho Stage I demo, step 1: synthetic drone frames cut from a CC-BY 4.0 orthophoto (OpenAerialMap).

Source imagery (attribution required, CC BY 4.0):
  "2020-09-13 新竹市頭前溪金雅－溪埔段 (Jinya - Sipu, Touchian River, Hsinchu City, Taiwan)"
  經濟部水利署, 新竹市環境保護局, 社團法人台灣生態學會, 康鷹空間資訊有限公司
  https://data.depositar.io/en/dataset/hcc_tcr_ecomap_2020-03 , via OpenAerialMap (5f642ef0363d930006324989)
  DJI Phantom 4 RTK, 3.64 cm/px, EPSG:3826, Cloud-Optimized GeoTIFF (read by HTTP range requests, no full download).

It reuses the mission planner / renderer / EXIF+XMP writer of scripts/sim/sim_aerial_gen.py and only replaces the
ground-truth source. Everything else (flat ground, pinhole camera, simulated pose noise) is the same simulation
convention: this validates the pipeline, NOT real-camera accuracy.

Usage: python make_frames.py --out <dir> [--center-e 248069 --center-n 2746772] [--size 200x140]
"""
import argparse, json, os, sys

import numpy as np
import pyproj
import rasterio
from rasterio.windows import from_bounds

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "scripts", "sim"))
import sim_aerial_gen as sag  # noqa: E402

COG = "https://oin-hotosm-temp.s3.amazonaws.com/5f642aab363d930006324987/0/5f642aab363d930006324988.tif"
GSD = 0.0364
ATTRIBUTION = ("Imagery: 2020-09-13 Touchian River (Jinya-Sipu), Hsinchu City; Water Resources Agency MOEA, "
               "Hsinchu City Environmental Protection Bureau, Taiwan Ecological Society, FeiMao/Kangying Spatial Info; "
               "CC BY 4.0, via OpenAerialMap")


def cog_ground_truth(e0, n0, e1, n1, cache):
    with rasterio.Env(GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR", CPL_VSIL_CURL_ALLOWED_EXTENSIONS=".tif"):
        d = rasterio.open(COG)
        b = d.bounds
        e0, n0, e1, n1 = max(e0, b.left), max(n0, b.bottom), min(e1, b.right), min(n1, b.top)
        win = from_bounds(e0, n0, e1, n1, d.transform).round_offsets().round_lengths()
        arr = d.read(window=win)
        t = d.window_transform(win)
    return np.ascontiguousarray(np.moveaxis(arr, 0, 2)), t.c, t.f


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--center-e", type=float, default=248069.0)
    ap.add_argument("--center-n", type=float, default=2746772.0)
    ap.add_argument("--size", default="200x140")
    ap.add_argument("--agl", type=float, default=100.0)
    ap.add_argument("--seed", type=int, default=1)
    a = ap.parse_args()

    # replace the NLSC ground truth by the CC-BY orthophoto; frame size 1536x1024, GSD 3.64 cm
    sag.RES, sag.W, sag.H = GSD, 1536, 1024
    sag.ground_truth = cog_ground_truth
    lon, lat = pyproj.Transformer.from_crs(3826, 4326, always_xy=True).transform(a.center_e, a.center_n)
    sys.argv = ["make_frames", "--out", a.out, "--center", f"{lon:.7f},{lat:.7f}", "--size", a.size,
                "--agl", str(a.agl), "--terrain", "100", "--seed", str(a.seed), "--rtk"]
    sag.main()

    mp = os.path.join(a.out, "manifest.json")
    m = json.load(open(mp, encoding="utf8"))
    m["gt_source"] = "OpenAerialMap 5f642ef0363d930006324989 (CC BY 4.0)"
    m["attribution"] = ATTRIBUTION
    m["gsd_per_agl"] = GSD / a.agl      # value to put in [ingest].gsd_per_agl for this mission
    json.dump(m, open(mp, "w", encoding="utf8"), ensure_ascii=False, indent=1)
    print("manifest updated; gsd_per_agl =", m["gsd_per_agl"])


if __name__ == "__main__":
    main()
