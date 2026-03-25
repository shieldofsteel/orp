use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Permission {
    EntitiesRead,
    EntitiesWrite,
    EntitiesDelete,
    GraphRead,
    MonitorsRead,
    MonitorsWrite,
    QueryExecute,
    ConnectorsManage,
    Admin,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PolicyEffect {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AbacPolicy {
    pub id: String,
    pub name: String,
    pub effect: PolicyEffect,
    pub required_permissions: Vec<Permission>,
    pub resource_type: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RequestContext {
    pub user_id: String,
    pub permissions: Vec<Permission>,
    pub resource_type: String,
    pub resource_id: String,
    pub action: String,
}

pub struct AbacEngine {
    policies: Vec<AbacPolicy>,
}

impl AbacEngine {
    pub fn new() -> Self {
        Self {
            policies: Vec::new(),
        }
    }

    /// Create a permissive engine that allows all requests (for development)
    pub fn default_permissive() -> Self {
        let mut engine = Self::new();
        engine.add_policy(AbacPolicy {
            id: "default-allow".to_string(),
            name: "Default Allow All".to_string(),
            effect: PolicyEffect::Allow,
            required_permissions: vec![],
            resource_type: None,
        });
        engine
    }

    pub fn add_policy(&mut self, policy: AbacPolicy) {
        self.policies.push(policy);
    }

    /// Evaluate whether a request is allowed. Default deny.
    pub fn evaluate(&self, ctx: &RequestContext) -> bool {
        let mut allowed = false;

        for policy in &self.policies {
            // Check resource type filter
            if let Some(ref rt) = policy.resource_type {
                if rt != &ctx.resource_type {
                    continue;
                }
            }

            // Check required permissions
            let has_permissions = policy.required_permissions.is_empty()
                || policy
                    .required_permissions
                    .iter()
                    .all(|p| ctx.permissions.contains(p));

            if has_permissions {
                match policy.effect {
                    PolicyEffect::Deny => return false,
                    PolicyEffect::Allow => allowed = true,
                }
            }
        }

        allowed
    }
}

impl Default for AbacEngine {
    fn default() -> Self {
        Self::default_permissive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permissive_engine() {
        let engine = AbacEngine::default_permissive();
        let ctx = RequestContext {
            user_id: "user-1".to_string(),
            permissions: vec![Permission::EntitiesRead],
            resource_type: "entity".to_string(),
            resource_id: "ship-1".to_string(),
            action: "read".to_string(),
        };
        assert!(engine.evaluate(&ctx));
    }

    #[test]
    fn test_deny_overrides() {
        let mut engine = AbacEngine::new();
        engine.add_policy(AbacPolicy {
            id: "allow-read".to_string(),
            name: "Allow Read".to_string(),
            effect: PolicyEffect::Allow,
            required_permissions: vec![Permission::EntitiesRead],
            resource_type: None,
        });
        engine.add_policy(AbacPolicy {
            id: "deny-secret".to_string(),
            name: "Deny Secret".to_string(),
            effect: PolicyEffect::Deny,
            required_permissions: vec![],
            resource_type: Some("secret".to_string()),
        });

        let ctx = RequestContext {
            user_id: "user-1".to_string(),
            permissions: vec![Permission::EntitiesRead],
            resource_type: "secret".to_string(),
            resource_id: "classified-1".to_string(),
            action: "read".to_string(),
        };
        assert!(!engine.evaluate(&ctx));
    }
}
