# EdgeOrtho — Stage I submission repository

2026 ASUS UGen AI League Innovation Hackathon · Workplace AI · Battlefield Lightning (ASUS UGen300 / Hailo-10H)

**EdgeOrtho** turns drone imagery from river patrols into an immediate, governed map: frames are georeferenced and
mosaicked into a WMTS tile pyramid within seconds, objects around the river channel and embankment are flagged as
candidates, and everything is served behind role-based access with a tamper-evident audit trail.

> Status: Stage I prototype. The detection model is a zero-shot open-vocabulary detector used only to exercise the
> interfaces. **No measurement on UGen300 / Hailo hardware exists yet.** See [ARCHITECTURE.md](ARCHITECTURE.md) for the
> exact built / simulated / planned boundary.

## Contents

| Path | What |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | System architecture, evidence, limitations, Stage II plan |
| `demo/make_frames.py` | Synthetic drone frames (EXIF + XMP pose) cut from a CC BY 4.0 orthophoto (read by HTTP range requests) |
| `demo/detect_frames.py` | Per-frame detection and georeferencing of the detections |
| `demo/merge_detections.py` | Merge duplicates across overlapping frames, write GeoJSON |
| `demo/run_demo.py`, `demo/viewer.html` | Ingest into a fresh server (EPSG:3857 z21) and show mosaic + candidates |
| `demo/run_viewer.bat` | One-click viewer on Windows: builds frames if missing, starts a fresh server, opens Edge with the live mosaic and candidates (set `EDGEORTHO_SIM` to reuse frames; extra arguments go to `run_demo.py`) |
| `demo/output/` | Detections GeoJSON and viewer screenshots |
| `src/`, `Cargo.toml` | Rust (Axum) WMTS server with ingest, roles, audit chain, availability (only in the full repository export) |

## Reproduce the demo (Windows / Linux)

```bash
# 1. server (feature `ingest`)
cargo build --release --features ingest
# 2. frames from the CC BY 4.0 orthophoto (no full download)
python demo/make_frames.py --out <dir>/sim --center-e 248302 --center-n 2746446 --size 220x140
# 3. detections (downloads the Apache-2.0 detector from Hugging Face on first run)
python demo/detect_frames.py --sim <dir>/sim --out <dir>/det --thr 0.2 --annot 0
python demo/merge_detections.py --raw <dir>/det/raw_detections.json --out demo/output/detections.geojson
# 4. ingest + screenshots (needs `pip install websockets` and Edge)
python demo/run_demo.py --sim <dir>/sim --shots <dir>/shots
```

Python deps: `numpy pillow rasterio pyproj torch transformers websockets`.

## Third-party material and attribution

| Item | Licence | Note |
|---|---|---|
| Orthophoto "2020-09-13 新竹市頭前溪金雅－溪埔段 (Jinya - Sipu, Touchian River, Hsinchu City)" — Water Resources Agency MOEA, Hsinchu City Environmental Protection Bureau, Taiwan Ecological Society, FeiMao Spatial Information; via OpenAerialMap (id 5f642ef0363d930006324989) | CC BY 4.0 | Used as simulation ground truth. Not redistributed in this repository except in small screenshots with attribution |
| `google/owlv2-base-patch16-ensemble` | Apache-2.0 | Zero-shot detector used by the prototype |
| OpenStreetMap | ODbL | Base map in the viewer, attribution shown |

No API keys, secrets or personal data are in this repository; keys are read from environment variables.
