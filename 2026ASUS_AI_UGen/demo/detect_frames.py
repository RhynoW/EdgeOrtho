"""EdgeOrtho Stage I demo, step 2: object detection on each frame + georeferencing of the detections.

Detector: google/owlv2-base-patch16-ensemble (open-vocabulary, Apache-2.0). This is a STAGE I PROTOTYPE used to exercise
the interfaces (frame -> detections -> GeoJSON). It is not the planned edge model (that would be a small INT8 detector
compiled for Hailo-10H). No accuracy claim is made by this script beyond what the report prints.

Pose -> ground uses the same convention as the server's direct georeferencing: footprint centre =
recorded position shifted by agl*tan(tilt); image up = heading direction; GSD = agl * gsd_per_agl.

Usage: python detect_frames.py --sim <dir with frames/ and manifest.json> --out <dir> [--limit N] [--thr 0.25]
"""
import argparse, json, math, os, time

import numpy as np
import pyproj
import torch
from PIL import Image, ImageDraw
from transformers import Owlv2ForObjectDetection, Owlv2Processor

# (text query, class name, category, plausible max-dimension range in metres at ground level)
CLASSES = [
    ("a car", "car", "vehicle", (2.0, 7.0)),
    ("a truck", "truck", "vehicle", (3.0, 14.0)),
    ("a bus", "bus", "vehicle", (6.0, 14.0)),
    ("a motorcycle", "motorcycle", "vehicle", (1.2, 3.0)),
    ("an excavator", "excavator", "machinery", (3.0, 12.0)),
    ("a person", "person", "person", (0.4, 2.5)),
    ("a boat", "boat", "boat", (2.0, 15.0)),
    ("a temporary shed with a metal roof", "shed", "structure", (3.0, 40.0)),
    ("a building", "building", "structure", (5.0, 60.0)),
    ("a pile of debris or waste", "debris", "debris", (1.5, 30.0)),
]
QUERIES = [c[0] for c in CLASSES]
CLASS_OF = {i: c[1] for i, c in enumerate(CLASSES)}
T_WGS = pyproj.Transformer.from_crs(3826, 4326, always_xy=True)


def px_to_en(rec, u, v, W, H, gsd, agl):
    """Pixel (u right, v down) -> TM2 (E, N)."""
    th = math.radians(rec["yaw"]); up = (math.sin(th), math.cos(th)); right = (math.cos(th), -math.sin(th))
    pitch_dev = rec["pitch"] + 90.0
    fe = rec["e"] + agl * (math.tan(math.radians(rec["roll"])) * right[0] + math.tan(math.radians(pitch_dev)) * up[0])
    fn = rec["n"] + agl * (math.tan(math.radians(rec["roll"])) * right[1] + math.tan(math.radians(pitch_dev)) * up[1])
    dx, dy = (u - W / 2) * gsd, (H / 2 - v) * gsd
    return fe + dx * right[0] + dy * up[0], fn + dx * right[1] + dy * up[1]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sim", required=True); ap.add_argument("--out", required=True)
    ap.add_argument("--limit", type=int, default=0); ap.add_argument("--thr", type=float, default=0.25)
    ap.add_argument("--device", default="cuda" if torch.cuda.is_available() else "cpu")
    ap.add_argument("--annot", type=int, default=12, help="number of annotated frames to save")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    man = json.load(open(os.path.join(a.sim, "manifest.json"), encoding="utf8"))
    frames = man["frames"][: a.limit or None]
    W, H = man["image_px"] if False else (1536, 1024)
    agl, gsd = man["agl_m"], man["gsd_m"]

    proc = Owlv2Processor.from_pretrained("google/owlv2-base-patch16-ensemble")
    model = Owlv2ForObjectDetection.from_pretrained("google/owlv2-base-patch16-ensemble").to(a.device).eval()
    lat = []; dets = []; shown = 0
    for k, fr in enumerate(frames):
        im = Image.open(os.path.join(a.sim, "frames", fr["file"])).convert("RGB")
        W, H = im.size
        t0 = time.time()
        inp = proc(text=[QUERIES], images=im, return_tensors="pt").to(a.device)
        with torch.no_grad():
            out = model(**inp)
        side = max(W, H)     # OWLv2 pads to a square; boxes are relative to the padded square
        res = proc.post_process_grounded_object_detection(out, threshold=a.thr, target_sizes=torch.tensor([[side, side]], device=a.device))[0]
        if a.device == "cuda": torch.cuda.synchronize()
        lat.append(time.time() - t0)
        rec = fr["recorded"]
        keep = []
        for box, score, lab in zip(res["boxes"].cpu().numpy(), res["scores"].cpu().numpy(), res["labels"].cpu().numpy()):
            x0, y0, x1, y1 = [float(v) for v in box]
            cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
            if cx >= W or cy >= H: continue                      # centre in the padding
            w_m, h_m = (x1 - x0) * gsd, (y1 - y0) * gsd
            lo, hi = CLASSES[int(lab)][3]
            if not (lo <= max(w_m, h_m) <= hi): continue          # size sanity per class (ground metres)
            e, n = px_to_en(rec, cx, cy, W, H, gsd, agl)
            lon, la = T_WGS.transform(e, n)
            keep.append(dict(frame=fr["file"], cls=CLASS_OF[int(lab)], category=CLASSES[int(lab)][2], score=round(float(score), 3), box=[round(v, 1) for v in (x0, y0, x1, y1)],
                             size_m=[round(w_m, 2), round(h_m, 2)], e=e, n=n, lon=lon, lat=la))
        dets += keep
        if shown < a.annot and keep:
            d = ImageDraw.Draw(im)
            for q in keep:
                d.rectangle(q["box"], outline=(255, 40, 40), width=3); d.text((q["box"][0], max(0, q["box"][1] - 12)), f"{q['cls']} {q['score']:.2f}", fill=(255, 255, 0))
            im.save(os.path.join(a.out, f"annot_{k:04d}.jpg"), quality=85); shown += 1
        if k % 20 == 0: print(f"{k}/{len(frames)} frames, {len(dets)} raw detections, last {lat[-1]*1000:.0f} ms", flush=True)

    json.dump(dict(device=a.device, threshold=a.thr, queries=QUERIES, frames=len(frames), per_frame_ms=lat, detections=dets),
              open(os.path.join(a.out, "raw_detections.json"), "w", encoding="utf8"), ensure_ascii=False)
    l = np.array(lat[1:] if len(lat) > 1 else lat) * 1000
    print(f"frames {len(frames)}; raw detections {len(dets)}; latency ms p50 {np.percentile(l, 50):.0f} p95 {np.percentile(l, 95):.0f} on {a.device}")


if __name__ == "__main__":
    main()
