# EdgeOrtho — Demo Video: Structure and Narration Script

**Contest:** 2026 ASUS UGen AI League Innovation Hackathon, Stage I · **Length:** 2:52 (limit 3:00) · **Language:** English narration and subtitles · **Upload:** YouTube, "Unlisted" (anyone with the link can view)
**Version:** matches ARCHITECTURE v2.2 — the video must not claim more than that document does.

---

## 1. Always-on label (bottom-left, whole video)

> **Synthetic flight replay · Precomputed detections · Desktop GPU prototype · No UGen300 measurements yet**

Candidate boxes additionally carry **"Candidates — not verified"**.

## 2. Structure

| # | Time | Scene | What is on screen | Source of footage |
|---|---|---|---|---|
| 1 | 0:00–0:12 | The problem | Simple line animation of a drone flying along a river on an OSM base map; title card **EdgeOrtho** | Made for the video (no third-party imagery) |
| 2 | 0:12–0:30 | What agencies need | Three words appear in turn: *Immediate · Governed · Standard (WMTS)* | Text animation |
| 3 | 0:30–0:50 | The idea | Architecture diagram lights up left to right: Drone → Edge host + UGen300 *(Stage II)* → Server → Viewer | Diagram from ARCHITECTURE §3 |
| 4 | 0:50–1:30 | Live mosaic | Screen recording: the river mosaic grows tile by tile in the viewer, at 3.6 cm/px source detail | `run_demo.py --watch --pace 4` (CC BY 4.0 orthophoto) |
| 5 | 1:30–1:55 | Candidates | Zoom to the north bank; candidate boxes on tarpaulin plots and debris; toggle detections off and on | `river_closeup_z20.png` / `z21.png` or live viewer |
| 6 | 1:55–2:15 | Governance | Terminal: audit chain verifies, one row altered, verification fails; role list | `test_audit_chain.py`, `test_roles.py` (secrets masked) |
| 7 | 2:15–2:38 | Scale (simulation + estimate) | Two bars: 79 s → 4.4 s median capture-to-visible ("22 simulated drones · one machine"); then one line card: **5 cm imagery ≈ one frame every 3.7 s per drone — estimate** | Drawn from numbers, no imagery |
| 8 | 2:38–2:52 | Stage II | Timeline: reference model on device → labelled set → measured latency/throughput; closing title and attribution | Text animation |

## 3. Narration (read straight through)

> **[1]** River patrols are flown by drone, but the results usually arrive after the flight, as files, apart from the map.
>
> **[2]** Public agencies also need standard WMTS delivery, role-based access, an audit trail, and availability reporting. EdgeOrtho brings these together.
>
> **[3]** Each frame is georeferenced and merged into a tile pyramid within seconds. In stage two, a detector running on an edge host with an ASUS UGen300 will flag objects of interest at capture time.
>
> **[4]** This is a replay of a simulated flight over the Touchian River in Hsinchu, built from real orthophotography at three point six centimetres per pixel. As frames arrive, the map grows tile by tile. The detections you will see next were computed ahead of time on a desktop GPU.
>
> **[5]** Objects around the channel and embankment appear as candidates: tarpaulin-covered plots, piles, vehicles. They are for human review. They are not verified findings, and the labels are still noisy.
>
> **[6]** Access is role-based. Denied requests, uploads and exports go into a hash-chained log. Change one row, and verification fails.
>
> **[7]** In a simulation with twenty-two drones uploading at once, tile-level locking cut the median capture-to-visible delay from seventy-nine seconds to four point four. For five-centimetre imagery, our capacity model estimates one frame every three to four seconds per drone. That is an estimate, not a measurement.
>
> **[8]** Stage two measures what is still missing: detector quality on a labelled set, and latency and throughput on real hardware. EdgeOrtho.

*≈ 240 words ≈ 95–100 s of speech at 150 words per minute; the remainder is picture and pauses. Do not exceed 3:00 in total.*

## 4. On-screen captions (English, per scene)

| # | Caption |
|---|---|
| 1 | EdgeOrtho · River patrol, mapped as it is flown |
| 2 | Immediate · Governed · Standard WMTS |
| 3 | Edge host + UGen300 — Stage II |
| 4 | Simulated flight on a real CC BY 4.0 orthophoto |
| 5 | Candidates — not verified |
| 6 | Hash-chained audit log · Least privilege |
| 7 | Simulation · 22 drones · single machine · 0.265 m GSD  /  5 cm capacity: estimate, not measured |
| 8 | Stage II: labelled set · on-device latency · throughput |
| End | Imagery: Water Resources Agency MOEA, Hsinchu City EPB, Taiwan Ecological Society, FeiMao (CC BY 4.0, via OpenAerialMap) · Map: © OpenStreetMap contributors |

## 5. 中文說明稿（給團隊對照，不放進影片）

| # | 旁白意思 |
|---|---|
| 1 | 河川巡查用無人機飛，但成果通常飛完才以檔案形式交付，和地圖是分開的。 |
| 2 | 公部門還需要標準 WMTS、角色權限、稽核軌跡與可用率報告。EdgeOrtho 把這些整合起來。 |
| 3 | 每一張影像在數秒內完成地理定位並併入圖磚金字塔。第二階段，邊緣主機搭配華碩 UGen300 會在拍攝當下標出值得注意的物件。 |
| 4 | 這是新竹頭前溪模擬飛行的重播，使用解析度 3.6 公分的真實正射影像；影像送達時，地圖一塊塊長出來。接下來看到的偵測結果是事先用桌機 GPU 算好的。 |
| 5 | 河道與堤防周邊的物件以「候選」呈現（防水布覆蓋地、堆置物、車輛），供人工覆核；它們不是已驗證的發現，標籤仍有雜訊。 |
| 6 | 存取依角色控管；被拒絕的請求、上傳與匯出會寫入雜湊鏈日誌，改動任何一筆，驗證就會失敗。 |
| 7 | 模擬 22 架無人機同時上傳時，圖磚級鎖把「拍攝到可見」的中位數延遲從 79 秒降到 4.4 秒。5 公分解析度的容量模型估計每架約每 3～4 秒一張；這是估算，不是量測。 |
| 8 | 第二階段要量測目前缺少的部分：標註資料集上的偵測品質，以及真實硬體上的延遲與吞吐量。 |

## 6. Claims allowed and not allowed

| Allowed | Not allowed |
|---|---|
| Synthetic frames from a real CC BY orthophoto are ingested and published as tiles within seconds | "Real-time AI detection", "detected live", "on the UGen300" (present tense) |
| Candidates for human review, shown from precomputed output | Any precision, recall, accuracy or "violations found" statement |
| Tile-level locking: 79 s → 4.4 s median in a 22-drone simulation on one machine | That one UGen300 can serve many drones; any latency/FPS/power number for the device |
| Hash-chained log detects edits; roles enforce least privilege | "Tamper-proof", "complete audit of every access" (reads are not logged) |
| Planned: edge host + UGen300 in Stage II | 5 cm output tiles, survey-grade accuracy, orthorectified product |

## 7. Recording and production notes

| Item | How |
|---|---|
| Scene 4 | `python 2026ASUS_AI_UGen/demo/run_demo.py --sim <river_sim> --watch --pace 4 --det output/detections_river.geojson`; start the screen recorder when the console prints "Uploading starts in 8 s"; window 1600×900 |
| Scene 5 | Add `&lat=<lat>&lng=<lon>&z=20` to the viewer URL, or use the stills in `demo/output/screens/` with a slow push-in |
| Scenes 4–5, alternative source | A pre-built static version of the demo may be screen-recorded instead (precomputed tiles and candidates; no server needed). Unlock it before recording and hide the browser's address bar and tabs (app/kiosk window or full screen, or crop in the edit). **The address of that page must not appear anywhere in the video or the deck.** |
| Scene 6 | Run the two scripts on a fresh server and clean database, one run each; crop the terminal so only PASS/FAIL lines show; **mask any key or IP** |
| Scene 7 | Draw two bars from the numbers (PowerPoint or an image library) — do **not** reuse the earlier before/after GIF (its base map is third-party imagery with unconfirmed terms) |
| Audio | Original or clearly licensed royalty-free music, below the narration level |
| Subtitles | English, burned in or uploaded as a caption track |

## 8. Pre-upload checklist

- [ ] Total length ≤ 3:00; narration in English
- [ ] Always-on label visible in every scene; "Candidates — not verified" on every candidate shot
- [ ] No national-mapping-agency imagery, no keys, no real IPs, no personal or contest-provided data
- [ ] End card has CC BY 4.0 attribution and OSM credit
- [ ] No statement that anything ran on the UGen300
- [ ] No web address of any demo page visible in any frame (address bar, hover tooltip, tab strip, end card)
- [ ] 1938 historical map not shown (service terms restrict commercial use) unless permission is confirmed
- [ ] YouTube visibility "Unlisted"; link opens in a private window
- [ ] The deck's last slide lists the video link and the repository link
