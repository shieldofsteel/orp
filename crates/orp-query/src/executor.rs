use crate::ast::*;
use crate::parser::parse_orpql;
use orp_storage::traits::{Storage, StorageError};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct QueryResult {
    pub rows: Vec<HashMap<String, JsonValue>>,
    pub columns: Vec<String>,
    pub execution_time_ms: f64,
    pub row_count: usize,
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

        let query = parse_orpql(query_str)
            .map_err(|e| StorageError::QueryError(e.to_string()))?;

        let mut rows = self.execute_query(&query).await?;

        // Apply ORDER BY
        if let Some(ref order) = query.order_by {
            rows.sort_by(|a, b| {
                let va = a.get(&order.field).cloned().unwrap_or(JsonValue::Null);
                let vb = b.get(&order.field).cloned().unwrap_or(JsonValue::Null);
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

        // Get base entities
        let entities = if let Some(etype) = entity_type {
            self.storage.get_entities_by_type(etype, 10000, 0).await?
        } else {
            self.storage.get_entities_by_type("ship", 10000, 0).await?
        };

        // Apply WHERE filters
        let filtered: Vec<_> = entities
            .into_iter()
            .filter(|e| {
                if let Some(ref wc) = query.where_clause {
                    wc.conditions.iter().all(|c| match c {
                        Condition::Comparison { left, op, right } => {
                            let prop_name = left
                                .strip_prefix(&format!("{}.", variable))
                                .unwrap_or(left);
                            let val = e.properties.get(prop_name);
                            match (val, right) {
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
                                        ComparisonOp::Like => {
                                            entity_val.contains(s.trim_matches('%'))
                                        }
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
                        _ => true,
                    })
                } else {
                    true
                }
            })
            .collect();

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
                                "id" | "entity_id" => JsonValue::String(e.entity_id.clone()),
                                "name" => JsonValue::String(
                                    e.name.clone().unwrap_or_default(),
                                ),
                                "type" | "entity_type" => {
                                    JsonValue::String(e.entity_type.clone())
                                }
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
                        ReturnExpr::Function { name, alias, .. } => {
                            let key =
                                alias.as_deref().unwrap_or(name.as_str());
                            // For aggregates, handled separately
                            row.insert(key.to_string(), JsonValue::Null);
                        }
                    }
                }
                row
            })
            .collect();

        // Handle aggregate functions
        let has_aggregate = query.return_clause.expressions.iter().any(|e| {
            matches!(e, ReturnExpr::Function { name, .. } if matches!(name.as_str(), "COUNT" | "SUM" | "AVG" | "MIN" | "MAX"))
        });

        if has_aggregate {
            let mut agg_row = HashMap::new();
            for expr in &query.return_clause.expressions {
                if let ReturnExpr::Function { name, args, alias } = expr {
                    let key = alias.as_deref().unwrap_or(name.as_str());
                    match name.as_str() {
                        "COUNT" => {
                            agg_row.insert(
                                key.to_string(),
                                JsonValue::Number(serde_json::Number::from(filtered.len())),
                            );
                        }
                        "AVG" => {
                            if let Some(prop) = args.first() {
                                let prop_name = prop
                                    .split('.')
                                    .last()
                                    .unwrap_or(prop);
                                let sum: f64 = filtered
                                    .iter()
                                    .filter_map(|e| {
                                        e.properties
                                            .get(prop_name)
                                            .and_then(|v| v.as_f64())
                                    })
                                    .sum();
                                let count = filtered.len().max(1) as f64;
                                agg_row.insert(
                                    key.to_string(),
                                    serde_json::json!(sum / count),
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            return Ok(vec![agg_row]);
        }

        Ok(rows)
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
            ReturnExpr::Variable { name, alias } => alias.clone().unwrap_or_else(|| name.clone()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orp_proto::{Entity, GeoPoint};
    use orp_storage::DuckDbStorage;

    #[tokio::test]
    async fn test_simple_query_execution() {
        let storage = Arc::new(DuckDbStorage::new_in_memory().unwrap());

        // Insert test data
        for i in 0..5 {
            let mut props = HashMap::new();
            props.insert("speed".to_string(), serde_json::json!(10.0 + i as f64 * 5.0));
            let entity = Entity {
                entity_id: format!("ship-{}", i),
                entity_type: "Ship".to_string(),
                name: Some(format!("Ship {}", i)),
                properties: props,
                geometry: Some(GeoPoint {
                    lat: 51.92 + i as f64 * 0.01,
                    lon: 4.47,
                    alt: None,
                }),
                ..Entity::default()
            };
            storage.insert_entity(&entity).await.unwrap();
        }

        let executor = QueryExecutor::new(storage);
        let result = executor
            .execute("MATCH (s:Ship) WHERE s.speed > 20 RETURN s.id, s.name, s.speed")
            .await
            .unwrap();

        assert!(result.row_count > 0);
    }
}
