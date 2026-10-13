---
title: EdgeOrtho demo
emoji: 🛰️
colorFrom: blue
colorTo: gray
sdk: static
pinned: false
license: cc-by-4.0
short_description: River-patrol map demo with review candidates
---

# EdgeOrtho — static demo

2026 ASUS UGen AI League Innovation Hackathon · Stage I · Workplace AI × Battlefield Lightning

A **static** demonstration: mosaic tiles and candidate detections were produced offline from synthetic frames; no server is running.

- **What you see:** a simulated drone flight over the Touchian River (Hsinchu City), built from a real 3.64 cm orthophoto, published as a z17–z21 tile pyramid, with object *candidates* for human review.
- **What it is not:** live inference, UGen300 measurements, verified detections, or an orthorectified survey product.
- **Label on screen:** *Synthetic flight replay · Precomputed detections · Desktop GPU prototype · No UGen300 measurements yet.*
- **Access code:** the entry code is a convenience gate for the review period. It is checked in the browser and is **not** a security control; the content is CC BY 4.0 data and prototype output.

Source and architecture: https://github.com/RhynoW/EdgeOrtho

## Attribution and licences

| Item | Licence |
|---|---|
| Orthophoto "2020-09-13 新竹市頭前溪金雅－溪埔段 (Jinya – Sipu, Touchian River, Hsinchu City)" — Water Resources Agency MOEA, Hsinchu City Environmental Protection Bureau, Taiwan Ecological Society, FeiMao Spatial Information; via OpenAerialMap (id 5f642ef0363d930006324989). Cut into synthetic frames, mosaicked, re-encoded as WebP tiles. | CC BY 4.0 |
| Detector `google/owlv2-base-patch16-ensemble` (prototype) | Apache-2.0 |
| Base map © OpenStreetMap contributors | ODbL |
| Viewer: Leaflet | BSD-2-Clause |
