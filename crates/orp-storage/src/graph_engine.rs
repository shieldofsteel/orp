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
    ("ship_nodes",         "SELECT * FROM graph_nodes WHERE node_type = 'ship'"),
    ("port_nodes",         "SELECT * FROM graph_nodes WHERE node_type = 'port'"),
    ("aircraft_nodes",     "SELECT * FROM graph_nodes WHERE node_type = 'aircraft'"),
    ("weather_nodes",      "SELECT * FROM graph_nodes WHERE node_type = 'weather'"),
    ("organization_nodes", "SELECT * FROM graph_nodes WHERE node_type = 'organization'"),
    ("route_nodes",        "SELECT * FROM graph_nodes WHERE node_type = 'route'"),
    ("sensor_nodes",       "SELECT * FROM graph_nodes WHERE node_type = 'sensor'"),
];

const GRAPH_EDGE_VIEWS: &[(&str, &str)] = &[
    ("docked_at",     "SELECT * FROM graph_edges WHERE edge_type = 'docked_at'"),
    ("heading_to",    "SELECT * FROM graph_edges WHERE edge_type = 'heading_to'"),
    ("owned_by",      "SELECT * FROM graph_edges WHERE edge_type = 'owned_by'"),
    ("managed_by",    "SELECT * FROM graph_edges WHERE edge_type = 'managed_by'"),
    ("insures",       "SELECT * FROM graph_edges WHERE edge_type = 'insures'"),
    ("threatens",     "SELECT * FROM graph_edges WHERE edge_type = 'threatens'"),
    ("near_rel",      "SELECT * FROM graph_edges WHERE edge_type = 'near'"),
    ("follows_route", "SELECT * FROM graph_edges WHERE edge_type = 'follows'"),
    ("traverse_rel",  "SELECT * FROM graph_edges WHERE edge_type = 'traverse'"),
    ("deployed_on",   "SELECT * FROM graph_edges WHERE edge_type = 'deployed_on'"),
    ("measures_at",   "SELECT * FROM graph_edges WHERE edge_type = 'measures_at'"),
    ("in_region",     "SELECT * FROM graph_edges WHERE edge_type = 'in_region'"),
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
        conn.execute_batch(r#"
            DELETE FROM graph_nodes
            WHERE node_id NOT IN (
                SELECT entity_id FROM entities WHERE is_active = TRUE
            );
        "#)
        .map_err(|e| StorageError::DatabaseError(format!("graph node prune: {e}")))?;

        // Sync edges (denormalise node types for speed).
        conn.execute_batch(r#"
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
        "#)
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
    ) -> StorageResult<(HashMap<String, Vec<EdgeRecord>>, HashMap<String, EdgeRecord>)> {
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
            out.entry(rec.from_node.clone()).or_default().push(rec.clone());
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
            m.insert("node_id".into(), JsonValue::String(row.get(0).unwrap_or_default()));
            m.insert("node_type".into(), opt_jstr(row.get::<_, Option<String>>(1).ok().flatten()));
            m.insert("label".into(), opt_jstr(row.get::<_, Option<String>>(2).ok().flatten()));

            let props_raw: String = row.get::<_, Option<String>>(3).ok().flatten().unwrap_or_default();
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
    /// Supported patterns:
    /// - `MATCH (n:ship) RETURN n`
    /// - `MATCH (n:ship) WHERE n.name = 'Ever Given' RETURN n`
    /// - `MATCH (a)-[r:docked_at]->(b) RETURN a, r, b`
    /// - `MATCH (a)-[r:docked_at]->(b) WHERE a.node_id = 'ship_x' RETURN a, b`
    /// - `MATCH (a)-[*..3]->(b) WHERE a.node_id = 'ship_x' RETURN b`
    /// - Raw SQL passthrough when no MATCH keyword is found
    pub fn cypher_query(
        &self,
        query: &str,
    ) -> StorageResult<Vec<HashMap<String, JsonValue>>> {
        let q = query.trim();

        if let Some(sql) = try_translate_cypher(q) {
            return self.execute_raw_sql(&sql);
        }

        // Passthrough: treat as raw SQL against the graph tables.
        self.execute_raw_sql(q)
    }

    /// Execute arbitrary SQL against the graph tables.
    pub fn execute_raw_sql(
        &self,
        sql: &str,
    ) -> StorageResult<Vec<HashMap<String, JsonValue>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| StorageError::QueryError(format!("prepare: {e}")))?;

        // Collect all row data first (positional), then map column names.
        let mut raw_rows: Vec<Vec<JsonValue>> = Vec::new();
        let mut col_count = 0usize;

        {
            let mut rows = stmt
                .query([])
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
                        if c > 50 { break; }
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

        let results: Vec<HashMap<String, JsonValue>> = raw_rows
            .into_iter()
            .map(|vals| {
                let mut m = HashMap::new();
                for (i, val) in vals.into_iter().enumerate() {
                    let name = col_names.get(i).cloned().unwrap_or_else(|| format!("col_{i}"));
                    m.insert(name, val);
                }
                m
            })
            .collect();

        Ok(results)
    }

    // ── Stats ─────────────────────────────────────────────────────────────────

    /// Return (node_count, edge_count).
    pub fn stats(&self) -> StorageResult<(u64, u64)> {
        let conn = self.conn.lock().unwrap();
        let nodes: i64 = conn
            .query_row("SELECT COUNT(*) FROM graph_nodes", [], |r| r.get(0))
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        let edges: i64 = conn
            .query_row("SELECT COUNT(*) FROM graph_edges WHERE is_active = TRUE", [], |r| r.get(0))
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok((nodes as u64, edges as u64))
    }
}

// ── Cypher translator ─────────────────────────────────────────────────────────

fn try_translate_cypher(q: &str) -> Option<String> {
    let upper = q.to_uppercase();
    if !upper.contains("MATCH") {
        return None;
    }

    // Pattern 1: MATCH (n:node_type) [WHERE ...] RETURN n
    if let Some(sql) = translate_single_node_match(q) {
        return Some(sql);
    }

    // Pattern 2: MATCH (a)-[r:edge_type]->(b) [WHERE ...] RETURN ...
    if let Some(sql) = translate_edge_match(q) {
        return Some(sql);
    }

    // Pattern 3: MATCH (a)-[*..N]->(b) WHERE a.node_id = 'x' RETURN b
    if let Some(sql) = translate_multihop_match(q) {
        return Some(sql);
    }

    None
}

fn translate_single_node_match(q: &str) -> Option<String> {
    let (alias, node_type) = parse_single_node_match(q)?;
    let table = node_type_to_table(&node_type);
    let where_clause = extract_where_for_alias(q, &alias);
    let limit = extract_limit(q).unwrap_or(1000);

    let sql = if let Some(wc) = where_clause {
        format!(
            "SELECT node_id, node_type, label, properties, latitude, longitude, confidence \
             FROM {table} WHERE {wc} LIMIT {limit}"
        )
    } else {
        format!(
            "SELECT node_id, node_type, label, properties, latitude, longitude, confidence \
             FROM {table} LIMIT {limit}"
        )
    };
    Some(sql)
}

fn translate_edge_match(q: &str) -> Option<String> {
    let upper = q.to_uppercase();
    // Must have ]->(  pattern
    if !upper.contains("]->(") && !upper.contains("]->" ) {
        return None;
    }
    // Extract edge type between : and ]
    let edge_type = extract_between(q, ":", "]")?;
    let edge_type = edge_type.trim().to_lowercase();
    if edge_type.is_empty() {
        return None;
    }

    let where_str = extract_edge_where(q);
    let limit = extract_limit(q).unwrap_or(1000);

    Some(format!(
        "SELECT src.node_id AS from_id, src.node_type AS from_type, src.label AS from_label, \
                e.edge_type, e.properties AS edge_props, e.confidence AS edge_confidence, \
                tgt.node_id AS to_id, tgt.node_type AS to_type, tgt.label AS to_label \
         FROM graph_edges e \
         JOIN graph_nodes src ON src.node_id = e.from_node \
         JOIN graph_nodes tgt ON tgt.node_id = e.to_node \
         WHERE e.edge_type = '{edge_type}' AND e.is_active = TRUE{where_str} \
         LIMIT {limit}"
    ))
}

fn translate_multihop_match(q: &str) -> Option<String> {
    let upper = q.to_uppercase();
    if !upper.contains("[*") {
        return None;
    }
    let hops_str = extract_between(q, "..", "]").unwrap_or_else(|| "3".into());
    let max_hops: i64 = hops_str.trim().parse().unwrap_or(3);
    let src_id = extract_id_from_where(q, "a")?;

    Some(format!(
        r#"WITH RECURSIVE multi_hop AS (
            SELECT e.from_node, e.to_node, e.edge_type, e.edge_id, 1 AS hop, [e.edge_id] AS visited
            FROM graph_edges e WHERE e.from_node = '{src_id}' AND e.is_active = TRUE
            UNION ALL
            SELECT e2.from_node, e2.to_node, e2.edge_type, e2.edge_id, mh.hop + 1,
                   list_append(mh.visited, e2.edge_id)
            FROM graph_edges e2
            JOIN multi_hop mh ON e2.from_node = mh.to_node
            WHERE e2.is_active = TRUE AND mh.hop < {max_hops}
              AND NOT list_contains(mh.visited, e2.edge_id)
        )
        SELECT DISTINCT n.node_id, n.node_type, n.label, n.properties,
               n.latitude, n.longitude, mh.hop AS depth
        FROM multi_hop mh
        JOIN graph_nodes n ON n.node_id = mh.to_node
        ORDER BY mh.hop LIMIT 500"#
    ))
}

// ── Small parsing helpers ─────────────────────────────────────────────────────

fn node_type_to_table(node_type: &str) -> &'static str {
    match node_type.to_lowercase().as_str() {
        "ship"                    => "ship_nodes",
        "port"                    => "port_nodes",
        "aircraft"                => "aircraft_nodes",
        "weather" | "weather_system" => "weather_nodes",
        "organization" | "org"   => "organization_nodes",
        "route"                   => "route_nodes",
        "sensor"                  => "sensor_nodes",
        _                         => "graph_nodes",
    }
}

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

fn extract_where_for_alias(q: &str, alias: &str) -> Option<String> {
    let upper = q.to_uppercase();
    let where_pos = upper.find(" WHERE ")?;
    let after = &q[where_pos + 7..];
    let clause = if let Some(ret_pos) = after.to_uppercase().find(" RETURN") {
        &after[..ret_pos]
    } else {
        after
    };
    let out = clause.replace(&format!("{}.", alias), "");
    Some(out.trim().to_string())
}

fn extract_edge_where(q: &str) -> String {
    let upper = q.to_uppercase();
    if let Some(where_pos) = upper.find(" WHERE ") {
        let after = &q[where_pos + 7..];
        let clause = if let Some(ret_pos) = after.to_uppercase().find(" RETURN") {
            &after[..ret_pos]
        } else {
            after
        };
        let out = clause
            .replace("a.", "src.")
            .replace("b.", "tgt.")
            .replace("r.", "e.");
        format!(" AND {}", out.trim())
    } else {
        String::new()
    }
}

fn extract_limit(q: &str) -> Option<usize> {
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

        let aircraft = engine.execute_raw_sql("SELECT * FROM aircraft_nodes").unwrap();
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
        assert!(paths_2.is_empty(), "should not find 3-hop path with max_hops=2");

        // max_hops=3: should find path
        let paths_3 = engine.path_query("a", "d", 3).unwrap();
        assert!(!paths_3.is_empty(), "should find 3-hop path with max_hops=3");
        assert_eq!(paths_3[0].len(), 3);
    }
}
