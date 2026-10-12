use super::model::TrackPointRow;
use crate::xmlutil::escape;

fn lon_lat_alt(geometry: &serde_json::Value) -> Option<(f64, f64, f64)> {
    let coords = geometry.get("coordinates")?.as_array()?;
    let lon = coords.first()?.as_f64()?;
    let lat = coords.get(1)?.as_f64()?;
    let alt = coords.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0);
    Some((lon, lat, alt))
}

/// 匯出為 GPX 1.1（`<trk>` + 逐點 `<trkpt>`），供不支援 GeoJSON 的下游系統匯入。
pub fn to_gpx(session_id: &str, points: &[TrackPointRow]) -> String {
    let mut trkpts = String::new();
    for p in points {
        let Some((lon, lat, alt)) = lon_lat_alt(&p.geometry) else { continue };
        trkpts.push_str(&format!(
            "      <trkpt lat=\"{lat}\" lon=\"{lon}\">\n        <ele>{alt}</ele>\n        <time>{time}</time>\n      </trkpt>\n",
            lat = lat,
            lon = lon,
            alt = alt,
            time = escape(&p.properties.timestamp),
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="wmts_server" xmlns="http://www.topografix.com/GPX/1/1">
  <trk>
    <name>{session_id}</name>
    <trkseg>
{trkpts}    </trkseg>
  </trk>
</gpx>
"#,
        session_id = escape(session_id),
        trkpts = trkpts,
    )
}

/// 匯出為 KML，使用 Google gx 擴充的 `gx:Track` 以保留逐點時間戳（適合飛行軌跡回放）。
pub fn to_kml(session_id: &str, points: &[TrackPointRow]) -> String {
    let mut whens = String::new();
    let mut coords = String::new();
    for p in points {
        let Some((lon, lat, alt)) = lon_lat_alt(&p.geometry) else { continue };
        whens.push_str(&format!("      <when>{}</when>\n", escape(&p.properties.timestamp)));
        coords.push_str(&format!("      <gx:coord>{lon} {lat} {alt}</gx:coord>\n"));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://www.opengis.net/kml/2.2" xmlns:gx="http://www.google.com/kml/ext/2.2">
  <Document>
    <Placemark>
      <name>{session_id}</name>
      <gx:Track>
{whens}{coords}      </gx:Track>
    </Placemark>
  </Document>
</kml>
"#,
        session_id = escape(session_id),
        whens = whens,
        coords = coords,
    )
}
