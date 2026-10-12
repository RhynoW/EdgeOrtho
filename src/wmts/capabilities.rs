use crate::config::AppConfig;
use crate::tilematrix::resolve_levels;
use crate::xmlutil::escape;
use serde_json::json;

/// 產生符合 OGC WMTS 1.0.0 規範的 GetCapabilities XML。
/// 同時揭露 KVP 與 RESTful 兩種介接方式（透過 OperationsMetadata + ResourceURL）。
pub fn build_capabilities_xml(cfg: &AppConfig) -> String {
    let base = &cfg.server.public_base_url;
    let kvp_endpoint = format!("{base}/wmts");
    let restful_base = format!("{base}/wmts/1.0.0");

    let mut layers_xml = String::new();
    for layer in &cfg.layers {
        let bbox = layer.bbox_wgs84;
        let mut tms_links = String::new();
        let mut resource_urls = String::new();
        for tms_id in &layer.tilematrixsets {
            tms_links.push_str(&format!(
                "      <TileMatrixSetLink>\n        <TileMatrixSet>{tms}</TileMatrixSet>\n      </TileMatrixSetLink>\n",
                tms = escape(tms_id)
            ));
            resource_urls.push_str(&format!(
                "      <ResourceURL format=\"{fmt}\" resourceType=\"tile\" template=\"{restful_base}/{layer}/default/{tms}/{{TileMatrix}}/{{TileRow}}/{{TileCol}}.png\"/>\n",
                fmt = escape(&layer.format),
                restful_base = restful_base,
                layer = escape(&layer.identifier),
                tms = escape(tms_id),
            ));
        }

        layers_xml.push_str(&format!(
            r#"    <Layer>
      <ows:Title>{title}</ows:Title>
      <ows:Abstract>{abstract_text}</ows:Abstract>
      <ows:Identifier>{identifier}</ows:Identifier>
      <ows:WGS84BoundingBox>
        <ows:LowerCorner>{minx} {miny}</ows:LowerCorner>
        <ows:UpperCorner>{maxx} {maxy}</ows:UpperCorner>
      </ows:WGS84BoundingBox>
      <Style isDefault="true">
        <ows:Identifier>default</ows:Identifier>
      </Style>
      <Format>{format}</Format>
{tms_links}{resource_urls}    </Layer>
"#,
            title = escape(&layer.title),
            abstract_text = escape(layer.abstract_text.as_deref().unwrap_or("")),
            identifier = escape(&layer.identifier),
            minx = bbox[0],
            miny = bbox[1],
            maxx = bbox[2],
            maxy = bbox[3],
            format = escape(&layer.format),
            tms_links = tms_links,
            resource_urls = resource_urls,
        ));
    }

    let mut tms_defs = String::new();
    for tms in &cfg.tilematrixsets {
        let levels = resolve_levels(tms);
        let mut matrices = String::new();
        for lvl in &levels {
            matrices.push_str(&format!(
                r#"      <TileMatrix>
        <ows:Identifier>{id}</ows:Identifier>
        <ScaleDenominator>{scale}</ScaleDenominator>
        <TopLeftCorner>{tlx} {tly}</TopLeftCorner>
        <TileWidth>{tw}</TileWidth>
        <TileHeight>{th}</TileHeight>
        <MatrixWidth>{mw}</MatrixWidth>
        <MatrixHeight>{mh}</MatrixHeight>
      </TileMatrix>
"#,
                id = escape(&lvl.identifier),
                scale = lvl.scale_denominator,
                tlx = lvl.top_left_corner[0],
                tly = lvl.top_left_corner[1],
                tw = lvl.tile_width,
                th = lvl.tile_height,
                mw = lvl.matrix_width,
                mh = lvl.matrix_height,
            ));
        }
        let wkss = tms
            .well_known_scale_set
            .as_ref()
            .map(|w| format!("      <WellKnownScaleSet>{}</WellKnownScaleSet>\n", escape(w)))
            .unwrap_or_default();

        tms_defs.push_str(&format!(
            r#"    <TileMatrixSet>
      <ows:Identifier>{id}</ows:Identifier>
      <ows:SupportedCRS>{crs}</ows:SupportedCRS>
{wkss}{matrices}    </TileMatrixSet>
"#,
            id = escape(&tms.identifier),
            crs = escape(&tms.crs),
            wkss = wkss,
            matrices = matrices,
        ));
    }

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Capabilities xmlns="http://www.opengis.net/wmts/1.0"
              xmlns:ows="http://www.opengis.net/ows/1.1"
              xmlns:xlink="http://www.w3.org/1999/xlink"
              xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
              xsi:schemaLocation="http://www.opengis.net/wmts/1.0 http://schemas.opengis.net/wmts/1.0/wmtsGetCapabilities_response.xsd"
              version="1.0.0">
  <ows:ServiceIdentification>
    <ows:Title>無人機正射影像圖磚服務</ows:Title>
    <ows:ServiceType>OGC WMTS</ows:ServiceType>
    <ows:ServiceTypeVersion>1.0.0</ows:ServiceTypeVersion>
  </ows:ServiceIdentification>
  <ows:OperationsMetadata>
    <ows:Operation name="GetCapabilities">
      <ows:DCP>
        <ows:HTTP>
          <ows:Get xlink:href="{kvp_endpoint}?">
            <ows:Constraint name="GetEncoding">
              <ows:AllowedValues><ows:Value>KVP</ows:Value></ows:AllowedValues>
            </ows:Constraint>
          </ows:Get>
          <ows:Get xlink:href="{restful_base}/WMTSCapabilities.xml">
            <ows:Constraint name="GetEncoding">
              <ows:AllowedValues><ows:Value>RESTful</ows:Value></ows:AllowedValues>
            </ows:Constraint>
          </ows:Get>
        </ows:HTTP>
      </ows:DCP>
    </ows:Operation>
    <ows:Operation name="GetTile">
      <ows:DCP>
        <ows:HTTP>
          <ows:Get xlink:href="{kvp_endpoint}?">
            <ows:Constraint name="GetEncoding">
              <ows:AllowedValues><ows:Value>KVP</ows:Value></ows:AllowedValues>
            </ows:Constraint>
          </ows:Get>
          <ows:Get xlink:href="{restful_base}/">
            <ows:Constraint name="GetEncoding">
              <ows:AllowedValues><ows:Value>RESTful</ows:Value></ows:AllowedValues>
            </ows:Constraint>
          </ows:Get>
        </ows:HTTP>
      </ows:DCP>
    </ows:Operation>
  </ows:OperationsMetadata>
  <Contents>
{layers_xml}{tms_defs}  </Contents>
  <ServiceMetadataURL xlink:href="{restful_base}/WMTSCapabilities.xml"/>
</Capabilities>
"#,
        kvp_endpoint = kvp_endpoint,
        restful_base = restful_base,
        layers_xml = layers_xml,
        tms_defs = tms_defs,
    )
}

/// 需求文件額外要求 GetCapabilities 需同時提供 JSON 詮釋資料（OGC WMTS 標準本身
/// 未定義 JSON 表示法，此為對接現代 Web 前端 / SDK 而設的擴充表示）。
pub fn build_capabilities_json(cfg: &AppConfig) -> serde_json::Value {
    let base = &cfg.server.public_base_url;
    let restful_base = format!("{base}/wmts/1.0.0");
    let kvp_endpoint = format!("{base}/wmts");

    let layers: Vec<_> = cfg
        .layers
        .iter()
        .map(|layer| {
            let tile_urls: serde_json::Map<String, serde_json::Value> = layer
                .tilematrixsets
                .iter()
                .map(|tms| {
                    (
                        tms.clone(),
                        json!({
                            "kvp": format!("{kvp_endpoint}?SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0&LAYER={}&STYLE=default&TILEMATRIXSET={tms}&TILEMATRIX={{TileMatrix}}&TILEROW={{TileRow}}&TILECOL={{TileCol}}&FORMAT={}", layer.identifier, layer.format),
                            "restful": format!("{restful_base}/{}/default/{tms}/{{TileMatrix}}/{{TileRow}}/{{TileCol}}.png", layer.identifier),
                        }),
                    )
                })
                .collect();
            json!({
                "identifier": layer.identifier,
                "title": layer.title,
                "abstract": layer.abstract_text,
                "format": layer.format,
                "wgs84BoundingBox": layer.bbox_wgs84,
                "tileMatrixSets": layer.tilematrixsets,
                "tileUrlTemplates": tile_urls,
            })
        })
        .collect();

    let tile_matrix_sets: Vec<_> = cfg
        .tilematrixsets
        .iter()
        .map(|tms| {
            let levels = resolve_levels(tms);
            json!({
                "identifier": tms.identifier,
                "supportedCRS": tms.crs,
                "wellKnownScaleSet": tms.well_known_scale_set,
                "tileWidth": tms.tile_width,
                "tileHeight": tms.tile_height,
                "tileMatrices": levels.iter().map(|l| json!({
                    "identifier": l.identifier,
                    "scaleDenominator": l.scale_denominator,
                    "topLeftCorner": l.top_left_corner,
                    "matrixWidth": l.matrix_width,
                    "matrixHeight": l.matrix_height,
                    "resolution": l.resolution,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();

    json!({
        "serviceIdentification": {
            "title": "無人機正射影像圖磚服務",
            "serviceType": "OGC WMTS",
            "serviceTypeVersion": "1.0.0",
        },
        "endpoints": {
            "kvp": kvp_endpoint,
            "restful": restful_base,
            "getCapabilitiesXml": format!("{restful_base}/WMTSCapabilities.xml"),
            "getCapabilitiesJson": format!("{restful_base}/WMTSCapabilities.json"),
        },
        "layers": layers,
        "tileMatrixSets": tile_matrix_sets,
    })
}
