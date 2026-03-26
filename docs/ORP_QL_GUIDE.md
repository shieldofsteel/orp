# ORP-QL Query Language Guide

ORP-QL is a purpose-built query language for reasoning about real-world entities. It combines SQL-style analytics with Cypher-style graph traversal, and is compiled to either DuckDB SQL or Kuzu Cypher by the query planner.

---

## Table of Contents

1. [Basics](#1-basics)
2. [MATCH Patterns](#2-match-patterns)
3. [WHERE Conditions](#3-where-conditions)
4. [Geospatial Queries](#4-geospatial-queries)
5. [Graph Traversal](#5-graph-traversal)
6. [Aggregations](#6-aggregations)
7. [Temporal Queries](#7-temporal-queries)
8. [Subqueries](#8-subqueries)
9. [Complete Examples](#9-complete-examples)
10. [Grammar Reference](#10-grammar-reference)

---

## 1. Basics

Every ORP-QL query has this structure:

```sql
MATCH <pattern>
[WHERE <conditions>]
RETURN <fields>
[ORDER BY <field> [ASC|DESC]]
[LIMIT <n>]
```

### Your First Query

```sql
MATCH (s:Ship)
RETURN s.name, s.speed
LIMIT 10
```

This returns the name and speed of up to 10 ships.

### Naming Aliases

The alias (e.g., `s`) is how you reference the entity in WHERE and RETURN:

```sql
MATCH (vessel:Ship)
WHERE vessel.speed > 15
RETURN vessel.name, vessel.entity_id, vessel.speed
```

### Returning the Full Entity

Use just the alias to return all properties:

```sql
MATCH (s:Ship)
WHERE s.speed > 20
RETURN s
LIMIT 5
```

---

## 2. MATCH Patterns

### Match a Single Entity Type

```sql
-- All ships
MATCH (s:Ship) RETURN s.name

-- All ports
MATCH (p:Port) RETURN p.name, p.country

-- All weather systems
MATCH (w:WeatherSystem) RETURN w.name, w.severity

-- Any entity type (no type filter)
MATCH (e) RETURN e.entity_id, e.entity_type, e.name LIMIT 100
```

### Match by Property in the Pattern

```sql
-- A specific ship by MMSI
MATCH (s:Ship {entity_id: "mmsi:353136000"})
RETURN s

-- All tanker ships
MATCH (s:Ship {ship_type: "tanker"})
RETURN s.name, s.speed

-- Critical weather systems
MATCH (w:WeatherSystem {severity: "CRITICAL"})
RETURN w.name, w.center, w.radius_km
```

---

## 3. WHERE Conditions

### Comparison Operators

```sql
-- Standard comparisons
MATCH (s:Ship)
WHERE s.speed > 20
  AND s.speed < 30
  AND s.ship_type = "cargo"
  AND s.flag != "PA"
RETURN s.name, s.speed, s.flag
```

| Operator | Meaning |
|----------|---------|
| `=` | Equal |
| `!=` | Not equal |
| `>` | Greater than |
| `>=` | Greater than or equal |
| `<` | Less than |
| `<=` | Less than or equal |
| `IN [...]` | Value in list |
| `NOT IN [...]` | Value not in list |
| `CONTAINS` | String contains substring |
| `STARTS WITH` | String prefix match |
| `IS NULL` | Property is absent |
| `IS NOT NULL` | Property is present |

```sql
-- IN operator
MATCH (s:Ship)
WHERE s.flag IN ["PA", "MH", "LR"]
RETURN s.name, s.flag

-- CONTAINS
MATCH (p:Port)
WHERE p.name CONTAINS "Rotterdam"
RETURN p

-- IS NULL (no destination set)
MATCH (s:Ship)
WHERE s.destination IS NULL
  AND s.speed > 5
RETURN s.name, s.position

-- Boolean logic
MATCH (s:Ship)
WHERE (s.speed > 20 OR s.ship_type = "fishing_vessel")
  AND NOT (s.flag = "XX")
RETURN s.name, s.speed, s.ship_type
```

### Confidence Filter

Every entity has a `confidence` score (0.0–1.0). Filter on data quality:

```sql
MATCH (s:Ship)
WHERE s.confidence > 0.8
  AND s.speed > 15
RETURN s.name, s.speed, s.confidence
```

---

## 4. Geospatial Queries

### near() — Radius Search

Find entities within a radius of a point:

```sql
-- Ships within 50km of Rotterdam
MATCH (s:Ship)
WHERE near(s.position, point(51.9225, 4.4792), 50km)
RETURN s.name, s.speed, distance(s.position, point(51.9225, 4.4792)) AS dist_km
ORDER BY dist_km
```

```sql
-- Points of interest near a ship
MATCH (s:Ship {entity_id: "mmsi:353136000"}), (p:Port)
WHERE near(p.position, s.position, 200km)
RETURN p.name, p.country, distance(p.position, s.position) AS dist_km
ORDER BY dist_km
```

### within() — Bounding Box or Polygon

```sql
-- Ships in North Sea bounding box
MATCH (s:Ship)
WHERE within(s.position, bbox(-5.0, 48.0, 10.0, 60.0))
RETURN s.name, s.entity_id

-- Entities within a custom polygon
MATCH (s:Ship)
WHERE within(s.position, polygon([
  [3.0, 51.0], [6.0, 51.0],
  [6.0, 53.0], [3.0, 53.0],
  [3.0, 51.0]
]))
RETURN s.name, s.speed
```

### distance() — Compute Distance

Use `distance()` in RETURN or ORDER BY:

```sql
MATCH (s:Ship)
WHERE near(s.position, point(51.9225, 4.4792), 100km)
RETURN s.name,
       s.speed,
       distance(s.position, point(51.9225, 4.4792)) AS km_from_rotterdam
ORDER BY km_from_rotterdam
LIMIT 20
```

### point() — Define a Geographic Point

```sql
-- point(latitude, longitude)
point(51.9225, 4.4792)      -- Rotterdam
point(48.8566, 2.3522)      -- Paris
point(-33.8688, 151.2093)   -- Sydney
```

### bbox() — Define a Bounding Box

```sql
-- bbox(min_lon, min_lat, max_lon, max_lat)
bbox(-5.0, 48.0, 10.0, 60.0)    -- North Sea + Channel
bbox(-80.0, 25.0, -65.0, 35.0)  -- Caribbean
```

---

## 5. Graph Traversal

### Basic Relationship Traversal

```sql
-- Ships heading to Rotterdam
MATCH (s:Ship)-[:HEADING_TO]->(p:Port)
WHERE p.name = "Rotterdam"
RETURN s.name, s.mmsi, s.eta, s.speed
ORDER BY s.eta
```

```sql
-- Ships docked at any port
MATCH (s:Ship)-[:DOCKED_AT]->(p:Port)
RETURN s.name, p.name AS port, s.docked_since
```

### Inbound Relationships

```sql
-- Organizations that own ships in the North Sea
MATCH (org:Organization)-[:OWNS]->(s:Ship)
WHERE within(s.position, bbox(-5, 48, 10, 60))
RETURN org.name, count(s) AS ships_in_area
ORDER BY ships_in_area DESC
```

### Two-Hop Traversal

```sql
-- Organizations whose ships are heading to congested ports
MATCH (org:Organization)-[:OWNS]->(s:Ship)-[:HEADING_TO]->(p:Port)
WHERE p.congestion > 0.7
RETURN org.name, s.name AS ship, p.name AS port, p.congestion
ORDER BY p.congestion DESC
```

### Three-Hop Traversal (Maximum in Phase 1)

```sql
-- Weather systems threatening ports where ships are heading
MATCH (s:Ship)-[:HEADING_TO]->(p:Port)<-[:THREATENS]-(w:WeatherSystem)
WHERE w.severity IN ["WARNING", "CRITICAL"]
RETURN s.name AS ship, p.name AS destination, w.name AS storm, w.severity
ORDER BY w.severity DESC
```

### Undirected Relationships

Use `-[]-` (no direction arrows) to match in either direction:

```sql
-- All entities related to a specific port
MATCH (e)-[]-(p:Port {name: "Rotterdam"})
RETURN e.entity_type, e.name
```

### Ships Near Other Ships (NEAR relationship)

```sql
-- Vessels following the same ship
MATCH (s1:Ship {entity_id: "mmsi:353136000"})<-[:NEAR]-(s2:Ship)
RETURN s2.name, s2.mmsi, s2.speed
```

### Relationship Properties in WHERE

```sql
-- Ships with ETA within 6 hours
MATCH (s:Ship)-[r:HEADING_TO]->(p:Port)
WHERE r.eta < now() + interval(6, hours)
RETURN s.name, p.name, r.eta, r.distance_km
ORDER BY r.eta
```

---

## 6. Aggregations

### count()

```sql
-- Count ships by type
MATCH (s:Ship)
WHERE within(s.position, bbox(-5, 48, 10, 60))
RETURN s.ship_type, count(*) AS vessel_count
GROUP BY s.ship_type
ORDER BY vessel_count DESC
```

### avg(), min(), max(), sum()

```sql
-- Average speed and count by flag state
MATCH (s:Ship)
WHERE s.speed > 0
RETURN s.flag,
       count(*) AS vessel_count,
       avg(s.speed) AS avg_speed_kn,
       max(s.speed) AS max_speed_kn,
       min(s.speed) AS min_speed_kn
GROUP BY s.flag
ORDER BY vessel_count DESC
LIMIT 20
```

### collect() — Aggregate into List

```sql
-- Ports with list of ships heading to them
MATCH (s:Ship)-[:HEADING_TO]->(p:Port)
RETURN p.name,
       count(s) AS inbound_ships,
       collect(s.name) AS ship_names
GROUP BY p.name
ORDER BY inbound_ships DESC
LIMIT 10
```

### HAVING Clause

Filter on aggregated results:

```sql
MATCH (s:Ship)-[:HEADING_TO]->(p:Port)
RETURN p.name, count(s) AS inbound_ships
GROUP BY p.name
HAVING count(s) > 10
ORDER BY inbound_ships DESC
```

---

## 7. Temporal Queries

### AT TIME — Point in Time

Query the state of entities at a past timestamp:

```sql
-- Where was a ship 6 hours ago?
MATCH (s:Ship {entity_id: "mmsi:353136000"})
AT TIME now() - interval(6, hours)
RETURN s.position, s.speed, s.heading
```

```sql
-- All ships in Rotterdam yesterday at noon
MATCH (s:Ship)
AT TIME timestamp("2026-03-25T12:00:00Z")
WHERE near(s.position, point(51.9225, 4.4792), 50km)
RETURN s.name, s.speed
```

### Time Functions

```sql
now()                        -- current time
today()                      -- today at midnight UTC
interval(6, hours)           -- duration
interval(2, days)
interval(30, minutes)
timestamp("2026-03-26T12:00:00Z")  -- specific time
```

### SINCE — Events in a Time Window

```sql
-- Position changes for a ship in the last 12 hours
MATCH (s:Ship {entity_id: "mmsi:353136000"})
RETURN s.position, s.speed, s.event_time
SINCE now() - interval(12, hours)
ORDER BY s.event_time DESC
```

---

## 8. Subqueries

Use `WITH` to chain query stages (like SQL CTEs):

```sql
-- Find congested ports, then find ships heading there
MATCH (p:Port)
WHERE p.congestion > 0.75
WITH p

MATCH (s:Ship)-[:HEADING_TO]->(p)
WHERE s.speed > 10
RETURN p.name, p.congestion, s.name AS inbound_ship, s.eta
ORDER BY p.congestion DESC, s.eta
```

```sql
-- Find ships in stormy areas, then check their destinations
MATCH (s:Ship), (w:WeatherSystem)
WHERE w.severity = "CRITICAL"
  AND near(s.position, w.center, w.radius_km)
WITH s, w

MATCH (s)-[:HEADING_TO]->(p:Port)
RETURN s.name, w.name AS storm, p.name AS destination, s.eta
ORDER BY s.eta
```

---

## 9. Complete Examples

### Example 1: Maritime Situational Awareness Dashboard Query

What's happening in the North Sea right now?

```sql
MATCH (s:Ship)
WHERE within(s.position, bbox(-5.0, 48.0, 10.0, 60.0))
  AND s.speed > 0
  AND s.confidence > 0.7
RETURN s.entity_id,
       s.name,
       s.ship_type,
       s.flag,
       s.speed,
       s.course,
       s.destination,
       s.position
ORDER BY s.updated_at DESC
LIMIT 500
```

### Example 2: Port Congestion Impact Analysis

Which organizations have ships heading to congested ports?

```sql
MATCH (org:Organization)-[:OWNS]->(s:Ship)-[r:HEADING_TO]->(p:Port)
WHERE p.congestion > 0.7
  AND r.eta < now() + interval(24, hours)
RETURN org.name,
       count(DISTINCT s) AS affected_ships,
       collect(DISTINCT p.name) AS congested_ports,
       avg(p.congestion) AS avg_congestion
GROUP BY org.name
ORDER BY affected_ships DESC
LIMIT 20
```

### Example 3: Storm Risk Assessment

Which ships are at risk from the current storm system?

```sql
MATCH (s:Ship), (w:WeatherSystem)
WHERE w.severity IN ["WARNING", "CRITICAL"]
  AND near(s.position, w.center, w.radius_km * 1.5)
  AND s.speed > 3
RETURN s.name,
       s.entity_id,
       s.ship_type,
       w.name AS storm,
       w.severity,
       distance(s.position, w.center) AS dist_to_center_km,
       w.radius_km AS storm_radius_km
ORDER BY dist_to_center_km
```

### Example 4: Route Efficiency Analysis

Are ships taking efficient routes to Rotterdam?

```sql
MATCH (s:Ship)-[r:HEADING_TO]->(p:Port {name: "Rotterdam"})
WHERE r.distance_km IS NOT NULL
WITH s, r,
     distance(s.position, p.position) AS straight_line_km,
     r.distance_km AS planned_route_km

RETURN s.name,
       s.ship_type,
       straight_line_km,
       planned_route_km,
       (planned_route_km / straight_line_km) AS route_efficiency_ratio
WHERE planned_route_km > straight_line_km * 1.3  -- 30% longer than straight line
ORDER BY route_efficiency_ratio DESC
```

### Example 5: Fleet Position Report for an Organization

All vessels owned by Maersk, their positions, and status:

```sql
MATCH (org:Organization {name: "Maersk"})-[:OWNS]->(s:Ship)
OPTIONAL MATCH (s)-[:HEADING_TO]->(p:Port)
OPTIONAL MATCH (s)-[:DOCKED_AT]->(dp:Port)
RETURN s.name,
       s.mmsi,
       s.ship_type,
       s.position,
       s.speed,
       CASE
         WHEN dp IS NOT NULL THEN "docked at " + dp.name
         WHEN p IS NOT NULL THEN "heading to " + p.name
         ELSE "at sea"
       END AS status,
       s.updated_at
ORDER BY s.name
```

### Example 6: Historical Position Trail

Replay a ship's voyage over the last 7 days:

```sql
MATCH (s:Ship {entity_id: "mmsi:353136000"})
SINCE now() - interval(7, days)
RETURN s.position,
       s.speed,
       s.course,
       s.event_time
ORDER BY s.event_time ASC
```

### Example 7: Suspicious Behavior Detection

Ships that have been near the same location for more than 6 hours without being docked:

```sql
MATCH (s:Ship)
WHERE s.speed < 1.0
  AND NOT EXISTS { MATCH (s)-[:DOCKED_AT]->() }
WITH s, s.position AS anchor

MATCH (s)
AT TIME now() - interval(6, hours)
WHERE near(s.position, anchor, 2km)

RETURN s.name,
       s.entity_id,
       s.ship_type,
       s.flag,
       anchor AS stationary_position,
       s.confidence
ORDER BY s.updated_at DESC
```

### Example 8: Cross-Domain: Ships + Weather + Ports

Full situational picture for a maritime authority:

```sql
MATCH (s:Ship)-[:HEADING_TO]->(p:Port)
WHERE near(s.position, p.position, 500km)
OPTIONAL MATCH (w:WeatherSystem)
WHERE w.severity = "CRITICAL"
  AND near(s.position, w.center, w.radius_km)
RETURN p.name AS destination_port,
       p.congestion,
       count(DISTINCT s) AS inbound_ships,
       collect(DISTINCT CASE WHEN w IS NOT NULL THEN w.name ELSE NULL END) AS active_storms,
       avg(s.speed) AS avg_approach_speed
GROUP BY p.name, p.congestion
ORDER BY p.congestion DESC
```

### Example 9: Vessel Traffic Service Query

All ships in the approaches to a strait (e.g., English Channel):

```sql
MATCH (s:Ship)
WHERE within(s.position, polygon([
  [-2.0, 50.5], [2.5, 50.5],
  [2.5, 51.5], [-2.0, 51.5],
  [-2.0, 50.5]
]))
  AND s.speed > 3
RETURN s.name,
       s.ship_type,
       s.flag,
       s.speed,
       s.course,
       s.destination,
       s.length,
       s.beam
ORDER BY s.speed DESC
```

### Example 10: Aggregate Statistics for Reporting

Monthly port traffic summary:

```sql
MATCH (s:Ship)-[r:HEADING_TO]->(p:Port)
WHERE r.eta >= timestamp("2026-03-01T00:00:00Z")
  AND r.eta < timestamp("2026-04-01T00:00:00Z")
RETURN p.name,
       p.country,
       count(*) AS expected_arrivals,
       count(DISTINCT s.flag) AS flag_diversity,
       avg(s.length) AS avg_vessel_length_m,
       sum(CASE WHEN s.ship_type = "cargo" THEN 1 ELSE 0 END) AS cargo_vessels,
       sum(CASE WHEN s.ship_type = "tanker" THEN 1 ELSE 0 END) AS tankers
GROUP BY p.name, p.country
ORDER BY expected_arrivals DESC
LIMIT 20
```

### Example 11: Network Centrality — Busiest Ports

Which ports are the most connected hubs?

```sql
MATCH (p:Port)
WITH p,
  size { MATCH ()-[:HEADING_TO]->(p) } AS inbound_count,
  size { MATCH ()-[:DOCKED_AT]->(p) } AS docked_count,
  size { MATCH ()-[:OPERATES]->(p) } AS operator_count
RETURN p.name,
       p.country,
       inbound_count,
       docked_count,
       operator_count,
       inbound_count + docked_count AS connectivity_score
ORDER BY connectivity_score DESC
LIMIT 10
```

---

## 10. Grammar Reference

```ebnf
(* Top-level query structure *)
query         = match_clause, { with_clause, match_clause },
                [ where_clause ],
                return_clause,
                [ order_clause ],
                [ limit_clause ] ;

(* Clauses *)
match_clause  = "MATCH", pattern, { ",", pattern }
              | "OPTIONAL", "MATCH", pattern ;
where_clause  = "WHERE", condition, { ( "AND" | "OR" ), condition } ;
return_clause = "RETURN", return_item, { ",", return_item } ;
with_clause   = "WITH", return_item, { ",", return_item } ;
order_clause  = "ORDER", "BY", order_item, { ",", order_item } ;
limit_clause  = "LIMIT", integer ;
group_clause  = "GROUP", "BY", alias, ".", property, { ",", alias, ".", property } ;
having_clause = "HAVING", condition ;

(* Patterns *)
pattern         = node_pattern, { rel_pattern, node_pattern } ;
node_pattern    = "(", alias, [ ":", type ], [ properties ], ")" ;
rel_pattern     = ( "-[" | "<-[" ), [ ":", rel_type ], "]->", "-"
                | "-[", [ ":", rel_type ], "]-" ;
properties      = "{", property_kv, { ",", property_kv }, "}" ;
property_kv     = identifier, ":", literal ;

(* Conditions *)
condition       = property_filter
                | geo_filter
                | temporal_filter
                | exist_filter
                | "(", condition, ")" ;
property_filter = alias, ".", property, op, value
                | alias, ".", property, "IN", "[", value_list, "]"
                | alias, ".", property, ( "IS" "NULL" | "IS" "NOT" "NULL" )
                | alias, ".", property, "CONTAINS", string_literal
                | alias, ".", property, "STARTS", "WITH", string_literal ;
geo_filter      = ( "near" | "within" ), "(", alias, ".", "position", ",",
                  geo_expr, [ ",", distance ], ")" ;
temporal_filter = "AT", "TIME", time_expr
                | "SINCE", time_expr ;
exist_filter    = [ "NOT" ], "EXISTS", "{", match_clause, "}" ;

(* Geo expressions *)
geo_expr        = "point", "(", float, ",", float, ")"
                | "bbox", "(", float, ",", float, ",", float, ",", float, ")"
                | "polygon", "(", "[", coordinate_pair, { ",", coordinate_pair }, "]", ")" ;
distance        = float, ( "km" | "m" | "mi" ) ;
coordinate_pair = "[", float, ",", float, "]" ;

(* Time expressions *)
time_expr       = "now", "(", ")"
                | "today", "(", ")"
                | "timestamp", "(", string_literal, ")"
                | time_expr, ( "+" | "-" ), interval_expr ;
interval_expr   = "interval", "(", integer, ",", time_unit, ")" ;
time_unit       = "seconds" | "minutes" | "hours" | "days" | "weeks" ;

(* Aggregation functions *)
agg_fn          = "count", "(", ( "*" | alias ), ")"
                | "avg", "(", alias, ".", property, ")"
                | "sum", "(", alias, ".", property, ")"
                | "min", "(", alias, ".", property, ")"
                | "max", "(", alias, ".", property, ")"
                | "collect", "(", alias, ".", property, ")" ;

(* Scalar functions *)
scalar_fn       = "distance", "(", geo_expr, ",", geo_expr, ")"
                | "near", "(", geo_expr, ",", geo_expr, ",", distance, ")"
                | "within", "(", geo_expr, ",", geo_expr, ")" ;

(* Operators *)
op              = "=" | "!=" | ">" | ">=" | "<" | "<=" ;
```

---

## Tips and Gotchas

**1. Use LIMIT in exploratory queries.** Without LIMIT, a MATCH on all ships can return millions of rows.

**2. Graph queries max out at 3 hops in Phase 1.** Deep graph traversal (> 3 hops) requires the API's raw Cypher endpoint.

**3. Geospatial functions are DuckDB-only.** If you combine `near()` with a graph pattern, the planner runs DuckDB first, then feeds entity IDs to Kuzu.

**4. The `AT TIME` clause applies to the DuckDB events table.** Historical entity state is reconstructed from the event log. Queries over long time windows may be slower.

**5. Use `confidence` filters for critical decisions.** Sources vary in reliability. Filter `WHERE s.confidence > 0.8` when you need high-quality data.

**6. String comparisons are case-sensitive.** `s.flag = "PA"` won't match `"pa"`. Use `lower(s.flag) = "pa"` for case-insensitive matching.

---

_For API integration, see [API_REFERENCE.md](API_REFERENCE.md)._
_For language grammar specification, see [ARCHITECTURE.md](../ARCHITECTURE.md#61-orp-ql-grammar-v01-ebnf)._
