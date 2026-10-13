# EdgeOrtho — System Architecture (v2.2)

**EdgeOrtho: On-Device AI Turns Drone Imagery into Immediate, Auditable River-Patrol Map Intelligence**

2026 ASUS UGen AI League Innovation Hackathon · Stage I · Theme: Workplace AI · Battlefield: Lightning (ASUS UGen300 / Hailo-10H) · Finals track: Onsite
Document status: v2.2 for external technical review. v1 (architecture-first) is kept as `ARCHITECTURE.v1-review.md`.
v2.2 incorporates an independent review of v2.1 (scope of the live loop, resolution vs accuracy, edge-host split, terminology, audit scope, acceptance targets).
Repository: `github.com/RhynoW/EdgeOrtho` (private until submission). The repository is a clean export: server source, simulation scripts, this document and the demo.

---

## 0. Evidence tags

Every claim in this document carries one of three tags. This is deliberate: the project is partly built and partly planned.

| Tag | Meaning |
|---|---|
| **[Built]** | Implemented in the repository and exercised by tests or measurements shown here |
| **[Simulated]** | Works end to end, but only on synthetic or replayed inputs, not a real drone/camera/edge device |
| **[Planned]** | Stage II design intent. **Nothing tagged [Planned] has been implemented or measured.** |

**Not yet existing:** any measurement on UGen300/Hailo hardware; any trained or fine-tuned detection model; any labelled ground truth for detection.

---

## 1. Executive summary

**What.** River-management agencies patrol channels and embankments with drones, but the imagery is processed in batches after the flight, and what is found is kept apart from the map. EdgeOrtho closes that loop: each uploaded frame is georeferenced and merged into a WMTS tile pyramid within seconds **[Built]**; objects around the channel and embankment (vehicles, temporary structures, tarpaulin-covered plots, debris piles, people, boats) are shown as map-native *candidates for human review* — today as precomputed overlays **[Simulated]**, per-frame on an edge device in Stage II **[Planned]**; delivery sits behind role-based access with a tamper-evident log of denied requests, uploads and exports.

**For whom.** Water-resources and river-patrol units, county-level water bureaus, engineering consultants, drone survey contractors. Public-sector delivery requirements — standard OGC WMTS, API-key/token access, HTTPS, audit, availability reporting — are first-class, not an afterthought.

**Where is the AI.** *Today (Stage I):* a zero-shot open-vocabulary model (Apache-2.0) runs offline on a desktop GPU over synthetic frames; its output is merged into GeoJSON and displayed. *Stage II (planned):* the same model assists annotation, a small INT8 detector is trained on the corrected labels, compiled for the Hailo-10H and run on an edge host with the UGen300, posting detections alongside each frame **[Planned]**. Candidates always require human review; none is a verified finding.

**What is demonstrated today** (all **[Simulated]**, real orthophoto of the Touchian River, Hsinchu City, 3.64 cm/px, CC BY 4.0):

| Result | Value |
|---|---|
| Frames processed into a z21 (≈ 6.8 cm/px) tile pyramid | 270 frames in 38.6 s (7.0 frames/s, 4 uploaders, same machine) |
| Server processing time per frame | p50 395 ms, p95 748 ms |
| 22-drone fleet, capture → tile visible | p50 79 s → 4.4 s after tile-level locking (single machine, 0.265 m GSD) |
| Detection candidates on the river segment | 6 merged candidates from 28 raw detections, **unverified** |
| Detector latency (development GPU, not UGen300) | p50 1.0 s, p95 1.7 s per 1536×1024 frame |

**What is not demonstrated.** Live per-frame inference, sidecar ingestion and server-managed candidate publication; detection accuracy (no ground truth); UGen300 latency/throughput/power; real-camera georeferencing accuracy; long-term availability. Section 9 lists these.

---

## 2. Problem and users

### 2.1 Today's workflow
1. Fly the segment.
2. Download images, run photogrammetry in batch (hours).
3. Inspect the result manually, in a different tool from the map that is delivered.
4. Deliver files; access and audit are whatever the file share provides.

### 2.2 Gaps
| Gap | Consequence |
|---|---|
| Results arrive after the mission | Findings cannot guide the same flight (re-fly, closer look) |
| Findings are separate from the map | No place for a reviewer to see "what was flagged, where, when, by what" |
| Delivery lacks governance | Procurement specifications ask for role-based access, audit trail and availability reporting |

### 2.3 Users and what each needs
| User | Needs |
|---|---|
| Patrol officer | Latest tiles and flagged objects while the drone is still in the air |
| Case reviewer | Candidates with position, time, score, and the source frames |
| Data owner / auditor | Who was denied, who uploaded and exported, and proof the log was not altered (successful tile reads are **not** logged today, see 5.3) |
| Contractor / uploader | Push frames; no access to admin functions |

### 2.4 Requirement source
A public-sector drone procurement specification (summary only; not republished) asked for: ~2,000 raw images per month, 5 cm GSD, WMTS in EPSG:3857 and EPSG:3826, API key/token with HTTPS, 99.5 % monthly availability, and track synchronisation.

---

## 3. Solution overview

```
 FIELD                               SERVER (Rust / Axum)                                   CLIENTS
┌────────┐   ┌───────────────────┐   ┌─────────────────────────────────────────────┐   ┌───────────────────┐
│ Drone  │──▶│ Edge host         │──▶│ auth ▶ role ▶ admission control             │   │ Leaflet viewer    │
│ JPEG + │   │ + UGen300 (USB)   │   │ POST /api/v1/ingest/frame                   │   │  • live mosaic    │
│EXIF/XMP│   │ detector [Planned]│   │  decode ▶ pose ▶ georeference               │──▶│  • candidates     │
└────────┘   │ → GeoJSON sidecar │   │  ▶ mosaic (3857 z21 / 3826 L13) ▶ pyramid   │   │  • historical     │
             └───────────────────┘   │  ▶ atomic tile writes                       │   │    river maps     │
                                     │ GET /wmts … tiles                           │   └───────────────────┘
                                     │ audit chain · availability · /readyz        │
                                     └─────────────────────────────────────────────┘
```

**Three paths — do not conflate them.**

| Path | What runs | Status |
|---|---|---|
| A. Offline prototype | Synthetic frames → desktop-GPU detector → merged GeoJSON → viewer (precomputed overlay) | **[Simulated]**, what the demo shows |
| B. Live ingest | Frame upload → georeference → mosaic pyramid → WMTS tiles within seconds | **[Built]**, exercised with synthetic frames |
| C. Live AI | Edge host + UGen300 detect each frame → detections posted with the frame → server stores and serves a candidates layer → reviewer workflow | **[Planned]** (Stage II) |

**Edge host split (planned).** The UGen300 is a USB accelerator, not a standalone computer. The chain is: camera or flight controller → acquisition link → **edge host** (ground-station laptop or on-board computer; image decode, tiling, resize, NMS, georeferencing, upload) → **UGen300 over USB** (inference only) → server. How frames reach the host *during* flight depends on the aircraft's link; if frames are only available after landing, the system gives post-flight rather than in-flight candidates. This is an open design decision.

| Component | Status |
|---|---|
| WMTS server (KVP + RESTful, capabilities), tile matrix sets 3857 and official NLSC 3826 | **[Built]** |
| Ingest: decode, pose, direct georeferencing, incremental mosaic, pyramid, atomic writes | **[Built]** / **[Simulated]** inputs |
| Concurrency: per-tile locks, bounded queue, in-flight de-duplication | **[Built]** |
| Roles, hash-chained audit log, availability report, `/readyz` | **[Built]** |
| Frame generator from a real orthophoto, prototype detector, merge to GeoJSON, demo viewer | **[Built]** (this repository, `demo/`) |
| Detector on UGen300 (HailoRT / HEF), sidecar ingestion, candidates API with review status, detections layer served by the server | **[Planned]** |
| Historical 1938 Touchian River map as a comparison layer | **[Built]** (viewer layer list) |
| Recording mode of the demo (`run_demo.py --watch`): paced upload with a live-refreshing viewer, used to film the mosaic growing | **[Built]** |

---

## 4. AI architecture

### 4.1 Role of the edge device
Run detection at capture time so that (a) candidates can be known before the drone leaves the segment (if frames reach the edge host in flight, see section 3), (b) the AI result is a compact metadata record travelling alongside the image stream that mapping already requires — it does not require sending the full image a second time — and (c) the same pose used for the mosaic georeferences the detections.

### 4.2 Stage I prototype **[Built]** / **[Simulated]**
| Item | Value |
|---|---|
| Detector | `google/owlv2-base-patch16-ensemble` (open-vocabulary, Apache-2.0), text queries for ten classes |
| Classes | car, truck, bus, motorcycle, excavator, person, boat, temporary shed with a metal roof, building, pile of debris/waste |
| Filters | Per-class plausible size range in ground metres; per-class minimum score; duplicates merged across overlapping frames by ground proximity within the same category (radius grows with object size) |
| Merge limits | No time window, no box-overlap test, no event identity or review status; the merged GeoJSON does not keep the list of source frames (the raw JSON does). Moving objects can split and nearby objects can merge |
| Output | GeoJSON points/boxes with `frames` seen, `best_score`, `class_votes`; marked as candidates |

**Observed behaviour (honest).** On the river segment the highest-scoring "boat" candidates were blue and green tarpaulin-covered plots on the riverbank, not boats; a "debris" candidate was a bare patch with scattered material; one candidate sits in the channel and has not been checked. On the parking-lot/bridge segment the vehicle localisation was good, but buses were frequently labelled "truck". **The class names from a zero-shot model are therefore not reliable; the useful signal is "a man-made object of interest is here".** The prototype is not presented as a detector with a known precision.

### 4.3 Planned edge model **[Planned]**
1. **Teacher-assisted annotation and supervised training.** Use the open-vocabulary model to pre-label river-corridor tiles; correct labels by hand, **including missed objects in regions the teacher did not box**; train a small detector on the corrected labels. This is not knowledge distillation (no teacher logits or feature loss are used). The architecture and weights must have a licence compatible with the contest terms; none has been selected or checked yet.
2. **Compile.** Quantise to INT8 and compile with the Hailo Dataflow Compiler (x86 Linux) to a HEF; run with HailoRT on the UGen300.
3. **Tiling.** Full-resolution frames (e.g. 5472×3648 ≈ 20 MP on the Phantom 4 RTK that produced the source data) are cut into overlapping crops sized to the model input; boxes are merged, then georeferenced.
4. **Output.** A compact detections record (event identity, source frame, session, capture time, geometry, model version, score, class, source box, review status) posted with the frame; retries are idempotent on (frame, model version); the server stores it and serves a candidates layer next to the mosaic.

**Evaluation protocol (planned).** Split by river reach, date or independent flight — never by overlapping crops of the same image. Keep a fully hand-annotated evaluation set outside teacher selection and tuning. Start with two or three classes that can be labelled reliably. Report per-class precision, recall, missed-object rate and false alarms per reach.

### 4.4 Hardware (vendor figures, not measured here)
UGen300 = Hailo-10H, up to 40 TOPS INT4 / 20 TOPS INT8, 8 GB LPDDR4, ~2.5 W typical for the device alone (not the whole host-plus-accelerator system), USB or M.2; HailoRT with PyTorch/ONNX/TensorFlow conversion.

**Toolchain compatibility is a gate, not an assumption.** The public Hailo Model Zoo targets Hailo-10/15 on its `master` branch and Hailo-8 on a `v2.x` branch with an older compiler; a HEF built for one device family is not a HEF for another. Stage II starts by pinning host OS/architecture, driver, HailoRT, Dataflow Compiler, Model Zoo tag and model, then running a known-compatible reference model before any custom training. Access terms for the compiler have not been checked.

### 4.5 Why the zero-shot model is not the edge model
It was not selected for direct edge deployment; compatibility with the Hailo toolchain has **not** been tested. It is also slow (about 1 s per frame on a desktop GPU in our run). It is used where it is convenient — labelling and exploration — and a smaller detector on a verified toolchain path is planned for the edge.

---

## 5. Demonstrated foundation

### 5.1 Geospatial pipeline **[Built]**
- **Direct georeferencing (preliminary)**: EXIF GPS and DJI-style XMP attitude; footprint centre shifted by `agl · tan(tilt)`; a frame is then rotated and scaled onto the grid with `GSD = agl × gsd_per_agl`. There is **no per-pixel ray–ground intersection, no perspective correction, no lens distortion model and no terrain model**. Suitable for a fast preliminary map; not an orthorectified product. Embankment crests, channel bed relief, bridges and buildings are exactly where this approximation is weakest.
- **Own TM2 ↔ lon/lat** (Krüger series; < 1 mm against pyproj in tests).
- **Official NLSC EPSG:3826 grid**: origin (50000, 3000000), 14 levels, 256 px; pinned by a unit test and checked against the official GetCapabilities.
- **Mosaic and pyramid**: edge feathering, resample into the target grid, four coarser levels rebuilt, atomic temp-file + rename writes.

**Resolution is not accuracy.** These quantities are different and must not be conflated:

| Quantity | Value in this project | What may be claimed |
|---|---|---|
| Source imagery GSD | 3.64 cm/px (simulation ground truth) | Source detail only |
| Synthetic frame size | 1536×1024 px | Not the real full-resolution camera workload |
| EPSG:3857 z21 output | ≈ 6.8 cm/px at this latitude (varies with latitude and zoom) | Current demo layer; not a 5 cm output |
| EPSG:3826 level 13 (official NLSC grid) | ≈ 26.5 cm/px | Official-grid compatible; not a 5 cm output; not used by the river demo |
| Real positioning accuracy | not measured | No survey-grade or enforcement-grade claim |

If "5 cm" is meant as an *output* tile resolution in EPSG:3826, a separately named high-resolution TWD97 tile matrix set would be needed; the official NLSC grid must not be changed in place.

### 5.2 Concurrency **[Built]**, fleet measurements **[Simulated]**
Per-tile mutex registry keyed by tile file path (pruned only of entries nobody holds); a thread holds at most one tile lock; pyramid parents read their children inside the lock; bounded Tokio semaphore plus a queue limit (503 + `Retry-After`); identical frames in flight → 409.

**Consistency model.** A single tile is never half-written (temp file + rename). A frame and its pyramid are *not* published atomically: a reader may briefly see a mix of old and new levels; after writes stop the pyramid converges. Locks are process-local, so one server process per tile store is assumed. There is no crash-recovery pass for a pyramid interrupted mid-update, and no immutable mission snapshot.

| 22 simulated drones, 1,056 frames | Global lock | Tile locks, concurrency 4 |
|---|---|---|
| Throughput | 2.65 frames/s | 4.56 frames/s (≈ arrival rate) |
| Capture → visible p50 / p95 / max | 79 / 163 / 177 s | 4.4 / 7.5 / 10.2 s |
| Backlog drain | 180 s | 12 s |

Concurrency scan under 3× overload peaks at 8 workers (5.49 frames/s). The consistency test mosaics six overlapping frames concurrently and compares every pyramid level pixel-for-pixel with a recomputation; removing the parent lock makes it fail.
*Caveats:* single machine, no network, one run per configuration, client model assumed, 0.265 m GSD, 5 cm frames not measured.

### 5.3 Governance **[Built]**
- **Roles** `client` (default) < `uploader` < `admin`; API key `role=` or JWT `role` claim (missing → `client`); denials → 403 and audited. A `client` can also upload tracks (`/api/v1/tracks`), so "client" is read-mostly, not strictly read-only.
- **Audit log**: SHA-256 hash chain, throttled per (IP[+actor], reason), secrets never logged; `verify` with an external anchor catches a recomputed chain; JSONL export is itself audited.
- **Audit scope**: denied requests (401/403/429/503), ingest outcomes, exports and retention pruning. **Successful tile reads, candidate queries and human review actions are not logged today**, so the log cannot answer "who viewed what". Review events, status changes and sessions are Stage II work.
- **Availability**: per-minute samples, minute is up iff there is a record, all probes pass and 5xx ≤ 5 %; unauthenticated `/readyz` for an external prober.
*Limits:* the chain head must be anchored externally by an operator (and the anchor store must sit in a different permission domain from the database, otherwise it adds nothing); the log is tamper-**evident**, not tamper-proof. Availability is self-measured until an external prober runs long-term; it is a monitoring report, not a contractual SLA (reporting period, scheduled service time, maintenance exclusions and probe path would have to be defined, and the probe should fetch an authenticated tile, not only `/readyz`).

---

## 6. Evidence from the river demo **[Simulated]**

**Data.** "2020-09-13 新竹市頭前溪金雅－溪埔段 (Jinya – Sipu, Touchian River, Hsinchu City)", Water Resources Agency MOEA / Hsinchu City EPB / Taiwan Ecological Society / FeiMao, DJI Phantom 4 RTK, **3.64 cm/px**, EPSG:3826, CC BY 4.0, read as a Cloud-Optimized GeoTIFF by HTTP range requests.
**Frames.** Synthetic: a lawn-mower mission over the real orthophoto, 1536×1024 px, 80 % / 70 % overlap, EXIF + XMP pose with simulated attitude and RTK noise.

| Segment | Frames | Ingest (4 uploaders) | Server per frame p50 / p95 | Tiles written | Raw detections → candidates |
|---|---|---|---|---|---|
| Bridge and parking lot (vehicle-rich) | 243 | 16.8 s (14.5 frames/s) | 117 / 509 ms | 6,089 | 357 → 34 |
| River channel and north bank | 270 | 38.6 s (7.0 frames/s) | 395 / 748 ms | 6,954 | 28 (after score and size filters) → 6 |

The two ingest runs differ more than the frame counts explain; other load on the machine was not controlled and the difference is not attributed. They are different segments and must not be read as a controlled comparison. A controlled report would record CPU/GPU/RAM/disk, OS, build profile, commit, frame size, source GSD, tile zoom, worker count, cache state, and repeat each run several times.

**Screenshots** (`demo/output/screens/`, with attribution): live mosaic with candidate boxes, close-ups at z20 and z21, and a version with detections off.

**Reproduce.**
```
python demo/make_frames.py --out <dir>/sim --center-e 248302 --center-n 2746446 --size 220x140
python demo/detect_frames.py --sim <dir>/sim --out <dir>/det --thr 0.2 --annot 0
python demo/merge_detections.py --raw <dir>/det/raw_detections.json --out demo/output/detections_river.geojson --min-frames 1
python demo/run_demo.py --sim <dir>/sim --shots <dir>/shots --det output/detections_river.geojson --prefix river
python demo/run_demo.py --sim <dir>/sim --watch --pace 4 --det output/detections_river.geojson   # recording mode
```
The server must be built with `--features ingest`; the demo uses `config/layers.demo.toml` (one live layer, one demo key from an environment variable).

**What this shows.** Synthetic-frame ingestion, preliminary direct georeferencing, incremental WMTS publishing at about 6.8 cm/px, and visualisation of *precomputed* AI candidates, using real 3.6 cm source imagery. Detection and ingestion are separate steps in this demo; the replay pace in recording mode is a presentation setting, not an inference-latency result.
**What it does not show.** Live per-frame inference, sidecar ingestion or server-managed candidates; detection precision/recall; real-camera pose accuracy; edge-host or UGen300 latency; 5 cm output tiles.

**Label every demo asset:** *Synthetic flight replay · Precomputed detections · Desktop GPU prototype · No UGen300 measurements yet.*

---

## 7. Use case and business case

*All items in this section are hypotheses to be validated with a pilot customer; no market sizing is claimed.*

### 7.1 Minimum viable customer
One county water bureau (or a river-management office) piloting patrols for a single river reach: flag temporary structures, equipment and piles in the channel and on embankments, and keep a traceable record of candidates and, once the review workflow exists, of who reviewed what.

### 7.2 Why edge, why now
- Candidates are known during the flight, so the operator can re-fly or look closer.
- The AI result is compact metadata alongside the image stream that mapping already requires; it does not need the full image to be sent a second time.
- The accelerator itself is in the few-hundred-dollar range (vendor listing around USD 300). The per-site cost also includes the edge host, communications, deployment, labelling, maintenance and security; the accelerator price is not the system price.

### 7.3 Buyers and revenue (hypothesised)
| Buyer | Pays for |
|---|---|
| Public agencies (water resources, county bureaus) | Project licence and annual maintenance; compliance features (roles, audit, availability) |
| Drone survey contractors | Per-project or subscription access to the live-delivery stack |
| Integrators | API access and edge-box bundles |

### 7.4 Differentiation (to be substantiated with a competitor table in Stage II)
Live mosaic + object candidates + governed WMTS delivery in one loop, on standard OGC interfaces that existing GIS clients can already consume.

**Pilot questions and value metrics (to ask, not yet asked):** how long image delivery and review take today; which object types must never be missed; how many false alarms per kilometre are tolerable; whether frames can be obtained in flight; whether WMTS in two CRS is truly required; who owns the budget. Value is measured as time to first visible candidate, review minutes per kilometre, valid candidates, share of missions needing a re-fly, and report turnaround — not as model FPS.

### 7.5 Historical context layer **[Built]**
The 1938 1:2,500 Touchian River topographic map (Academia Sinica Hsinchu centennial maps service) is available as a viewer layer, which allows comparing today's channel and embankment with the 1938 channel. It is context, not evidence for the AI claims, and its service terms restrict commercial use: keep it behind a feature flag, off by default in any commercial deployment, and confirm permission before using it in public submission material.

---

## 8. Stage II plan **[Planned]**

The acceptance values below are **design targets proposed for discussion**, not results; they are to be adjusted with users' tolerance for misses and review cost.

| Window | Work | Draft acceptance (design targets) |
|---|---|---|
| 10/26 – 11/6 | Receive UGen300; pin the toolchain matrix; run a known-compatible reference HEF on the actual host | Reference model runs on device; per-stage latency logged with versions and device logs |
| 11/6 – 11/20 | Build a labelled river-corridor set from the CC BY imagery; train the detector; independent evaluation set (split by reach/date) | Per-class precision/recall, missed-object rate and false alarms per reach reported on the held-out set; class list reduced to what can be labelled reliably |
| 11/20 – 12/4 | Quantise/compile, integrate sidecar upload, candidates API and review status, long-run test | Single stream at one frame per 4 s: sustained ≥ 0.25 frame/s end to end with p95 ≤ 4 s (host + UGen300 + upload + publish); 24 h run with a defined input rate, memory ceiling, retry limit and restart policy |
| 12/4 – 12/10 | Finals submission: repository, README, architecture, performance data, on-device video | Everything above reproducible from the repository |

**Measurement breakdown (always reported separately):** crops per frame, decode, preprocess, inference, NMS/georeference, upload, server publish, viewer refresh; request latency, throughput and capture-to-visible are different metrics. Start with one drone, one river reach, a fixed input rate, two or three classes and a bounded queue. Server-side fleet simulations (22 drones) say nothing about how many streams one UGen300 can serve.

Tests are staged: (A) synthetic offline, (B) real-camera replay with network, (C) field flight; each stage widens what may be claimed, and A's numbers never stand in for C.

---

## 9. Limitations and risks

| # | Limitation / risk | Effect |
|---|---|---|
| 1 | No detector accuracy measurement; no labelled ground truth | Candidates only; no performance claim |
| 2 | Zero-shot class names unreliable (tarpaulins labelled "boat", buses labelled "truck") | Treat as "object of interest"; classes to be reduced in Stage II |
| 3 | No UGen300 measurement | All edge numbers are plans |
| 4 | Frames are synthetic, flat-ground, pinhole camera, simulated pose noise | Georeferencing accuracy of real cameras unknown |
| 5 | Demo frames are 1536×1024 (1.6 MP); real P4RTK frames are 5472×3648 (≈ 20 MP), about 13× the pixels | Per-frame processing time on real frames is unmeasured |
| 6 | Single machine, no network, one run per configuration | Throughput numbers are relative evidence only |
| 7 | Availability is self-measured | 99.5 % is design support, not a demonstrated SLA |
| 8 | Audit chain head needs external anchoring | Database-write attacker can rewrite a consistent chain without an anchor |
| 9 | Business case unvalidated | No customer interviews yet |
| 10 | Model conversion/quantisation loss, label scarcity, USB bandwidth for large frames | Stage II schedule risk |
| 11 | `scripts/sim/` was developed in a larger workspace; only `demo/` is the supported entry point in the export | Some simulation scripts may need adjustment |
| 12 | Geometry is a footprint shift plus rotation/scale; no per-pixel ray–ground intersection or terrain | Preliminary map only; weakest at embankment crests, bridges, buildings |
| 13 | Audit log covers denials, ingest and export, not successful reads or reviews | Cannot answer "who viewed what" |
| 14 | Candidate merge uses ground proximity within a category; no event identity, time window or review status | Moving objects can split, nearby objects can merge |
| 15 | Edge host, in-flight frame access and USB data path are undecided | In-flight candidates are not guaranteed; end-to-end latency unknown |
| 16 | Hailo toolchain compatibility, compiler access and model licences unconfirmed | Stage II schedule risk |
| 17 | Process-local locks; no crash recovery of an interrupted pyramid update | One server process per tile store; manual repair after a crash |

---

## 10. Data provenance and licensing

| Item | Licence | Handling |
|---|---|---|
| Orthophoto, Touchian River, Hsinchu City, 2020-09-13 (Water Resources Agency MOEA, Hsinchu City EPB, Taiwan Ecological Society, FeiMao; via OpenAerialMap id 5f642ef0363d930006324989) | CC BY 4.0 | Used as simulation ground truth; attribution in the viewer and screenshots |
| `google/owlv2-base-patch16-ensemble` | Apache-2.0 | Prototype detector / teacher |
| OpenStreetMap | ODbL | Viewer base map, attribution shown |
| Academia Sinica Hsinchu centennial maps | Service terms: Academia Sinica copyright, not for commercial use without permission | Viewer comparison layer only; confirm terms before any commercial use |
| National mapping agency imagery used in earlier simulations | Redistribution terms unconfirmed | Kept local, not in the repository, not shown in submission materials |
| Procurement specification | — | Requirements summarised only |
| Secrets | — | Environment variables / platform secrets; none in the repository |

---

## Appendix A — Public interfaces

| Method / path | Role | Purpose |
|---|---|---|
| `GET /wmts`, `/wmts/1.0.0/{layer}/…` | client | WMTS KVP / RESTful tiles, capabilities |
| `POST /api/v1/ingest/frame` | uploader | Frame ingest (feature `ingest`) |
| `GET /api/v1/ingest/stats` | uploader | Ingest statistics |
| `GET /api/v1/audit`, `/verify`, `/export` | admin | Audit read / verify / JSONL export |
| `GET /api/v1/availability` | admin | Availability report |
| `GET /readyz`, `/healthz` | none | Readiness / liveness |
| `GET/POST /api/v1/tracks…` | client | Track storage |

## Appendix B — Test status

| Suite | Result |
|---|---|
| Default build unit tests | 25 passed |
| `--features ingest` unit tests | 35 passed, 1 ignored (replay helper) |
| Role matrix, audit tamper, dedupe, health/audit end-to-end scripts | passed (local, `scripts/sim/`) |
| River demo (this repository) | 270/270 and 243/243 frames ingested, viewer with no script errors |
| Clean-export repository | rebuilt and re-tested after de-identification: 25 passed (default), 35 passed + 1 ignored (`--features ingest`) |

There is no hardware-in-the-loop test and no detection accuracy test.

## Appendix C — Questions for the reviewer

1. **Concurrency:** is a per-tile mutex registry with "read children inside the parent lock" a sound way to keep a read-modify-write pyramid consistent? Which interleavings would you test that a mutation test would not catch?
2. **Audit chain:** with an external anchor, is the threat model (database-write attacker) correctly bounded? How would you automate anchoring without creating a new trust problem?
3. **Availability:** is "missing minute = down, from first sample" defensible against a 99.5 % requirement given the self-measurement blind spot?
4. **Edge placement:** for a Hailo-10H class device, is it better to detect before upload on the edge box (bandwidth, latency) or beside the server (simpler), given ~20 MP frames over USB?
5. **Teacher-assisted annotation:** is using an open-vocabulary model to pre-label a river-corridor set, correcting by hand, then training a small INT8 detector a sound plan given label scarcity? What validation would convince you the detector is not just copying the pre-labeller's mistakes (e.g. "tarpaulin → boat") or its selection bias (objects it never boxed)?
6. **Claims discipline:** are there statements here that read as stronger than the evidence, especially in sections 1 and 6?
