# EdgeOrtho — Stage I Proposal Deck (full English text)

**Contest:** 2026 ASUS UGen AI League Innovation Hackathon · Stage I · **Body:** 17 slides (limit 20) + appendix
**Consistent with:** ARCHITECTURE v2.2 and VIDEO_SCRIPT. Every number below appears in those documents.
**Still to fill:** demo video link and contact (slide 17). Team name is Sat_Dashboard; members are intentionally not listed.

Design guidance (not slide text): one message per slide; always show the status tag *Built / Simulated / Planned* where a claim appears; use the screenshots in `demo/output/screens/` with attribution; no national-mapping-agency imagery, no keys, no real IPs.

---

## Slide 1 — Title

**EdgeOrtho**
*On-Device AI Turns Drone Imagery into Immediate, Auditable River-Patrol Map Intelligence*

2026 ASUS UGen AI League Innovation Hackathon · Theme: Workplace AI · Battlefield: Lightning (ASUS UGen300 / Hailo-10H) · Finals track: Onsite
Team: Sat_Dashboard · Repository: github.com/RhynoW/EdgeOrtho

*Visual:* river-mosaic screenshot (`river_overview.png`) with attribution line.

---

## Slide 2 — Summary

**What.** River-management teams patrol channels and embankments by drone, but results arrive after the flight, as files, apart from the map. EdgeOrtho publishes each uploaded frame as WMTS tiles within seconds and shows objects of interest as *candidates for human review* on the same map, behind role-based access and a tamper-evident log.

**AI.** A detector flags man-made objects around the channel and embankment. Today: a zero-shot open-vocabulary model on a desktop GPU over synthetic flights (precomputed overlay). Stage II: a small detector on an edge host with the ASUS UGen300.

**Proven so far (simulation, real 3.6 cm orthophoto):**
- 270 frames ingested into a z21 tile pyramid in 38.6 s
- 22 simulated drones: median capture-to-visible delay 79 s → 4.4 s
- 6 merged candidates on a river segment — *unverified*

**Not yet proven:** detector accuracy, UGen300 latency/throughput, real-camera georeferencing accuracy.

---

## Slide 3 — The problem

**Today's river patrol workflow**
1. Fly the reach.
2. Download images; batch photogrammetry takes hours.
3. Inspect results in a tool separate from the delivered map.
4. Deliver files; access and audit are whatever the file share provides.

**Three gaps**
| Gap | Consequence |
|---|---|
| Results arrive after the mission | Findings cannot guide the same flight (re-fly, closer look) |
| Findings live apart from the map | No single place to see what was flagged, where, when and by what |
| Delivery lacks governance | Public tenders ask for role-based access, audit trail, availability reporting |

---

## Slide 4 — Who it is for and the use case

**Use case:** drone patrol along a river channel and embankment: spot temporary structures, equipment, tarpaulin-covered plots, debris piles, vehicles, people and boats that need a second look.

| User | Needs |
|---|---|
| Patrol officer | Latest tiles and flagged objects while the drone is still flying |
| Case reviewer | Candidates with position, time, score and source frames |
| Data owner / auditor | Record of denied access, uploads and exports; proof the log was not altered |
| Contractor | Push frames with an upload-only role |

**Minimum viable customer:** a river-management office, one river reach. **Proposed pilot candidate:** Second River Management Branch, Water Resources Agency, Ministry of Economic Affairs (no agreement yet; cooperation intent and trial procedure to be discussed).

---

## Slide 5 — Solution overview

*Diagram:* Drone → Edge host + UGen300 (USB) → Server (auth · role · ingest · mosaic · pyramid · WMTS) → Viewer (live mosaic · candidates).

**Three paths, kept distinct**
| Path | What runs | Status |
|---|---|---|
| A. Offline prototype | Synthetic frames → desktop-GPU detector → merged GeoJSON → viewer | **Simulated** (the demo) |
| B. Live ingest | Frame upload → georeference → mosaic pyramid → WMTS tiles in seconds | **Built** |
| C. Live AI | Edge host + UGen300 detect each frame; server stores and serves candidates; reviewer workflow | **Planned** (Stage II) |

---

## Slide 6 — AI architecture

**Stage I prototype (built, offline)**
- Detector: `google/owlv2-base-patch16-ensemble` (open-vocabulary, Apache-2.0), ten text-defined classes
- Per-class size sanity ranges; per-class score thresholds; duplicates merged across overlapping frames
- Output: GeoJSON candidates with frames seen, best score, class votes

**Observed (honest):** the strongest "boat" candidates were tarpaulin-covered plots; buses were often labelled "truck". Class names from a zero-shot model are not reliable; the useful signal is "a man-made object is here".

**Stage II plan (not built)**
1. Teacher-assisted annotation: pre-label, hand-correct *including missed objects*, train a small detector
2. Quantise to INT8, compile for Hailo-10H with the Hailo toolchain
3. Tile full-resolution frames, merge, georeference with the mosaic pose
4. Post a compact detection record (event id, frame, session, time, geometry, model version, score, class, review status)

---

## Slide 7 — Edge hardware and who does what

The **UGen300 is a USB accelerator** (Hailo-10H, vendor figures: up to 40 TOPS INT4 / 20 TOPS INT8, 8 GB, ~2.5 W for the device alone). It does inference only.

| Unit | Responsibility |
|---|---|
| Camera / flight link | Deliver frames (in flight or after landing — *open decision*) |
| Edge host | Decode, tile, resize, NMS, georeference, upload |
| UGen300 | Inference |
| Server | Mosaic, pyramid, WMTS, access control, audit |

**Toolchain is a gate:** pin host OS, driver, HailoRT, compiler and model-zoo versions, and run a known-compatible reference model before any custom training.

---

## Slide 8 — Geospatial pipeline and what it is

- **Direct georeferencing (preliminary):** EXIF GPS + gimbal attitude; footprint shifted by `agl·tan(tilt)`; rotate and scale onto the grid
- **Not** orthorectified: no per-pixel ray–ground intersection, no terrain, no lens model
- **Official NLSC EPSG:3826 grid** pinned by a unit test; EPSG:3857 pyramid

**Resolution is not accuracy**
| Quantity | Value | May claim |
|---|---|---|
| Source imagery | 3.64 cm/px | Source detail |
| EPSG:3857 z21 output | ≈ 6.8 cm/px here | Current demo layer |
| EPSG:3826 level 13 | ≈ 26.5 cm/px | Official-grid compatible |
| Real positioning accuracy | not measured | No survey-grade claim |

---

## Slide 9 — Live ingest and concurrency (built)

Per-tile locks, bounded queue with back-pressure, in-flight de-duplication, atomic tile writes.

| 22 simulated drones, 1,056 frames | Global lock | Tile locks |
|---|---|---|
| Throughput | 2.65 frames/s | 4.56 frames/s |
| Capture → visible, median / p95 | 79 / 163 s | 4.4 / 7.5 s |
| Backlog drain | 180 s | 12 s |

*Caveats on slide:* single machine, no network, one run per configuration, 0.265 m GSD, small frames.
Consistency: a tile is never half-written; the pyramid converges after writes stop; a concurrency test fails if the parent lock is removed.

---

## Slide 10 — Governance (built, with its limits)

- **Roles:** client (default) < uploader < admin; denial → 403 and logged
- **Hash-chained audit log:** denied requests, uploads, exports; edits are detected; external anchor catches a recomputed chain
- **Availability report** and `/readyz` for an external prober
- **Limits stated:** successful reads and review actions are not logged yet; tamper-*evident*, not tamper-proof; availability is self-measured, not an SLA

---

## Slide 11 — Demo evidence: Touchian River, Hsinchu

*Visuals:* `river_overview.png`, `river_closeup_z21.png`.

**Data:** real CC BY 4.0 orthophoto, DJI Phantom 4 RTK, 3.64 cm/px; synthetic flight with simulated pose noise.

| Segment | Frames | Ingest | Server per frame p50 / p95 | Raw → candidates |
|---|---|---|---|---|
| Bridge and parking lot | 243 | 16.8 s | 117 / 509 ms | 357 → 34 |
| River channel and bank | 270 | 38.6 s | 395 / 748 ms | 28 → 6 |

**Label on slide:** Synthetic flight replay · Precomputed detections · Desktop GPU prototype · No UGen300 measurements yet.

---

## Slide 12 — 5 cm-class capacity model (estimate)

| | 3.64 cm (source) | 5 cm (requirement) |
|---|---|---|
| Flight height (Phantom 4 RTK class) | ≈ 133 m | ≈ 182 m |
| Footprint | 199 × 133 m | 274 × 182 m |
| Exposure interval at 10 m/s | ≈ 2.7 s | ≈ 3.7 s (0.27 frame/s) |
| New ground per frame | ≈ 0.53 ha | ≈ 1.0 ha |
| 2,000 frames per month | ≈ 1,060 ha | ≈ 2,000 ha |

**Budgets the system must meet (not results):** ≈ 96 crops per full frame → ≈ 38 ms per crop at one frame per 3.7 s; server ingest of full-size frames estimated at 0.5–1 frame/s per machine *if* cost scales with pixels (to be measured).
**Claim:** "a capacity model is established; Stage II measures it" — not "5 cm is supported".

---

## Slide 13 — What is new

| Existing approach | Limitation | EdgeOrtho |
|---|---|---|
| Batch photogrammetry after landing | Hours of delay; results as files | Tiles within seconds of upload |
| General GIS / map server | No detection, no review loop | Candidates are map-native, with review status (planned) |
| Detection-only tools | Boxes without a governed map | Georeferenced candidates on standard OGC WMTS |

**The combination:** live mosaic + object candidates + governed delivery in one loop, on interfaces existing GIS clients already read.

---

## Slide 14 — Business case (hypotheses to validate)

| Buyer | Pays for |
|---|---|
| Public agencies (water resources, county bureaus) | Project licence and annual maintenance; compliance features |
| Drone survey contractors | Per-project or subscription access |
| Integrators | API access, edge-box bundles |

**Cost to a customer** includes edge host, accelerator (vendor listing ≈ USD 300), communications, deployment, labelling, maintenance and security. Pricing assumptions will be evaluated after ASUS hardware is obtained.
**Value measured as:** time to first visible candidate, review minutes per kilometre, valid candidates, share of missions needing a re-fly, report turnaround.
**Next step:** discuss cooperation intent and trial procedures with the Second River Management Branch, Water Resources Agency, Ministry of Economic Affairs.

---

## Slide 15 — Feasibility and risks

| Risk | Mitigation |
|---|---|
| No detector accuracy yet | Independent, hand-annotated evaluation set split by reach/date |
| Zero-shot labels noisy | Reduce to 2–3 reliably labelled classes; human review always |
| Hailo toolchain/model licence unconfirmed | Reference-model gate in the first week; licence check per model |
| Frames may not reach the host in flight | Declare post-flight mode if needed |
| Geometry is preliminary | Position as fast preliminary map; offline photogrammetry remains the precise product |
| Business case unvalidated | Pilot with one reach and one customer |

---

## Slide 16 — Stage II plan (design targets)

| Window | Work | Draft acceptance (targets for discussion) |
|---|---|---|
| 10/26 – 11/6 | Receive UGen300; pin toolchain; reference HEF on the real host | Runs on device; per-stage latency logged |
| 11/6 – 11/20 | Labelled river set; train detector; held-out evaluation | Per-class precision/recall, missed-object rate, false alarms per reach |
| 11/20 – 12/4 | Compile, sidecar upload, candidates API with review status, long run | Single stream at one frame per 4 s: ≥ 0.25 frame/s end to end, p95 ≤ 4 s; 24 h run |
| 12/4 – 12/10 | Finals package | Reproducible from the repository |

---

## Slide 17 — Team, repository and references

**Team:** Sat_Dashboard (members are not listed in the deck)
**Repository:** github.com/RhynoW/EdgeOrtho (architecture, demo, tests; private until submission)
**Demo video:** `[TO FILL: YouTube unlisted link]`
**Contact:** `[TO FILL]`

**References**
1. OGC WMTS 1.0.0 specification
2. NLSC TWD97 TM2 WMTS service (`wmts.nlsc.gov.tw/97/wmts`) — official grid
3. Orthophoto "2020-09-13 Touchian River, Hsinchu City", Water Resources Agency MOEA, Hsinchu City EPB, Taiwan Ecological Society, FeiMao; CC BY 4.0 via OpenAerialMap
4. `google/owlv2-base-patch16-ensemble` (Apache-2.0)
5. ASUS UGen300 and Hailo-10H vendor documentation
6. Hailo Model Zoo and Dataflow Compiler documentation
7. OpenStreetMap contributors (ODbL)

---

# Appendix

## A1 — Interfaces and roles
| Method / path | Role |
|---|---|
| `GET /wmts…` | client |
| `POST /api/v1/ingest/frame`, `GET /api/v1/ingest/stats` | uploader |
| `GET /api/v1/audit`, `/verify`, `/export`; `GET /api/v1/availability` | admin |
| `GET /readyz`, `/healthz` | none |
| `GET/POST /api/v1/tracks…` | client |

## A2 — Test status
Default build 25 unit tests passed; `--features ingest` 35 passed (1 helper ignored); role matrix, audit tamper, de-duplication and health scripts passed locally; river demo ingested 270/270 and 243/243 frames. No hardware-in-the-loop test; no detection accuracy test.

## A3 — Licensing and attribution
| Item | Licence | Handling |
|---|---|---|
| Touchian River orthophoto (OpenAerialMap id 5f642ef0363d930006324989) | CC BY 4.0 | Attribution shown; modified into synthetic frames |
| OWLv2 detector | Apache-2.0 | Prototype only |
| OpenStreetMap | ODbL | Attribution shown |
| Other imagery sources | terms unconfirmed | Not used in submission materials |

## A4 — Questions we would like reviewers to challenge
1. Per-tile locking with "read children inside the parent lock" — which interleavings would you test?
2. External anchoring of the audit chain without a new trust problem?
3. Detect before upload on the edge host, or beside the server, for ~20 MP frames over USB?
4. What validation shows a detector trained on pre-labels is not copying the pre-labeller's mistakes?
