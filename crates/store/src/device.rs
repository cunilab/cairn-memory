use crate::{Result, Store, StoreError, epoch};
use cairn_core::account::{
    Account, ClientInfo, ClientKind, DEVICE_TOKEN_PREFIX, Device, DeviceStatus,
    device::LAST_SEEN_THROTTLE_SECONDS, hash_token, random_token,
};
use cairn_core::id::new_id;
use sqlx::Row;
use sqlx::postgres::PgRow;

/// A device credential: the token is shown once and stored hashed.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, serde::Serialize)]
pub struct DeviceRegistration {
    pub device: Device,
    pub token: String,
}

fn device_from_row(row: &PgRow) -> Result<Device> {
    Ok(Device {
        device_id: row.try_get("device_id")?,
        person_id: row.try_get("person_id")?,
        name: row.try_get("name")?,
        status: if row.try_get::<String, _>("status")? == "revoked" {
            DeviceStatus::Revoked
        } else {
            DeviceStatus::Active
        },
        created_at: row.try_get("created_at")?,
        last_seen_at: row.try_get("last_seen_at")?,
        agent: row.try_get("agent")?,
        client_version: row.try_get("client_version")?,
        last_hook_at: row.try_get("last_hook_at")?,
        legacy_hook: row.try_get("legacy_hook")?,
    })
}

const DEVICE_COLUMNS: &str = "device_id, person_id, name, status,
    EXTRACT(EPOCH FROM created_at)::bigint AS created_at,
    EXTRACT(EPOCH FROM last_seen_at)::bigint AS last_seen_at,
    agent, client_version, legacy_hook, EXTRACT(EPOCH FROM last_hook_at)::bigint AS last_hook_at";

impl Store {
    /// A new device, or a new token for an existing active device id of the same person.
    pub async fn register_device(
        &self,
        person_id: &str,
        device_id: Option<&str>,
        name: &str,
    ) -> Result<DeviceRegistration> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::Validation(
                "device name must not be empty".into(),
            ));
        }
        let device_id = device_id.map_or_else(|| new_id("dev"), str::to_owned);
        let token = random_token("cairn-device");
        let mut tx = self.pool.begin().await?;
        let existing =
            sqlx::query("SELECT person_id, status FROM devices WHERE device_id = $1 FOR UPDATE")
                .bind(&device_id)
                .fetch_optional(&mut *tx)
                .await?;
        let query = if let Some(existing) = existing {
            if existing.try_get::<String, _>("person_id")? != person_id {
                return Err(StoreError::Validation(
                    "device already belongs to another person".into(),
                ));
            }
            if existing.try_get::<String, _>("status")? == "revoked" {
                return Err(StoreError::Validation(
                    "device is revoked; register a new device id".into(),
                ));
            }
            format!(
                "UPDATE devices SET token_hash = $3, name = $2, last_seen_at = now() WHERE device_id = $1 RETURNING {DEVICE_COLUMNS}"
            )
        } else {
            format!(
                "INSERT INTO devices (device_id, name, token_hash, person_id) VALUES ($1, $2, $3, $4) RETURNING {DEVICE_COLUMNS}"
            )
        };
        let row = sqlx::query(&query)
            .bind(&device_id)
            .bind(name)
            .bind(hash_token(&token))
            .bind(person_id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(DeviceRegistration {
            device: device_from_row(&row)?,
            token,
        })
    }

    /// Replaces the token of an active device. The old token stops working at once.
    pub async fn rotate_device(
        &self,
        person_id: &str,
        device_id: &str,
    ) -> Result<DeviceRegistration> {
        let name: Option<String> =
            sqlx::query_scalar("SELECT name FROM devices WHERE device_id = $1 AND person_id = $2 AND status = 'active'")
                .bind(device_id)
                .bind(person_id)
                .fetch_optional(&self.pool)
                .await?;
        let name = name.ok_or_else(|| StoreError::NotFound("device".into()))?;
        self.register_device(person_id, Some(device_id), &name)
            .await
    }

    pub async fn revoke_device(&self, person_id: &str, device_id: &str) -> Result<()> {
        let done = sqlx::query(
            "UPDATE devices SET status = 'revoked' WHERE device_id = $1 AND person_id = $2",
        )
        .bind(device_id)
        .bind(person_id)
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound("device".into()));
        }
        Ok(())
    }

    pub async fn list_devices(&self, person_id: &str) -> Result<Vec<Device>> {
        let rows = sqlx::query(&format!("SELECT {DEVICE_COLUMNS} FROM devices WHERE person_id = $1 ORDER BY created_at, device_id"))
            .bind(person_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(device_from_row).collect()
    }

    /// The device and account behind an active device token. `last_seen_at` moves at most once a
    /// minute, so read-only calls do not rewrite rows.
    pub async fn authenticate_device(&self, token: &str) -> Result<(Device, Account)> {
        let denied = || StoreError::Forbidden("invalid device token".into());
        if !token.starts_with(DEVICE_TOKEN_PREFIX) {
            return Err(denied());
        }
        let row = sqlx::query(&format!(
            "SELECT d.device_id, d.person_id, d.name, d.status,
                    {} AS created_at, {} AS last_seen_at,
                    d.agent, d.client_version, d.legacy_hook, {} AS last_hook_at,
                    a.email, a.is_admin, a.status AS account_status,
                    {} AS account_created_at
               FROM devices d JOIN accounts a USING (person_id)
              WHERE d.token_hash = $1 AND d.status = 'active' AND a.status = 'active'",
            epoch!("d.created_at"),
            epoch!("d.last_seen_at"),
            epoch!("d.last_hook_at"),
            epoch!("a.created_at")
        ))
        .bind(hash_token(token))
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(denied)?;
        sqlx::query(&format!(
            "UPDATE devices SET last_seen_at = now()
              WHERE device_id = $1 AND last_seen_at < now() - make_interval(secs => {LAST_SEEN_THROTTLE_SECONDS})"
        ))
        .bind(row.try_get::<String, _>("device_id")?)
        .execute(&self.pool)
        .await?;
        let account = Account {
            person_id: row.try_get("person_id")?,
            email: row.try_get("email")?,
            is_admin: row.try_get("is_admin")?,
            status: cairn_core::account::AccountStatus::Active,
            created_at: row.try_get("account_created_at")?,
        };
        Ok((device_from_row(&row)?, account))
    }

    /// Remembers who a device's client says it is (`User-Agent`). A hook call is a check-in
    /// (`last_hook_at`), at most once a minute; nothing is written when nothing changed. An MCP
    /// or CLI call names the client but is not a check-in, and an agent already known is kept.
    pub async fn note_client(&self, device_id: &str, info: &ClientInfo) -> Result<()> {
        let hook = info.legacy || info.kind == Some(ClientKind::Hook);
        sqlx::query(&format!(
            "UPDATE devices d SET client_version = n.version, agent = n.agent, legacy_hook = n.legacy,
                    last_hook_at = CASE WHEN $4 THEN now() ELSE d.last_hook_at END
               FROM (SELECT x.device_id, COALESCE($2::text, x.client_version) AS version,
                            COALESCE($3::text, x.agent) AS agent,
                            CASE WHEN $5 THEN true WHEN $2::text IS NOT NULL THEN false ELSE x.legacy_hook END AS legacy
                       FROM devices x WHERE x.device_id = $1) n
              WHERE d.device_id = n.device_id
                AND (d.client_version IS DISTINCT FROM n.version OR d.agent IS DISTINCT FROM n.agent
                     OR d.legacy_hook IS DISTINCT FROM n.legacy
                     OR ($4 AND (d.last_hook_at IS NULL
                                 OR d.last_hook_at < now() - make_interval(secs => {LAST_SEEN_THROTTLE_SECONDS}))))"
        ))
        .bind(device_id)
        .bind(&info.version)
        .bind(&info.agent)
        .bind(hook)
        .bind(info.legacy)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

impl Store {
    pub async fn record_hook_health(
        &self,
        person_id: &str,
        device_id: &str,
        class: Option<&str>,
        at: Option<i64>,
        depth: i32,
    ) -> Result<()> {
        sqlx::query("UPDATE devices SET outbox_depth=$3, hook_error_class=COALESCE($4,hook_error_class), hook_error_at=COALESCE(CASE WHEN $5::bigint <= EXTRACT(EPOCH FROM now())::bigint + 86400 THEN to_timestamp($5::double precision) END,hook_error_at) WHERE person_id=$1 AND device_id=$2")
            .bind(person_id).bind(device_id).bind(depth).bind(class).bind(at).execute(&self.pool).await?;
        Ok(())
    }
}
