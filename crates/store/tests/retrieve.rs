use cairn_core::capture::WorkContext;
use cairn_core::memory::{Scope, Status};
use cairn_core::process::jobs::JobKind;
use cairn_core::retrieve::hybrid::fuse;
use cairn_store::testing::{TestDb, rule, save_input};

/// A unit vector with `weight` on axis 0 and the rest on axis `axis`, so cosine to axis 0 is `weight`.
fn tilted(weight: f32, axis: usize) -> Vec<f32> {
    let mut v = vec![0.0; 384];
    v[0] = weight;
    v[axis] = (1.0 - weight * weight).sqrt();
    v
}

#[tokio::test]
async fn embeddings_upsert_by_source_hash() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let id = store
        .save(&me, rule("Embed me"), &WorkContext::default())
        .await
        .unwrap()
        .id;

    let pending = store.pending_embeddings(10).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(
        pending[0]
            .text
            .starts_with("Embed me\nEmbed me\nEmbed me\n")
    );

    assert!(
        store
            .upsert_embedding(&id, &pending[0].source_hash, &tilted(1.0, 1))
            .await
            .unwrap()
    );
    assert!(
        !store
            .upsert_embedding(&id, &pending[0].source_hash, &tilted(1.0, 2))
            .await
            .unwrap(),
        "same hash is a no-op"
    );
    assert_eq!(
        store.embedding_source_hash(&id).await.unwrap().as_deref(),
        Some(pending[0].source_hash.as_str())
    );
    assert!(
        store
            .upsert_embedding(&id, "other-hash", &tilted(1.0, 2))
            .await
            .unwrap(),
        "new hash replaces the vector"
    );
    assert!(store.pending_embeddings(10).await.unwrap().is_empty());
    assert!(
        store.upsert_embedding(&id, "h", &[1.0, 0.0]).await.is_err(),
        "wrong dimensions"
    );
    assert!(
        store
            .upsert_embedding("mem_missing", "h", &tilted(1.0, 1))
            .await
            .is_ok_and(|written| !written)
    );

    // Saving enqueued the embed job; a written vector enqueued consolidate.
    assert_eq!(store.claim_jobs(JobKind::Embed, 5).await.unwrap().len(), 1);
    assert_eq!(
        store
            .claim_jobs(JobKind::Consolidate, 5)
            .await
            .unwrap()
            .len(),
        1
    );
    db.finish().await;
}

#[tokio::test]
async fn candidate_channels_respect_visibility_and_fuse() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let other = db.person("other").await;
    let work = WorkContext {
        files: vec!["crates/store/src/lib.rs".into()],
        ..WorkContext::default()
    };
    let pool = store
        .save(&me, save_input(serde_json::json!({"kind":"fact","title":"Connection pool","summary":"The pool sets search_path on connect.","reason":"r"})), &work)
        .await
        .unwrap()
        .id;
    let unrelated = store
        .save(&me, rule("Naming"), &WorkContext::default())
        .await
        .unwrap()
        .id;
    for (id, vector) in [(&pool, tilted(1.0, 1)), (&unrelated, tilted(0.2, 2))] {
        store.upsert_embedding(id, "h", &vector).await.unwrap();
    }

    let lexical = store
        .lexical_candidates(&me, "pool search_path", 10)
        .await
        .unwrap();
    assert_eq!(
        lexical
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        [pool.as_str()]
    );
    let by_entity = store
        .entity_candidates(&me, &["crates/store/src/lib.rs".to_owned()], 10)
        .await
        .unwrap();
    assert_eq!(by_entity, [(pool.clone(), 1.0)]);
    let semantic = store
        .vector_candidates(&me, &tilted(1.0, 1), 10)
        .await
        .unwrap();
    assert_eq!(semantic[0].0, pool);
    assert!(semantic[0].1 > 0.99 && semantic[1].1 < 0.5);

    for channel in [
        store.lexical_candidates(&other, "pool", 10).await.unwrap(),
        store
            .entity_candidates(&other, &["crates/store/src/lib.rs".to_owned()], 10)
            .await
            .unwrap(),
        store
            .vector_candidates(&other, &tilted(1.0, 1), 10)
            .await
            .unwrap(),
    ] {
        assert!(channel.is_empty(), "another person's memories never appear");
    }
    let fused = fuse(vec![lexical, by_entity, semantic]);
    assert_eq!(fused[0].0, pool);
    assert!(
        store
            .lexical_candidates(&me, "   ", 10)
            .await
            .unwrap()
            .is_empty(),
        "blank query matches nothing"
    );
    db.finish().await;
}

/// 10,000 synthetic rows: the three channels use their indexes, and the planner says so.
#[tokio::test]
async fn explain_uses_the_search_vector_and_entity_indexes() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    sqlx::query(
        "INSERT INTO memories (memory_id, owner_id, scope, kind, title, summary, reason, fields, evidence, evidence_hash, identity_hash)
         SELECT 'm' || n, $1, 'personal', 'fact', 'topic zeta' || n || 'x',
                repeat('summary words and some filler text ', 6),
                repeat('a reason that explains why this was saved ', 12),
                jsonb_build_object('source', repeat('where the fact came from ', 8)),
                ARRAY['crates/store/src/lib.rs', 'docs/specs/0.1.6-foundation.md'], 'e', 'i' || n
           FROM generate_series(1, 10000) AS n",
    )
    .bind(&me)
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO entities (memory_id, kind, name) SELECT 'm' || n, 'file', 'src/file' || n || '.rs' FROM generate_series(1, 10000) AS n",
    )
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO memory_embeddings (memory_id, model, source_hash, embedding)
         SELECT m.memory_id, 'm', 'h', v.embedding::vector
           FROM memories m,
                LATERAL (SELECT array_agg(random()::real) AS embedding FROM generate_series(1, 384) g WHERE m.memory_id IS NOT NULL) v",
    )
    .execute(store.pool())
    .await
    .unwrap();
    for table in ["memories", "entities", "memory_embeddings"] {
        sqlx::raw_sql(&format!("ANALYZE {table}"))
            .execute(store.pool())
            .await
            .unwrap();
    }

    let probe = format!("[{}]", vec!["0.05"; 384].join(","));
    let plans = [
        ("SELECT memory_id FROM memories WHERE search_tsv @@ websearch_to_tsquery('english', 'zeta77x')".to_owned(), "memories_search_idx"),
        (format!("SELECT memory_id FROM memory_embeddings ORDER BY embedding <=> '{probe}'::vector LIMIT 50"), "memory_embeddings_hnsw"),
        ("SELECT memory_id FROM entities WHERE name = ANY(ARRAY['src/file77.rs','src/file1.rs'])".to_owned(), "entities_lookup_idx"),
    ];
    for (sql, index) in plans {
        let plan: Vec<String> = sqlx::query_scalar(&format!("EXPLAIN {sql}"))
            .fetch_all(store.pool())
            .await
            .unwrap();
        let plan = plan.join("\n");
        assert!(plan.contains(index), "expected {index} in plan:\n{plan}");
    }
    // The real channels run on the same data.
    assert_eq!(
        store
            .entity_candidates(&me, &["src/file77.rs".to_owned()], 5)
            .await
            .unwrap(),
        [("m77".to_owned(), 1.0)]
    );
    assert!(
        !store
            .lexical_candidates(&me, "zeta77x", 5)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .vector_candidates(&me, &tilted(1.0, 1), 50)
            .await
            .unwrap()
            .len(),
        50
    );
    db.finish().await;
}

#[tokio::test]
async fn consolidate_supersedes_in_cohort_and_relates_in_band() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let other = db.person("other").await;
    let work = WorkContext::default();
    let mut ids = Vec::new();
    for title in ["alpha", "beta", "gamma", "delta"] {
        let mut input = rule(title);
        input.evidence = vec![format!("evidence-{title}")];
        ids.push(store.save(&me, input, &work).await.unwrap().id);
    }
    let (alpha, beta, gamma, delta) = (
        ids[0].clone(),
        ids[1].clone(),
        ids[2].clone(),
        ids[3].clone(),
    );
    // A fact with the same vector as alpha: other kind, so no cohort.
    let fact = store
        .save(
            &me,
            save_input(
                serde_json::json!({"kind":"fact","title":"same text","summary":"s","reason":"r"}),
            ),
            &work,
        )
        .await
        .unwrap()
        .id;
    // The same vector under another owner.
    let theirs = store.save(&other, rule("theirs"), &work).await.unwrap().id;
    // A global rule of mine: other scope.
    let mut global = rule("global");
    global.scope = Some(Scope::Global);
    let global = store.save(&me, global, &work).await.unwrap().id;

    store
        .upsert_embedding(&alpha, "h", &tilted(1.0, 1))
        .await
        .unwrap();
    store
        .upsert_embedding(&beta, "h", &tilted(0.95, 1))
        .await
        .unwrap(); // cos 0.95 to alpha: supersede
    store
        .upsert_embedding(&gamma, "h", &tilted(0.80, 2))
        .await
        .unwrap(); // 0.80: relates only
    store
        .upsert_embedding(&delta, "h", &tilted(0.30, 3))
        .await
        .unwrap(); // 0.30: left alone
    for id in [&fact, &theirs, &global] {
        store
            .upsert_embedding(id, "h", &tilted(1.0, 1))
            .await
            .unwrap();
    }

    // beta is newer than alpha, so beta supersedes alpha and takes its evidence.
    let outcome = store.consolidate_memory(&beta).await.unwrap();
    assert_eq!(outcome.superseded, std::slice::from_ref(&alpha));
    let alpha_memory = store.get(&me, &alpha).await.unwrap();
    assert_eq!(alpha_memory.status, Status::Superseded, "stays in history");
    let beta_memory = store.get(&me, &beta).await.unwrap();
    assert_eq!(beta_memory.status, Status::Active);
    assert_eq!(beta_memory.evidence, ["evidence-beta", "evidence-alpha"]);
    assert_eq!(beta_memory.evidence_count, 2);
    assert!(
        beta_memory
            .links
            .iter()
            .any(|l| l.auto && l.source == beta && l.target == alpha)
    );
    for untouched in [&fact, &theirs, &global] {
        let m = store.visible_memories(&me).await.unwrap();
        assert!(
            m.iter()
                .filter(|m| &m.id == untouched)
                .all(|m| m.status == Status::Active)
        );
    }
    assert_eq!(
        store.get(&other, &theirs).await.unwrap().status,
        Status::Active
    );

    // gamma (0.80 to alpha) relates; alpha is superseded and no longer a candidate, so its
    // neighbours among live memories are beta (cos 0.80*0.95+...), which is below the band here.
    let gamma_outcome = store.consolidate_memory(&gamma).await.unwrap();
    assert!(gamma_outcome.superseded.is_empty());
    let delta_outcome = store.consolidate_memory(&delta).await.unwrap();
    assert!(delta_outcome.superseded.is_empty() && delta_outcome.related.is_empty());

    // Running it again changes nothing.
    assert_eq!(
        store.consolidate_memory(&beta).await.unwrap(),
        Default::default()
    );

    // A declared contradiction is never overridden.
    let mut disputing = rule("epsilon");
    disputing.contradicts = Some(beta.clone());
    let epsilon = store.save(&me, disputing, &work).await.unwrap().id;
    store
        .upsert_embedding(&epsilon, "h", &tilted(0.97, 1))
        .await
        .unwrap();
    let disputed = store.consolidate_memory(&epsilon).await.unwrap();
    assert!(
        disputed.superseded.is_empty() && !disputed.related.contains(&beta),
        "{disputed:?}"
    );
    assert_eq!(store.get(&me, &beta).await.unwrap().status, Status::Active);
    db.finish().await;
}

#[tokio::test]
async fn relates_band_adds_one_auto_link() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let a = store
        .save(&me, rule("first"), &WorkContext::default())
        .await
        .unwrap()
        .id;
    let b = store
        .save(&me, rule("second"), &WorkContext::default())
        .await
        .unwrap()
        .id;
    store
        .upsert_embedding(&a, "h", &tilted(1.0, 1))
        .await
        .unwrap();
    store
        .upsert_embedding(&b, "h", &tilted(0.80, 2))
        .await
        .unwrap();
    let outcome = store.consolidate_memory(&b).await.unwrap();
    assert_eq!(outcome.related, std::slice::from_ref(&a));
    assert!(outcome.superseded.is_empty());
    assert_eq!(
        store.consolidate_memory(&a).await.unwrap().related.len(),
        0,
        "the pair is linked once"
    );
    let memory = store.get(&me, &a).await.unwrap();
    assert_eq!(memory.status, Status::Active);
    assert_eq!(memory.links.len(), 1);
    assert!(memory.links[0].auto);
    db.finish().await;
}
