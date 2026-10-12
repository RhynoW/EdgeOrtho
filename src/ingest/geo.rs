//! TWD97 / TM2 zone 121（EPSG:3826）與經緯度、Web Mercator（EPSG:3857）的轉換。
//! 橫麥卡托使用 Krüger 級數（到 n^4 項，GRS80 橢球誤差遠小於 1 mm），與 pyproj 逐點核對見下方測試。

const A: f64 = 6378137.0;
const F: f64 = 1.0 / 298.257222101;
const K0: f64 = 0.9999;
const LON0_DEG: f64 = 121.0;
const FE: f64 = 250000.0;
const MERC_R: f64 = 6378137.0;

struct Series { a_hat: f64, alpha: [f64; 4], beta: [f64; 4], delta: [f64; 4], n: f64 }

fn series() -> Series {
    let n = F / (2.0 - F);
    let (n2, n3, n4) = (n * n, n * n * n, n * n * n * n);
    Series {
        n,
        a_hat: A / (1.0 + n) * (1.0 + n2 / 4.0 + n4 / 64.0),
        alpha: [
            n / 2.0 - 2.0 * n2 / 3.0 + 5.0 * n3 / 16.0 + 41.0 * n4 / 180.0,
            13.0 * n2 / 48.0 - 3.0 * n3 / 5.0 + 557.0 * n4 / 1440.0,
            61.0 * n3 / 240.0 - 103.0 * n4 / 140.0,
            49561.0 * n4 / 161280.0,
        ],
        beta: [
            n / 2.0 - 2.0 * n2 / 3.0 + 37.0 * n3 / 96.0 - n4 / 360.0,
            n2 / 48.0 + n3 / 15.0 - 437.0 * n4 / 1440.0,
            17.0 * n3 / 480.0 - 37.0 * n4 / 840.0,
            4397.0 * n4 / 161280.0,
        ],
        delta: [
            2.0 * n - 2.0 * n2 / 3.0 - 2.0 * n3 + 116.0 * n4 / 45.0,
            7.0 * n2 / 3.0 - 8.0 * n3 / 5.0 - 227.0 * n4 / 45.0,
            56.0 * n3 / 15.0 - 136.0 * n4 / 35.0,
            4279.0 * n4 / 630.0,
        ],
    }
}

/// 經緯度（度）→ TM2 (E, N)，單位公尺。
pub fn lonlat_to_tm2(lon: f64, lat: f64) -> (f64, f64) {
    let s = series();
    let phi = lat.to_radians();
    let dl = (lon - LON0_DEG).to_radians();
    let c = 2.0 * s.n.sqrt() / (1.0 + s.n);
    let t = (phi.sin().atanh() - c * (c * phi.sin()).atanh()).sinh();
    let xi0 = (t / dl.cos()).atan();
    let eta0 = (dl.sin() / (1.0 + t * t).sqrt()).atanh();
    let xi_n = xi0 + (1..=4).map(|j| { let jf = j as f64; s.alpha[j - 1] * (2.0 * jf * xi0).sin() * (2.0 * jf * eta0).cosh() }).sum::<f64>();
    let eta_e = eta0 + (1..=4).map(|j| { let jf = j as f64; s.alpha[j - 1] * (2.0 * jf * xi0).cos() * (2.0 * jf * eta0).sinh() }).sum::<f64>();
    (FE + K0 * s.a_hat * eta_e, K0 * s.a_hat * xi_n)
}

/// TM2 (E, N) → 經緯度（度）。
pub fn tm2_to_lonlat(e: f64, n: f64) -> (f64, f64) {
    let s = series();
    let xi = n / (K0 * s.a_hat);
    let eta = (e - FE) / (K0 * s.a_hat);
    let mut xi_p = xi;
    let mut eta_p = eta;
    for j in 1..=4 {
        let jf = j as f64;
        xi_p -= s.beta[j - 1] * (2.0 * jf * xi).sin() * (2.0 * jf * eta).cosh();
        eta_p -= s.beta[j - 1] * (2.0 * jf * xi).cos() * (2.0 * jf * eta).sinh();
    }
    let chi = (xi_p.sin() / eta_p.cosh()).asin();
    let phi = chi + (1..=4).map(|j| s.delta[j - 1] * (2.0 * j as f64 * chi).sin()).sum::<f64>();
    let lam = LON0_DEG.to_radians() + eta_p.sinh().atan2(xi_p.cos());
    (lam.to_degrees(), phi.to_degrees())
}

/// Web Mercator (EPSG:3857) 公尺 → 經緯度（度）。
pub fn mercator_to_lonlat(x: f64, y: f64) -> (f64, f64) {
    let lon = (x / MERC_R).to_degrees();
    let lat = (2.0 * (y / MERC_R).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees();
    (lon, lat)
}

pub fn lonlat_to_mercator(lon: f64, lat: f64) -> (f64, f64) {
    (MERC_R * lon.to_radians(), MERC_R * (std::f64::consts::FRAC_PI_4 + lat.to_radians() / 2.0).tan().ln())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 參考值由 pyproj 3.7.2 `Transformer.from_crs(4326, 3826, always_xy=True)` 產生。
    const REF: [(f64, f64, f64, f64); 6] = [
        (120.31, 23.13, 179333.2535, 2558845.6790),
        (121.56454, 25.03397, 306969.5831, 2769658.9175),
        (122.0, 24.5, 351348.0477, 2710765.3026),
        (120.0, 22.0, 146744.3798, 2433894.6700),
        (121.0, 23.5, 250000.0000, 2599651.3488),
        (120.2, 23.0, 167988.3091, 2544506.8477),
    ];

    #[test]
    fn tm2_forward_matches_pyproj_within_1mm() {
        for (lon, lat, e, n) in REF {
            let (ge, gn) = lonlat_to_tm2(lon, lat);
            assert!((ge - e).abs() < 1e-3 && (gn - n).abs() < 1e-3, "({lon},{lat}): got ({ge},{gn}) want ({e},{n})");
        }
    }

    #[test]
    fn tm2_roundtrip_within_0_1_mm() {
        for (lon, lat, e, n) in REF {
            let (rl, rp) = tm2_to_lonlat(e, n);
            assert!((rl - lon).abs() < 1e-9 && (rp - lat).abs() < 1e-9, "({e},{n}) -> ({rl},{rp}) want ({lon},{lat})");
        }
    }

    #[test]
    fn mercator_roundtrip() {
        let (x, y) = lonlat_to_mercator(120.31, 23.13);
        assert!((x - 13392847.9373).abs() < 1e-3 && (y - 2647747.5275).abs() < 1e-3, "{x} {y}");
        let (lon, lat) = mercator_to_lonlat(x, y);
        assert!((lon - 120.31).abs() < 1e-9 && (lat - 23.13).abs() < 1e-9);
    }
}
