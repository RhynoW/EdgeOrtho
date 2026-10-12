# EdgeOrtho — System Architecture

**Working title:** EdgeOrtho — on-device AI inspection for drone orthophotos, published through an access-controlled, auditable WMTS
**Event:** 2026 ASUS UGen AI League Innovation Hackathon (Stage I proposal) · Theme: Workplace AI · Battlefield: Lightning (ASUS UGen300 / Hailo-10H)
**Document status:** draft for external technical review · based on repository state at commit `ee2d80d7` plus later UI-only changes
**Language:** English (the contest requires an English submission)

---

## 0. How to read this document (for reviewers)

This document separates three kinds of statements, because the project is partly built and partly planned:

| Tag | Meaning |
|---|---|
| **[Built]** | Implemented in the repository and exercised by tests or measurements described here |
| **[Simulated]** | Works end-to-end, but only against synthetic or replayed inputs, not real drones/cameras |
| **[Planned]** | Design intent for Stage II; **nothing in this category has been implemented or measured** |

Anything marked **[Planned]** must not be read as a result. In particular, **no measurement on UGen300/Hailo hardware exists yet**, and there is **no trained detection model** in the repository at this point.

Section 12 lists the questions I would most like reviewed.

---

## 1. Problem and scope

Drone mapping workflows for public-sector and enterprise customers are typically *fly → batch process → deliver*. Three gaps follow:

1. **Latency** — orthophoto tiles are not available until processing finishes.
2. **Governance** — delivery rarely has role-based access, rate limiting, and a tamper-evident audit trail, which procurement specifications increasingly require.
3. **Separation of inspection from imagery** — detections live in a different system from the map.

The requirements used as a design target come from a public-sector drone procurement specification that I was given (summary only; I have not republished the document):

| Requirement (paraphrased) | Where addressed |
|---|---|
| ~2,000 raw images per month | §6 (capacity far above this) |
| 5 cm ground sampling distance | **Not met in simulation** — see §10 |
| OGC WMTS delivery, EPSG:3857 and EPSG:3826 | §5.3, §5.4 |
| API key / token authentication, HTTPS | §7 (HTTPS is expected at a reverse proxy) |
| 99.5 % monthly availability | §8 |
| Track synchronisation | existing tracks API (not detailed here) |

**Non-goals:** photogrammetric bundle adjustment (SfM/MVS); orthorectification against a DEM; replacing a GIS. The mosaic assumes flat ground and a pinhole camera (§10).

---

## 2. System overview

```
                    FIELD                                   SERVER (Rust, Axum)                         CLIENTS
┌──────────┐   ┌─────────────────────────┐        ┌─────────────────────────────────────────┐   ┌──────────────────┐
│  Drone   │──▶│ Edge box                │ HTTPS  │ require_auth ─▶ role check ─▶ handler    │   │ Leaflet viewer   │
│ (JPEG +  │   │  • frame ingest          │ POST   │                                         │   │  - WMTS tiles    │
│ EXIF/XMP)│   │  • [Planned] HailoRT     │───────▶│ POST /api/v1/ingest/frame               │   │  - detection     │
└──────────┘   │    detector on UGen300   │ frame  │   decode ─▶ pose ─▶ georeference        │   │    layer         │
               │  • [Planned] detections  │ + det. │   ─▶ mosaic (3857 z19, 3826 L13)         │   │  - near-real-    │
               │    sidecar (GeoJSON)     │        │   ─▶ pyramid (4 levels down)             │   │    time refresh  │
               └─────────────────────────┘        │   ─▶ tile store (atomic writes)          │   └────────▲─────────┘
                                                  │                                         │            │ GetTile / KVP / REST
                                                  │ GET /wmts … ───────────────────────────────────────────┘
                                                  │ GET /api/v1/audit{,/verify,/export}  (admin)           │
                                                  │ GET /api/v1/availability, /readyz                      │
                                                  │                                                        │
                                                  │ SQLite: audit_log (hash chain), ingest_frames,         │
                                                  │         uptime_samples, tracks                         │
                                                  └─────────────────────────────────────────┘
```

**Languages / stack:** Rust (Axum, Tokio, SQLite via rusqlite, `image`, `kamadak-exif`, `sha2`), vanilla JS + Leaflet front end. Default build has no ingest code (`--features ingest` enables it).

---

## 3. Component inventory

| Component | Path | Status | Notes |
|---|---|---|---|
| WMTS server (KVP + RESTful, capabilities XML/JSON) | `src/wmts/` | **[Built]** | Layer/TileMatrixSet config in TOML |
| Tile matrix sets (3857 and TWD97/TM2) | `src/tilematrix.rs` | **[Built]** | Official NLSC 3826 grid, see §5.4 |
| Authentication (API key + HS256 JWT) | `src/auth/` | **[Built]** | Roles, §7 |
| Ingest (decode, pose, georeference, mosaic, pyramid) | `src/ingest/` | **[Built]**/**[Simulated]** | Feature-gated |
| Audit log with hash chain | `src/audit.rs` | **[Built]** | §7.3 |
| Health, availability report, readiness | `src/health.rs` | **[Built]** | §8 |
| External availability prober | `scripts/probe_readyz.py` | **[Built]** | Needs a separate host to be meaningful |
| Synthetic aerial generator / replay / fleet simulator | `scripts/sim/` | **[Built]** | Local-only; uses non-redistributable ground truth |
| Edge detector on UGen300 (HailoRT, HEF) | — | **[Planned]** | Not started |
| Detection layer publishing (GeoJSON + WMTS overlay) | — | **[Planned]** | Not started |

---

## 4. Data flow (ingest path)

1. **Upload.** Client sends `POST /api/v1/ingest/frame?session_id=<id>` with the raw JPEG as body. Auth is checked first, then the `uploader` role (§7).
2. **Admission control.** A Tokio semaphore bounds concurrent processing (default `max(2, cores/4)`). If more than `max_queue` (default 64) requests are already waiting, respond **503 + `Retry-After: 2`** and audit `busy:queue_full` (throttled).
3. **Deduplicate.** SHA-256 of the body. Same hash already processed → `200 duplicate`. Same hash currently in flight → **409**.
4. **Pose.** Read EXIF GPS (lon/lat/altitude) and DJI-style XMP (`GimbalYaw/Pitch/RollDegree`, `RelativeAltitude`). Missing GPS/XMP → 400.
5. **Georeference.** Direct georeferencing (no tie points): footprint centre shifted by `agl · tan(tilt)`; ground sampling distance `GSD = agl × gsd_per_agl`. Own implementation of TM2 ↔ lon/lat (Krüger series, < 1 mm vs pyproj in tests) and Web Mercator.
6. **Mosaic.** Resample into each target grid (EPSG:3857 z19 and EPSG:3826 level 13), with edge feathering (`feather_px`, default 64). Each affected tile is read, blended, and written back **under a per-tile mutex**.
7. **Pyramid.** For each target, rebuild 4 coarser levels; a parent tile reads its children *inside* the parent's lock (no cross-parent cache), so the last writer always sees all children.
8. **Write.** Temp file + rename (readers never see a half-written tile). Fast PNG (`CompressionType::Fast`, `FilterType::Sub`).
9. **Record.** Row in `ingest_frames`; audit entry; response JSON with processing time and tile counts.

`orthophoto_live` sets `Cache-Control: max-age=2`; other layers keep the long default. The viewer's "live" mode preloads tiles off-screen and swaps them in.

---

## 5. Geospatial design

### 5.1 Direct georeferencing
Single-frame, flat-ground, pinhole camera. Position and attitude come from EXIF/XMP. No DEM, no distortion model, no bundle adjustment.

### 5.2 Why the error numbers in this document are optimistic
The synthetic generator produces frames *from the same ground truth used to verify them*, with a known pose and added noise (σ ≈ 1° attitude, σ ≈ 2 cm RTK position). The measured 0.24 m mean footprint error therefore measures the **geometry code**, not the accuracy a real camera and real terrain would give.

### 5.3 EPSG:3857
Standard `GoogleMapsCompatible`. Mosaic at z19; pyramid down to z15.

### 5.4 EPSG:3826 (TWD97 / TM2 zone 121)
Uses the **official NLSC WMTS grid** (`https://wmts.nlsc.gov.tw/97/wmts`, `default028mm`): origin (50000, 3000000), 14 levels (0–13), 256 px tiles, resolution = scale denominator × 0.00028 (level 13 ≈ 0.2646 m/px). A unit test pins the grid to these values; `scripts/verify_tms_nlsc.py` compares against the official GetCapabilities. Taipei City's separate TWD97 grid uses a different origin and is deliberately not mixed in.

---

## 6. Concurrency and capacity

### 6.1 Design
| Mechanism | Purpose |
|---|---|
| Per-tile `std::sync::Mutex` registry (pruned above 50k entries) | Two frames touching the same tile never interleave read-modify-write |
| Each thread holds at most one tile lock at a time | No lock-order deadlocks |
| Pyramid parent reads children inside the lock | Last writer sees consistent children |
| Tokio `Semaphore` + `WAITING`/`BUSY_TOTAL` atomics | Bounded concurrency, observable queue |
| `IN_FLIGHT` set | Identical frames in flight → 409 |
| `max_queue` | Back-pressure instead of unbounded latency |

A unit test (`concurrent_frames_keep_pyramid_consistent`) mosaics six overlapping frames concurrently and checks every pyramid level pixel-for-pixel against a recomputation from the final children. **Mutation check:** removing the parent-tile lock makes it fail. This catches *missing locks*; it does not prove absence of every ordering bug.

### 6.2 Measured fleet behaviour (simulation)
Setup: 22 simulated drones, 1,056 frames total, one upload in flight per drone, ~4 s capture cadence, store-and-forward with retry on 503/409. Server and client on the same 32-core development machine; no real network latency or bandwidth limit.

| Metric | Single global lock (before) | Tile locks + concurrency 4 (after) |
|---|---|---|
| Server throughput | 2.65 frames/s | 4.56 frames/s (≈ arrival rate 4.70) |
| Capture → tile visible p50 / p95 / max | 79 / 163 / 177 s | 4.4 / 7.5 / 10.2 s |
| Backlog drain after mission end | 180 s | 12 s |
| Per-drone fairness (mean latency) | 76–84 s | 3.5–4.9 s |

Concurrency scan under 3× overload: 2 → 3.57, 4 → 4.66, **8 → 5.49**, 16 → 5.20 frames/s (saturates at 8; CPU and disk bound). Estimated ceiling ≈ 22–25 continuously uploading drones **at 0.265 m GSD**.

Admission control (concurrency 4, queue 8): 276 × 503, no frames lost, p95 12.7 s.

**Caveats:** the client model is my assumption; the “after” run is a single run per configuration (no confidence intervals); 5 cm GSD frames are several times more work per frame and are **not measured**; the capacity estimate is an extrapolation.

---

## 7. Security and governance

### 7.1 Authentication
- **API key** in header, compared against configured keys (values from environment variables; never in the repository).
- **JWT (HS256)** with a `role` claim. Missing or unknown role → `client`.
- X-Forwarded-For is trusted only when `behind_reverse_proxy` is set.

### 7.2 Roles (least privilege by default)
| Role | Capabilities |
|---|---|
| `client` (default) | Read tiles, search (feature `overpass`), tracks list/read/upload (`/api/v1/tracks…`) |
| `uploader` | `client` + `POST /api/v1/ingest/frame`, `GET /api/v1/ingest/stats` |
| `admin` | `uploader` + audit read/verify/export, availability report |

Denial → **403** and audit `forbidden:insufficient_role`. An end-to-end test covers 9 endpoints × 4 identities plus JWT edge cases.

### 7.3 Audit log
- SQLite `audit_log`, **SHA-256 hash chain** with length-prefixed fields; `audit_anchor` supports pruning.
- Logged: 401 / 403 / 429 / 503 / ingest outcomes. **Secrets are never logged.**
- Throttle per (IP[+actor], `outcome:reason`) within 10 s, recorded as `suppressed=N`.
- `GET /api/v1/audit/verify` recomputes the chain; an **external anchor** (`expected_id` / `expected_hash`) detects a wholesale recomputed chain. Without an externally stored anchor, an attacker with database write access can rewrite the entire chain consistently — this is a known limit, tested and documented.
- `GET /api/v1/audit/export` → JSONL with a manifest line; the export is itself audited.
- Optional retention (`retention_days`, default 0 = keep).

### 7.4 Not done
Chain-head auto-export and signed exports; key rotation procedure; TLS termination (expected at a proxy); request-signing of uploads; per-tenant isolation.

---

## 8. Availability measurement

- A sampler runs every 10 s: internal probe + request/5xx counters, accumulated into per-minute rows (`uptime_samples`).
- A minute is **up** iff it has a record, all probes succeeded, and 5xx ≤ 5 %. Missing minutes count as **down**. Measured from the first sample.
- `GET /api/v1/availability` (admin) reports month-to-date and custom windows (`strict` mode).
- `/readyz` is unauthenticated for external probes; `scripts/probe_readyz.py` compares an external prober's view against the internal report.
- **Known blind spot:** the minute in which the process dies can count as up internally. Hence the external prober, which has **not** been run long-term from another host. The 99.5 % figure is therefore *self-measured design support*, not a demonstrated SLA.

---

## 9. Edge AI design [Planned]

> Everything in this section is a plan. No model has been trained, compiled, or run on UGen300.

### 9.1 Role of the edge box
Run a detector on every incoming frame at capture time, so that (a) only frames with candidates need high-priority handling, and (b) detections are georeferenced with the same pose used for the mosaic.

### 9.2 Hardware assumptions (from public vendor material)
UGen300 = Hailo-10H, up to 40 TOPS INT4 / 20 TOPS INT8, 8 GB LPDDR4, ~2.5 W typical, USB or M.2; HailoRT SDK with PyTorch/ONNX/TensorFlow conversion. These are vendor figures; I have no measurement.

### 9.3 Proposed pipeline
1. Frame (e.g. 16 MP) → tiling (overlapping crops) sized to the model input.
2. INT8-quantised detector (starting point: a YOLO-family model from the Hailo Model Zoo, fine-tuned on aerial data) compiled to HEF with the Hailo Dataflow Compiler (x86 Linux).
3. Post-processing: NMS, tile merge, pixel → ground via the same georeferencing as the mosaic.
4. Output: GeoJSON sidecar posted with the frame; server stores it and serves a detections layer.

### 9.4 Stage I vs Stage II
- **Stage I (now):** CPU/ONNX prototype on synthetic frames to exercise the interfaces. No accuracy claim.
- **Stage II (11/6–12/4):** compile and run on UGen300; report per-frame latency, throughput, power, and detection quality on a labelled set.

### 9.5 Acceptance criteria to be fixed before Stage II
Per-frame end-to-end latency on UGen300; sustained frames/s; detection precision/recall on a held-out labelled set; 24 h stability. **No numeric targets are asserted here** — they should be set with reviewer input.

### 9.6 Risks
Model conversion/quantisation loss; lack of labelled aerial data for the chosen class; USB bandwidth for 16 MP frames; whether detection should run before upload or on the server side of the same box.

---

## 10. Known limitations

1. **GSD is 0.265 m/px in all experiments, not 5 cm.** Ground truth resolution limits the simulation.
2. Flat ground, pinhole camera, no distortion, no DEM: real-camera accuracy is **unknown**.
3. Attitude angle conventions are a **simulation convention** (DJI-like field names, synthetic values). Real DJI raw images have not been validated.
4. Fleet results come from one development machine, one run per configuration.
5. Availability is self-measured; external probing not yet run long-term.
6. Audit chain head must be anchored externally by an operator (manual today).
7. Frames whose response is lost after processing are re-processed on resend (a "processing" record would fix this; not done).
8. Pyramid levels are rebuilt per frame (no coalescing); shared coarse tiles are the main lock-contention point.
9. Detection: no model, no data, no results.

---

## 11. Data provenance and licensing

| Item | Status |
|---|---|
| Ground-truth imagery for simulation | Derived from a national mapping agency's public WMTS; **licence terms for redistribution unconfirmed** → kept local, excluded from the repository and from any public deployment, and **not shown in submission materials** |
| Procurement specification | Used only to derive requirements; not republished |
| Third-party basemap in the viewer | OpenStreetMap (ODbL, attribution shown) |
| Code | Own work; dependencies are standard crates |
| Secrets | API keys and JWT secrets come from environment variables / platform secrets; none are in the repository |

---

## 12. Questions for the reviewer

1. **Concurrency:** is a per-tile mutex registry plus "read children inside the parent lock" a sound way to keep a read-modify-write pyramid consistent? Which interleavings would you test that the mutation test would not catch?
2. **Audit chain:** with an external anchor, is the threat model (database-write attacker) correctly bounded? What would you change to make anchoring automatic without introducing a new trust problem?
3. **Availability:** is the "missing minute = down, from first sample" rule defensible against a 99.5 % requirement, given the self-measurement blind spot?
4. **Edge placement:** for a Hailo-10H class device, is it better to run detection *before upload* on the edge box (bandwidth/latency) or *beside the server* (simpler), given 16 MP frames over USB?
5. **Claims discipline:** are there statements in this document that read as stronger than the evidence (especially §6.2 and §5.2)?
6. **Scope:** for a Stage I proposal judged on innovation (30 %), commercial feasibility (35 %), practical value (25 %) and narrative (10 %), which part of this architecture is weakest and should be cut or strengthened first?

---

## Appendix A — Public interface summary

| Method / path | Role | Purpose |
|---|---|---|
| `GET /wmts`, `/wmts/1.0.0/{layer}/…` | client | WMTS KVP and RESTful tiles, capabilities |
| `GET /api/v1/search` | client | Name search (separate feature) |
| `GET/POST /api/v1/tracks…` | client | Track storage |
| `POST /api/v1/ingest/frame` | uploader | Frame ingest (feature `ingest`) |
| `GET /api/v1/ingest/stats` | uploader | Ingest statistics |
| `GET /api/v1/audit`, `/verify`, `/export` | admin | Audit read / verify / JSONL export |
| `GET /api/v1/availability` | admin | Availability report |
| `GET /readyz`, `/healthz` | none | Readiness / liveness |

## Appendix B — Test status at the referenced commit

| Suite | Result |
|---|---|
| Default build, unit tests | 25 passed |
| `--features ingest`, unit tests | 35 passed, 1 ignored (replay helper) |
| Role matrix e2e, audit-chain tamper e2e, dedupe e2e, health/audit e2e | passed (local scripts in `scripts/sim/`) |
| HTTP replay regression (accuracy unchanged) | mean 0.27 m, grey diff 4.94 |

Tests exercise the simulation and the server logic. There is **no** hardware-in-the-loop test.
