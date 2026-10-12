use crate::config::TileMatrixSetConfig;

/// OGC 標準化渲染像素尺寸 0.28mm，用於推算 ScaleDenominator。
const STANDARDIZED_PIXEL_SIZE_M: f64 = 0.00028;
const WEB_MERCATOR_RADIUS_M: f64 = 6378137.0;

#[derive(Debug, Clone)]
pub struct ResolvedTileMatrix {
    /// TileMatrix identifier，依 WMTS 慣例採用 zoom level 字串（如 "0", "1", ...）。
    pub identifier: String,
    pub scale_denominator: f64,
    pub top_left_corner: [f64; 2],
    pub tile_width: u32,
    pub tile_height: u32,
    pub matrix_width: u32,
    pub matrix_height: u32,
    pub resolution: f64,
}

/// 依設定檔展開單一 TileMatrixSet 底下所有 zoom level 的 TileMatrix。
///
/// - GoogleMapsCompatible 系列（未提供 `levels`）採 OGC 公式推導：
///   resolution(z) = 2πR / (tileWidth * 2^z)，matrixWidth = matrixHeight = 2^z。
/// - 非公式格網（如 NLSC TWD97/TM2）要求設定檔逐一列出每層的
///   [resolution, matrix_width, matrix_height]，因為其格網並非全球等比例縮放，
///   不能用公式推導，否則會產生幾何漂移。
pub fn resolve_levels(tms: &TileMatrixSetConfig) -> Vec<ResolvedTileMatrix> {
    if let Some(levels) = &tms.levels {
        levels
            .iter()
            .enumerate()
            .map(|(i, [resolution, mw, mh])| ResolvedTileMatrix {
                identifier: (tms.start_zoom + i as u32).to_string(),
                scale_denominator: resolution / STANDARDIZED_PIXEL_SIZE_M,
                top_left_corner: tms.top_left_corner,
                tile_width: tms.tile_width,
                tile_height: tms.tile_height,
                matrix_width: *mw as u32,
                matrix_height: *mh as u32,
                resolution: *resolution,
            })
            .collect()
    } else {
        let min_z = tms.min_zoom.unwrap_or(0);
        let max_z = tms.max_zoom.unwrap_or(21);
        let base_resolution = 2.0 * std::f64::consts::PI * WEB_MERCATOR_RADIUS_M / tms.tile_width as f64;
        (min_z..=max_z)
            .map(|z| {
                let resolution = base_resolution / 2f64.powi(z as i32);
                let dim = 2u32.pow(z);
                ResolvedTileMatrix {
                    identifier: z.to_string(),
                    scale_denominator: resolution / STANDARDIZED_PIXEL_SIZE_M,
                    top_left_corner: tms.top_left_corner,
                    tile_width: tms.tile_width,
                    tile_height: tms.tile_height,
                    matrix_width: dim,
                    matrix_height: dim,
                    resolution,
                }
            })
            .collect()
    }
}

/// 檢查 (tile_matrix, row, col) 是否落在該 TileMatrixSet 的有效範圍內。
pub fn validate_tile_index<'a>(
    levels: &'a [ResolvedTileMatrix],
    tile_matrix: &str,
    row: u32,
    col: u32,
) -> Option<&'a ResolvedTileMatrix> {
    let level = levels.iter().find(|l| l.identifier == tile_matrix)?;
    if row < level.matrix_height && col < level.matrix_width {
        Some(level)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use std::path::PathBuf;

    /// 國土測繪中心 https://wmts.nlsc.gov.tw/97/wmts 的 EPSG:3826 格網（2026-10-10 取 GetCapabilities）：
    /// 原點 (50000, 3000000)、14 層、256×256；各層 (比例尺分母, MatrixWidth, MatrixHeight)。
    /// 若這個測試失敗，代表有人改了設定檔的 TWD97 格網；請先執行 scripts/verify_tms_nlsc.py 對照官方服務。
    const NLSC_3826: [(f64, u32, u32); 14] = [
        (11811779.57593862, 1, 1), (9449423.660750896, 1, 1), (4724711.830375448, 2, 2), (2362355.915187724, 3, 4),
        (944942.3660750897, 8, 9), (472471.18303754483, 15, 18), (236235.59151877242, 29, 36), (94494.23660750895, 72, 90),
        (47247.118303754476, 143, 179), (23623.559151877238, 285, 358), (9449.423660750896, 711, 895),
        (4724.711830375448, 1422, 1790), (2362.355915187724, 2844, 3579), (944.9423660750896, 7109, 8946),
    ];

    #[test]
    fn twd97_grid_matches_nlsc_official() {
        for file in ["config/layers.toml", "config/layers.local.toml"] {
            let cfg = AppConfig::load(&PathBuf::from(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
            let tms = cfg.tilematrixsets.iter().find(|t| t.identifier == "TWD97_TM2_NLSC").expect("TWD97_TM2_NLSC");
            assert_eq!(tms.crs, "urn:ogc:def:crs:EPSG::3826");
            assert_eq!(tms.top_left_corner, [50000.0, 3000000.0], "{file}");
            let lv = resolve_levels(tms);
            assert_eq!(lv.len(), NLSC_3826.len(), "{file}");
            for (l, (sd, w, h)) in lv.iter().zip(NLSC_3826) {
                assert!((l.scale_denominator - sd).abs() / sd < 1e-9, "{file} z{}: {} vs {}", l.identifier, l.scale_denominator, sd);
                assert_eq!((l.matrix_width, l.matrix_height), (w, h), "{file} z{}", l.identifier);
            }
        }
    }
}
