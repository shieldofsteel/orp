//! Attribute-Based Access Control (ABAC) engine.
//!
//! Implements the policy evaluation model from ORP spec Section 6.2.
//! Policies support:
//! - Principal matching (user type, role, attribute conditions)
//! - Action matching (exact + wildcard)
//! - Resource matching (type + attribute conditions)
//! - Variable interpolation (`${subject.sub}` etc.)
//!
//! Evaluation order: explicit DENY beats any ALLOW (deny-overrides).
//! Default result: DENY (if no policy matches).
//!
//! Target latency: <10ms per evaluation.

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use thiserror::Error;

/// Errors from the ABAC engine.
#[derive(Debug, Error)]
pub enum AbacError {
    #[error("Policy not found: {0}")]
    PolicyNotFound(String),
    #[error("Evaluation error: {0}")]
    EvaluationError(String),
    #[error("Storage error: {0}")]
    Storage(String),
}

// ─── Permission Enum ──────────────────────────────────────────────────────────

/// Typed permission constants matching the ORP spec scope strings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Permission {
    EntitiesRead,
    EntitiesWrite,
    EntitiesDelete,
    GraphRead,
    GraphWrite,
    MonitorsRead,
    MonitorsWrite,
    QueryExecute,
    ConnectorsManage,
    ApiKeysManage,
    Admin,
}

impl Permission {
    /// Parse from a scope string like `"entities:read"`.
    pub fn from_scope(s: &str) -> Option<Self> {
        match s {
            "entities:read" => Some(Self::EntitiesRead),
            "entities:write" => Some(Self::EntitiesWrite),
            "entities:delete" => Some(Self::EntitiesDelete),
            "graph:read" => Some(Self::GraphRead),
            "graph:write" => Some(Self::GraphWrite),
            "monitors:read" => Some(Self::MonitorsRead),
            "monitors:write" => Some(Self::MonitorsWrite),
            "query:execute" => Some(Self::QueryExecute),
            "connectors:manage" => Some(Self::ConnectorsManage),
            "api-keys:manage" => Some(Self::ApiKeysManage),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EntitiesRead => "entities:read",
            Self::EntitiesWrite => "entities:write",
            Self::EntitiesDelete => "entities:delete",
            Self::GraphRead => "graph:read",
            Self::GraphWrite => "graph:write",
            Self::MonitorsRead => "monitors:read",
            Self::MonitorsWrite => "monitors:write",
            Self::QueryExecute => "query:execute",
            Self::ConnectorsManage => "connectors:manage",
            Self::ApiKeysManage => "api-keys:manage",
            Self::Admin => "admin",
        }
    }
}

// ─── Policy Model ─────────────────────────────────────────────────────────────

/// Effect of a matched policy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PolicyEffect {
    Allow,
    Deny,
}

/// Conditions on the principal (who).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PrincipalSpec {
    /// Match on principal type ("user", "api_key", "*")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    /// Match on specific attribute values of the principal
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub attribute_match: HashMap<String, serde_json::Value>,
}

/// Conditions on the resource (what).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ResourceSpec {
    /// Match on resource type ("entity", "relationship", "monitor", "*")
    pub r#type: String,
    /// Match on specific attributes of the resource
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub attribute_match: HashMap<String, serde_json::Value>,
}

/// A single ABAC policy.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AbacPolicy {
    pub id: String,
    pub name: String,
    pub effect: PolicyEffect,
    pub principal: PrincipalSpec,
    /// Actions this policy applies to (e.g. `["entities:read", "entities:write"]`, `["*"]`)
    pub action: Vec<String>,
    pub resource: ResourceSpec,
    /// Optional description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Priority — higher wins when both Allow and Deny match (deny-override still applies)
    #[serde(default)]
    pub priority: i32,
}

// ─── Evaluation Context ───────────────────────────────────────────────────────

/// Subject (principal) attributes for policy evaluation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Subject {
    pub sub: String,
    pub permissions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    /// Extra attributes (e.g. department, clearance_level)
    #[serde(default)]
    pub attributes: HashMap<String, serde_json::Value>,
}

/// Resource being accessed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resource {
    pub r#type: String,
    pub id: String,
    /// Resource attributes (e.g. sensitivity, owner_id, tags)
    #[serde(default)]
    pub attributes: HashMap<String, serde_json::Value>,
}

/// Full evaluation context.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvaluationContext {
    pub subject: Subject,
    /// The action being attempted (e.g. `"entities:read"`)
    pub action: String,
    pub resource: Resource,
}

/// Result of a policy evaluation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum EvaluationResult {
    Allow,
    Deny,
}

/// Reason for a policy decision — for audit logging.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub result: EvaluationResult,
    /// The policy that triggered the decision (if any)
    pub matched_policy: Option<String>,
    /// Human-readable explanation
    pub reason: String,
}

// ─── ABAC Engine ─────────────────────────────────────────────────────────────

/// The ABAC policy evaluation engine.
///
/// Evaluation is synchronous and targets <10ms latency for typical policy sets.
///
/// Algorithm:
/// 1. Filter policies applicable to (action, resource type)
/// 2. For each matching policy, check principal and resource attribute conditions
/// 3. Any DENY match → immediate Deny (deny-overrides)
/// 4. At least one ALLOW match → Allow
/// 5. No match → Deny (default-deny)
#[derive(Clone, Debug)]
pub struct AbacEngine {
    policies: Arc<RwLock<Vec<AbacPolicy>>>,
}

impl AbacEngine {
    /// Create an empty engine (default-deny for all requests).
    pub fn new() -> Self {
        Self {
            policies: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Create a permissive engine that allows all requests — for development.
    pub fn default_permissive() -> Self {
        let engine = Self::new();
        engine
            .add_policy(AbacPolicy {
                id: "dev-allow-all".to_string(),
                name: "Dev: Allow all requests".to_string(),
                effect: PolicyEffect::Allow,
                principal: PrincipalSpec {
                    r#type: Some("*".to_string()),
                    attribute_match: HashMap::new(),
                },
                action: vec!["*".to_string()],
                resource: ResourceSpec {
                    r#type: "*".to_string(),
                    attribute_match: HashMap::new(),
                },
                description: Some("Development permissive policy — disable in production".to_string()),
                priority: -100,
            })
            .unwrap_or_else(|e| {
                tracing::error!("Failed to register default permissive policy: {e}");
            });
        engine
    }

    /// Create a standard production engine with the ORP default policies.
    pub fn default_production() -> Self {
        let engine = Self::new();

        // Allow admin to do anything
        engine
            .add_policy(AbacPolicy {
                id: "admin-allow-all".to_string(),
                name: "Admins can do anything".to_string(),
                effect: PolicyEffect::Allow,
                principal: PrincipalSpec {
                    r#type: Some("user".to_string()),
                    attribute_match: [("role".to_string(), serde_json::json!("admin"))]
                        .into_iter()
                        .collect(),
                },
                action: vec!["*".to_string()],
                resource: ResourceSpec {
                    r#type: "*".to_string(),
                    attribute_match: HashMap::new(),
                },
                description: None,
                priority: 100,
            })
            .unwrap_or_else(|e| {
                tracing::error!("Failed to register admin-allow-all policy: {e}");
            });

        // Allow users with entities:read to read entities
        engine
            .add_policy(AbacPolicy {
                id: "entities-read".to_string(),
                name: "Users can read entities".to_string(),
                effect: PolicyEffect::Allow,
                principal: PrincipalSpec {
                    r#type: Some("user".to_string()),
                    attribute_match: HashMap::new(),
                },
                action: vec!["entities:read".to_string()],
                resource: ResourceSpec {
                    r#type: "entity".to_string(),
                    attribute_match: HashMap::new(),
                },
                description: None,
                priority: 0,
            })
            .unwrap_or_else(|e| {
                tracing::error!("Failed to register entities-read policy: {e}");
            });

        // Deny access to sensitive resources for non-admin users
        engine
            .add_policy(AbacPolicy {
                id: "deny-secret-resources".to_string(),
                name: "Deny access to secret resources for regular users".to_string(),
                effect: PolicyEffect::Deny,
                principal: PrincipalSpec {
                    r#type: Some("user".to_string()),
                    attribute_match: HashMap::new(),
                },
                action: vec!["*".to_string()],
                resource: ResourceSpec {
                    r#type: "entity".to_string(),
                    attribute_match: [(
                        "sensitivity".to_string(),
                        serde_json::json!("secret"),
                    )]
                    .into_iter()
                    .collect(),
                },
                description: None,
                priority: 50, // Evaluated before allow rules
            })
            .unwrap_or_else(|e| {
                tracing::error!("Failed to register deny-secret-resources policy: {e}");
            });

        engine
    }

    /// Add a policy to the engine.
    pub fn add_policy(&self, policy: AbacPolicy) -> Result<(), AbacError> {
        let mut policies = self
            .policies
            .write()
            .map_err(|e| AbacError::Storage(e.to_string()))?;
        policies.push(policy);
        // Keep sorted by priority (descending) for evaluation order
        policies.sort_by(|a, b| b.priority.cmp(&a.priority));
        Ok(())
    }

    /// Remove a policy by ID.
    pub fn remove_policy(&self, id: &str) -> Result<(), AbacError> {
        let mut policies = self
            .policies
            .write()
            .map_err(|e| AbacError::Storage(e.to_string()))?;
        let before = policies.len();
        policies.retain(|p| p.id != id);
        if policies.len() == before {
            return Err(AbacError::PolicyNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Evaluate a request context and return Allow/Deny.
    ///
    /// This is the hot path — target <10ms.
    pub fn evaluate(&self, ctx: &EvaluationContext) -> PolicyDecision {
        let policies = match self.policies.read() {
            Ok(p) => p,
            Err(_) => {
                return PolicyDecision {
                    result: EvaluationResult::Deny,
                    matched_policy: None,
                    reason: "Policy store unavailable".to_string(),
                }
            }
        };

        // Fast-path: admin users skip policy evaluation
        if ctx.subject.permissions.iter().any(|p| p == "admin") {
            // Still check explicit denies first
            for policy in policies.iter() {
                if policy.effect == PolicyEffect::Deny && self.matches_policy(policy, ctx) {
                    return PolicyDecision {
                        result: EvaluationResult::Deny,
                        matched_policy: Some(policy.id.clone()),
                        reason: format!("Explicit deny by policy '{}'", policy.name),
                    };
                }
            }
            return PolicyDecision {
                result: EvaluationResult::Allow,
                matched_policy: None,
                reason: "Admin user — allowed by default".to_string(),
            };
        }

        // Check if the user has the required permission for the action
        if !self.user_has_action_permission(ctx) {
            return PolicyDecision {
                result: EvaluationResult::Deny,
                matched_policy: None,
                reason: format!(
                    "Missing permission '{}' in token claims",
                    ctx.action
                ),
            };
        }

        let mut allowed = false;
        let mut allow_policy_id: Option<String> = None;
        let mut allow_policy_name = String::new();

        for policy in policies.iter() {
            if !self.matches_policy(policy, ctx) {
                continue;
            }
            match policy.effect {
                PolicyEffect::Deny => {
                    return PolicyDecision {
                        result: EvaluationResult::Deny,
                        matched_policy: Some(policy.id.clone()),
                        reason: format!("Explicit deny by policy '{}'", policy.name),
                    };
                }
                PolicyEffect::Allow => {
                    if !allowed {
                        allowed = true;
                        allow_policy_id = Some(policy.id.clone());
                        allow_policy_name = policy.name.clone();
                    }
                }
            }
        }

        if allowed {
            PolicyDecision {
                result: EvaluationResult::Allow,
                matched_policy: allow_policy_id,
                reason: format!("Allowed by policy '{allow_policy_name}'"),
            }
        } else {
            PolicyDecision {
                result: EvaluationResult::Deny,
                matched_policy: None,
                reason: "No matching allow policy (default deny)".to_string(),
            }
        }
    }

    /// Convenience method — returns true iff evaluate returns Allow.
    pub fn is_allowed(&self, ctx: &EvaluationContext) -> bool {
        self.evaluate(ctx).result == EvaluationResult::Allow
    }

    // ─── Internal helpers ────────────────────────────────────────────────────

    fn matches_policy(&self, policy: &AbacPolicy, ctx: &EvaluationContext) -> bool {
        self.principal_matches(&policy.principal, ctx)
            && self.action_matches(&policy.action, &ctx.action)
            && self.resource_matches(&policy.resource, &ctx.resource, ctx)
    }

    fn principal_matches(&self, spec: &PrincipalSpec, ctx: &EvaluationContext) -> bool {
        // Type check
        if let Some(ptype) = &spec.r#type {
            if ptype != "*" {
                // "user" matches anyone with no special role; "api_key" could be added.
                // For now, "user" always matches authenticated subjects.
            }
        }

        // Build a combined attribute map that includes known subject fields
        // This avoids lifetime issues with temporary values.
        let mut combined_attrs = ctx.subject.attributes.clone();
        if let Some(ref role) = ctx.subject.role {
            combined_attrs
                .entry("role".to_string())
                .or_insert_with(|| serde_json::json!(role));
        }
        if let Some(ref org_id) = ctx.subject.org_id {
            combined_attrs
                .entry("org_id".to_string())
                .or_insert_with(|| serde_json::json!(org_id));
        }

        // Attribute matching
        for (key, expected) in &spec.attribute_match {
            if let Some(actual_val) = combined_attrs.get(key) {
                if !value_matches(actual_val, expected) {
                    return false;
                }
            } else {
                return false;
            }
        }

        true
    }

    fn action_matches(&self, policy_actions: &[String], request_action: &str) -> bool {
        policy_actions
            .iter()
            .any(|a| a == "*" || a == request_action)
    }

    fn resource_matches(
        &self,
        spec: &ResourceSpec,
        resource: &Resource,
        ctx: &EvaluationContext,
    ) -> bool {
        // Resource type
        if spec.r#type != "*" && spec.r#type != resource.r#type {
            return false;
        }

        // Resource attributes
        for (key, expected) in &spec.attribute_match {
            // Interpolate ${subject.sub} style variables
            let resolved = interpolate_value(expected, ctx);
            let actual = resource.attributes.get(key);
            match actual {
                Some(v) => {
                    if !value_matches(v, &resolved) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        true
    }

    /// Check that the subject's token permissions include the requested action.
    fn user_has_action_permission(&self, ctx: &EvaluationContext) -> bool {
        ctx.subject
            .permissions
            .iter()
            .any(|p| p == &ctx.action || p == "admin")
    }
}

impl Default for AbacEngine {
    fn default() -> Self {
        Self::default_permissive()
    }
}

// ─── Helper functions ─────────────────────────────────────────────────────────

/// Check if an actual JSON value matches an expected pattern.
///
/// - String: exact match
/// - Array: actual must be in the array (OR logic)
/// - Other: deep equality
fn value_matches(actual: &serde_json::Value, expected: &serde_json::Value) -> bool {
    match expected {
        serde_json::Value::Array(arr) => arr.iter().any(|e| actual == e),
        _ => actual == expected,
    }
}

/// Interpolate `${subject.sub}` etc. in a policy attribute value.
fn interpolate_value(
    value: &serde_json::Value,
    ctx: &EvaluationContext,
) -> serde_json::Value {
    if let Some(s) = value.as_str() {
        if s.starts_with("${") && s.ends_with('}') {
            let var = &s[2..s.len() - 1];
            let resolved = match var {
                "subject.sub" => serde_json::json!(ctx.subject.sub),
                "subject.org_id" => ctx
                    .subject
                    .org_id
                    .as_ref()
                    .map(|o| serde_json::json!(o))
                    .unwrap_or(serde_json::Value::Null),
                _ => value.clone(),
            };
            return resolved;
        }
    }
    value.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx(
        permissions: Vec<&str>,
        role: Option<&str>,
        action: &str,
        resource_type: &str,
        resource_attrs: HashMap<&str, serde_json::Value>,
    ) -> EvaluationContext {
        EvaluationContext {
            subject: Subject {
                sub: "user-1".to_string(),
                permissions: permissions.iter().map(|s| s.to_string()).collect(),
                role: role.map(|s| s.to_string()),
                org_id: None,
                attributes: HashMap::new(),
            },
            action: action.to_string(),
            resource: Resource {
                r#type: resource_type.to_string(),
                id: "res-1".to_string(),
                attributes: resource_attrs
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
            },
        }
    }

    #[test]
    fn test_permissive_engine_allows_all() {
        let engine = AbacEngine::default_permissive();
        let ctx = make_ctx(
            vec!["entities:read"],
            None,
            "entities:read",
            "entity",
            HashMap::new(),
        );
        assert!(engine.is_allowed(&ctx));
    }

    #[test]
    fn test_empty_engine_denies_all() {
        let engine = AbacEngine::new();
        let ctx = make_ctx(
            vec!["entities:read"],
            None,
            "entities:read",
            "entity",
            HashMap::new(),
        );
        assert!(!engine.is_allowed(&ctx));
    }

    #[test]
    fn test_admin_bypass() {
        let engine = AbacEngine::new(); // no policies
        let ctx = make_ctx(
            vec!["admin"],
            Some("admin"),
            "entities:delete",
            "entity",
            HashMap::new(),
        );
        assert!(engine.is_allowed(&ctx));
    }

    #[test]
    fn test_deny_overrides_allow() {
        let engine = AbacEngine::new();

        // Allow all entities:read
        engine
            .add_policy(AbacPolicy {
                id: "allow-read".into(),
                name: "Allow Read".into(),
                effect: PolicyEffect::Allow,
                principal: PrincipalSpec::default(),
                action: vec!["entities:read".into()],
                resource: ResourceSpec {
                    r#type: "entity".into(),
                    attribute_match: HashMap::new(),
                },
                description: None,
                priority: 0,
            })
            .unwrap();

        // Deny secret resources
        engine
            .add_policy(AbacPolicy {
                id: "deny-secret".into(),
                name: "Deny Secret".into(),
                effect: PolicyEffect::Deny,
                principal: PrincipalSpec::default(),
                action: vec!["*".into()],
                resource: ResourceSpec {
                    r#type: "entity".into(),
                    attribute_match: [(
                        "sensitivity".to_string(),
                        serde_json::json!("secret"),
                    )]
                    .into_iter()
                    .collect(),
                },
                description: None,
                priority: 10,
            })
            .unwrap();

        // Non-secret entity should be allowed
        let ctx_public = make_ctx(
            vec!["entities:read"],
            None,
            "entities:read",
            "entity",
            [("sensitivity", serde_json::json!("public"))]
                .into_iter()
                .collect(),
        );
        assert!(engine.is_allowed(&ctx_public));

        // Secret entity should be denied even with entities:read
        let ctx_secret = make_ctx(
            vec!["entities:read"],
            None,
            "entities:read",
            "entity",
            [("sensitivity", serde_json::json!("secret"))]
                .into_iter()
                .collect(),
        );
        assert!(!engine.is_allowed(&ctx_secret));
    }

    #[test]
    fn test_missing_permission_denied() {
        let engine = AbacEngine::new();
        engine
            .add_policy(AbacPolicy {
                id: "allow-write".into(),
                name: "Allow Write".into(),
                effect: PolicyEffect::Allow,
                principal: PrincipalSpec::default(),
                action: vec!["entities:write".into()],
                resource: ResourceSpec {
                    r#type: "entity".into(),
                    attribute_match: HashMap::new(),
                },
                description: None,
                priority: 0,
            })
            .unwrap();

        // User only has read, trying write
        let ctx = make_ctx(
            vec!["entities:read"],
            None,
            "entities:write",
            "entity",
            HashMap::new(),
        );
        assert!(!engine.is_allowed(&ctx));
    }

    #[test]
    fn test_wildcard_action_policy() {
        let engine = AbacEngine::new();
        engine
            .add_policy(AbacPolicy {
                id: "allow-all-actions".into(),
                name: "Allow All".into(),
                effect: PolicyEffect::Allow,
                principal: PrincipalSpec::default(),
                action: vec!["*".into()],
                resource: ResourceSpec {
                    r#type: "*".into(),
                    attribute_match: HashMap::new(),
                },
                description: None,
                priority: 0,
            })
            .unwrap();

        let ctx = make_ctx(
            vec!["entities:read"],
            None,
            "entities:read",
            "entity",
            HashMap::new(),
        );
        assert!(engine.is_allowed(&ctx));
    }

    #[test]
    fn test_subject_interpolation() {
        let engine = AbacEngine::new();

        // Policy: allow reading only own entities (owner_id == subject.sub)
        engine
            .add_policy(AbacPolicy {
                id: "own-entities".into(),
                name: "Read own entities".into(),
                effect: PolicyEffect::Allow,
                principal: PrincipalSpec::default(),
                action: vec!["entities:read".into()],
                resource: ResourceSpec {
                    r#type: "entity".into(),
                    attribute_match: [(
                        "owner_id".to_string(),
                        serde_json::json!("${subject.sub}"),
                    )]
                    .into_iter()
                    .collect(),
                },
                description: None,
                priority: 0,
            })
            .unwrap();

        // Own entity
        let own = EvaluationContext {
            subject: Subject {
                sub: "user-42".into(),
                permissions: vec!["entities:read".into()],
                role: None,
                org_id: None,
                attributes: HashMap::new(),
            },
            action: "entities:read".into(),
            resource: Resource {
                r#type: "entity".into(),
                id: "ent-1".into(),
                attributes: [("owner_id".to_string(), serde_json::json!("user-42"))]
                    .into_iter()
                    .collect(),
            },
        };
        assert!(engine.is_allowed(&own));

        // Someone else's entity
        let other = EvaluationContext {
            resource: Resource {
                attributes: [("owner_id".to_string(), serde_json::json!("user-99"))]
                    .into_iter()
                    .collect(),
                ..own.resource.clone()
            },
            ..own.clone()
        };
        assert!(!engine.is_allowed(&other));
    }

    #[test]
    fn test_production_defaults() {
        let engine = AbacEngine::default_production();

        // Admin can do anything
        let admin_ctx = EvaluationContext {
            subject: Subject {
                sub: "admin-1".into(),
                permissions: vec!["admin".into()],
                role: Some("admin".into()),
                org_id: None,
                attributes: HashMap::new(),
            },
            action: "entities:delete".into(),
            resource: Resource {
                r#type: "entity".into(),
                id: "ent-1".into(),
                attributes: HashMap::new(),
            },
        };
        assert!(engine.is_allowed(&admin_ctx));

        // Regular user can't access secret resources
        let secret_ctx = make_ctx(
            vec!["entities:read"],
            None,
            "entities:read",
            "entity",
            [("sensitivity", serde_json::json!("secret"))]
                .into_iter()
                .collect(),
        );
        assert!(!engine.is_allowed(&secret_ctx));
    }

    #[test]
    fn test_remove_policy() {
        let engine = AbacEngine::new();
        engine
            .add_policy(AbacPolicy {
                id: "temp-policy".into(),
                name: "Temp".into(),
                effect: PolicyEffect::Allow,
                principal: PrincipalSpec::default(),
                action: vec!["*".into()],
                resource: ResourceSpec {
                    r#type: "*".into(),
                    attribute_match: HashMap::new(),
                },
                description: None,
                priority: 0,
            })
            .unwrap();

        engine.remove_policy("temp-policy").unwrap();
        let ctx = make_ctx(vec![], None, "entities:read", "entity", HashMap::new());
        assert!(!engine.is_allowed(&ctx));
    }

    #[test]
    fn test_value_matches_array() {
        let actual = serde_json::json!("internal");
        let expected = serde_json::json!(["public", "internal"]);
        assert!(value_matches(&actual, &expected));

        let not_in = serde_json::json!("secret");
        assert!(!value_matches(&not_in, &expected));
    }

    #[test]
    fn test_permission_from_str() {
        assert_eq!(
            Permission::from_scope("entities:read"),
            Some(Permission::EntitiesRead)
        );
        assert_eq!(Permission::from_scope("admin"), Some(Permission::Admin));
        assert_eq!(Permission::from_scope("unknown:perm"), None);
    }
}
