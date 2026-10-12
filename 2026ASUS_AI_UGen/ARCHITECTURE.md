# EdgeOrtho — System Architecture (v2)

**EdgeOrtho: On-Device AI Turns Drone Imagery into Immediate, Auditable River-Patrol Map Intelligence**

2026 ASUS UGen AI League Innovation Hackathon · Stage I · Theme: Workplace AI · Battlefield: Lightning (ASUS UGen300 / Hailo-10H) · Finals track: Onsite
Document status: v2 for external technical review. v1 (architecture-first) is kept as `ARCHITECTURE.v1-review.md`.

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

**What.** River-management agencies patrol channels and embankments with drones, but the imagery is processed in batches after the flight, and what is found is kept apart from the map. EdgeOrtho closes that loop: a drone flies along the river; each frame is georeferenced and merged into a WMTS tile pyramid within seconds; objects around the channel and embankment (vehicles, temporary structures, tarpaulin-covered plots, debris piles, people, boats) are flagged as map-native candidates; all of it is served behind role-based access with a tamper-evident audit trail.

**For whom.** Water-resources and river-patrol units, county-level water bureaus, engineering consultants, drone survey contractors. Public-sector delivery requirements — standard OGC WMTS, API-key/token access, HTTPS, audit, availability reporting — are first-class, not an afterthought.

**Where is the AI.** A detector runs on every frame at capture time. In Stage I a zero-shot open-vocabulary model (Apache-2.0) is used as a **prototype and as the planned teacher**; in Stage II a small INT8 detector distilled from it is compiled for the Hailo-10H on the UGen300 and run at the edge **[Planned]**.

**What is demonstrated today** (all **[Simulated]**, real orthophoto of the Touchian River, Hsinchu City, 3.64 cm/px, CC BY 4.0):

| Result | Value |
|---|---|
| Frames processed into a z21 (≈ 6.8 cm/px) tile pyramid | 270 frames in 38.6 s (7.0 frames/s, 4 uploaders, same machine) |
| Server processing time per frame | p50 395 ms, p95 748 ms |
| 22-drone fleet, capture → tile visible | p50 79 s → 4.4 s after tile-level locking (single machine, 0.265 m GSD) |
| Detection candidates on the river segment | 6 merged candidates from 28 raw detections, **unverified** |
| Detector latency (development GPU, not UGen300) | p50 1.0 s, p95 1.7 s per 1536×1024 frame |

**What is not demonstrated.** Detection accuracy (no ground truth), UGen300 latency/throughput/power, real-camera georeferencing accuracy, long-term availability. Section 9 lists these.

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
| Data owner / auditor | Who accessed what; proof the log was not altered |
| Contractor / uploader | Push frames; no access to admin functions |

### 2.4 Requirement source
A public-sector drone procurement specification (summary only; not republished) asked for: ~2,000 raw images per month, 5 cm GSD, WMTS in EPSG:3857 and EPSG:3826, API key/token with HTTPS, 99.5 % monthly availability, and track synchronisation.

---

## 3. Solution overview

```
 FIELD                               SERVER (Rust / Axum)                                   CLIENTS
┌────────┐   ┌───────────────────┐   ┌─────────────────────────────────────────────┐   ┌───────────────────┐
│ Drone  │──▶│ Edge box          │──▶│ auth ▶ role ▶ admission control             │   │ Leaflet viewer    │
│ JPEG + │   │ [Planned]         │   │ POST /api/v1/ingest/frame                   │   │  • live mosaic    │
│EXIF/XMP│   │ UGen300 detector  │   │  decode ▶ pose ▶ georeference               │──▶│  • candidates     │
└────────┘   │ → GeoJSON sidecar │   │  ▶ mosaic (3857 z21 / 3826 L13) ▶ pyramid   │   │  • historical     │
             └───────────────────┘   │  ▶ atomic tile writes                       │   │    river maps     │
                                     │ GET /wmts … tiles                           │   └───────────────────┘
                                     │ audit chain · availability · /readyz        │
                                     └─────────────────────────────────────────────┘
```

| Component | Status |
|---|---|
| WMTS server (KVP + RESTful, capabilities), tile matrix sets 3857 and official NLSC 3826 | **[Built]** |
| Ingest: decode, pose, direct georeferencing, incremental mosaic, pyramid, atomic writes | **[Built]** / **[Simulated]** inputs |
| Concurrency: per-tile locks, bounded queue, in-flight de-duplication | **[Built]** |
| Roles, hash-chained audit log, availability report, `/readyz` | **[Built]** |
| Frame generator from a real orthophoto, prototype detector, merge to GeoJSON, demo viewer | **[Built]** (this repository, `demo/`) |
| Detector on UGen300 (HailoRT / HEF) and the detections layer served by the server | **[Planned]** |
| Historical 1938 Touchian River map as a comparison layer | **[Built]** (viewer layer list) |

---

## 4. AI architecture

### 4.1 Role of the edge device
Run detection at capture time so that (a) candidates are known before the drone leaves the segment, (b) only small GeoJSON sidecars, not extra image traffic, are added, and (c) the same pose used for the mosaic georeferences the detections.

### 4.2 Stage I prototype **[Built]** / **[Simulated]**
| Item | Value |
|---|---|
| Detector | `google/owlv2-base-patch16-ensemble` (open-vocabulary, Apache-2.0), text queries for ten classes |
| Classes | car, truck, bus, motorcycle, excavator, person, boat, temporary shed with a metal roof, building, pile of debris/waste |
| Filters | Per-class plausible size range in ground metres; per-class minimum score; duplicates merged across overlapping frames by ground proximity |
| Output | GeoJSON points/boxes with `frames` seen, `best_score`, `class_votes`; marked as candidates |

**Observed behaviour (honest).** On the river segment the highest-scoring "boat" candidates were blue and green tarpaulin-covered plots on the riverbank, not boats; a "debris" candidate was a bare patch with scattered material; one candidate sits in the channel and has not been checked. On the parking-lot/bridge segment the vehicle localisation was good, but buses were frequently labelled "truck". **The class names from a zero-shot model are therefore not reliable; the useful signal is "a man-made object of interest is here".** The prototype is not presented as a detector with a known precision.

### 4.3 Planned edge model **[Planned]**
1. **Teacher–student.** Use the open-vocabulary model to pre-label river-corridor tiles; correct a subset by hand; train a small YOLO-family student (permissive licence) on the corrected labels.
2. **Compile.** Quantise to INT8 and compile with the Hailo Dataflow Compiler (x86 Linux) to a HEF; run with HailoRT on the UGen300.
3. **Tiling.** 16 MP frames are cut into overlapping crops sized to the model input; boxes are merged, then georeferenced.
4. **Output.** GeoJSON sidecar posted with the frame; the server stores it and serves a detections layer next to the mosaic.

### 4.4 Hardware (vendor figures, not measured here)
UGen300 = Hailo-10H, up to 40 TOPS INT4 / 20 TOPS INT8, 8 GB LPDDR4, ~2.5 W typical, USB or M.2; HailoRT with PyTorch/ONNX/TensorFlow conversion.

### 4.5 Why the zero-shot model is not the edge model
It does not map onto Hailo's supported operator set, and it is slow (about 1 s per frame on a desktop GPU in our run). It is used where it is strong — labelling and exploration — and replaced by a compiled student for the edge.

---

## 5. Demonstrated foundation

### 5.1 Geospatial pipeline **[Built]**
- **Direct georeferencing**: EXIF GPS and DJI-style XMP attitude; footprint centre shifted by `agl · tan(tilt)`; `GSD = agl × gsd_per_agl`. Flat ground, pinhole camera.
- **Own TM2 ↔ lon/lat** (Krüger series; < 1 mm against pyproj in tests).
- **Official NLSC EPSG:3826 grid**: origin (50000, 3000000), 14 levels, 256 px; pinned by a unit test and checked against the official GetCapabilities.
- **Mosaic and pyramid**: edge feathering, resample into the target grid, four coarser levels rebuilt, atomic temp-file + rename writes.

### 5.2 Concurrency **[Built]**, fleet measurements **[Simulated]**
Per-tile mutex registry; a thread holds at most one tile lock; pyramid parents read their children inside the lock; bounded Tokio semaphore plus a queue limit (503 + `Retry-After`); identical frames in flight → 409.

| 22 simulated drones, 1,056 frames | Global lock | Tile locks, concurrency 4 |
|---|---|---|
| Throughput | 2.65 frames/s | 4.56 frames/s (≈ arrival rate) |
| Capture → visible p50 / p95 / max | 79 / 163 / 177 s | 4.4 / 7.5 / 10.2 s |
| Backlog drain | 180 s | 12 s |

Concurrency scan under 3× overload peaks at 8 workers (5.49 frames/s). The consistency test mosaics six overlapping frames concurrently and compares every pyramid level pixel-for-pixel with a recomputation; removing the parent lock makes it fail.
*Caveats:* single machine, no network, one run per configuration, client model assumed, 0.265 m GSD, 5 cm frames not measured.

### 5.3 Governance **[Built]**
- **Roles** `client` (default) < `uploader` < `admin`; API key `role=` or JWT `role` claim (missing → `client`); denials → 403 and audited.
- **Audit log**: SHA-256 hash chain, throttled per (IP[+actor], reason), secrets never logged; `verify` with an external anchor catches a recomputed chain; JSONL export is itself audited.
- **Availability**: per-minute samples, minute is up iff there is a record, all probes pass and 5xx ≤ 5 %; unauthenticated `/readyz` for an external prober.
*Limits:* the chain head must be anchored externally by an operator; availability is self-measured until an external prober runs long-term.

---

## 6. Evidence from the river demo **[Simulated]**

**Data.** "2020-09-13 新竹市頭前溪金雅－溪埔段 (Jinya – Sipu, Touchian River, Hsinchu City)", Water Resources Agency MOEA / Hsinchu City EPB / Taiwan Ecological Society / FeiMao, DJI Phantom 4 RTK, **3.64 cm/px**, EPSG:3826, CC BY 4.0, read as a Cloud-Optimized GeoTIFF by HTTP range requests.
**Frames.** Synthetic: a lawn-mower mission over the real orthophoto, 1536×1024 px, 80 % / 70 % overlap, EXIF + XMP pose with simulated attitude and RTK noise.

| Segment | Frames | Ingest (4 uploaders) | Server per frame p50 / p95 | Tiles written | Raw detections → candidates |
|---|---|---|---|---|---|
| Bridge and parking lot (vehicle-rich) | 243 | 16.8 s (14.5 frames/s) | 117 / 509 ms | 6,089 | 357 → 34 |
| River channel and north bank | 270 | 38.6 s (7.0 frames/s) | 395 / 748 ms | 6,954 | 28 (after score and size filters) → 6 |

The two ingest runs differ more than the frame counts explain; I did not control for other load on the machine and do not attribute the difference.

**Screenshots** (`demo/output/screens/`, with attribution): live mosaic with candidate boxes, close-ups at z20 and z21, and a version with detections off.

**What this shows.** The path *frame → pose → mosaic pyramid → WMTS tile → map overlay with candidates* runs end to end at 3.6 cm input quality, which is close to the 5 cm requirement and removes the earlier "0.265 m only" limitation for the imagery side.
**What it does not show.** Detection precision/recall, real-camera pose accuracy, edge latency.

---

## 7. Use case and business case

*All items in this section are hypotheses to be validated with a pilot customer; no market sizing is claimed.*

### 7.1 Minimum viable customer
One county water bureau (or a river-management office) piloting patrols for a single river reach: flag temporary structures, equipment and piles in the channel and on embankments, and keep an audit trail of who reviewed what.

### 7.2 Why edge, why now
- Candidates are known during the flight, so the operator can re-fly or look closer.
- Only small sidecars, not raw frames or extra image traffic, are added to the uplink.
- Accelerators of the UGen300 class are in the few-hundred-dollar range (vendor listing around USD 300), so the per-site hardware cost is small relative to the drone itself.

### 7.3 Buyers and revenue (hypothesised)
| Buyer | Pays for |
|---|---|
| Public agencies (water resources, county bureaus) | Project licence and annual maintenance; compliance features (roles, audit, availability) |
| Drone survey contractors | Per-project or subscription access to the live-delivery stack |
| Integrators | API access and edge-box bundles |

### 7.4 Differentiation (to be substantiated with a competitor table in Stage II)
Live mosaic + object candidates + governed WMTS delivery in one loop, on standard OGC interfaces that existing GIS clients can already consume.

### 7.5 Historical context layer **[Built]**
The 1938 1:2,500 Touchian River topographic map (Academia Sinica Hsinchu centennial maps service) is available as a viewer layer, which allows comparing today's channel and embankment with the 1938 channel.

---

## 8. Stage II plan **[Planned]**

| Window | Work | Acceptance (to be fixed with reviewers; values intentionally not asserted here) |
|---|---|---|
| 10/26 – 11/6 | Receive UGen300; HailoRT bring-up; run a reference detector from the Model Zoo to validate the toolchain | Reference model runs on device; per-frame latency measured |
| 11/6 – 11/20 | Build a labelled river-corridor set from the CC BY imagery (open-vocabulary pre-labels + manual correction); train the student | Precision/recall on a held-out labelled split; class list reduced to what can be labelled reliably |
| 11/20 – 12/4 | Quantise/compile, integrate sidecar upload and the detections layer, long-run test | Sustained frames/s on device; 24 h stability; power draw; end-to-end latency per frame |
| 12/4 – 12/10 | Finals submission: repository, README, architecture, performance data, on-device video | Everything above reproducible from the repository |

---

## 9. Limitations and risks

| # | Limitation / risk | Effect |
|---|---|---|
| 1 | No detector accuracy measurement; no labelled ground truth | Candidates only; no performance claim |
| 2 | Zero-shot class names unreliable (tarpaulins labelled "boat", buses labelled "truck") | Treat as "object of interest"; classes to be reduced in Stage II |
| 3 | No UGen300 measurement | All edge numbers are plans |
| 4 | Frames are synthetic, flat-ground, pinhole camera, simulated pose noise | Georeferencing accuracy of real cameras unknown |
| 5 | 1536×1024 frames; real P4RTK frames are ~9× larger | Per-frame processing time on real frames is unmeasured |
| 6 | Single machine, no network, one run per configuration | Throughput numbers are relative evidence only |
| 7 | Availability is self-measured | 99.5 % is design support, not a demonstrated SLA |
| 8 | Audit chain head needs external anchoring | Database-write attacker can rewrite a consistent chain without an anchor |
| 9 | Business case unvalidated | No customer interviews yet |
| 10 | Model conversion/quantisation loss, label scarcity, USB bandwidth for large frames | Stage II schedule risk |

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

There is no hardware-in-the-loop test and no detection accuracy test.

## Appendix C — Questions for the reviewer

1. **Concurrency:** is a per-tile mutex registry with "read children inside the parent lock" a sound way to keep a read-modify-write pyramid consistent? Which interleavings would you test that a mutation test would not catch?
2. **Audit chain:** with an external anchor, is the threat model (database-write attacker) correctly bounded? How would you automate anchoring without creating a new trust problem?
3. **Availability:** is "missing minute = down, from first sample" defensible against a 99.5 % requirement given the self-measurement blind spot?
4. **Edge placement:** for a Hailo-10H class device, is it better to detect before upload on the edge box (bandwidth, latency) or beside the server (simpler), given 16 MP frames over USB?
5. **Teacher–student:** is using an open-vocabulary teacher to pre-label a river-corridor set, then distilling to a small INT8 student, a sound plan given label scarcity? What validation would convince you the student is not just copying the teacher's mistakes (e.g. "tarpaulin → boat")?
6. **Claims discipline:** are there statements here that read as stronger than the evidence, especially in sections 1 and 6?
