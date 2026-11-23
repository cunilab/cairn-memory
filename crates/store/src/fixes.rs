use crate::{Result, Store};

impl Store {
    pub async fn record_lookup(
        &self,
        identity: (&str, &str),
        session_id: &str,
        kind: &str,
        query: Option<&str>,
        ids: &[String],
        hit: Option<bool>,
    ) -> Result<()> {
        sqlx::query("INSERT INTO lookup_log (person_id,device_id,session_id,kind,query,result_ids,hit) VALUES ($1,$2,$3,$4,$5,$6,$7)")
            .bind(identity.0).bind(identity.1).bind(session_id).bind(kind).bind(query).bind(ids).bind(hit)
            .execute(&self.pool).await?;
        Ok(())
    }
}
