//! 增量鑲嵌：把一張已定位的影像投影到各目標圖磚格網，只重寫受影響的圖磚。
//!
//! 幾何（與 scripts/sim/sim_aerial_gen.py 相同的「模擬慣例」）：
//! - 影像上方 = 航向 `yaw`（真北順時針）；影像右方 = 航向右側；像素大小 `gsd = agl × gsd_per_agl`。
//! - 正下視（pitch = -90）時 footprint 中心 = 機下點；俯仰／橫滾偏離會讓 footprint 位移 `agl × tan(傾角)`。
//! - 平坦地面、針孔相機、無鏡頭畸變、無地形起伏。
//! 混合：影像邊緣 `feather_px` 範圍線性羽化；舊圖磚該處無資料時直接採用新影像，否則 lerp。

use super::geo::{lonlat_to_mercator, lonlat_to_tm2, mercator_to_lonlat, tm2_to_lonlat};
use super::pose::Pose;
use crate::config::TileMatrixSetConfig;
use crate::tilematrix::{resolve_levels, ResolvedTileMatrix};
use image::{RgbImage, RgbaImage};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Geom {
    fe: f64,
    fnn: f64,
    c: f64,
    s: f64,
    gsd: f64,
    w: f64,
    h: f64,
}

impl Geom {
    pub fn new(p: &Pose, gsd_per_agl: f64, w: u32, h: u32) -> Self {
        let (e0, n0) = lonlat_to_tm2(p.lon, p.lat);
        let th = p.yaw.to_radians();
        let (s, c) = th.sin_cos();
        let (tr, tp) = (p.roll.to_radians().tan(), (p.pitch + 90.0).to_radians().tan());
        Geom {
            fe: e0 + p.agl * (tr * c + tp * s),
            fnn: n0 + p.agl * (-tr * s + tp * c),
            c,
            s,
            gsd: p.agl * gsd_per_agl,
            w: w as f64,
            h: h as f64,
        }
    }

    pub fn footprint_center(&self) -> (f64, f64) {
        (self.fe, self.fnn)
    }

    pub fn gsd(&self) -> f64 {
        self.gsd
    }

    /// 地面 TM2 → 影像像素。
    pub fn to_frame(&self, e: f64, n: f64) -> (f64, f64) {
        let (de, dn) = (e - self.fe, n - self.fnn);
        (
            self.w / 2.0 + (de * self.c - dn * self.s) / self.gsd,
            self.h / 2.0 - (de * self.s + dn * self.c) / self.gsd,
        )
    }

    /// 影像像素 → 地面 TM2。
    pub fn to_ground(&self, u: f64, v: f64) -> (f64, f64) {
        let (r, up) = ((u - self.w / 2.0) * self.gsd, (self.h / 2.0 - v) * self.gsd);
        (self.fe + r * self.c + up * self.s, self.fnn - r * self.s + up * self.c)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Crs {
    Mercator,
    Tm2,
}

fn crs_of(tms: &TileMatrixSetConfig) -> Option<Crs> {
    if tms.crs.ends_with("3857") {
        Some(Crs::Mercator)
    } else if tms.crs.ends_with("3826") {
        Some(Crs::Tm2)
    } else {
        None
    }
}

/// 目標座標（目標 CRS）→ TM2。
fn to_tm2(crs: Crs, x: f64, y: f64) -> (f64, f64) {
    match crs {
        Crs::Tm2 => (x, y),
        Crs::Mercator => {
            let (lon, lat) = mercator_to_lonlat(x, y);
            lonlat_to_tm2(lon, lat)
        }
    }
}

/// TM2 → 目標座標。
fn from_tm2(crs: Crs, e: f64, n: f64) -> (f64, f64) {
    match crs {
        Crs::Tm2 => (e, n),
        Crs::Mercator => {
            let (lon, lat) = tm2_to_lonlat(e, n);
            lonlat_to_mercator(lon, lat)
        }
    }
}

fn bilinear(img: &RgbImage, u: f64, v: f64) -> [f32; 3] {
    let (x0, y0) = (u.floor() as u32, v.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(img.width() - 1), (y0 + 1).min(img.height() - 1));
    let (fx, fy) = ((u - x0 as f64) as f32, (v - y0 as f64) as f32);
    let (a, b, c, d) = (
        img.get_pixel(x0, y0).0,
        img.get_pixel(x1, y0).0,
        img.get_pixel(x0, y1).0,
        img.get_pixel(x1, y1).0,
    );
    let mut o = [0f32; 3];
    for k in 0..3 {
        o[k] = (a[k] as f32 * (1.0 - fx) + b[k] as f32 * fx) * (1.0 - fy) + (c[k] as f32 * (1.0 - fx) + d[k] as f32 * fx) * fy;
    }
    o
}

pub fn tile_path(tile_dir: &str, tms: &str, z: &str, col: u32, row: u32) -> PathBuf {
    [tile_dir, tms, z, &col.to_string(), &format!("{row}.png")].iter().collect()
}

/// 圖磚級鎖：同一塊圖磚的「讀舊內容→混合→寫回」（鑲嵌）與「讀子圖磚→重算→寫回」（金字塔）必須互斥，
/// 不同圖磚則可由不同影像並行處理。每個執行緒同一時間只持有一把圖磚鎖（不巢狀），所以不會死結。
/// 金字塔父圖磚**在鎖內才讀子圖磚、且不跨父圖磚快取**：這樣最後一個重算該父圖磚的影像，
/// 讀到的一定是所有影像寫完子圖磚之後的內容（每張影像都是在自己寫完子圖磚之後才重算父圖磚）。
static TILE_LOCKS: std::sync::LazyLock<std::sync::Mutex<HashMap<PathBuf, std::sync::Arc<std::sync::Mutex<()>>>>> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

fn tile_lock(path: &Path) -> std::sync::Arc<std::sync::Mutex<()>> {
    let mut m = TILE_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    if m.len() > 50_000 {
        m.retain(|_, v| std::sync::Arc::strong_count(v) > 1); // 只留下正被持有／等待的鎖，避免無限成長
    }
    m.entry(path.to_path_buf()).or_default().clone()
}

/// 累計寫入圖磚（PNG 編碼＋檔案系統）的奈秒與次數；診斷用（測試輔助會印出），用來分辨「運算慢」與「I/O 慢」。
pub static WRITE_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static WRITE_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 以多執行緒對 items 逐一套用 f（圖磚之間互不相干），結果依原順序回傳。執行緒數 = min(CPU 核心數, 8, 項目數)。
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).min(8).min(items.len());
    if n <= 1 {
        return items.iter().map(&f).collect();
    }
    let next = AtomicUsize::new(0);
    let out: std::sync::Mutex<Vec<(usize, R)>> = std::sync::Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|sc| {
        for _ in 0..n {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= items.len() {
                    break;
                }
                let r = f(&items[i]);
                out.lock().unwrap().push((i, r));
            });
        }
    });
    let mut v = out.into_inner().unwrap();
    v.sort_by_key(|x| x.0);
    v.into_iter().map(|x| x.1).collect()
}

fn write_atomic(path: &Path, img: &RgbaImage) -> Result<(), String> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::ImageEncoder;
    let t0 = std::time::Instant::now();
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp.png");
    {
        // 快速壓縮：圖磚會被頻繁重寫，編碼時間比檔案大小重要
        let f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        PngEncoder::new_with_quality(std::io::BufWriter::new(f), CompressionType::Fast, FilterType::Sub)
            .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)
            .map_err(|e| e.to_string())?;
    }
    let r = std::fs::rename(&tmp, path).map_err(|e| e.to_string());
    WRITE_NS.fetch_add(t0.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    WRITE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    r
}

/// 把影像鑲嵌到一個目標格網（TMS＋zoom）；回傳寫入的圖磚索引 (col, row)。
pub fn mosaic_into(
    frame: &RgbImage,
    g: &Geom,
    tms: &TileMatrixSetConfig,
    zoom: u32,
    tile_dir: &str,
    feather_px: f64,
) -> Result<Vec<(u32, u32)>, String> {
    let crs = crs_of(tms).ok_or_else(|| format!("不支援的 CRS：{}", tms.crs))?;
    let levels = resolve_levels(tms);
    let m: &ResolvedTileMatrix = levels
        .iter()
        .find(|l| l.identifier == zoom.to_string())
        .ok_or_else(|| format!("{} 沒有 zoom {zoom}", tms.identifier))?;
    let (tw, th) = (m.tile_width as usize, m.tile_height as usize);
    let (span_x, span_y) = (m.tile_width as f64 * m.resolution, m.tile_height as f64 * m.resolution);
    // 影像四角 → 目標 CRS → 圖磚索引範圍
    let (fw, fh) = (frame.width() as f64, frame.height() as f64);
    let corners: Vec<(f64, f64)> = [(0.0, 0.0), (fw, 0.0), (fw, fh), (0.0, fh)]
        .iter()
        .map(|&(u, v)| {
            let (e, n) = g.to_ground(u, v);
            from_tm2(crs, e, n)
        })
        .collect();
    let (xmin, xmax) = corners.iter().fold((f64::MAX, f64::MIN), |a, c| (a.0.min(c.0), a.1.max(c.0)));
    let (ymin, ymax) = corners.iter().fold((f64::MAX, f64::MIN), |a, c| (a.0.min(c.1), a.1.max(c.1)));
    let col0 = ((xmin - m.top_left_corner[0]) / span_x).floor().max(0.0) as u32;
    let col1 = (((xmax - m.top_left_corner[0]) / span_x).floor().max(0.0) as u32).min(m.matrix_width.saturating_sub(1));
    let row0 = ((m.top_left_corner[1] - ymax) / span_y).floor().max(0.0) as u32;
    let row1 = (((m.top_left_corner[1] - ymin) / span_y).floor().max(0.0) as u32).min(m.matrix_height.saturating_sub(1));
    let jobs: Vec<(u32, u32)> = (row0..=row1).flat_map(|r| (col0..=col1).map(move |c| (c, r))).collect();
    let results = par_map(&jobs, |&(col, row)| -> Result<Option<(u32, u32)>, String> {
        {
            // 圖磚四角（目標 CRS）→ TM2，之後每個像素用雙線性內插取地面座標（圖磚內的投影曲率可忽略）。
            let (x_l, x_r) = (m.top_left_corner[0] + col as f64 * span_x, m.top_left_corner[0] + (col + 1) as f64 * span_x);
            let (y_t, y_b) = (m.top_left_corner[1] - row as f64 * span_y, m.top_left_corner[1] - (row + 1) as f64 * span_y);
            let (tl, tr, bl, br) = (to_tm2(crs, x_l, y_t), to_tm2(crs, x_r, y_t), to_tm2(crs, x_l, y_b), to_tm2(crs, x_r, y_b));
            let path = tile_path(tile_dir, &tms.identifier, &m.identifier, col, row);
            let lock = tile_lock(&path);
            let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
            let mut tile: Option<RgbaImage> = None;
            for j in 0..th {
                let fy = (j as f64 + 0.5) / th as f64;
                for i in 0..tw {
                    let fx = (i as f64 + 0.5) / tw as f64;
                    let e = (tl.0 * (1.0 - fx) + tr.0 * fx) * (1.0 - fy) + (bl.0 * (1.0 - fx) + br.0 * fx) * fy;
                    let n = (tl.1 * (1.0 - fx) + tr.1 * fx) * (1.0 - fy) + (bl.1 * (1.0 - fx) + br.1 * fx) * fy;
                    let (u, v) = g.to_frame(e, n);
                    if u < 0.0 || v < 0.0 || u > fw - 1.0 || v > fh - 1.0 {
                        continue;
                    }
                    let t = tile.get_or_insert_with(|| {
                        image::open(&path)
                            .map(|d| d.to_rgba8())
                            .ok()
                            .filter(|d| d.width() as usize == tw && d.height() as usize == th)
                            .unwrap_or_else(|| RgbaImage::new(tw as u32, th as u32))
                    });
                    let edge = u.min(fw - 1.0 - u).min(v).min(fh - 1.0 - v);
                    let wgt = if feather_px > 0.0 { (edge / feather_px).clamp(0.0, 1.0) as f32 } else { 1.0 };
                    let new = bilinear(frame, u, v);
                    let px = t.get_pixel_mut(i as u32, j as u32);
                    if px.0[3] == 0 {
                        *px = image::Rgba([new[0].round() as u8, new[1].round() as u8, new[2].round() as u8, 255]);
                    } else {
                        for k in 0..3 {
                            px.0[k] = (px.0[k] as f32 * (1.0 - wgt) + new[k] * wgt).round().clamp(0.0, 255.0) as u8;
                        }
                    }
                }
            }
            if let Some(t) = tile {
                write_atomic(&path, &t)?;
                return Ok(Some((col, row)));
            }
            Ok(None)
        }
    });
    let mut written = Vec::new();
    for r in results {
        if let Some(x) = r? {
            written.push(x);
        }
    }
    Ok(written)
}

/// 讀取並解碼一塊圖磚（不快取：見 TILE_LOCKS 的說明）。
fn load_tile(dir: &str, tms: &str, z: &str, col: u32, row: u32, tw: u32, th: u32) -> Option<RgbaImage> {
    image::open(tile_path(dir, tms, z, col, row)).ok().map(|d| d.to_rgba8()).filter(|d| d.width() == tw && d.height() == th)
}

/// 金字塔：由「已寫入的原生級別圖磚」往下（較粗的縮放級別）逐級重算受影響的圖磚。
/// 每個粗級別圖磚都由下一級（較細）的圖磚以 k×k 取樣箱形濾波重算（k = ceil(解析度比)，上限 4）；
/// 不假設 2 倍比例，所以 TWD97 的 2.5 倍層級也適用。只計入有資料的取樣點，全無資料則保持透明。
/// 回傳寫入（重算）的圖磚數。
pub fn build_pyramid(
    tms: &TileMatrixSetConfig,
    native_zoom: u32,
    changed: &[(u32, u32)],
    levels_down: u32,
    tile_dir: &str,
) -> Result<usize, String> {
    let levels = resolve_levels(tms);
    let mut idx = levels
        .iter()
        .position(|l| l.identifier == native_zoom.to_string())
        .ok_or_else(|| format!("{} 沒有 zoom {native_zoom}", tms.identifier))?;
    let mut cur: std::collections::HashSet<(u32, u32)> = changed.iter().cloned().collect();
    let mut total = 0;
    for _ in 0..levels_down {
        if idx == 0 || cur.is_empty() {
            break;
        }
        let (fine, coarse) = (&levels[idx], &levels[idx - 1]);
        if coarse.resolution <= fine.resolution {
            break;
        }
        let (tw, th) = (coarse.tile_width, coarse.tile_height);
        let k = ((coarse.resolution / fine.resolution).ceil() as usize).clamp(1, 4);
        let (fspan_x, fspan_y) = (fine.tile_width as f64 * fine.resolution, fine.tile_height as f64 * fine.resolution);
        let (cspan_x, cspan_y) = (tw as f64 * coarse.resolution, th as f64 * coarse.resolution);
        // 受影響的粗級別圖磚
        let mut parents = std::collections::BTreeSet::new();
        for &(c, r) in &cur {
            let (x0, x1) = (c as f64 * fspan_x, (c + 1) as f64 * fspan_x - 1e-6);
            let (y0, y1) = (r as f64 * fspan_y, (r + 1) as f64 * fspan_y - 1e-6);
            let pr_max = ((y1 / cspan_y).floor() as u32).min(coarse.matrix_height.saturating_sub(1));
            let pc_max = ((x1 / cspan_x).floor() as u32).min(coarse.matrix_width.saturating_sub(1));
            for pr in (y0 / cspan_y).floor() as u32..=pr_max {
                for pc in (x0 / cspan_x).floor() as u32..=pc_max {
                    parents.insert((pc, pr));
                }
            }
        }
        let ratio = coarse.resolution / fine.resolution;
        let parents_v: Vec<(u32, u32)> = parents.iter().cloned().collect();
        let results = par_map(&parents_v, |&(pc, pr)| -> Result<Option<(u32, u32)>, String> {
            let parent_path = tile_path(tile_dir, &tms.identifier, &coarse.identifier, pc, pr);
            let lock = tile_lock(&parent_path);
            let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
            // 取樣座標表：粗級別像素 (i, sub) → (細級別圖磚欄索引, 圖磚內像素)；列同理。每個父圖磚只算一次。
            let axis = |origin_tiles: u32, n_px: u32, tile_px: u32, matrix: u32| -> Vec<Option<(u32, u32)>> {
                let mut v = Vec::with_capacity(n_px as usize * k);
                for i in 0..n_px {
                    for s_ in 0..k {
                        let g = ((origin_tiles * n_px + i) as f64 + (s_ as f64 + 0.5) / k as f64) * ratio;
                        let t = (g / tile_px as f64) as u32;
                        v.push(if t >= matrix { None } else { Some((t, ((g - t as f64 * tile_px as f64) as u32).min(tile_px - 1))) });
                    }
                }
                v
            };
            let xs = axis(pc, tw, fine.tile_width, fine.matrix_width);
            let ys = axis(pr, th, fine.tile_height, fine.matrix_height);
            let (c0, c1) = xs.iter().flatten().fold((u32::MAX, 0), |a, x| (a.0.min(x.0), a.1.max(x.0)));
            let (r0, r1) = ys.iter().flatten().fold((u32::MAX, 0), |a, y| (a.0.min(y.0), a.1.max(y.0)));
            if c0 == u32::MAX || r0 == u32::MAX {
                return Ok(None);
            }
            let ncols = (c1 - c0 + 1) as usize;
            let mut grid: Vec<Option<RgbaImage>> = Vec::with_capacity(ncols * (r1 - r0 + 1) as usize);
            for fr in r0..=r1 {
                for fc in c0..=c1 {
                    grid.push(load_tile(tile_dir, &tms.identifier, &fine.identifier, fc, fr, fine.tile_width, fine.tile_height));
                }
            }
            if grid.iter().all(|g| g.is_none()) {
                return Ok(None);
            }
            let mut out = RgbaImage::new(tw, th);
            let mut any = false;
            for j in 0..th as usize {
                for i in 0..tw as usize {
                    let (mut sum, mut n) = ([0u32; 3], 0u32);
                    for sj in 0..k {
                        let Some((fr, py)) = ys[j * k + sj] else { continue };
                        for si in 0..k {
                            let Some((fc, px)) = xs[i * k + si] else { continue };
                            if let Some(t) = &grid[(fc - c0) as usize + (fr - r0) as usize * ncols] {
                                let p = t.get_pixel(px, py).0;
                                if p[3] > 0 {
                                    sum[0] += p[0] as u32;
                                    sum[1] += p[1] as u32;
                                    sum[2] += p[2] as u32;
                                    n += 1;
                                }
                            }
                        }
                    }
                    if n > 0 {
                        out.put_pixel(i as u32, j as u32, image::Rgba([((sum[0] + n / 2) / n) as u8, ((sum[1] + n / 2) / n) as u8, ((sum[2] + n / 2) / n) as u8, 255]));
                        any = true;
                    }
                }
            }
            if any {
                write_atomic(&parent_path, &out)?;
                return Ok(Some((pc, pr)));
            }
            Ok(None)
        });
        let mut next = std::collections::HashSet::new();
        for r in results {
            if let Some(x) = r? {
                next.insert(x);
                total += 1;
            }
        }
        cur = next;
        idx -= 1;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    fn pose() -> Pose {
        Pose { lat: 23.13, lon: 120.31, alt_asl: 115.0, agl: 100.0, yaw: 0.0, pitch: -90.0, roll: 0.0, captured_at: None, rtk_flag: None }
    }

    #[test]
    fn geom_roundtrip_and_orientation() {
        let g = Geom::new(&Pose { yaw: 90.0, ..pose() }, 0.0026458, 1024, 768);
        let (e, n) = g.to_ground(700.0, 200.0);
        let (u, v) = g.to_frame(e, n);
        assert!((u - 700.0).abs() < 1e-6 && (v - 200.0).abs() < 1e-6);
        // 航向東（yaw=90）：影像上方指向東，所以影像中心往上的點，E 比中心大
        let (ce, _) = g.to_ground(512.0, 384.0);
        let (ue, _) = g.to_ground(512.0, 100.0);
        assert!(ue > ce + 50.0);
    }

    #[test]
    fn mosaic_places_pixels_at_the_right_ground_position() {
        let cfg = AppConfig::load(&PathBuf::from("config/layers.local.toml")).unwrap();
        let tms = cfg.tilematrixset("TWD97_TM2_NLSC").unwrap();
        let dir = std::env::temp_dir().join(format!("ingest_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // 影像：R 隨 u 增加、G 隨 v 增加，B 固定；agl=100、gsd_per_agl=0.01 → 1 m/像素，64×48 m
        let mut img = RgbImage::new(64, 48);
        for (u, v, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([(u * 4) as u8, (v * 5) as u8, 100]);
        }
        let g = Geom::new(&pose(), 0.01, 64, 48);
        let n = mosaic_into(&img, &g, tms, 13, dir.to_str().unwrap(), 0.0).unwrap();
        assert!(!n.is_empty(), "tiles written: {n:?}");
        let n = n.len();
        // 取地面上「影像像素 (40.5, 10.5)」對應位置，到圖磚裡讀，顏色應接近 (162, 52, 100)
        let (e, nn) = g.to_ground(40.5, 10.5);
        let levels = resolve_levels(tms);
        let m = levels.iter().find(|l| l.identifier == "13").unwrap();
        let span = m.tile_width as f64 * m.resolution;
        let (col, row) = (
            ((e - m.top_left_corner[0]) / span).floor() as u32,
            ((m.top_left_corner[1] - nn) / span).floor() as u32,
        );
        let tile = image::open(tile_path(dir.to_str().unwrap(), "TWD97_TM2_NLSC", "13", col, row)).unwrap().to_rgba8();
        let (px, py) = (
            ((e - m.top_left_corner[0]) / m.resolution - col as f64 * 256.0) as u32,
            ((m.top_left_corner[1] - nn) / m.resolution - row as f64 * 256.0) as u32,
        );
        let p = tile.get_pixel(px, py).0;
        assert!(
            (p[0] as i32 - 162).abs() <= 6 && (p[1] as i32 - 52).abs() <= 8 && p[2] == 100 && p[3] == 255,
            "pixel {p:?} at tile ({col},{row}) px ({px},{py})"
        );
        // 第二次寫入（羽化 0、相同影像）涵蓋的圖磚數不變
        let n2 = mosaic_into(&img, &g, tms, 13, dir.to_str().unwrap(), 0.0).unwrap().len();
        assert_eq!(n, n2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn solid(color: [u8; 3]) -> RgbaImage {
        RgbaImage::from_pixel(256, 256, image::Rgba([color[0], color[1], color[2], 255]))
    }

    #[test]
    fn pyramid_mercator_quadrants_map_to_children() {
        let cfg = AppConfig::load(&PathBuf::from("config/layers.local.toml")).unwrap();
        let tms = cfg.tilematrixset("GoogleMapsCompatible").unwrap();
        let dir = std::env::temp_dir().join(format!("pyr_m_{}", std::process::id()));
        let d = dir.to_str().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let (c, r) = (1000u32, 2000u32);
        let colors = [[10, 20, 30], [200, 20, 30], [10, 200, 30], [10, 20, 200]];
        let kids = [(2 * c, 2 * r), (2 * c + 1, 2 * r), (2 * c, 2 * r + 1), (2 * c + 1, 2 * r + 1)];
        for (k, (cc, rr)) in kids.iter().enumerate() {
            write_atomic(&tile_path(d, "GoogleMapsCompatible", "19", *cc, *rr), &solid(colors[k])).unwrap();
        }
        let n = build_pyramid(tms, 19, &kids, 3, d).unwrap();
        assert_eq!(n, 3, "每級 1 塊 × 3 級");
        let t = image::open(tile_path(d, "GoogleMapsCompatible", "18", c, r)).unwrap().to_rgba8();
        assert_eq!(t.get_pixel(20, 20).0, [10, 20, 30, 255]);
        assert_eq!(t.get_pixel(200, 20).0, [200, 20, 30, 255]);
        assert_eq!(t.get_pixel(20, 200).0, [10, 200, 30, 255]);
        assert_eq!(t.get_pixel(200, 200).0, [10, 20, 200, 255]);
        assert!(tile_path(d, "GoogleMapsCompatible", "16", c / 4, r / 4).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pyramid_twd97_handles_non_integer_ratio() {
        // 第 13 → 12 層的解析度比是 2.5（不是 2），仍要能由子磚重算出同色的父磚像素
        let cfg = AppConfig::load(&PathBuf::from("config/layers.local.toml")).unwrap();
        let tms = cfg.tilematrixset("TWD97_TM2_NLSC").unwrap();
        let dir = std::env::temp_dir().join(format!("pyr_t_{}", std::process::id()));
        let d = dir.to_str().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let (c, r) = (1500u32, 3000u32);
        write_atomic(&tile_path(d, "TWD97_TM2_NLSC", "13", c, r), &solid([50, 100, 150])).unwrap();
        let n = build_pyramid(tms, 13, &[(c, r)], 2, d).unwrap();
        assert!(n >= 2, "至少一塊父磚與一塊祖父磚，實際 {n}");
        let ratio = 0.26458386250102506 / 0.6614596562525626;
        let (pc, pr) = ((c as f64 * ratio).floor() as u32, (r as f64 * ratio).floor() as u32);
        let t = image::open(tile_path(d, "TWD97_TM2_NLSC", "12", pc, pr)).unwrap().to_rgba8();
        let solid_px = t.pixels().filter(|p| p.0 == [50, 100, 150, 255]).count();
        assert!(solid_px > 256 * 256 / 10, "父磚應有一塊同色區域，實際 {solid_px} 像素");
        assert!(t.pixels().all(|p| p.0[3] == 0 || p.0[..3] == [50, 100, 150]), "不應混入其他顏色");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 測試輔助（預設不執行）：依序把資料夾內的假航照鑲嵌成圖磚（與 ingest 端點相同的 mosaic＋金字塔程式），每張之間等待 INTERVAL_MS。
    /// 用在無法長時間執行伺服器程序的環境，讓內建檢視器的「近即時更新」能對著一直變動的圖磚驗證：
    ///   FRAMES_DIR=<資料夾> TILE_DIR=<輸出> INTERVAL_MS=300 cargo test --features ingest stream_frames_to_tiles -- --ignored --nocapture
    #[test]
    #[ignore]
    fn stream_frames_to_tiles() {
        let frames = std::env::var("FRAMES_DIR").expect("FRAMES_DIR");
        let dir = std::env::var("TILE_DIR").expect("TILE_DIR");
        let interval = std::env::var("INTERVAL_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0u64);
        let limit = std::env::var("LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
        let cfg = AppConfig::load(&PathBuf::from("config/layers.local.toml")).unwrap();
        let ing = cfg.ingest.clone().expect("[ingest] in config/layers.local.toml");
        let mut files: Vec<_> = std::fs::read_dir(&frames).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().map(|x| x == "jpg").unwrap_or(false)).collect();
        files.sort();
        for (i, f) in files.iter().take(limit).enumerate() {
            let bytes = std::fs::read(f).unwrap();
            let pose = super::super::pose::parse(&bytes).unwrap();
            let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg).unwrap().to_rgb8();
            let g = Geom::new(&pose, ing.gsd_per_agl, img.width(), img.height());
            let mut n = 0;
            let (mut t_mos, mut t_pyr) = (0u128, 0u128);
            for t in &ing.targets {
                let tms = cfg.tilematrixset(&t.tms).unwrap();
                let t0 = std::time::Instant::now();
                let ch = mosaic_into(&img, &g, tms, t.zoom, &dir, ing.feather_px).unwrap();
                t_mos += t0.elapsed().as_millis();
                let t1 = std::time::Instant::now();
                n += ch.len() + if t.pyramid > 0 { build_pyramid(tms, t.zoom, &ch, t.pyramid, &dir).unwrap() } else { 0 };
                t_pyr += t1.elapsed().as_millis();
            }
            let (wn, wc) = (WRITE_NS.swap(0, std::sync::atomic::Ordering::Relaxed), WRITE_COUNT.swap(0, std::sync::atomic::Ordering::Relaxed));
            eprintln!("frame {} {} tiles={n} mosaic_ms={t_mos} pyramid_ms={t_pyr} write_ms={} ({} writes, {:.1} ms each)", i + 1, f.file_name().unwrap().to_string_lossy(), wn / 1_000_000, wc, wn as f64 / 1e6 / wc.max(1) as f64);
            std::thread::sleep(std::time::Duration::from_millis(interval));
        }
    }

    /// 並行正確性：多張互相重疊的影像同時鑲嵌＋金字塔後，每個金字塔層級的圖磚必須和「由最終子圖磚重新計算」的結果逐像素相同。
    /// 若有競爭（舊的父圖磚蓋掉新的），重算後就會不同。
    #[test]
    fn concurrent_frames_keep_pyramid_consistent() {
        let cfg = AppConfig::load(&PathBuf::from("config/layers.local.toml")).unwrap();
        for (tms_id, zoom) in [("GoogleMapsCompatible", 19u32), ("TWD97_TM2_NLSC", 13)] {
            let tms = cfg.tilematrixset(tms_id).unwrap();
            let dir = std::env::temp_dir().join(format!("conc_{tms_id}_{}", std::process::id()));
            let d = dir.to_str().unwrap().to_string();
            let _ = std::fs::remove_dir_all(&dir);
            // 6 張影像：位置彼此只差幾十公尺（大量重疊），航向不同，內容各異
            let frames: Vec<(RgbImage, Geom)> = (0..6)
                .map(|k| {
                    let mut img = RgbImage::new(160, 120);
                    for (u, v, p) in img.enumerate_pixels_mut() {
                        *p = image::Rgb([((u * 3 + k * 40) % 256) as u8, ((v * 4 + k * 25) % 256) as u8, (k * 40) as u8]);
                    }
                    let pose = Pose { lat: 23.13 + 0.0002 * k as f64, lon: 120.31 + 0.0003 * k as f64, alt_asl: 115.0, agl: 100.0, yaw: 60.0 * k as f64, pitch: -90.0, roll: 0.0, captured_at: None, rtk_flag: None };
                    (img, Geom::new(&pose, 0.0026458386 * 4.0, 160, 120))
                })
                .collect();
            let changed_all = std::sync::Mutex::new(std::collections::BTreeSet::new());
            std::thread::scope(|sc| {
                for (img, g) in &frames {
                    sc.spawn(|| {
                        let ch = mosaic_into(img, g, tms, zoom, &d, 16.0).unwrap();
                        changed_all.lock().unwrap().extend(ch.iter().cloned());
                        build_pyramid(tms, zoom, &ch, 4, &d).unwrap();
                    });
                }
            });
            // 快照所有金字塔層級的圖磚
            let levels = resolve_levels(tms);
            let idx = levels.iter().position(|l| l.identifier == zoom.to_string()).unwrap();
            let snap = |dir: &str| -> Vec<(String, Vec<u8>)> {
                let mut v = Vec::new();
                for k in 1..=4 {
                    let z = &levels[idx - k].identifier;
                    let zdir = std::path::Path::new(dir).join(tms_id).join(z);
                    if let Ok(cols) = std::fs::read_dir(&zdir) {
                        for c in cols.flatten() {
                            for f in std::fs::read_dir(c.path()).unwrap().flatten() {
                                v.push((format!("{z}/{}/{}", c.file_name().to_string_lossy(), f.file_name().to_string_lossy()), image::open(f.path()).unwrap().to_rgba8().into_raw()));
                            }
                        }
                    }
                }
                v.sort_by(|a, b| a.0.cmp(&b.0));
                v
            };
            let before = snap(&d);
            assert!(before.len() >= 4, "金字塔層級應該有圖磚");
            let all: Vec<(u32, u32)> = changed_all.lock().unwrap().iter().cloned().collect();
            build_pyramid(tms, zoom, &all, 4, &d).unwrap(); // 由最終的子圖磚單執行緒式地重算一遍
            let after = snap(&d);
            assert_eq!(before.len(), after.len(), "{tms_id}");
            for (b, a) in before.iter().zip(after.iter()) {
                assert_eq!(b.0, a.0);
                assert!(b.1 == a.1, "{tms_id}: 金字塔圖磚 {} 與重算結果不同（並行競爭）", b.0);
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
