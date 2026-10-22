//! Durable audit trail of admin requests (#1267), written by the `AdminUser`
//! extractor once a caller has passed every admin check.

use sqlx::PgPool;
use uuid::Uuid;

/// Longest `action` stored; paths are bounded but query strings are not.
const MAX_ACTION_LEN: usize = 255;

/// `("users", Some(id))` for `/admin/users/{id}/...`, `("overview", None)`
/// for `/admin/overview`, `(None, None)` for `/admin` itself.
fn resource_from_path(path: &str) -> (Option<String>, Option<Uuid>) {
    let mut segments = path
        .trim_start_matches('/')
        .split('/')
        .skip(1)
        .filter(|s| !s.is_empty());
    let resource_type = segments.next().map(str::to_string);
    let resource_id = segments.next().and_then(|s| Uuid::parse_str(s).ok());
    (resource_type, resource_id)
}

pub async fn record(
    db: &PgPool,
    admin_user_id: Uuid,
    method: &str,
    path: &str,
    ip_address: Option<String>,
) -> Result<(), sqlx::Error> {
    let mut action = format!("{method} {path}");
    if action.len() > MAX_ACTION_LEN {
        let mut end = MAX_ACTION_LEN;
        while !action.is_char_boundary(end) {
            end -= 1;
        }
        action.truncate(end);
    }
    let (resource_type, resource_id) = resource_from_path(path);
    sqlx::query(
        "INSERT INTO admin_audit_log (admin_user_id, action, resource_type, resource_id, ip_address)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(admin_user_id)
    .bind(action)
    .bind(resource_type)
    .bind(resource_id)
    .bind(ip_address)
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_is_derived_from_the_admin_path() {
        let id = Uuid::new_v4();
        assert_eq!(resource_from_path("/admin"), (None, None));
        assert_eq!(
            resource_from_path("/admin/overview"),
            (Some("overview".into()), None)
        );
        assert_eq!(
            resource_from_path(&format!("/admin/users/{id}/unlock")),
            (Some("users".into()), Some(id))
        );
        assert_eq!(
            resource_from_path("/admin/users/not-a-uuid"),
            (Some("users".into()), None)
        );
    }
}
