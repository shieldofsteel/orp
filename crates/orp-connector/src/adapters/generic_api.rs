//! Generic REST API connector — plug any HTTP/JSON data source into ORP via YAML config.
//!
//! Supports:
//! - Arbitrary REST endpoints with configurable polling
//! - JSONPath-style field mapping to ORP entity fields
//! - Authentication: API key (header/query), OAuth2 bearer token
//! - Pagination: offset/page-based and cursor-based
//! - Built-in templates: Shodan, VirusTotal, AbuseIPDB, FlightAware, OpenWeatherMap, USGS

use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Configuration types (deserialized from YAML / JSON properties)
// ---------------------------------------------------------------------------

/// Authentication method for the API.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[derive(Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    /// No authentication.
    #[default]
    None,
    /// Fixed API key sent as an HTTP header.
    ApiKeyHeader { header: String, key: String },
    /// Fixed API key appended as a query parameter.
    ApiKeyQuery { param: String, key: String },
    /// Bearer token (static or refreshed via OAuth2 client-credentials).
    Bearer { token: String },
}

/// Pagination strategy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[derive(Default)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum PaginationConfig {
    /// No pagination — single request per poll cycle.
    #[default]
    None,
    /// Offset / limit pagination (e.g. `?offset=0&limit=100`).
    Offset {
        offset_param: String,
        limit_param: String,
        page_size: u64,
        /// JSONPath to the "next page exists" boolean or total count field.
        /// If omitted the connector fetches until an empty page.
        total_field: Option<String>,
    },
    /// Page-number pagination (e.g. `?page=1&per_page=50`).
    Page {
        page_param: String,
        size_param: String,
        page_size: u64,
        total_pages_field: Option<String>,
    },
    /// Cursor / token-based pagination.
    Cursor {
        cursor_param: String,
        next_cursor_field: String,
    },
}

/// Describes how to locate the array of items inside a JSON response and how
/// to map each item's fields to ORP entity fields.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MappingConfig {
    /// Dot-separated path to the items array inside the JSON response.
    /// e.g. `"matches"`, `"data.features"`, `""` (root array).
    #[serde(default)]
    pub items_path: String,

    /// Field in each item whose value becomes the `entity_id`.
    pub id_field: String,

    /// Optional field that holds the latitude value.
    pub lat_field: Option<String>,

    /// Optional field that holds the longitude value.
    pub lon_field: Option<String>,

    /// Optional field that holds an ISO-8601 timestamp; falls back to `Utc::now()`.
    pub timestamp_field: Option<String>,

    /// Optional override for `entity_type` from a field value in the item.
    pub entity_type_field: Option<String>,

    /// Fields to include in the entity's `properties` map.
    /// If empty, all fields are included.
    #[serde(default)]
    pub include_fields: Vec<String>,

    /// Fields to always exclude from `properties`.
    #[serde(default)]
    pub exclude_fields: Vec<String>,

    /// Static extra properties merged into every entity produced by this connector.
    #[serde(default)]
    pub static_properties: HashMap<String, JsonValue>,
}

impl Default for MappingConfig {
    fn default() -> Self {
        Self {
            items_path: String::new(),
            id_field: "id".to_string(),
            lat_field: None,
            lon_field: None,
            timestamp_field: None,
            entity_type_field: None,
            include_fields: vec![],
            exclude_fields: vec![],
            static_properties: HashMap::new(),
        }
    }
}

/// Full configuration for the generic API connector.
/// Stored serialised in `ConnectorConfig::properties["generic_api"]`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenericApiConfig {
    /// Base URL (may contain `{page}`, `{offset}`, `{cursor}` placeholders).
    pub url: String,

    /// Additional HTTP headers sent with every request.
    #[serde(default)]
    pub headers: HashMap<String, String>,

    /// Authentication configuration.
    #[serde(default)]
    pub auth: AuthConfig,

    /// Polling interval in seconds.
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,

    /// Pagination configuration.
    #[serde(default)]
    pub pagination: PaginationConfig,

    /// Field mapping from JSON response → ORP entity.
    #[serde(default)]
    pub mapping: MappingConfig,

    /// Optional request timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_poll_interval() -> u64 {
    60
}
fn default_timeout() -> u64 {
    30
}

// ---------------------------------------------------------------------------
// Built-in templates
// ---------------------------------------------------------------------------

/// Return a ready-made `GenericApiConfig` for well-known APIs.
/// Pass the template name as a string; api_key is injected into the auth block.
pub fn builtin_template(name: &str, api_key: &str) -> Option<GenericApiConfig> {
    match name {
        // ── Shodan ──────────────────────────────────────────────────────────
        "shodan" => Some(GenericApiConfig {
            url: format!(
                "https://api.shodan.io/shodan/host/search?key={api_key}&query=port:22"
            ),
            headers: HashMap::new(),
            auth: AuthConfig::None, // key embedded in URL
            poll_interval_secs: 300,
            pagination: PaginationConfig::Page {
                page_param: "page".to_string(),
                size_param: "minify".to_string(), // Shodan uses minify, not size
                page_size: 100,
                total_pages_field: Some("total".to_string()),
            },
            mapping: MappingConfig {
                items_path: "matches".to_string(),
                id_field: "ip_str".to_string(),
                lat_field: Some("location.latitude".to_string()),
                lon_field: Some("location.longitude".to_string()),
                timestamp_field: Some("timestamp".to_string()),
                include_fields: vec![
                    "ip_str".to_string(),
                    "port".to_string(),
                    "org".to_string(),
                    "hostnames".to_string(),
                    "os".to_string(),
                    "vulns".to_string(),
                    "product".to_string(),
                    "version".to_string(),
                ],
                static_properties: {
                    let mut m = HashMap::new();
                    m.insert("source".to_string(), serde_json::json!("shodan"));
                    m
                },
                ..Default::default()
            },
            timeout_secs: 30,
        }),

        // ── VirusTotal ──────────────────────────────────────────────────────
        "virustotal" => Some(GenericApiConfig {
            url: "https://www.virustotal.com/api/v3/intelligence/search?query=type:file".to_string(),
            headers: HashMap::new(),
            auth: AuthConfig::ApiKeyHeader {
                header: "x-apikey".to_string(),
                key: api_key.to_string(),
            },
            poll_interval_secs: 600,
            pagination: PaginationConfig::Cursor {
                cursor_param: "cursor".to_string(),
                next_cursor_field: "meta.cursor".to_string(),
            },
            mapping: MappingConfig {
                items_path: "data".to_string(),
                id_field: "id".to_string(),
                lat_field: None,
                lon_field: None,
                timestamp_field: Some("attributes.last_analysis_date".to_string()),
                entity_type_field: Some("type".to_string()),
                include_fields: vec![
                    "id".to_string(),
                    "type".to_string(),
                    "attributes.meaningful_name".to_string(),
                    "attributes.sha256".to_string(),
                    "attributes.last_analysis_stats".to_string(),
                    "attributes.names".to_string(),
                ],
                static_properties: {
                    let mut m = HashMap::new();
                    m.insert("source".to_string(), serde_json::json!("virustotal"));
                    m
                },
                ..Default::default()
            },
            timeout_secs: 30,
        }),

        // ── AbuseIPDB ───────────────────────────────────────────────────────
        "abuseipdb" => Some(GenericApiConfig {
            url: "https://api.abuseipdb.com/api/v2/blacklist?confidenceMinimum=90&limit=10000"
                .to_string(),
            headers: {
                let mut h = HashMap::new();
                h.insert("Accept".to_string(), "application/json".to_string());
                h
            },
            auth: AuthConfig::ApiKeyHeader {
                header: "Key".to_string(),
                key: api_key.to_string(),
            },
            poll_interval_secs: 3600,
            pagination: PaginationConfig::None,
            mapping: MappingConfig {
                items_path: "data".to_string(),
                id_field: "ipAddress".to_string(),
                lat_field: None,
                lon_field: None,
                timestamp_field: Some("lastReportedAt".to_string()),
                include_fields: vec![
                    "ipAddress".to_string(),
                    "abuseConfidenceScore".to_string(),
                    "countryCode".to_string(),
                    "usageType".to_string(),
                    "isp".to_string(),
                    "domain".to_string(),
                    "totalReports".to_string(),
                ],
                static_properties: {
                    let mut m = HashMap::new();
                    m.insert("source".to_string(), serde_json::json!("abuseipdb"));
                    m.insert("threat_type".to_string(), serde_json::json!("malicious_ip"));
                    m
                },
                ..Default::default()
            },
            timeout_secs: 60,
        }),

        // ── FlightAware AeroAPI ─────────────────────────────────────────────
        "flightaware" => Some(GenericApiConfig {
            url: "https://aeroapi.flightaware.com/aeroapi/flights/search?query=-latlong \"25 -130 50 -60\"".to_string(),
            headers: HashMap::new(),
            auth: AuthConfig::ApiKeyHeader {
                header: "x-apikey".to_string(),
                key: api_key.to_string(),
            },
            poll_interval_secs: 60,
            pagination: PaginationConfig::Cursor {
                cursor_param: "cursor".to_string(),
                next_cursor_field: "next".to_string(),
            },
            mapping: MappingConfig {
                items_path: "flights".to_string(),
                id_field: "fa_flight_id".to_string(),
                lat_field: Some("last_position.latitude".to_string()),
                lon_field: Some("last_position.longitude".to_string()),
                timestamp_field: Some("last_position.timestamp".to_string()),
                include_fields: vec![
                    "fa_flight_id".to_string(),
                    "ident".to_string(),
                    "aircraft_type".to_string(),
                    "origin.code".to_string(),
                    "destination.code".to_string(),
                    "status".to_string(),
                    "last_position.altitude".to_string(),
                    "last_position.groundspeed".to_string(),
                    "last_position.heading".to_string(),
                ],
                static_properties: {
                    let mut m = HashMap::new();
                    m.insert("source".to_string(), serde_json::json!("flightaware"));
                    m
                },
                ..Default::default()
            },
            timeout_secs: 30,
        }),

        // ── OpenWeatherMap ──────────────────────────────────────────────────
        "openweathermap" => Some(GenericApiConfig {
            url: format!(
                "https://api.openweathermap.org/data/2.5/find?lat=0&lon=0&cnt=50&appid={api_key}"
            ),
            headers: HashMap::new(),
            auth: AuthConfig::None,
            poll_interval_secs: 600,
            pagination: PaginationConfig::None,
            mapping: MappingConfig {
                items_path: "list".to_string(),
                id_field: "id".to_string(),
                lat_field: Some("coord.lat".to_string()),
                lon_field: Some("coord.lon".to_string()),
                timestamp_field: Some("dt".to_string()),
                include_fields: vec![
                    "name".to_string(),
                    "weather".to_string(),
                    "main.temp".to_string(),
                    "main.humidity".to_string(),
                    "wind.speed".to_string(),
                    "wind.deg".to_string(),
                    "clouds.all".to_string(),
                    "visibility".to_string(),
                ],
                static_properties: {
                    let mut m = HashMap::new();
                    m.insert("source".to_string(), serde_json::json!("openweathermap"));
                    m
                },
                ..Default::default()
            },
            timeout_secs: 30,
        }),

        // ── USGS Earthquake Feed ────────────────────────────────────────────
        "usgs" => Some(GenericApiConfig {
            url:
                "https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/significant_hour.geojson"
                    .to_string(),
            headers: HashMap::new(),
            auth: AuthConfig::None,
            poll_interval_secs: 300,
            pagination: PaginationConfig::None,
            mapping: MappingConfig {
                items_path: "features".to_string(),
                id_field: "id".to_string(),
                // GeoJSON geometry.coordinates: [lon, lat, depth]
                lat_field: Some("geometry.coordinates[1]".to_string()),
                lon_field: Some("geometry.coordinates[0]".to_string()),
                timestamp_field: Some("properties.time".to_string()),
                include_fields: vec![
                    "properties.mag".to_string(),
                    "properties.place".to_string(),
                    "properties.status".to_string(),
                    "properties.alert".to_string(),
                    "properties.tsunami".to_string(),
                    "properties.sig".to_string(),
                ],
                static_properties: {
                    let mut m = HashMap::new();
                    m.insert("source".to_string(), serde_json::json!("usgs"));
                    m
                },
                ..Default::default()
            },
            timeout_secs: 30,
        }),

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// JSON path resolution helper
// ---------------------------------------------------------------------------

/// Navigate a dot-separated path (e.g. `"a.b.c"`) into a JSON value.
/// Supports simple bracket notation like `"arr[0]"`.
pub fn resolve_path<'a>(value: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    if path.is_empty() {
        return Some(value);
    }
    let mut current = value;
    for segment in path.split('.') {
        // Handle array index: `foo[0]`
        if let Some(bracket_pos) = segment.find('[') {
            let field = &segment[..bracket_pos];
            let idx_str = segment
                .get(bracket_pos + 1..segment.len().saturating_sub(1))
                .unwrap_or("0");
            let idx: usize = idx_str.parse().unwrap_or(0);
            if !field.is_empty() {
                current = current.get(field)?;
            }
            current = current.get(idx)?;
        } else {
            current = current.get(segment)?;
        }
    }
    Some(current)
}

/// Resolve a path to an `f64`.
fn resolve_f64(value: &JsonValue, path: &str) -> Option<f64> {
    resolve_path(value, path)?.as_f64()
}

/// Resolve a path to a `String`.
fn resolve_string(value: &JsonValue, path: &str) -> Option<String> {
    let v = resolve_path(value, path)?;
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    if let Some(n) = v.as_i64() {
        return Some(n.to_string());
    }
    if let Some(n) = v.as_u64() {
        return Some(n.to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// Item → SourceEvent mapping
// ---------------------------------------------------------------------------

fn map_item(
    item: &JsonValue,
    mapping: &MappingConfig,
    entity_type: &str,
    connector_id: &str,
) -> Option<SourceEvent> {
    let raw_id = resolve_string(item, &mapping.id_field)?;
    if raw_id.is_empty() {
        return None;
    }

    let effective_entity_type = mapping
        .entity_type_field
        .as_deref()
        .and_then(|f| resolve_string(item, f))
        .unwrap_or_else(|| entity_type.to_string());

    let entity_id = format!("{}:{}", effective_entity_type, raw_id);

    let latitude = mapping.lat_field.as_deref().and_then(|f| resolve_f64(item, f));
    let longitude = mapping.lon_field.as_deref().and_then(|f| resolve_f64(item, f));

    let timestamp = mapping
        .timestamp_field
        .as_deref()
        .and_then(|f| resolve_path(item, f))
        .and_then(|v| {
            // Unix epoch seconds/milliseconds or ISO-8601 string
            if let Some(ts_ms) = v.as_i64() {
                // Heuristic: > 1e12 means milliseconds
                let secs = if ts_ms > 1_000_000_000_000 {
                    ts_ms / 1000
                } else {
                    ts_ms
                };
                chrono::DateTime::from_timestamp(secs, 0)
            } else if let Some(s) = v.as_str() {
                chrono::DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc))
            } else {
                None
            }
        })
        .unwrap_or_else(Utc::now);

    // Build properties map
    let mut properties: HashMap<String, JsonValue> = mapping.static_properties.clone();

    if let Some(obj) = item.as_object() {
        let include_all = mapping.include_fields.is_empty();
        for (k, v) in obj {
            let excluded = mapping.exclude_fields.iter().any(|e| e == k);
            if excluded {
                continue;
            }
            if include_all || mapping.include_fields.iter().any(|i| i == k) {
                properties.insert(k.clone(), v.clone());
            }
        }
    }

    // Include nested fields referenced by dot-paths in include_fields
    for field_path in &mapping.include_fields {
        if field_path.contains('.') || field_path.contains('[') {
            if let Some(v) = resolve_path(item, field_path) {
                let key = field_path.replace(['.', '['], "_").replace(']', "");
                properties.entry(key).or_insert_with(|| v.clone());
            }
        }
    }

    Some(SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type: effective_entity_type,
        properties,
        timestamp,
        latitude,
        longitude,
    })
}

/// Extract all items from a JSON response according to the mapping config.
pub fn extract_events(
    json: &JsonValue,
    mapping: &MappingConfig,
    entity_type: &str,
    connector_id: &str,
) -> Vec<SourceEvent> {
    // Locate the items array
    let items: Vec<JsonValue> = if mapping.items_path.is_empty() {
        if let Some(arr) = json.as_array() {
            arr.clone()
        } else {
            vec![json.clone()]
        }
    } else if let Some(v) = resolve_path(json, &mapping.items_path) {
        if let Some(arr) = v.as_array() {
            arr.clone()
        } else {
            vec![v.clone()]
        }
    } else {
        vec![]
    };

    items
        .iter()
        .filter_map(|item| map_item(item, mapping, entity_type, connector_id))
        .collect()
}

// ---------------------------------------------------------------------------
// HTTP client helpers
// ---------------------------------------------------------------------------

fn build_request(
    client: &reqwest::Client,
    url: &str,
    auth: &AuthConfig,
    extra_headers: &HashMap<String, String>,
) -> reqwest::RequestBuilder {
    let mut req = client.get(url);

    // Auth
    match auth {
        AuthConfig::None => {}
        AuthConfig::Bearer { token } => {
            req = req.bearer_auth(token);
        }
        AuthConfig::ApiKeyHeader { header, key } => {
            req = req.header(header.as_str(), key.as_str());
        }
        AuthConfig::ApiKeyQuery { param, key } => {
            req = req.query(&[(param.as_str(), key.as_str())]);
        }
    }

    // Extra headers
    for (k, v) in extra_headers {
        req = req.header(k.as_str(), v.as_str());
    }

    req
}

/// Fetch a single page and return the JSON body.
async fn fetch_page(
    client: &reqwest::Client,
    url: &str,
    api_cfg: &GenericApiConfig,
) -> Result<JsonValue, ConnectorError> {
    let req = build_request(client, url, &api_cfg.auth, &api_cfg.headers);
    let resp = req.send().await.map_err(|e| {
        ConnectorError::ConnectionError(format!("HTTP request failed: {e}"))
    })?;

    if !resp.status().is_success() {
        return Err(ConnectorError::ConnectionError(format!(
            "HTTP {} from {url}",
            resp.status()
        )));
    }

    resp.json::<JsonValue>().await.map_err(|e| {
        ConnectorError::ParseError(format!("JSON parse error: {e}"))
    })
}

/// Fetch all pages according to the pagination strategy, collecting all
/// raw JSON responses.
async fn fetch_all_pages(
    client: &reqwest::Client,
    api_cfg: &GenericApiConfig,
) -> Result<Vec<JsonValue>, ConnectorError> {
    let mut responses = vec![];

    match &api_cfg.pagination {
        PaginationConfig::None => {
            let json = fetch_page(client, &api_cfg.url, api_cfg).await?;
            responses.push(json);
        }

        PaginationConfig::Offset {
            offset_param,
            limit_param,
            page_size,
            total_field,
        } => {
            let mut offset: u64 = 0;
            loop {
                let url = format!(
                    "{}&{}={}&{}={}",
                    api_cfg.url, offset_param, offset, limit_param, page_size
                );
                let json = fetch_page(client, &url, api_cfg).await?;

                // Determine whether there are more pages
                let items_count = if let Some(path) = total_field.as_deref() {
                    resolve_path(&json, path)
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                } else {
                    // Count items in the response
                    if let Some(arr) = resolve_path(&json, &api_cfg.mapping.items_path)
                        .and_then(|v| v.as_array())
                    {
                        arr.len() as u64
                    } else {
                        0
                    }
                };

                responses.push(json);
                offset += page_size;

                if items_count < *page_size {
                    break;
                }
                // Safety cap: never fetch more than 100 pages
                if offset / page_size > 100 {
                    break;
                }
            }
        }

        PaginationConfig::Page {
            page_param,
            size_param,
            page_size,
            total_pages_field,
        } => {
            let mut page: u64 = 1;
            loop {
                let url = format!(
                    "{}&{}={}&{}={}",
                    api_cfg.url, page_param, page, size_param, page_size
                );
                let json = fetch_page(client, &url, api_cfg).await?;

                let more_pages = if let Some(path) = total_pages_field.as_deref() {
                    resolve_path(&json, path)
                        .and_then(|v| v.as_u64())
                        .map(|total| page < total)
                        .unwrap_or(false)
                } else {
                    // Stop when page returns fewer items than page_size
                    resolve_path(&json, &api_cfg.mapping.items_path)
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.len() as u64 >= *page_size)
                        .unwrap_or(false)
                };

                responses.push(json);
                if !more_pages || page > 100 {
                    break;
                }
                page += 1;
            }
        }

        PaginationConfig::Cursor {
            cursor_param,
            next_cursor_field,
        } => {
            let mut cursor: Option<String> = None;
            let mut iterations = 0u32;
            loop {
                let url = if let Some(ref c) = cursor {
                    format!("{}&{}={}", api_cfg.url, cursor_param, c)
                } else {
                    api_cfg.url.clone()
                };
                let json = fetch_page(client, &url, api_cfg).await?;
                let next = resolve_path(&json, next_cursor_field)
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                responses.push(json);
                cursor = next.clone();
                iterations += 1;
                if cursor.is_none() || cursor.as_deref() == Some("") || iterations > 100 {
                    break;
                }
            }
        }
    }

    Ok(responses)
}

// ---------------------------------------------------------------------------
// Connector implementation
// ---------------------------------------------------------------------------

/// Universal REST API connector.
pub struct GenericApiConnector {
    config: ConnectorConfig,
    api_config: GenericApiConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl GenericApiConnector {
    /// Create a new connector.  
    /// `api_config` can be loaded from YAML or built via `builtin_template`.
    pub fn new(config: ConnectorConfig, api_config: GenericApiConfig) -> Self {
        Self {
            config,
            api_config,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Convenience: load a `GenericApiConfig` from a `serde_json::Value`
    /// stored in `ConnectorConfig::properties["generic_api"]`.
    pub fn from_connector_config(config: ConnectorConfig) -> Result<Self, ConnectorError> {
        let api_cfg_val = config
            .properties
            .get("generic_api")
            .ok_or_else(|| {
                ConnectorError::ConfigError(
                    "Missing 'generic_api' key in connector properties".to_string(),
                )
            })?;
        let api_config: GenericApiConfig =
            serde_json::from_value(api_cfg_val.clone()).map_err(|e| {
                ConnectorError::ConfigError(format!("Invalid generic_api config: {e}"))
            })?;
        Ok(Self::new(config, api_config))
    }

    /// Build from a named template plus API key.
    pub fn from_template(
        connector_id: &str,
        template: &str,
        api_key: &str,
        entity_type: &str,
    ) -> Result<Self, ConnectorError> {
        let api_config = builtin_template(template, api_key).ok_or_else(|| {
            ConnectorError::ConfigError(format!("Unknown template: {template}"))
        })?;
        let config = ConnectorConfig {
            connector_id: connector_id.to_string(),
            connector_type: format!("generic_api/{template}"),
            url: Some(api_config.url.clone()),
            entity_type: entity_type.to_string(),
            enabled: true,
            trust_score: 0.8,
            properties: {
                let mut m = HashMap::new();
                m.insert(
                    "generic_api".to_string(),
                    serde_json::to_value(&api_config)
                        .map_err(|e| ConnectorError::ConfigError(format!("Failed to serialise api_config: {e}")))?,
                );
                m
            },
        };
        Ok(Self::new(config, api_config))
    }
}

#[async_trait]
impl Connector for GenericApiConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        self.running.store(true, Ordering::SeqCst);
        tracing::info!(connector_id = %self.config.connector_id, "GenericApiConnector started");

        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();
        let api_config = self.api_config.clone();
        let connector_id = self.config.connector_id.clone();
        let entity_type = self.config.entity_type.clone();

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(api_config.timeout_secs))
            .user_agent("orp-connector/0.1")
            .build()
            .map_err(|e| ConnectorError::ConnectionError(e.to_string()))?;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(
                api_config.poll_interval_secs,
            ));

            while running.load(Ordering::SeqCst) {
                interval.tick().await;

                match fetch_all_pages(&client, &api_config).await {
                    Ok(pages) => {
                        for page in &pages {
                            let events = extract_events(
                                page,
                                &api_config.mapping,
                                &entity_type,
                                &connector_id,
                            );
                            for event in events {
                                if tx.send(event).await.is_err() {
                                    return; // channel closed
                                }
                                events_count.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            connector_id = %connector_id,
                            error = %e,
                            "GenericApiConnector fetch error"
                        );
                        errors_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(connector_id = %self.config.connector_id, "GenericApiConnector stopped");
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "GenericApiConnector not running".to_string(),
            ))
        }
    }

    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn stats(&self) -> ConnectorStats {
        use crate::traits::ConnectorStats;
        ConnectorStats {
            events_processed: self.events_count.load(Ordering::Relaxed),
            errors: self.errors_count.load(Ordering::Relaxed),
            last_event_timestamp: Some(Utc::now()),
            uptime_seconds: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── resolve_path ──────────────────────────────────────────────────────

    #[test]
    fn test_resolve_path_simple() {
        let v = json!({"a": {"b": 42}});
        assert_eq!(resolve_path(&v, "a.b"), Some(&json!(42)));
    }

    #[test]
    fn test_resolve_path_array_index() {
        let v = json!({"coords": [10.0, 20.0, 5.0]});
        assert_eq!(resolve_path(&v, "coords[0]"), Some(&json!(10.0)));
        assert_eq!(resolve_path(&v, "coords[1]"), Some(&json!(20.0)));
    }

    #[test]
    fn test_resolve_path_missing() {
        let v = json!({"a": 1});
        assert!(resolve_path(&v, "b.c").is_none());
    }

    #[test]
    fn test_resolve_path_empty_is_identity() {
        let v = json!({"x": 1});
        assert_eq!(resolve_path(&v, ""), Some(&json!({"x": 1})));
    }

    // ── extract_events ────────────────────────────────────────────────────

    #[test]
    fn test_extract_events_root_array() {
        let json = json!([
            {"id": "d1", "ip": "1.2.3.4", "port": 22},
            {"id": "d2", "ip": "5.6.7.8", "port": 443},
        ]);
        let mapping = MappingConfig {
            items_path: String::new(),
            id_field: "id".to_string(),
            ..Default::default()
        };
        let events = extract_events(&json, &mapping, "device", "test");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].entity_id, "device:d1");
        assert_eq!(events[1].entity_id, "device:d2");
    }

    #[test]
    fn test_extract_events_nested_path() {
        let json = json!({"matches": [
            {"ip_str": "10.0.0.1", "port": 22, "org": "ACME"},
        ]});
        let mapping = MappingConfig {
            items_path: "matches".to_string(),
            id_field: "ip_str".to_string(),
            ..Default::default()
        };
        let events = extract_events(&json, &mapping, "host", "shodan-1");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].entity_id, "host:10.0.0.1");
    }

    #[test]
    fn test_extract_events_with_lat_lon() {
        let json = json!([{"id": "q1", "lat": 35.6, "lon": 139.7}]);
        let mapping = MappingConfig {
            items_path: String::new(),
            id_field: "id".to_string(),
            lat_field: Some("lat".to_string()),
            lon_field: Some("lon".to_string()),
            ..Default::default()
        };
        let events = extract_events(&json, &mapping, "quake", "usgs-1");
        assert_eq!(events[0].latitude, Some(35.6));
        assert_eq!(events[0].longitude, Some(139.7));
    }

    #[test]
    fn test_extract_events_static_properties() {
        let json = json!([{"id": "x1"}]);
        let mut static_props = HashMap::new();
        static_props.insert("source".to_string(), json!("myapi"));
        let mapping = MappingConfig {
            items_path: String::new(),
            id_field: "id".to_string(),
            static_properties: static_props,
            ..Default::default()
        };
        let events = extract_events(&json, &mapping, "item", "c1");
        assert_eq!(events[0].properties["source"], json!("myapi"));
    }

    #[test]
    fn test_extract_events_exclude_fields() {
        let json = json!([{"id": "x1", "secret": "hidden", "name": "public"}]);
        let mapping = MappingConfig {
            items_path: String::new(),
            id_field: "id".to_string(),
            exclude_fields: vec!["secret".to_string()],
            ..Default::default()
        };
        let events = extract_events(&json, &mapping, "item", "c1");
        assert!(!events[0].properties.contains_key("secret"));
        assert!(events[0].properties.contains_key("name"));
    }

    #[test]
    fn test_extract_events_entity_type_from_field() {
        let json = json!([{"id": "f1", "type": "file"}]);
        let mapping = MappingConfig {
            items_path: String::new(),
            id_field: "id".to_string(),
            entity_type_field: Some("type".to_string()),
            ..Default::default()
        };
        let events = extract_events(&json, &mapping, "default", "c1");
        assert_eq!(events[0].entity_type, "file");
        assert_eq!(events[0].entity_id, "file:f1");
    }

    #[test]
    fn test_extract_events_unix_ms_timestamp() {
        let json = json!([{"id": "e1", "ts": 1_700_000_000_000i64}]);
        let mapping = MappingConfig {
            items_path: String::new(),
            id_field: "id".to_string(),
            timestamp_field: Some("ts".to_string()),
            ..Default::default()
        };
        let events = extract_events(&json, &mapping, "event", "c1");
        assert!(events[0].timestamp.timestamp() > 0);
    }

    // ── builtin_template ──────────────────────────────────────────────────

    #[test]
    fn test_builtin_template_shodan() {
        let cfg = builtin_template("shodan", "TESTKEY").unwrap();
        assert!(cfg.url.contains("shodan"));
        assert_eq!(cfg.mapping.id_field, "ip_str");
    }

    #[test]
    fn test_builtin_template_virustotal() {
        let cfg = builtin_template("virustotal", "TESTKEY").unwrap();
        matches!(cfg.auth, AuthConfig::ApiKeyHeader { .. });
    }

    #[test]
    fn test_builtin_template_usgs() {
        let cfg = builtin_template("usgs", "").unwrap();
        assert_eq!(cfg.mapping.items_path, "features");
    }

    #[test]
    fn test_builtin_template_unknown_returns_none() {
        assert!(builtin_template("nonexistent", "key").is_none());
    }

    #[test]
    fn test_all_builtin_templates_valid() {
        for name in &["shodan", "virustotal", "abuseipdb", "flightaware", "openweathermap", "usgs"] {
            let cfg = builtin_template(name, "dummy_key")
                .unwrap_or_else(|| panic!("template {name} should exist"));
            assert!(!cfg.url.is_empty(), "template {name} has empty URL");
            assert!(!cfg.mapping.id_field.is_empty(), "template {name} has empty id_field");
        }
    }

    // ── connector construction ────────────────────────────────────────────

    #[test]
    fn test_from_template_creates_connector() {
        let c = GenericApiConnector::from_template("usgs-1", "usgs", "", "earthquake").unwrap();
        assert_eq!(c.connector_id(), "usgs-1");
        assert_eq!(c.config().entity_type, "earthquake");
    }

    #[test]
    fn test_from_connector_config_missing_key_errors() {
        let config = ConnectorConfig {
            connector_id: "x".to_string(),
            connector_type: "generic_api".to_string(),
            url: None,
            entity_type: "item".to_string(),
            enabled: true,
            trust_score: 1.0,
            properties: HashMap::new(), // missing "generic_api"
        };
        assert!(GenericApiConnector::from_connector_config(config).is_err());
    }

    #[test]
    fn test_initial_stats_zero() {
        let c = GenericApiConnector::from_template("t1", "usgs", "", "quake").unwrap();
        let stats = c.stats();
        assert_eq!(stats.events_processed, 0);
        assert_eq!(stats.errors, 0);
    }

    #[tokio::test]
    async fn test_health_check_not_running() {
        let c = GenericApiConnector::from_template("t2", "usgs", "", "quake").unwrap();
        assert!(c.health_check().await.is_err());
    }

    #[tokio::test]
    async fn test_stop_before_start_is_ok() {
        let c = GenericApiConnector::from_template("t3", "usgs", "", "quake").unwrap();
        assert!(c.stop().await.is_ok());
    }
}
