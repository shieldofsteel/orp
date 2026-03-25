use serde::{Deserialize, Serialize};

/// ORP-QL Abstract Syntax Tree
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Query {
    pub match_clause: MatchClause,
    pub where_clause: Option<WhereClause>,
    pub return_clause: ReturnClause,
    pub order_by: Option<OrderByClause>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchClause {
    pub patterns: Vec<Pattern>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pattern {
    pub entity: EntityPattern,
    pub relationships: Vec<(RelPattern, EntityPattern)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntityPattern {
    pub variable: String,
    pub entity_type: Option<String>,
    pub properties: Vec<(String, Literal)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelPattern {
    pub variable: Option<String>,
    pub rel_type: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WhereClause {
    pub conditions: Vec<Condition>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Condition {
    Comparison {
        left: String,
        op: ComparisonOp,
        right: Literal,
    },
    Near {
        variable: String,
        lat: f64,
        lon: f64,
        radius_km: f64,
    },
    Within {
        variable: String,
        min_lat: f64,
        min_lon: f64,
        max_lat: f64,
        max_lon: f64,
    },
    And(Box<Condition>, Box<Condition>),
    Or(Box<Condition>, Box<Condition>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ComparisonOp {
    Eq,
    Neq,
    Gt,
    Lt,
    Gte,
    Lte,
    Like,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Literal {
    String(String),
    Number(f64),
    Boolean(bool),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReturnClause {
    pub expressions: Vec<ReturnExpr>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ReturnExpr {
    Property {
        variable: String,
        property: String,
        alias: Option<String>,
    },
    Function {
        name: String,
        args: Vec<String>,
        alias: Option<String>,
    },
    Variable {
        name: String,
        alias: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderByClause {
    pub field: String,
    pub ascending: bool,
}
