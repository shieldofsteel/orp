use crate::ast::*;
use crate::parser::parse_orpql;
use orp_storage::traits::{Storage, StorageError};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::Arc;

/// Classification of query type for routing to the appropriate executor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryType {
    /// Analytical query — routes to DuckDB
    Analytical,
    /// Graph traversal query — routes to Kuzu
    Graph,
    /// Hybrid query — both DuckDB and Kuzu
    Hybrid,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct QueryResult {
    pub rows: Vec<HashMap<String, JsonValue>>,
    pub columns: Vec<String>,
    pub execution_time_ms: f64,
    pub row_count: usize,
}

/// Query plan step for cost-based optimization
#[derive(Debug, Clone)]
pub enum PlanStep {
    /// Scan entities by type
    EntityScan {
        entity_type: Option<String>,
        estimated_rows: u64,
    },
    /// Apply filter predicates
    Filter {
        conditions: Vec<Condition>,
    },
    /// Apply geospatial filter (optimized path)
    GeoFilter {
        lat: f64,
        lon: f64,
        radius_km: f64,
        entity_type: Option<String>,
    },
    /// Project specific columns
    Project {
        expressions: Vec<ReturnExpr>,
    },
    /// Aggregate functions
    Aggregate {
        functions: Vec<ReturnExpr>,
    },
    /// Sort results
    Sort {
        field: String,
        ascending: bool,
    },
    /// Limit results
    Limit {
        count: usize,
    },
}

/// Simple query planner that generates an execution plan
pub struct QueryPlanner;

impl QueryPlanner {
    /// Generate an optimized execution plan from a parsed query
    pub fn plan(query: &Query) -> Vec<PlanStep> {
        let mut steps = Vec::new();
        let pattern = &query.match_clause.patterns[0];
        let entity_type = pattern.entity.entity_type.clone();

        // Check if we can use geospatial index first (most selective)
        let mut has_geo_filter = false;
        if let Some(ref wc) = query.where_clause {
            for cond in &wc.conditions {
                if let Condition::Near {
                    lat,
                    lon,
                    radius_km,
                    ..
                } = cond
                {
                    steps.push(PlanStep::GeoFilter {
                        lat: *lat,
                        lon: *lon,
                        radius_km: *radius_km,
                        entity_type: entity_type.clone(),
                    });
                    has_geo_filter = true;
                    break;
                }
            }
        }

        // Entity scan (skipped if we used geo filter)
        if !has_geo_filter {
            steps.push(PlanStep::EntityScan {
                entity_type: entity_type.clone(),
                estimated_rows: 10000, // Default estimate
            });
        }

        // Apply non-geo filters
        if let Some(ref wc) = query.where_clause {
            let non_geo: Vec<Condition> = wc
                .conditions
                .iter()
                .filter(|c| !matches!(c, Condition::Near { .. }))
                .cloned()
                .collect();
            if !non_geo.is_empty() {
                steps.push(PlanStep::Filter {
                    conditions: non_geo,
                });
            }
        }

        // Check for aggregation vs projection
        let has_aggregate = query.return_clause.expressions.iter().any(|e| {
            matches!(
                e,
                ReturnExpr::Function { name, .. }
                    if matches!(name.as_str(), "COUNT" | "SUM" | "AVG" | "MIN" | "MAX")
            )
        });

        if has_aggregate {
            steps.push(PlanStep::Aggregate {
                functions: query.return_clause.expressions.clone(),
            });
        } else {
            steps.push(PlanStep::Project {
                expressions: query.return_clause.expressions.clone(),
            });
        }

        // Sort
        if let Some(ref order) = query.order_by {
            steps.push(PlanStep::Sort {
                field: order.field.clone(),
                ascending: order.ascending,
            });
        }

        // Limit
        if let Some(limit) = query.limit {
            steps.push(PlanStep::Limit { count: limit });
        }

        steps
    }
}

pub struct QueryExecutor {
    storage: Arc<dyn Storage>,
}

impl QueryExecutor {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self { storage }
    }

    /// Execute an ORP-QL query string
    pub async fn execute(&self, query_str: &str) -> Result<QueryResult, StorageError> {
        let start = std::time::Instant::now();

        let query =
            parse_orpql(query_str).map_err(|e| StorageError::QueryError(e.to_string()))?;

        // Generate plan (for future optimization)
        let _plan = QueryPlanner::plan(&query);

        let mut rows = self.execute_query(&query).await?;

        // Apply ORDER BY in Rust as a fallback (DuckDB already provides ORDER BY
        // on the entity scan; this handles cross-property sort on projected rows).
        if let Some(ref order) = query.order_by {
            let field = &order.field;
            let short_field = field.split('.').next_back().unwrap_or(field).to_string();
            rows.sort_by(|a, b| {
                let va = a
                    .get(field)
                    .or_else(|| a.get(&short_field))
                    .cloned()
                    .unwrap_or(JsonValue::Null);
                let vb = b
                    .get(field)
                    .or_else(|| b.get(&short_field))
                    .cloned()
                    .unwrap_or(JsonValue::Null);
                let cmp = compare_json(&va, &vb);
                if order.ascending {
                    cmp
                } else {
                    cmp.reverse()
                }
            });
        }

        // Apply LIMIT
        if let Some(limit) = query.limit {
            rows.truncate(limit);
        }

        let columns = if let Some(first) = rows.first() {
            first.keys().cloned().collect()
        } else {
            extract_column_names(&query.return_clause)
        };

        let row_count = rows.len();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;

        Ok(QueryResult {
            rows,
            columns,
            execution_time_ms: elapsed,
            row_count,
        })
    }

    async fn execute_query(
        &self,
        query: &Query,
    ) -> Result<Vec<HashMap<String, JsonValue>>, StorageError> {
        let pattern = &query.match_clause.patterns[0];
        let entity_type = pattern.entity.entity_type.as_deref();
        let variable = &pattern.entity.variable;

        // Check for geospatial index optimization
        let mut geo_filter = None;
        if let Some(ref wc) = query.where_clause {
            for cond in &wc.conditions {
                if let Condition::Near {
                    lat,
                    lon,
                    radius_km,
                    ..
                } = cond
                {
                    geo_filter = Some((*lat, *lon, *radius_km));
                    break;
                }
            }
        }

        // Push LIMIT into the SQL query when possible to avoid fetching
        // more rows than needed. Use a generous upper bound when WHERE
        // filters will reduce the result set in Rust.
        let has_where_filters = query.where_clause.as_ref().is_some_and(|wc| {
            wc.conditions.iter().any(|c| !matches!(c, Condition::Near { .. }))
        });
        let fetch_limit = if has_where_filters {
            // Need to over-fetch because Rust-side filters will reduce the set
            10000usize
        } else {
            // No additional WHERE filters — we can push LIMIT directly
            query.limit.unwrap_or(10000)
        };

        // Get base entities - use geo index if available
        let entities = if let Some((lat, lon, radius)) = geo_filter {
            self.storage
                .get_entities_in_radius(lat, lon, radius, entity_type)
                .await?
        } else if let Some(etype) = entity_type {
            self.storage.get_entities_by_type(etype, fetch_limit, 0).await?
        } else {
            // No type specified — scan all active entities via search
            self.storage.search_entities("", None, fetch_limit).await?
        };

        // Apply WHERE filters
        let filtered: Vec<_> = entities
            .into_iter()
            .filter(|e| {
                // Apply property filters from entity pattern
                if !pattern.entity.properties.is_empty() {
                    for (key, expected) in &pattern.entity.properties {
                        let val = e.properties.get(key);
                        let matches = match (val, expected) {
                            (Some(v), Literal::String(s)) => {
                                v.as_str().is_some_and(|vs| vs == s)
                            }
                            (Some(v), Literal::Number(n)) => {
                                v.as_f64().is_some_and(|vn| (vn - n).abs() < f64::EPSILON)
                            }
                            (Some(v), Literal::Boolean(b)) => {
                                v.as_bool() == Some(*b)
                            }
                            _ => false,
                        };
                        if !matches {
                            return false;
                        }
                    }
                }

                if let Some(ref wc) = query.where_clause {
                    wc.conditions.iter().all(|c| eval_condition(c, e, variable))
                } else {
                    true
                }
            })
            .collect();

        // Check for aggregate functions
        let has_aggregate = query.return_clause.expressions.iter().any(|e| {
            matches!(
                e,
                ReturnExpr::Function { name, .. }
                    if matches!(name.as_str(), "COUNT" | "SUM" | "AVG" | "MIN" | "MAX")
            )
        });

        if has_aggregate {
            return Ok(vec![self.compute_aggregates(
                &filtered,
                &query.return_clause,
            )]);
        }

        // Build result rows from RETURN clause
        let rows: Vec<HashMap<String, JsonValue>> = filtered
            .iter()
            .map(|e| {
                let mut row = HashMap::new();
                for expr in &query.return_clause.expressions {
                    match expr {
                        ReturnExpr::Property {
                            variable: _,
                            property,
                            alias,
                        } => {
                            let key = alias.as_deref().unwrap_or(property);
                            let val = match property.as_str() {
                                "id" | "entity_id" => {
                                    JsonValue::String(e.entity_id.clone())
                                }
                                "name" => JsonValue::String(
                                    e.name.clone().unwrap_or_default(),
                                ),
                                "type" | "entity_type" => {
                                    JsonValue::String(e.entity_type.clone())
                                }
                                "confidence" => serde_json::json!(e.confidence),
                                "lat" | "latitude" => e
                                    .geometry
                                    .as_ref()
                                    .map(|g| serde_json::json!(g.lat))
                                    .unwrap_or(JsonValue::Null),
                                "lon" | "longitude" => e
                                    .geometry
                                    .as_ref()
                                    .map(|g| serde_json::json!(g.lon))
                                    .unwrap_or(JsonValue::Null),
                                other => e
                                    .properties
                                    .get(other)
                                    .cloned()
                                    .unwrap_or(JsonValue::Null),
                            };
                            row.insert(key.to_string(), val);
                        }
                        ReturnExpr::Variable { name, alias } => {
                            let key = alias.as_deref().unwrap_or(name);
                            row.insert(
                                key.to_string(),
                                serde_json::to_value(e).unwrap_or(JsonValue::Null),
                            );
                        }
                        ReturnExpr::Function { name, args: _, alias } => {
                            let key =
                                alias.as_deref().unwrap_or(name.as_str());
                            // DISTANCE function
                            if name == "DISTANCE" {
                                if e.geometry.is_some() {
                                    // Placeholder: return 0 for now
                                    row.insert(
                                        key.to_string(),
                                        serde_json::json!(0.0),
                                    );
                                } else {
                                    row.insert(key.to_string(), JsonValue::Null);
                                }
                            } else {
                                row.insert(key.to_string(), JsonValue::Null);
                            }
                        }
                    }
                }
                row
            })
            .collect();

        Ok(rows)
    }

    fn compute_aggregates(
        &self,
        entities: &[orp_proto::Entity],
        return_clause: &ReturnClause,
    ) -> HashMap<String, JsonValue> {
        let mut agg_row = HashMap::new();

        for expr in &return_clause.expressions {
            if let ReturnExpr::Function { name, args, alias } = expr {
                let key = alias.as_deref().unwrap_or(name.as_str());
                match name.as_str() {
                    "COUNT" => {
                        agg_row.insert(
                            key.to_string(),
                            JsonValue::Number(serde_json::Number::from(entities.len())),
                        );
                    }
                    "SUM" => {
                        if let Some(prop) = args.first() {
                            let prop_name = prop.split('.').next_back().unwrap_or(prop);
                            let sum: f64 = entities
                                .iter()
                                .filter_map(|e| {
                                    e.properties
                                        .get(prop_name)
                                        .and_then(|v| v.as_f64())
                                })
                                .sum();
                            agg_row.insert(key.to_string(), serde_json::json!(sum));
                        }
                    }
                    "AVG" => {
                        if let Some(prop) = args.first() {
                            let prop_name = prop.split('.').next_back().unwrap_or(prop);
                            let values: Vec<f64> = entities
                                .iter()
                                .filter_map(|e| {
                                    e.properties
                                        .get(prop_name)
                                        .and_then(|v| v.as_f64())
                                })
                                .collect();
                            let count = values.len().max(1) as f64;
                            let sum: f64 = values.iter().sum();
                            agg_row
                                .insert(key.to_string(), serde_json::json!(sum / count));
                        }
                    }
                    "MIN" => {
                        if let Some(prop) = args.first() {
                            let prop_name = prop.split('.').next_back().unwrap_or(prop);
                            let min = entities
                                .iter()
                                .filter_map(|e| {
                                    e.properties
                                        .get(prop_name)
                                        .and_then(|v| v.as_f64())
                                })
                                .fold(f64::INFINITY, f64::min);
                            if min.is_finite() {
                                agg_row
                                    .insert(key.to_string(), serde_json::json!(min));
                            } else {
                                agg_row.insert(key.to_string(), JsonValue::Null);
                            }
                        }
                    }
                    "MAX" => {
                        if let Some(prop) = args.first() {
                            let prop_name = prop.split('.').next_back().unwrap_or(prop);
                            let max = entities
                                .iter()
                                .filter_map(|e| {
                                    e.properties
                                        .get(prop_name)
                                        .and_then(|v| v.as_f64())
                                })
                                .fold(f64::NEG_INFINITY, f64::max);
                            if max.is_finite() {
                                agg_row
                                    .insert(key.to_string(), serde_json::json!(max));
                            } else {
                                agg_row.insert(key.to_string(), JsonValue::Null);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        agg_row
    }
}

/// Recursively evaluate a single condition against an entity.
fn eval_condition(cond: &Condition, e: &orp_proto::Entity, variable: &str) -> bool {
    match cond {
        Condition::Comparison { left, op, right } => {
            let prop_name = left
                .strip_prefix(&format!("{}.", variable))
                .unwrap_or(left);

            let val = e.properties.get(prop_name);

            // Check built-in properties
            let builtin_val = match prop_name {
                "id" | "entity_id" => Some(JsonValue::String(e.entity_id.clone())),
                "name" => e.name.as_ref().map(|n| JsonValue::String(n.clone())),
                "type" | "entity_type" => Some(JsonValue::String(e.entity_type.clone())),
                "confidence" => Some(serde_json::json!(e.confidence)),
                _ => None,
            };

            let actual = val.or(builtin_val.as_ref());

            match (actual, right) {
                (Some(v), Literal::Number(n)) => {
                    let entity_val = v.as_f64().unwrap_or(0.0);
                    match op {
                        ComparisonOp::Gt => entity_val > *n,
                        ComparisonOp::Lt => entity_val < *n,
                        ComparisonOp::Gte => entity_val >= *n,
                        ComparisonOp::Lte => entity_val <= *n,
                        ComparisonOp::Eq => (entity_val - n).abs() < f64::EPSILON,
                        ComparisonOp::Neq => (entity_val - n).abs() >= f64::EPSILON,
                        _ => true,
                    }
                }
                (Some(v), Literal::String(s)) => {
                    let entity_val = v.as_str().unwrap_or("");
                    match op {
                        ComparisonOp::Eq => entity_val == s,
                        ComparisonOp::Neq => entity_val != s,
                        ComparisonOp::Like => entity_val.contains(s.trim_matches('%')),
                        _ => true,
                    }
                }
                (Some(v), Literal::Boolean(b)) => {
                    let entity_val = v.as_bool().unwrap_or(false);
                    match op {
                        ComparisonOp::Eq => entity_val == *b,
                        ComparisonOp::Neq => entity_val != *b,
                        _ => true,
                    }
                }
                _ => true,
            }
        }
        Condition::Near {
            lat,
            lon,
            radius_km,
            ..
        } => {
            if let Some(ref geo) = e.geometry {
                haversine_km(geo.lat, geo.lon, *lat, *lon) <= *radius_km
            } else {
                false
            }
        }
        Condition::Within {
            min_lat,
            min_lon,
            max_lat,
            max_lon,
            ..
        } => {
            if let Some(ref geo) = e.geometry {
                geo.lat >= *min_lat
                    && geo.lat <= *max_lat
                    && geo.lon >= *min_lon
                    && geo.lon <= *max_lon
            } else {
                false
            }
        }
        Condition::And(a, b) => {
            eval_condition(a, e, variable) && eval_condition(b, e, variable)
        }
        Condition::Or(a, b) => {
            eval_condition(a, e, variable) || eval_condition(b, e, variable)
        }
    }
}

fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6371.0; // Earth radius in km
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    r * c
}

fn compare_json(a: &JsonValue, b: &JsonValue) -> std::cmp::Ordering {
    match (a.as_f64(), b.as_f64()) {
        (Some(fa), Some(fb)) => fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal),
        _ => {
            let sa = a.as_str().unwrap_or("");
            let sb = b.as_str().unwrap_or("");
            sa.cmp(sb)
        }
    }
}

fn extract_column_names(ret: &ReturnClause) -> Vec<String> {
    ret.expressions
        .iter()
        .map(|e| match e {
            ReturnExpr::Property {
                variable,
                property,
                alias,
            } => alias
                .clone()
                .unwrap_or_else(|| format!("{}.{}", variable, property)),
            ReturnExpr::Function { name, alias, .. } => {
                alias.clone().unwrap_or_else(|| name.clone())
            }
            ReturnExpr::Variable { name, alias } => {
                alias.clone().unwrap_or_else(|| name.clone())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orp_proto::{Entity, GeoPoint};
    use orp_storage::DuckDbStorage;

    async fn setup_test_storage() -> Arc<dyn Storage> {
        let storage = Arc::new(DuckDbStorage::new_in_memory().unwrap());

        for i in 0..10 {
            let mut props = HashMap::new();
            props.insert(
                "speed".to_string(),
                serde_json::json!(5.0 + i as f64 * 3.0),
            );
            props.insert(
                "mmsi".to_string(),
                serde_json::json!(format!("{:09}", 200000000 + i)),
            );
            props.insert(
                "ship_type".to_string(),
                serde_json::json!(if i % 2 == 0 { "container" } else { "tanker" }),
            );
            let entity = Entity {
                entity_id: format!("ship-{}", i),
                entity_type: "Ship".to_string(),
                name: Some(format!("Ship {}", i)),
                properties: props,
                geometry: Some(GeoPoint {
                    lat: 51.92 + i as f64 * 0.01,
                    lon: 4.47 + i as f64 * 0.005,
                    alt: None,
                }),
                ..Entity::default()
            };
            storage.insert_entity(&entity).await.unwrap();
        }

        storage
    }

    #[tokio::test]
    async fn test_simple_query_execution() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute("MATCH (s:Ship) WHERE s.speed > 20 RETURN s.id, s.name, s.speed")
            .await
            .unwrap();

        assert!(result.row_count > 0);
        assert!(result.execution_time_ms >= 0.0);
    }

    #[tokio::test]
    async fn test_count_aggregation() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute("MATCH (s:Ship) RETURN COUNT(s) as total")
            .await
            .unwrap();

        assert_eq!(result.row_count, 1);
        let count = result.rows[0].get("total").unwrap().as_u64().unwrap();
        assert_eq!(count, 10);
    }

    #[tokio::test]
    async fn test_avg_aggregation() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute("MATCH (s:Ship) RETURN AVG(s.speed) as avg_speed")
            .await
            .unwrap();

        assert_eq!(result.row_count, 1);
        let avg = result.rows[0]
            .get("avg_speed")
            .unwrap()
            .as_f64()
            .unwrap();
        assert!(avg > 0.0);
    }

    #[tokio::test]
    async fn test_sum_aggregation() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute("MATCH (s:Ship) RETURN SUM(s.speed) as total_speed")
            .await
            .unwrap();

        assert_eq!(result.row_count, 1);
        let sum = result.rows[0]
            .get("total_speed")
            .unwrap()
            .as_f64()
            .unwrap();
        assert!(sum > 0.0);
    }

    #[tokio::test]
    async fn test_min_max_aggregation() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);

        let result = executor
            .execute("MATCH (s:Ship) RETURN MIN(s.speed) as min_speed")
            .await
            .unwrap();
        let min = result.rows[0]
            .get("min_speed")
            .unwrap()
            .as_f64()
            .unwrap();
        assert!((min - 5.0).abs() < 0.01);

        let result = executor
            .execute("MATCH (s:Ship) RETURN MAX(s.speed) as max_speed")
            .await
            .unwrap();
        let max = result.rows[0]
            .get("max_speed")
            .unwrap()
            .as_f64()
            .unwrap();
        assert!((max - 32.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_near_query() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute(
                "MATCH (s:Ship) WHERE NEAR(s, lat=51.92, lon=4.47, radius_km=5) RETURN s.id, s.name",
            )
            .await
            .unwrap();

        assert!(result.row_count > 0);
    }

    #[tokio::test]
    async fn test_order_by_limit() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute(
                "MATCH (s:Ship) RETURN s.id, s.speed ORDER BY s.speed DESC LIMIT 3",
            )
            .await
            .unwrap();

        assert_eq!(result.row_count, 3);
        // Verify ordering
        if result.rows.len() >= 2 {
            let first_speed = result.rows[0]
                .get("speed")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let second_speed = result.rows[1]
                .get("speed")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            assert!(first_speed >= second_speed);
        }
    }

    #[tokio::test]
    async fn test_string_comparison() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute(
                r#"MATCH (s:Ship) WHERE s.ship_type = "container" RETURN s.id, s.ship_type"#,
            )
            .await
            .unwrap();

        assert!(result.row_count > 0);
        for row in &result.rows {
            let st = row
                .get("ship_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            assert_eq!(st, "container");
        }
    }

    #[tokio::test]
    async fn test_within_query() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute(
                "MATCH (s:Ship) WHERE WITHIN(s, min_lat=51.0, min_lon=4.0, max_lat=53.0, max_lon=5.0) RETURN s.id",
            )
            .await
            .unwrap();

        assert!(result.row_count > 0);
    }

    #[tokio::test]
    async fn test_empty_result() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute("MATCH (s:Ship) WHERE s.speed > 9999 RETURN s.id")
            .await
            .unwrap();

        assert_eq!(result.row_count, 0);
    }

    #[tokio::test]
    async fn test_query_planner() {
        let query = parse_orpql(
            "MATCH (s:Ship) WHERE s.speed > 20 AND NEAR(s, lat=51.92, lon=4.47, radius_km=50) RETURN s.id ORDER BY s.speed DESC LIMIT 10",
        )
        .unwrap();

        let plan = QueryPlanner::plan(&query);
        assert!(!plan.is_empty());

        // Should start with geo filter (most selective)
        assert!(matches!(plan[0], PlanStep::GeoFilter { .. }));
    }

    #[tokio::test]
    async fn test_graph_traversal_query() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);

        // Graph traversal queries should parse and execute (returning entities)
        let result = executor
            .execute(
                r#"MATCH (s:Ship)-[:HEADING_TO]->(p:Port {name: "Rotterdam"}) RETURN s.id, s.name"#,
            )
            .await
            .unwrap();
        // May return 0 results since we don't have actual graph relationships, but should not error
        assert!(result.execution_time_ms >= 0.0);
    }

    #[tokio::test]
    async fn test_gte_comparison() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute("MATCH (s:Ship) WHERE s.speed >= 5 RETURN s.id")
            .await
            .unwrap();
        assert_eq!(result.row_count, 10); // all ships have speed >= 5
    }

    #[tokio::test]
    async fn test_lte_comparison() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute("MATCH (s:Ship) WHERE s.speed <= 5 RETURN s.id")
            .await
            .unwrap();
        assert_eq!(result.row_count, 1); // Only ship-0 has speed 5.0
    }

    #[tokio::test]
    async fn test_neq_comparison() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute(r#"MATCH (s:Ship) WHERE s.ship_type != "container" RETURN s.id"#)
            .await
            .unwrap();
        assert!(result.row_count > 0);
        for row in &result.rows {
            let st = row.get("ship_type").and_then(|v| v.as_str()).unwrap_or("");
            assert_ne!(st, "container");
        }
    }

    #[tokio::test]
    async fn test_no_where_clause() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute("MATCH (s:Ship) RETURN s.id")
            .await
            .unwrap();
        assert_eq!(result.row_count, 10);
    }

    #[tokio::test]
    async fn test_return_builtin_properties() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute("MATCH (s:Ship) RETURN s.entity_id, s.name, s.confidence LIMIT 1")
            .await
            .unwrap();
        assert_eq!(result.row_count, 1);
        let row = &result.rows[0];
        assert!(row.contains_key("entity_id") || row.contains_key("id"));
    }

    #[tokio::test]
    async fn test_multiple_and_conditions() {
        let storage = setup_test_storage().await;
        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute(r#"MATCH (s:Ship) WHERE s.speed > 10 AND s.ship_type = "container" RETURN s.id"#)
            .await
            .unwrap();
        // Some ships should match
        for row in &result.rows {
            assert!(row.get("id").is_some() || row.get("entity_id").is_some());
        }
    }

    #[tokio::test]
    async fn test_planner_no_geo_filter() {
        let query = parse_orpql(
            "MATCH (s:Ship) WHERE s.speed > 20 RETURN s.id LIMIT 10",
        )
        .unwrap();
        let plan = QueryPlanner::plan(&query);
        assert!(matches!(plan[0], PlanStep::EntityScan { .. }));
    }

    #[tokio::test]
    async fn test_planner_aggregate() {
        let query = parse_orpql(
            "MATCH (s:Ship) RETURN COUNT(s) as total",
        )
        .unwrap();
        let plan = QueryPlanner::plan(&query);
        assert!(plan.iter().any(|s| matches!(s, PlanStep::Aggregate { .. })));
    }

    #[test]
    fn test_haversine_km() {
        let dist = haversine_km(51.9225, 4.4792, 52.3676, 4.9041);
        assert!((dist - 57.0).abs() < 5.0);
    }

    #[test]
    fn test_haversine_km_same_point() {
        let dist = haversine_km(51.0, 4.0, 51.0, 4.0);
        assert!(dist.abs() < 0.001);
    }

    #[test]
    fn test_compare_json_numbers() {
        let a = serde_json::json!(10.0);
        let b = serde_json::json!(20.0);
        assert_eq!(compare_json(&a, &b), std::cmp::Ordering::Less);
    }

    #[test]
    fn test_compare_json_strings() {
        let a = serde_json::json!("abc");
        let b = serde_json::json!("xyz");
        assert_eq!(compare_json(&a, &b), std::cmp::Ordering::Less);
    }
}
