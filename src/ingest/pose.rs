//! 從原始 JPEG 讀取拍攝位置與姿態：EXIF（GPS 經緯度、海拔、拍攝時間）與 DJI 風格 XMP（雲台／機身姿態、相對高度）。

use std::io::Cursor;

#[derive(Debug, Clone, PartialEq)]
pub struct Pose {
    pub lat: f64,
    pub lon: f64,
    /// 海平面以上高度（GPSAltitude）。
    pub alt_asl: f64,
    /// 相對地面高度（XMP RelativeAltitude），用來換算地面解析度。
    pub agl: f64,
    /// 真北起算順時針的航向（度）：影像「上方」指向的方向。
    pub yaw: f64,
    /// 雲台俯仰（度）：-90＝正下視；偏離 -90 的部分造成 footprint 位移。
    pub pitch: f64,
    pub roll: f64,
    pub captured_at: Option<String>,
    pub rtk_flag: Option<i32>,
}

pub fn xmp_attr(xmp: &str, name: &str) -> Option<f64> {
    let key = format!("drone-dji:{name}=\"");
    let i = xmp.find(&key)? + key.len();
    let j = xmp[i..].find('"')?;
    xmp[i..i + j].trim().parse().ok()
}

fn dms(v: &exif::Value) -> Option<f64> {
    match v {
        exif::Value::Rational(r) if r.len() >= 3 => Some(r[0].to_f64() + r[1].to_f64() / 60.0 + r[2].to_f64() / 3600.0),
        _ => None,
    }
}

fn ascii(f: &exif::Field) -> Option<String> {
    match &f.value {
        exif::Value::Ascii(v) => v.first().map(|b| String::from_utf8_lossy(b).trim().to_string()),
        _ => None,
    }
}

pub fn parse(bytes: &[u8]) -> Result<Pose, String> {
    let exif = exif::Reader::new().read_from_container(&mut Cursor::new(bytes)).map_err(|e| format!("EXIF 讀取失敗：{e}"))?;
    let get = |t: exif::Tag| exif.get_field(t, exif::In::PRIMARY);
    let lat = get(exif::Tag::GPSLatitude).and_then(|f| dms(&f.value)).ok_or("缺少 GPSLatitude")?;
    let lon = get(exif::Tag::GPSLongitude).and_then(|f| dms(&f.value)).ok_or("缺少 GPSLongitude")?;
    let lat = if get(exif::Tag::GPSLatitudeRef).and_then(ascii).as_deref() == Some("S") { -lat } else { lat };
    let lon = if get(exif::Tag::GPSLongitudeRef).and_then(ascii).as_deref() == Some("W") { -lon } else { lon };
    let alt_asl = match get(exif::Tag::GPSAltitude).map(|f| &f.value) {
        Some(exif::Value::Rational(r)) if !r.is_empty() => r[0].to_f64(),
        _ => return Err("缺少 GPSAltitude".into()),
    };
    let head = &bytes[..bytes.len().min(262_144)];
    let xmp = String::from_utf8_lossy(head);
    let need = |n: &str| xmp_attr(&xmp, n).ok_or_else(|| format!("缺少 XMP 欄位 drone-dji:{n}"));
    Ok(Pose {
        lat,
        lon,
        alt_asl,
        agl: need("RelativeAltitude")?,
        yaw: need("GimbalYawDegree")?,
        pitch: need("GimbalPitchDegree")?,
        roll: need("GimbalRollDegree")?,
        captured_at: get(exif::Tag::DateTimeOriginal).and_then(ascii),
        rtk_flag: xmp_attr(&xmp, "RtkFlag").map(|v| v as i32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xmp_attr_parses_signed_numbers() {
        let x = r#"<rdf:Description drone-dji:GimbalYawDegree="+90.52" drone-dji:RelativeAltitude="+100.000" drone-dji:GimbalPitchDegree="-89.10"/>"#;
        assert_eq!(xmp_attr(x, "GimbalYawDegree"), Some(90.52));
        assert_eq!(xmp_attr(x, "RelativeAltitude"), Some(100.0));
        assert_eq!(xmp_attr(x, "GimbalPitchDegree"), Some(-89.10));
        assert_eq!(xmp_attr(x, "GimbalRollDegree"), None);
    }

    #[test]
    fn non_jpeg_is_rejected() {
        assert!(parse(b"not an image").is_err());
    }
}
