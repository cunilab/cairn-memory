use super::*;
use crate::testing::TestDb;

#[tokio::test]
async fn essential_order_ranks_proof_and_votes_then_use_then_recency() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let ids: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT m.memory_id FROM (VALUES
            ('new', 1, 4), ('old', 1, 1), ('used', 1, 2),
            ('helpful', 1, 1), ('proven-repeat', 3, 1), ('proven-votes', 1, 1)
         ) AS m(memory_id, proof_count, updated_at)
         LEFT JOIN (VALUES
            ('used', 10, 0), ('helpful', 0, 1), ('proven-votes', 0, 2)
         ) AS u(memory_id, fetched, helpful) USING (memory_id)
         {}",
        essential_order()
    ))
    .fetch_all(db.store.pool())
    .await
    .unwrap();
    assert_eq!(
        ids,
        [
            "proven-votes",
            "proven-repeat",
            "used",
            "helpful",
            "new",
            "old"
        ]
    );
    db.finish().await;
}
