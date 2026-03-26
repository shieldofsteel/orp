use crate::ast::*;
use nom::{
    branch::alt,
    bytes::complete::{tag, tag_no_case, take_while1},
    character::complete::{char, multispace0, multispace1},
    combinator::{map, opt},
    multi::separated_list0,
    number::complete::double,
    sequence::{delimited, preceded, tuple},
    IResult,
};

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Parse error at: {0}")]
    SyntaxError(String),
    #[error("Unexpected token: {0}")]
    UnexpectedToken(String),
}

/// Parse an ORP-QL query string into an AST
pub fn parse_orpql(input: &str) -> Result<Query, ParseError> {
    match parse_query(input.trim()) {
        Ok((_, query)) => Ok(query),
        Err(e) => Err(ParseError::SyntaxError(format!("{}", e))),
    }
}

fn parse_query(input: &str) -> IResult<&str, Query> {
    let (input, _) = multispace0(input)?;
    let (input, match_clause) = parse_match_clause(input)?;
    let (input, _) = multispace0(input)?;
    let (input, where_clause) = opt(parse_where_clause)(input)?;
    let (input, _) = multispace0(input)?;
    let (input, return_clause) = parse_return_clause(input)?;
    let (input, _) = multispace0(input)?;
    let (input, order_by) = opt(parse_order_by)(input)?;
    let (input, _) = multispace0(input)?;
    let (input, limit) = opt(parse_limit)(input)?;

    Ok((
        input,
        Query {
            match_clause,
            where_clause,
            return_clause,
            order_by,
            limit,
        },
    ))
}

fn parse_match_clause(input: &str) -> IResult<&str, MatchClause> {
    let (input, _) = tag_no_case("MATCH")(input)?;
    let (input, _) = multispace1(input)?;
    let (input, pattern) = parse_pattern(input)?;
    Ok((
        input,
        MatchClause {
            patterns: vec![pattern],
        },
    ))
}

fn parse_pattern(input: &str) -> IResult<&str, Pattern> {
    let (input, entity) = parse_entity_pattern(input)?;
    let (mut input, _) = multispace0(input)?;

    let mut relationships = Vec::new();
    loop {
        let trimmed = input.trim_start();
        if trimmed.starts_with('-') {
            let (rest, _) = char('-')(trimmed)?;
            let (rest, rel) = parse_rel_pattern(rest)?;
            let (rest, _) = char('-')(rest)?;
            let (rest, _) = multispace0(rest)?;
            let (rest, _) = opt(tag(">"))(rest)?;
            let (rest, _) = multispace0(rest)?;
            let (rest, target) = parse_entity_pattern(rest)?;
            relationships.push((rel, target));
            input = rest;
        } else {
            break;
        }
    }

    Ok((
        input,
        Pattern {
            entity,
            relationships,
        },
    ))
}

fn parse_entity_pattern(input: &str) -> IResult<&str, EntityPattern> {
    let (input, _) = char('(')(input)?;
    let (input, _) = multispace0(input)?;
    let (input, variable) = parse_identifier(input)?;
    let (input, _) = multispace0(input)?;

    let (input, entity_type) = opt(preceded(char(':'), parse_identifier))(input)?;
    let (input, _) = multispace0(input)?;

    let (input, properties) = opt(parse_property_filter)(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(')')(input)?;

    Ok((
        input,
        EntityPattern {
            variable: variable.to_string(),
            entity_type: entity_type.map(|s| s.to_string()),
            properties: properties.unwrap_or_default(),
        },
    ))
}

fn parse_property_filter(input: &str) -> IResult<&str, Vec<(String, Literal)>> {
    delimited(
        char('{'),
        separated_list0(
            tuple((multispace0, char(','), multispace0)),
            parse_property_pair,
        ),
        preceded(multispace0, char('}')),
    )(input)
}

fn parse_property_pair(input: &str) -> IResult<&str, (String, Literal)> {
    let (input, _) = multispace0(input)?;
    let (input, key) = parse_identifier(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(':')(input)?;
    let (input, _) = multispace0(input)?;
    let (input, value) = parse_literal(input)?;
    Ok((input, (key.to_string(), value)))
}

fn parse_rel_pattern(input: &str) -> IResult<&str, RelPattern> {
    let (input, _) = char('[')(input)?;
    let (input, _) = multispace0(input)?;
    let (input, rel_type) = opt(preceded(char(':'), parse_identifier))(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(']')(input)?;

    Ok((
        input,
        RelPattern {
            variable: None,
            rel_type: rel_type.map(|s| s.to_string()),
        },
    ))
}

fn parse_where_clause(input: &str) -> IResult<&str, WhereClause> {
    let (input, _) = tag_no_case("WHERE")(input)?;
    let (input, _) = multispace1(input)?;
    let (input, first) = parse_condition(input)?;

    let mut conditions = vec![first];
    let mut remaining = input;

    loop {
        let (input, _) = multispace0(remaining)?;
        if let Ok((input, _)) = tag_no_case::<&str, &str, nom::error::Error<&str>>("AND")(input) {
            let (input, _) = multispace1(input)?;
            let (input, cond) = parse_condition(input)?;
            conditions.push(cond);
            remaining = input;
        } else {
            remaining = input;
            break;
        }
    }

    Ok((remaining, WhereClause { conditions }))
}

fn parse_condition(input: &str) -> IResult<&str, Condition> {
    alt((parse_near_condition, parse_within_condition, parse_comparison))(input)
}

fn parse_near_condition(input: &str) -> IResult<&str, Condition> {
    let (input, _) = tag_no_case("NEAR")(input)?;
    let (input, _) = char('(')(input)?;
    let (input, _) = multispace0(input)?;
    let (input, var) = parse_identifier(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(',')(input)?;
    let (input, _) = multispace0(input)?;

    let (input, _) = tag_no_case("lat")(input)?;
    let (input, _) = char('=')(input)?;
    let (input, lat) = double(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(',')(input)?;
    let (input, _) = multispace0(input)?;

    let (input, _) = tag_no_case("lon")(input)?;
    let (input, _) = char('=')(input)?;
    let (input, lon) = double(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(',')(input)?;
    let (input, _) = multispace0(input)?;

    let (input, _) = tag_no_case("radius_km")(input)?;
    let (input, _) = char('=')(input)?;
    let (input, radius) = double(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(')')(input)?;

    Ok((
        input,
        Condition::Near {
            variable: var.to_string(),
            lat,
            lon,
            radius_km: radius,
        },
    ))
}

fn parse_within_condition(input: &str) -> IResult<&str, Condition> {
    let (input, _) = tag_no_case("WITHIN")(input)?;
    let (input, _) = char('(')(input)?;
    let (input, _) = multispace0(input)?;
    let (input, var) = parse_identifier(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(',')(input)?;
    let (input, _) = multispace0(input)?;

    let (input, _) = tag_no_case("min_lat")(input)?;
    let (input, _) = char('=')(input)?;
    let (input, min_lat) = double(input)?;
    let (input, _) = tuple((multispace0, char(','), multispace0))(input)?;
    let (input, _) = tag_no_case("min_lon")(input)?;
    let (input, _) = char('=')(input)?;
    let (input, min_lon) = double(input)?;
    let (input, _) = tuple((multispace0, char(','), multispace0))(input)?;
    let (input, _) = tag_no_case("max_lat")(input)?;
    let (input, _) = char('=')(input)?;
    let (input, max_lat) = double(input)?;
    let (input, _) = tuple((multispace0, char(','), multispace0))(input)?;
    let (input, _) = tag_no_case("max_lon")(input)?;
    let (input, _) = char('=')(input)?;
    let (input, max_lon) = double(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(')')(input)?;

    Ok((
        input,
        Condition::Within {
            variable: var.to_string(),
            min_lat,
            min_lon,
            max_lat,
            max_lon,
        },
    ))
}

fn parse_comparison(input: &str) -> IResult<&str, Condition> {
    let (input, left) = parse_dotted_identifier(input)?;
    let (input, _) = multispace0(input)?;
    let (input, op) = parse_comparison_op(input)?;
    let (input, _) = multispace0(input)?;
    let (input, right) = parse_literal(input)?;

    Ok((
        input,
        Condition::Comparison {
            left: left.to_string(),
            op,
            right,
        },
    ))
}

fn parse_comparison_op(input: &str) -> IResult<&str, ComparisonOp> {
    alt((
        map(tag(">="), |_| ComparisonOp::Gte),
        map(tag("<="), |_| ComparisonOp::Lte),
        map(tag("!="), |_| ComparisonOp::Neq),
        map(tag(">"), |_| ComparisonOp::Gt),
        map(tag("<"), |_| ComparisonOp::Lt),
        map(tag("="), |_| ComparisonOp::Eq),
        map(tag_no_case("LIKE"), |_| ComparisonOp::Like),
    ))(input)
}

fn parse_return_clause(input: &str) -> IResult<&str, ReturnClause> {
    let (input, _) = tag_no_case("RETURN")(input)?;
    let (input, _) = multispace1(input)?;
    let (input, expressions) = separated_list0(
        tuple((multispace0, char(','), multispace0)),
        parse_return_expr,
    )(input)?;

    Ok((input, ReturnClause { expressions }))
}

fn parse_return_expr(input: &str) -> IResult<&str, ReturnExpr> {
    alt((parse_function_expr, parse_property_expr))(input)
}

fn parse_function_expr(input: &str) -> IResult<&str, ReturnExpr> {
    let (input, name) = alt((
        tag_no_case("COUNT"),
        tag_no_case("SUM"),
        tag_no_case("AVG"),
        tag_no_case("MIN"),
        tag_no_case("MAX"),
        tag_no_case("DISTANCE"),
    ))(input)?;
    let (input, _) = char('(')(input)?;
    let (input, _) = multispace0(input)?;
    let (input, arg) = parse_dotted_identifier(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(')')(input)?;
    let (input, alias) = opt(preceded(
        tuple((multispace1, tag_no_case("as"), multispace1)),
        parse_identifier,
    ))(input)?;

    Ok((
        input,
        ReturnExpr::Function {
            name: name.to_uppercase(),
            args: vec![arg.to_string()],
            alias: alias.map(|s| s.to_string()),
        },
    ))
}

fn parse_property_expr(input: &str) -> IResult<&str, ReturnExpr> {
    let (input, ident) = parse_dotted_identifier(input)?;
    let (input, alias) = opt(preceded(
        tuple((multispace1, tag_no_case("as"), multispace1)),
        parse_identifier,
    ))(input)?;

    let parts: Vec<&str> = ident.split('.').collect();
    if parts.len() == 2 {
        Ok((
            input,
            ReturnExpr::Property {
                variable: parts[0].to_string(),
                property: parts[1].to_string(),
                alias: alias.map(|s| s.to_string()),
            },
        ))
    } else {
        Ok((
            input,
            ReturnExpr::Variable {
                name: ident.to_string(),
                alias: alias.map(|s| s.to_string()),
            },
        ))
    }
}

fn parse_order_by(input: &str) -> IResult<&str, OrderByClause> {
    let (input, _) = tag_no_case("ORDER")(input)?;
    let (input, _) = multispace1(input)?;
    let (input, _) = tag_no_case("BY")(input)?;
    let (input, _) = multispace1(input)?;
    let (input, field) = parse_dotted_identifier(input)?;
    let (input, _) = multispace0(input)?;
    let (input, dir) = opt(alt((tag_no_case("ASC"), tag_no_case("DESC"))))(input)?;

    Ok((
        input,
        OrderByClause {
            field: field.to_string(),
            ascending: dir.map(|d| d.eq_ignore_ascii_case("ASC")).unwrap_or(true),
        },
    ))
}

fn parse_limit(input: &str) -> IResult<&str, usize> {
    let (input, _) = tag_no_case("LIMIT")(input)?;
    let (input, _) = multispace1(input)?;
    let (input, n) = take_while1(|c: char| c.is_ascii_digit())(input)?;
    let limit: usize = n.parse().unwrap_or(100);
    Ok((input, limit))
}

fn parse_literal(input: &str) -> IResult<&str, Literal> {
    alt((parse_string_literal, parse_number_literal, parse_bool_literal))(input)
}

fn parse_string_literal(input: &str) -> IResult<&str, Literal> {
    let (input, _) = char('"')(input)?;
    let (input, s) = take_while1(|c: char| c != '"')(input)?;
    let (input, _) = char('"')(input)?;
    Ok((input, Literal::String(s.to_string())))
}

fn parse_number_literal(input: &str) -> IResult<&str, Literal> {
    let (input, n) = double(input)?;
    Ok((input, Literal::Number(n)))
}

fn parse_bool_literal(input: &str) -> IResult<&str, Literal> {
    alt((
        map(tag_no_case("true"), |_| Literal::Boolean(true)),
        map(tag_no_case("false"), |_| Literal::Boolean(false)),
    ))(input)
}

fn parse_identifier(input: &str) -> IResult<&str, &str> {
    take_while1(|c: char| c.is_alphanumeric() || c == '_')(input)
}

fn parse_dotted_identifier(input: &str) -> IResult<&str, &str> {
    take_while1(|c: char| c.is_alphanumeric() || c == '_' || c == '.')(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_query() {
        let query = r#"MATCH (s:Ship) WHERE s.speed > 15 RETURN s.id, s.name, s.speed LIMIT 100"#;
        let result = parse_orpql(query);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let q = result.unwrap();
        assert_eq!(q.limit, Some(100));
    }

    #[test]
    fn test_near_query() {
        let query = r#"MATCH (s:Ship) WHERE NEAR(s, lat=51.9225, lon=4.2706, radius_km=50) RETURN s.id, s.name"#;
        let result = parse_orpql(query);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
    }

    #[test]
    fn test_graph_traversal() {
        let query = r#"MATCH (s:Ship)-[:HEADING_TO]->(p:Port {name: "Rotterdam"}) RETURN s.id, s.name"#;
        let result = parse_orpql(query);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let q = result.unwrap();
        assert!(!q.match_clause.patterns[0].relationships.is_empty());
    }

    #[test]
    fn test_aggregation() {
        let query = r#"MATCH (s:Ship) RETURN COUNT(s) as ship_count"#;
        let result = parse_orpql(query);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
    }

    #[test]
    fn test_within_query() {
        let query = "MATCH (s:Ship) WHERE WITHIN(s, min_lat=48.0, min_lon=-5.0, max_lat=55.0, max_lon=10.0) RETURN s.id";
        let result = parse_orpql(query);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let q = result.unwrap();
        let cond = &q.where_clause.unwrap().conditions[0];
        match cond {
            Condition::Within { min_lat, min_lon, max_lat, max_lon, .. } => {
                assert!((*min_lat - 48.0).abs() < 0.01);
                assert!((*min_lon - -5.0).abs() < 0.01);
                assert!((*max_lat - 55.0).abs() < 0.01);
                assert!((*max_lon - 10.0).abs() < 0.01);
            }
            _ => panic!("Expected Within condition"),
        }
    }

    #[test]
    fn test_and_conditions() {
        let query = r#"MATCH (s:Ship) WHERE s.speed > 10 AND s.speed < 30 RETURN s.id"#;
        let result = parse_orpql(query);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let q = result.unwrap();
        assert_eq!(q.where_clause.unwrap().conditions.len(), 2);
    }

    #[test]
    fn test_order_by_asc() {
        let query = r#"MATCH (s:Ship) RETURN s.id ORDER BY s.speed ASC"#;
        let result = parse_orpql(query);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let q = result.unwrap();
        assert!(q.order_by.as_ref().unwrap().ascending);
    }

    #[test]
    fn test_order_by_desc() {
        let query = r#"MATCH (s:Ship) RETURN s.id ORDER BY s.speed DESC"#;
        let result = parse_orpql(query);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let q = result.unwrap();
        assert!(!q.order_by.as_ref().unwrap().ascending);
    }

    #[test]
    fn test_comparison_operators() {
        for (op, expected) in &[
            ("=", ComparisonOp::Eq),
            ("!=", ComparisonOp::Neq),
            (">", ComparisonOp::Gt),
            ("<", ComparisonOp::Lt),
            (">=", ComparisonOp::Gte),
            ("<=", ComparisonOp::Lte),
        ] {
            let query = format!(r#"MATCH (s:Ship) WHERE s.speed {} 10 RETURN s.id"#, op);
            let result = parse_orpql(&query);
            assert!(result.is_ok(), "Failed to parse '{}': {:?}", op, result.err());
            let q = result.unwrap();
            let cond = &q.where_clause.unwrap().conditions[0];
            if let Condition::Comparison { op: parsed_op, .. } = cond {
                assert!(
                    std::mem::discriminant(parsed_op) == std::mem::discriminant(expected),
                    "Operator mismatch for '{}': {:?} vs {:?}", op, parsed_op, expected
                );
            }
        }
    }

    #[test]
    fn test_string_literal_comparison() {
        let query = r#"MATCH (s:Ship) WHERE s.ship_type = "tanker" RETURN s.id"#;
        let result = parse_orpql(query);
        assert!(result.is_ok());
        let q = result.unwrap();
        if let Condition::Comparison { right, .. } = &q.where_clause.unwrap().conditions[0] {
            match right {
                Literal::String(s) => assert_eq!(s, "tanker"),
                _ => panic!("Expected string literal"),
            }
        }
    }

    #[test]
    fn test_boolean_literal() {
        let query = r#"MATCH (s:Ship) WHERE s.is_active = true RETURN s.id"#;
        let result = parse_orpql(query);
        assert!(result.is_ok());
        let q = result.unwrap();
        if let Condition::Comparison { right, .. } = &q.where_clause.unwrap().conditions[0] {
            match right {
                Literal::Boolean(b) => assert!(*b),
                _ => panic!("Expected boolean literal"),
            }
        }
    }

    #[test]
    fn test_sum_function() {
        let query = r#"MATCH (s:Ship) RETURN SUM(s.speed) as total_speed"#;
        let result = parse_orpql(query);
        assert!(result.is_ok());
        let q = result.unwrap();
        if let ReturnExpr::Function { name, alias, .. } = &q.return_clause.expressions[0] {
            assert_eq!(name, "SUM");
            assert_eq!(alias.as_deref(), Some("total_speed"));
        }
    }

    #[test]
    fn test_avg_function() {
        let query = r#"MATCH (s:Ship) RETURN AVG(s.speed) as avg_speed"#;
        let result = parse_orpql(query);
        assert!(result.is_ok());
    }

    #[test]
    fn test_min_max_functions() {
        for func in &["MIN", "MAX"] {
            let query = format!("MATCH (s:Ship) RETURN {}(s.speed)", func);
            let result = parse_orpql(&query);
            assert!(result.is_ok(), "Failed to parse {}: {:?}", func, result.err());
        }
    }

    #[test]
    fn test_distance_function() {
        let query = r#"MATCH (s:Ship) RETURN DISTANCE(s.position) as dist"#;
        let result = parse_orpql(query);
        assert!(result.is_ok());
    }

    #[test]
    fn test_multiple_return_expressions() {
        let query = r#"MATCH (s:Ship) RETURN s.id, s.name, s.speed, s.heading"#;
        let result = parse_orpql(query);
        assert!(result.is_ok());
        let q = result.unwrap();
        assert_eq!(q.return_clause.expressions.len(), 4);
    }

    #[test]
    fn test_return_with_alias() {
        let query = r#"MATCH (s:Ship) RETURN s.name as ship_name"#;
        let result = parse_orpql(query);
        assert!(result.is_ok());
        let q = result.unwrap();
        if let ReturnExpr::Property { alias, .. } = &q.return_clause.expressions[0] {
            assert_eq!(alias.as_deref(), Some("ship_name"));
        }
    }

    #[test]
    fn test_entity_pattern_no_type() {
        let query = r#"MATCH (s) RETURN s"#;
        let result = parse_orpql(query);
        assert!(result.is_ok());
        let q = result.unwrap();
        assert!(q.match_clause.patterns[0].entity.entity_type.is_none());
    }

    #[test]
    fn test_property_filter_in_match() {
        let query = r#"MATCH (s:Ship {mmsi: "123456789"}) RETURN s.id"#;
        let result = parse_orpql(query);
        assert!(result.is_ok());
        let q = result.unwrap();
        assert!(!q.match_clause.patterns[0].entity.properties.is_empty());
    }

    #[test]
    fn test_case_insensitive_keywords() {
        let query = r#"match (s:Ship) where s.speed > 10 return s.id order by s.speed limit 5"#;
        let result = parse_orpql(query);
        assert!(result.is_ok(), "Failed to parse case-insensitive: {:?}", result.err());
    }

    #[test]
    fn test_invalid_query_returns_error() {
        let result = parse_orpql("THIS IS NOT VALID");
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_query_returns_error() {
        let result = parse_orpql("");
        assert!(result.is_err());
    }

    #[test]
    fn test_near_with_decimal_values() {
        let query = "MATCH (s:Ship) WHERE NEAR(s, lat=51.922500, lon=4.270600, radius_km=0.5) RETURN s.id";
        let result = parse_orpql(query);
        assert!(result.is_ok());
        let q = result.unwrap();
        if let Condition::Near { radius_km, .. } = &q.where_clause.unwrap().conditions[0] {
            assert!((*radius_km - 0.5).abs() < 0.01);
        }
    }

    #[test]
    fn test_limit_zero() {
        let query = r#"MATCH (s:Ship) RETURN s.id LIMIT 0"#;
        let result = parse_orpql(query);
        assert!(result.is_ok());
        let q = result.unwrap();
        assert_eq!(q.limit, Some(0));
    }

    #[test]
    fn test_large_limit() {
        let query = r#"MATCH (s:Ship) RETURN s.id LIMIT 999999"#;
        let result = parse_orpql(query);
        assert!(result.is_ok());
        let q = result.unwrap();
        assert_eq!(q.limit, Some(999999));
    }
}
