//! Role-Based Access Control (RBAC) for ORP multi-user operations.
//!
//! Defines roles, their permission sets, and integration with the existing
//! ABAC engine. Role is used as one attribute in ABAC policy evaluation.
//!
//! ## Roles (ascending privilege)
//!
//! | Role       | Description                                                  |
//! |------------|--------------------------------------------------------------|
//! | Guest      | Health check only — unauthenticated or external probes       |
//! | Viewer     | Read-only map + entity view, no queries                      |
//! | Analyst    | View entities, run queries, view reports, export data        |
//! | Operator   | View all, run queries, acknowledge alerts, create monitors   |
//! | Admin      | Manage users, connectors, monitors — full CRUD               |
//! | SuperAdmin | Everything including system config and user admin            |

use crate::abac::{AbacEngine, AbacPolicy, EvaluationContext, PolicyEffect, PrincipalSpec, ResourceSpec};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Role ─────────────────────────────────────────────────────────────────────

/// ORP operator roles — ordered by ascending privilege level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Unauthenticated / external probe — health check only.
    Guest = 0,
    /// Read-only map and entity view, no query execution.
    Viewer = 1,
    /// View entities, run queries, view reports, export data.
    Analyst = 2,
    /// View all entities, run queries, acknowledge alerts, create monitors.
    Operator = 3,
    /// Manage users, connectors, monitors — full CRUD.
    Admin = 4,
    /// All capabilities including system-level configuration.
    SuperAdmin = 5,
}

impl Role {
    /// Parse a role from its canonical string name.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "guest" => Some(Self::Guest),
            "viewer" => Some(Self::Viewer),
            "analyst" => Some(Self::Analyst),
            "operator" => Some(Self::Operator),
            "admin" => Some(Self::Admin),
            "superadmin" | "super_admin" => Some(Self::SuperAdmin),
            _ => None,
        }
    }

    /// Canonical string name (matches JSON serialization).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Guest => "guest",
            Self::Viewer => "viewer",
            Self::Analyst => "analyst",
            Self::Operator => "operator",
            Self::Admin => "admin",
            Self::SuperAdmin => "super_admin",
        }
    }

    /// Returns true if this role is at least as privileged as `other`.
    pub fn at_least(&self, other: Role) -> bool {
        (*self as u8) >= (other as u8)
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ─── Action ──────────────────────────────────────────────────────────────────

/// Fine-grained actions that can be checked against a role.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    // ── Health / System ──
    HealthCheck,
    SystemConfig,

    // ── Entity operations ──
    EntitiesRead,
    EntitiesWrite,
    EntitiesDelete,

    // ── Graph / Relationships ──
    GraphRead,
    GraphWrite,

    // ── Query execution ──
    QueryExecute,

    // ── Monitors ──
    MonitorsRead,
    MonitorsWrite,
    MonitorsDelete,
    AlertsAcknowledge,

    // ── Reports & Export ──
    ReportsView,
    DataExport,

    // ── Connectors ──
    ConnectorsView,
    ConnectorsManage,

    // ── API Keys ──
    ApiKeysView,
    ApiKeysManage,

    // ── User management ──
    UsersView,
    UsersCreate,
    UsersDelete,
    UsersChangeRole,

    // ── Admin ──
    Admin,
}

impl Action {
    /// Map from the ABAC permission scope string.
    pub fn from_scope(s: &str) -> Option<Self> {
        match s {
            "health:check" => Some(Self::HealthCheck),
            "system:config" => Some(Self::SystemConfig),
            "entities:read" => Some(Self::EntitiesRead),
            "entities:write" => Some(Self::EntitiesWrite),
            "entities:delete" => Some(Self::EntitiesDelete),
            "graph:read" => Some(Self::GraphRead),
            "graph:write" => Some(Self::GraphWrite),
            "query:execute" => Some(Self::QueryExecute),
            "monitors:read" => Some(Self::MonitorsRead),
            "monitors:write" => Some(Self::MonitorsWrite),
            "monitors:delete" => Some(Self::MonitorsDelete),
            "alerts:acknowledge" => Some(Self::AlertsAcknowledge),
            "reports:view" => Some(Self::ReportsView),
            "data:export" => Some(Self::DataExport),
            "connectors:view" => Some(Self::ConnectorsView),
            "connectors:manage" => Some(Self::ConnectorsManage),
            "api-keys:view" => Some(Self::ApiKeysView),
            "api-keys:manage" => Some(Self::ApiKeysManage),
            "users:view" => Some(Self::UsersView),
            "users:create" => Some(Self::UsersCreate),
            "users:delete" => Some(Self::UsersDelete),
            "users:change_role" => Some(Self::UsersChangeRole),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }

    pub fn as_scope(&self) -> &'static str {
        match self {
            Self::HealthCheck => "health:check",
            Self::SystemConfig => "system:config",
            Self::EntitiesRead => "entities:read",
            Self::EntitiesWrite => "entities:write",
            Self::EntitiesDelete => "entities:delete",
            Self::GraphRead => "graph:read",
            Self::GraphWrite => "graph:write",
            Self::QueryExecute => "query:execute",
            Self::MonitorsRead => "monitors:read",
            Self::MonitorsWrite => "monitors:write",
            Self::MonitorsDelete => "monitors:delete",
            Self::AlertsAcknowledge => "alerts:acknowledge",
            Self::ReportsView => "reports:view",
            Self::DataExport => "data:export",
            Self::ConnectorsView => "connectors:view",
            Self::ConnectorsManage => "connectors:manage",
            Self::ApiKeysView => "api-keys:view",
            Self::ApiKeysManage => "api-keys:manage",
            Self::UsersView => "users:view",
            Self::UsersCreate => "users:create",
            Self::UsersDelete => "users:delete",
            Self::UsersChangeRole => "users:change_role",
            Self::Admin => "admin",
        }
    }
}

// ─── RolePermissions ─────────────────────────────────────────────────────────

/// Defines the permission set for each role.
pub struct RolePermissions;

impl RolePermissions {
    /// Returns the set of actions permitted for the given role.
    ///
    /// Each role is strictly additive — higher roles include all lower-role
    /// permissions plus their own.
    pub fn for_role(role: Role) -> Vec<Action> {
        match role {
            Role::Guest => vec![
                Action::HealthCheck,
            ],

            Role::Viewer => vec![
                // All Guest permissions
                Action::HealthCheck,
                // Viewer additions
                Action::EntitiesRead,
                Action::GraphRead,
                Action::MonitorsRead,
                Action::ConnectorsView,
            ],

            Role::Analyst => vec![
                // All Viewer permissions
                Action::HealthCheck,
                Action::EntitiesRead,
                Action::GraphRead,
                Action::MonitorsRead,
                Action::ConnectorsView,
                // Analyst additions
                Action::QueryExecute,
                Action::ReportsView,
                Action::DataExport,
                Action::ApiKeysView,
            ],

            Role::Operator => vec![
                // All Analyst permissions
                Action::HealthCheck,
                Action::EntitiesRead,
                Action::GraphRead,
                Action::MonitorsRead,
                Action::ConnectorsView,
                Action::QueryExecute,
                Action::ReportsView,
                Action::DataExport,
                Action::ApiKeysView,
                // Operator additions
                Action::EntitiesWrite,
                Action::GraphWrite,
                Action::MonitorsWrite,
                Action::AlertsAcknowledge,
            ],

            Role::Admin => vec![
                // All Operator permissions
                Action::HealthCheck,
                Action::EntitiesRead,
                Action::GraphRead,
                Action::MonitorsRead,
                Action::ConnectorsView,
                Action::QueryExecute,
                Action::ReportsView,
                Action::DataExport,
                Action::ApiKeysView,
                Action::EntitiesWrite,
                Action::GraphWrite,
                Action::MonitorsWrite,
                Action::AlertsAcknowledge,
                // Admin additions
                Action::EntitiesDelete,
                Action::MonitorsDelete,
                Action::ConnectorsManage,
                Action::ApiKeysManage,
                Action::UsersView,
                Action::UsersCreate,
                Action::UsersDelete,
                Action::UsersChangeRole,
            ],

            Role::SuperAdmin => {
                // All actions
                vec![
                    Action::HealthCheck,
                    Action::SystemConfig,
                    Action::EntitiesRead,
                    Action::EntitiesWrite,
                    Action::EntitiesDelete,
                    Action::GraphRead,
                    Action::GraphWrite,
                    Action::QueryExecute,
                    Action::MonitorsRead,
                    Action::MonitorsWrite,
                    Action::MonitorsDelete,
                    Action::AlertsAcknowledge,
                    Action::ReportsView,
                    Action::DataExport,
                    Action::ConnectorsView,
                    Action::ConnectorsManage,
                    Action::ApiKeysView,
                    Action::ApiKeysManage,
                    Action::UsersView,
                    Action::UsersCreate,
                    Action::UsersDelete,
                    Action::UsersChangeRole,
                    Action::Admin,
                ]
            }
        }
    }

    /// Returns the scope strings (ABAC permission strings) for the given role.
    pub fn scopes_for_role(role: Role) -> Vec<String> {
        Self::for_role(role)
            .iter()
            .map(|a| a.as_scope().to_string())
            .collect()
    }
}

// ─── Permission check ─────────────────────────────────────────────────────────

/// Check whether a role is permitted to perform an action.
///
/// This is the primary RBAC gate — fast, synchronous, no I/O.
///
/// ```rust
/// use orp_security::rbac::{Role, Action, check_role_permission};
///
/// assert!(check_role_permission(Role::Admin, &Action::UsersCreate));
/// assert!(!check_role_permission(Role::Viewer, &Action::QueryExecute));
/// ```
pub fn check_role_permission(role: Role, action: &Action) -> bool {
    RolePermissions::for_role(role).contains(action)
}

/// Check whether a role is permitted to perform an action given as a scope string.
///
/// Returns `false` if the scope string is unknown.
pub fn check_role_permission_str(role: Role, action_str: &str) -> bool {
    match Action::from_scope(action_str) {
        Some(action) => check_role_permission(role, &action),
        None => false,
    }
}

// ─── ABAC integration ─────────────────────────────────────────────────────────

/// Register role-based ABAC policies into an existing engine.
///
/// Call this during server startup to seed the ABAC engine with RBAC rules.
/// Role is evaluated as `subject.role` attribute, which is already populated
/// by the JWT middleware via `AuthContext`.
pub fn register_rbac_policies(engine: &AbacEngine) {
    for role in [
        Role::Guest,
        Role::Viewer,
        Role::Analyst,
        Role::Operator,
        Role::Admin,
        Role::SuperAdmin,
    ] {
        let allowed_actions = RolePermissions::for_role(role)
            .iter()
            .map(|a| a.as_scope().to_string())
            .collect::<Vec<_>>();

        if allowed_actions.is_empty() {
            continue;
        }

        let priority: i32 = match role {
            Role::Guest => 10,
            Role::Viewer => 20,
            Role::Analyst => 30,
            Role::Operator => 40,
            Role::Admin => 50,
            Role::SuperAdmin => 60,
        };

        let _ = engine.add_policy(AbacPolicy {
            id: format!("rbac-{}", role.as_str()),
            name: format!("RBAC: {} permissions", role.as_str()),
            effect: PolicyEffect::Allow,
            principal: PrincipalSpec {
                r#type: Some("user".to_string()),
                attribute_match: [("role".to_string(), serde_json::json!(role.as_str()))]
                    .into_iter()
                    .collect(),
            },
            action: allowed_actions,
            resource: ResourceSpec {
                r#type: "*".to_string(),
                attribute_match: HashMap::new(),
            },
            description: Some(format!(
                "Auto-registered RBAC policy for role: {}",
                role.as_str()
            )),
            priority,
        });
    }
}

/// Build an `EvaluationContext` for ABAC evaluation using a role.
///
/// This bridges RBAC → ABAC: injects the role into subject attributes so that
/// `register_rbac_policies` principal matching works correctly.
pub fn build_rbac_abac_context(
    subject_id: &str,
    role: Role,
    action: &str,
    resource_type: &str,
    resource_id: &str,
) -> EvaluationContext {
    use crate::abac::{Resource, Subject};
    EvaluationContext {
        subject: Subject {
            sub: subject_id.to_string(),
            permissions: RolePermissions::scopes_for_role(role),
            role: Some(role.as_str().to_string()),
            org_id: None,
            attributes: [("role".to_string(), serde_json::json!(role.as_str()))]
                .into_iter()
                .collect(),
        },
        action: action.to_string(),
        resource: Resource {
            r#type: resource_type.to_string(),
            id: resource_id.to_string(),
            attributes: HashMap::new(),
        },
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Guest ────────────────────────────────────────────────────────────────

    #[test]
    fn guest_can_health_check() {
        assert!(check_role_permission(Role::Guest, &Action::HealthCheck));
    }

    #[test]
    fn guest_cannot_read_entities() {
        assert!(!check_role_permission(Role::Guest, &Action::EntitiesRead));
    }

    #[test]
    fn guest_cannot_execute_queries() {
        assert!(!check_role_permission(Role::Guest, &Action::QueryExecute));
    }

    #[test]
    fn guest_cannot_manage_users() {
        assert!(!check_role_permission(Role::Guest, &Action::UsersCreate));
    }

    #[test]
    fn guest_cannot_access_admin() {
        assert!(!check_role_permission(Role::Guest, &Action::Admin));
    }

    // ── Viewer ───────────────────────────────────────────────────────────────

    #[test]
    fn viewer_can_read_entities() {
        assert!(check_role_permission(Role::Viewer, &Action::EntitiesRead));
    }

    #[test]
    fn viewer_can_read_graph() {
        assert!(check_role_permission(Role::Viewer, &Action::GraphRead));
    }

    #[test]
    fn viewer_can_read_monitors() {
        assert!(check_role_permission(Role::Viewer, &Action::MonitorsRead));
    }

    #[test]
    fn viewer_cannot_execute_queries() {
        assert!(!check_role_permission(Role::Viewer, &Action::QueryExecute));
    }

    #[test]
    fn viewer_cannot_write_entities() {
        assert!(!check_role_permission(Role::Viewer, &Action::EntitiesWrite));
    }

    #[test]
    fn viewer_cannot_export_data() {
        assert!(!check_role_permission(Role::Viewer, &Action::DataExport));
    }

    // ── Analyst ──────────────────────────────────────────────────────────────

    #[test]
    fn analyst_can_execute_queries() {
        assert!(check_role_permission(Role::Analyst, &Action::QueryExecute));
    }

    #[test]
    fn analyst_can_view_reports() {
        assert!(check_role_permission(Role::Analyst, &Action::ReportsView));
    }

    #[test]
    fn analyst_can_export_data() {
        assert!(check_role_permission(Role::Analyst, &Action::DataExport));
    }

    #[test]
    fn analyst_cannot_write_entities() {
        assert!(!check_role_permission(Role::Analyst, &Action::EntitiesWrite));
    }

    #[test]
    fn analyst_cannot_acknowledge_alerts() {
        assert!(!check_role_permission(Role::Analyst, &Action::AlertsAcknowledge));
    }

    #[test]
    fn analyst_cannot_manage_connectors() {
        assert!(!check_role_permission(Role::Analyst, &Action::ConnectorsManage));
    }

    // ── Operator ──────────────────────────────────────────────────────────────

    #[test]
    fn operator_can_write_entities() {
        assert!(check_role_permission(Role::Operator, &Action::EntitiesWrite));
    }

    #[test]
    fn operator_can_create_monitors() {
        assert!(check_role_permission(Role::Operator, &Action::MonitorsWrite));
    }

    #[test]
    fn operator_can_acknowledge_alerts() {
        assert!(check_role_permission(Role::Operator, &Action::AlertsAcknowledge));
    }

    #[test]
    fn operator_cannot_delete_entities() {
        assert!(!check_role_permission(Role::Operator, &Action::EntitiesDelete));
    }

    #[test]
    fn operator_cannot_manage_users() {
        assert!(!check_role_permission(Role::Operator, &Action::UsersCreate));
    }

    #[test]
    fn operator_cannot_manage_connectors() {
        assert!(!check_role_permission(Role::Operator, &Action::ConnectorsManage));
    }

    // ── Admin ─────────────────────────────────────────────────────────────────

    #[test]
    fn admin_can_manage_users() {
        assert!(check_role_permission(Role::Admin, &Action::UsersCreate));
        assert!(check_role_permission(Role::Admin, &Action::UsersDelete));
        assert!(check_role_permission(Role::Admin, &Action::UsersChangeRole));
    }

    #[test]
    fn admin_can_manage_connectors() {
        assert!(check_role_permission(Role::Admin, &Action::ConnectorsManage));
    }

    #[test]
    fn admin_can_delete_entities() {
        assert!(check_role_permission(Role::Admin, &Action::EntitiesDelete));
    }

    #[test]
    fn admin_cannot_system_config() {
        // SystemConfig is SuperAdmin only
        assert!(!check_role_permission(Role::Admin, &Action::SystemConfig));
    }

    #[test]
    fn admin_cannot_admin_action() {
        // The `Admin` action (full bypass) is SuperAdmin only
        assert!(!check_role_permission(Role::Admin, &Action::Admin));
    }

    // ── SuperAdmin ────────────────────────────────────────────────────────────

    #[test]
    fn superadmin_has_all_permissions() {
        let all_actions = [
            Action::HealthCheck,
            Action::SystemConfig,
            Action::EntitiesRead,
            Action::EntitiesWrite,
            Action::EntitiesDelete,
            Action::GraphRead,
            Action::GraphWrite,
            Action::QueryExecute,
            Action::MonitorsRead,
            Action::MonitorsWrite,
            Action::MonitorsDelete,
            Action::AlertsAcknowledge,
            Action::ReportsView,
            Action::DataExport,
            Action::ConnectorsView,
            Action::ConnectorsManage,
            Action::ApiKeysView,
            Action::ApiKeysManage,
            Action::UsersView,
            Action::UsersCreate,
            Action::UsersDelete,
            Action::UsersChangeRole,
            Action::Admin,
        ];
        for action in &all_actions {
            assert!(
                check_role_permission(Role::SuperAdmin, action),
                "SuperAdmin should have {:?}",
                action
            );
        }
    }

    // ── Role ordering ─────────────────────────────────────────────────────────

    #[test]
    fn role_ordering_is_correct() {
        assert!(Role::SuperAdmin > Role::Admin);
        assert!(Role::Admin > Role::Operator);
        assert!(Role::Operator > Role::Analyst);
        assert!(Role::Analyst > Role::Viewer);
        assert!(Role::Viewer > Role::Guest);
    }

    #[test]
    fn role_at_least_works() {
        assert!(Role::Admin.at_least(Role::Operator));
        assert!(Role::Admin.at_least(Role::Admin));
        assert!(!Role::Operator.at_least(Role::Admin));
    }

    // ── String conversion ─────────────────────────────────────────────────────

    #[test]
    fn role_from_str_roundtrips() {
        for (s, expected) in [
            ("guest", Role::Guest),
            ("viewer", Role::Viewer),
            ("analyst", Role::Analyst),
            ("operator", Role::Operator),
            ("admin", Role::Admin),
            ("superadmin", Role::SuperAdmin),
            ("super_admin", Role::SuperAdmin),
        ] {
            assert_eq!(Role::from_str(s), Some(expected), "failed for: {}", s);
        }
    }

    #[test]
    fn role_from_str_unknown_returns_none() {
        assert_eq!(Role::from_str("god"), None);
        assert_eq!(Role::from_str(""), None);
    }

    // ── Scope string checks ───────────────────────────────────────────────────

    #[test]
    fn check_role_permission_str_works() {
        assert!(check_role_permission_str(Role::Operator, "alerts:acknowledge"));
        assert!(!check_role_permission_str(Role::Viewer, "query:execute"));
        assert!(!check_role_permission_str(Role::Admin, "unknown:action"));
    }

    // ── ABAC integration ──────────────────────────────────────────────────────

    #[test]
    fn abac_context_includes_role_attribute() {
        let ctx = build_rbac_abac_context(
            "user-1",
            Role::Analyst,
            "query:execute",
            "entity",
            "ent-1",
        );
        assert_eq!(ctx.subject.role.as_deref(), Some("analyst"));
        assert!(ctx.subject.permissions.contains(&"query:execute".to_string()));
    }

    #[test]
    fn register_rbac_policies_into_abac_engine() {
        let engine = crate::abac::AbacEngine::new();
        register_rbac_policies(&engine);

        // Operator can acknowledge alerts via ABAC engine
        let ctx = build_rbac_abac_context(
            "op-1",
            Role::Operator,
            "alerts:acknowledge",
            "alert",
            "alert-1",
        );
        assert!(engine.is_allowed(&ctx));

        // Viewer cannot execute queries via ABAC engine
        let ctx_deny = build_rbac_abac_context(
            "view-1",
            Role::Viewer,
            "query:execute",
            "entity",
            "ent-1",
        );
        assert!(!engine.is_allowed(&ctx_deny));
    }

    #[test]
    fn superadmin_allowed_for_system_config_in_abac() {
        let engine = crate::abac::AbacEngine::new();
        register_rbac_policies(&engine);

        let ctx = build_rbac_abac_context(
            "sa-1",
            Role::SuperAdmin,
            "system:config",
            "system",
            "sys-1",
        );
        assert!(engine.is_allowed(&ctx));
    }
}
