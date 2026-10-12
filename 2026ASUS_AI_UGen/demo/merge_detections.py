"""EdgeOrtho Stage I demo, step 3: merge per-frame detections (overlapping frames see the same object several times)
into ground-referenced features and write GeoJSON (WGS84).

Greedy clustering on ground position (default 3 m, class-agnostic because truck/bus labels from the zero-shot prototype
are unreliable); each feature keeps the number of frames that saw it, the best score and the class votes.
Usage: python merge_detections.py --raw raw_detections.json --out detections.geojson [--radius 3.0] [--min-frames 2]
"""
import argparse, collections, json, math


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--raw", required=True); ap.add_argument("--out", required=True)
    ap.add_argument("--radius", type=float, default=3.0); ap.add_argument("--min-frames", type=int, default=2)
    ap.add_argument("--min-score", type=float, default=0.30, help="minimum score for vehicles/machinery/persons/boats")
    ap.add_argument("--min-score-generic", type=float, default=0.25, help="minimum score for structures and debris (generic phrases score lower)")
    a = ap.parse_args()
    raw = json.load(open(a.raw, encoding="utf8"))
    generic = {"structure", "debris"}
    dets = [d for d in raw["detections"] if d["score"] >= (a.min_score_generic if d.get("category") in generic else a.min_score)]
    dets.sort(key=lambda d: -d["score"])
    clusters = []   # each: dict(e, n, members)
    for d in dets:
        for c in clusters:
            if math.hypot(c["e"] - d["e"], c["n"] - d["n"]) <= max(a.radius, 0.4 * max(d["size_m"]), 0.4 * c["size"]) and c["cat"] == d.get("category", "vehicle"):
                c["members"].append(d); break
        else:
            clusters.append(dict(e=d["e"], n=d["n"], members=[d], size=max(d["size_m"]), cat=d.get("category", "vehicle")))
    feats, dropped = [], 0
    for c in clusters:
        frames = {m["frame"] for m in c["members"]}
        if len(frames) < a.min_frames: dropped += 1; continue
        w = sum(m["score"] for m in c["members"])
        lon = sum(m["lon"] * m["score"] for m in c["members"]) / w; lat = sum(m["lat"] * m["score"] for m in c["members"]) / w
        votes = collections.Counter(m["cls"] for m in c["members"])
        feats.append(dict(type="Feature", geometry=dict(type="Point", coordinates=[round(lon, 7), round(lat, 7)]),
                          properties=dict(label=f"{c['cat']} candidate", category=c["cat"], class_votes=dict(votes), frames=len(frames),
                                          best_score=max(m["score"] for m in c["members"]),
                                          size_m=[round(sum(m["size_m"][0] for m in c["members"]) / len(c["members"]), 1),
                                                  round(sum(m["size_m"][1] for m in c["members"]) / len(c["members"]), 1)])))
    gj = dict(type="FeatureCollection", features=feats,
              properties=dict(source="zero-shot prototype (OWLv2, Apache-2.0) on synthetic frames from a CC BY 4.0 orthophoto (OpenAerialMap 5f642ef0363d930006324989)",
                              note="candidates, not verified detections; no ground-truth labels exist", merged_radius_m=a.radius, min_frames=a.min_frames))
    json.dump(gj, open(a.out, "w", encoding="utf8"), ensure_ascii=False)
    print(f"raw {len(dets)} -> clusters {len(clusters)} -> kept {len(feats)} (seen in >= {a.min_frames} frames), dropped {dropped}")


if __name__ == "__main__":
    main()
