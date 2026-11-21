use cairn_core::capture::WorkContext;
use cairn_core::memory::{Scope, Status};
use cairn_core::process::jobs::JobKind;
use cairn_core::retrieve::hybrid::fuse;
use cairn_store::retrieve::Narrow;
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
        .save(&me, save_input(serde_json::json!({"kind":"fact","title":"Connection pool","summary":"The pool sets search_path on connect.","reason":"r and the test says why"})), &work)
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

    let mine = store.member_projects(&me).await.unwrap();
    let theirs = store.member_projects(&other).await.unwrap();
    let lexical = store
        .lexical_candidates(&me, &mine, "pool search_path", 10, &Narrow::NONE)
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
        .entity_candidates(
            &me,
            &mine,
            &["crates/store/src/lib.rs".to_owned()],
            10,
            &Narrow::NONE,
        )
        .await
        .unwrap();
    assert_eq!(by_entity, [(pool.clone(), 1.0)]);
    let semantic = store
        .vector_candidates(&me, &mine, &tilted(1.0, 1), 10, &Narrow::NONE)
        .await
        .unwrap();
    assert_eq!(semantic[0].0, pool);
    assert!(semantic[0].1 > 0.99 && semantic[1].1 < 0.5);

    for channel in [
        store
            .lexical_candidates(&other, &theirs, "pool", 10, &Narrow::NONE)
            .await
            .unwrap(),
        store
            .entity_candidates(
                &other,
                &theirs,
                &["crates/store/src/lib.rs".to_owned()],
                10,
                &Narrow::NONE,
            )
            .await
            .unwrap(),
        store
            .vector_candidates(&other, &theirs, &tilted(1.0, 1), 10, &Narrow::NONE)
            .await
            .unwrap(),
    ] {
        assert!(channel.is_empty(), "another person's memories never appear");
    }
    let fused = fuse(vec![lexical, by_entity, semantic]);
    assert_eq!(fused[0].0, pool);
    assert!(
        store
            .lexical_candidates(&me, &mine, "   ", 10, &Narrow::NONE)
            .await
            .unwrap()
            .is_empty(),
        "blank query matches nothing"
    );
    db.finish().await;
}

/// A deterministic 384-dimension vector with no structure, from a seed.
fn noise(seed: u64) -> Vec<f32> {
    let raw = raw_noise(seed);
    let norm = raw.iter().map(|x| x * x).sum::<f32>().sqrt();
    raw.into_iter().map(|x| x / norm).collect()
}

fn raw_noise(seed: u64) -> Vec<f32> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..384)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % 2001) as f32 / 1000.0 - 1.0
        })
        .collect()
}

/// `(owner, scope, rows)` groups of synthetic memories `m1..` with deterministic noise vectors and
/// a file entity each, then the HNSW index built over them (faster than row by row) and ANALYZE.
async fn synthetic(db: &TestDb, groups: &[(&str, &str, i32)]) {
    sqlx::raw_sql("DROP INDEX memories_embedding_hnsw")
        .execute(db.store.pool())
        .await
        .unwrap();
    let mut first = 1;
    for (owner, scope, rows) in groups {
        sqlx::query(
            "INSERT INTO memories (memory_id, owner_id, scope, kind, title, summary, reason, fields,
                                   evidence, evidence_hash, identity_hash, embedding, embedding_model,
                                   embedding_source)
             SELECT 'm' || n, $1, $2, 'fact', 'topic zeta' || n || 'x',
                    repeat('summary words and some filler text ', 6),
                    repeat('a reason that explains why this was saved ', 12),
                    jsonb_build_object('source', repeat('where the fact came from ', 8)),
                    ARRAY['crates/store/src/lib.rs'], 'e', 'i' || n,
                    (SELECT array_agg(((hashtext(n::text || ':' || d::text) % 2001) / 1000.0)::float4 ORDER BY d)
                       FROM generate_series(1, 384) d)::vector, 'm', 'h'
               FROM generate_series($3, $4) n",
        )
        .bind(owner)
        .bind(scope)
        .bind(first)
        .bind(first + rows - 1)
        .execute(db.store.pool())
        .await
        .unwrap();
        first += rows;
    }
    sqlx::query(
        "INSERT INTO entities (memory_id, kind, name) SELECT memory_id, 'file', 'src/file' || substr(memory_id, 2) || '.rs' FROM memories",
    )
    .execute(db.store.pool())
    .await
    .unwrap();
    sqlx::raw_sql(
        "CREATE INDEX memories_embedding_hnsw ON memories USING hnsw (embedding vector_cosine_ops)
             WHERE embedding IS NOT NULL AND status IN ('active','stale')",
    )
    .execute(db.store.pool())
    .await
    .unwrap();
    // VACUUM empties the GIN pending list, which the planner would otherwise charge to the index.
    for statement in ["VACUUM ANALYZE memories", "ANALYZE entities"] {
        sqlx::raw_sql(statement)
            .execute(db.store.pool())
            .await
            .unwrap();
    }
}

/// The plan of `sql` with its four binds `$1 person`, `$2 text`, `$3 projects`, `$4 limit`.
async fn explain(store: &cairn_store::Store, sql: &str, person: &str, text: &str) -> String {
    let plan: Vec<String> = sqlx::query_scalar(&format!("EXPLAIN (FORMAT TEXT) {sql}"))
        .bind(person)
        .bind(text)
        .bind(Vec::<String>::new())
        .bind(64_i64)
        .fetch_all(store.pool())
        .await
        .unwrap();
    plan.join("\n")
}

#[tokio::test]
async fn vector_filter_inside_scan() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let p = db.person("p").await;
    let mut others = Vec::new();
    for name in ["q", "r", "s", "t"] {
        others.push(db.person(name).await);
    }
    // P owns 200 of 10,000 rows (2 %); everything else belongs to four other people.
    synthetic(
        &db,
        &[
            (&p, "personal", 200),
            (&others[0], "personal", 3000),
            (&others[1], "personal", 3000),
            (&others[2], "personal", 2000),
            (&others[3], "personal", 1800),
        ],
    )
    .await;
    // The owner btree would answer this selective filter exactly; take it away so the HNSW
    // index is the only ordered path, and keep the planner off a sequential scan.
    sqlx::raw_sql("DROP INDEX memories_owner_idx, memories_project_idx, memories_global_idx, memories_identity_idx, memories_expiry_idx")
        .execute(store.pool())
        .await
        .unwrap();
    let url = format!("{}?options=-c%20enable_seqscan%3Doff", db.url());
    let indexed = cairn_store::Store::connect(&url, store.schema())
        .await
        .unwrap();
    let query = noise(999_999);
    let projects = indexed.member_projects(&p).await.unwrap();

    let plan = explain(
        &indexed,
        &cairn_store::retrieve::vector_sql(&Narrow::NONE),
        &p,
        &vector_literal(&query),
    )
    .await;
    assert!(plan.contains("memories_embedding_hnsw"), "{plan}");

    let found = indexed
        .vector_candidates(&p, &projects, &query, 64, &Narrow::NONE)
        .await
        .unwrap();
    assert_eq!(found.len(), 64, "a 2 % owner still gets K rows");
    assert!(
        found.windows(2).all(|w| w[0].1 >= w[1].1),
        "ordered by cosine"
    );
    let owners: Vec<String> =
        sqlx::query_scalar("SELECT owner_id FROM memories WHERE memory_id = ANY($1)")
            .bind(found.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>())
            .fetch_all(store.pool())
            .await
            .unwrap();
    assert!(
        owners.iter().all(|owner| owner == &p),
        "another person's memories never come from the vector channel"
    );

    // The exact answer, by a sequential scan and sort.
    let mut tx = store.pool().begin().await.unwrap();
    sqlx::raw_sql("SET LOCAL enable_indexscan = off; SET LOCAL enable_bitmapscan = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let exact: Vec<String> = sqlx::query_scalar::<_, String>(&format!(
        "SELECT memory_id FROM ({}) r",
        cairn_store::retrieve::vector_sql(&Narrow::NONE)
    ))
    .bind(&p)
    .bind(vector_literal(&query))
    .bind(&projects)
    .bind(64_i64)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    let ids: Vec<&str> = found.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, exact.iter().map(String::as_str).collect::<Vec<_>>());
    indexed.pool().close().await;
    db.finish().await;
}

#[tokio::test]
async fn plans_use_the_hnsw_gin_and_entity_indexes() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let heavy = db.person("heavy").await;
    // `heavy` owns nothing but sees the 5,000 global rows: half the table.
    synthetic(&db, &[(&me, "global", 5000), (&me, "personal", 5000)]).await;
    let vector = vector_literal(&noise(7));
    let plan = explain(
        store,
        &cairn_store::retrieve::vector_sql(&Narrow::NONE),
        &heavy,
        &vector,
    )
    .await;
    assert!(plan.contains("memories_embedding_hnsw"), "{plan}");
    let plan = explain(
        store,
        &cairn_store::retrieve::lexical_sql(&Narrow::NONE),
        &heavy,
        "zeta77x",
    )
    .await;
    assert!(plan.contains("memories_search_idx"), "{plan}");
    let plan: Vec<String> = sqlx::query_scalar("EXPLAIN SELECT memory_id FROM entities WHERE name = ANY(ARRAY['src/file77.rs','src/file1.rs'])")
        .fetch_all(store.pool())
        .await
        .unwrap();
    assert!(plan.join("\n").contains("entities_lookup_idx"), "{plan:?}");

    // The real channels run on the same data.
    let projects = store.member_projects(&heavy).await.unwrap();
    assert_eq!(
        store
            .entity_candidates(
                &me,
                &projects,
                &["src/file77.rs".to_owned()],
                5,
                &Narrow::NONE
            )
            .await
            .unwrap(),
        [("m77".to_owned(), 1.0)]
    );
    assert_eq!(
        store
            .lexical_candidates(&heavy, &projects, "zeta77x", 5, &Narrow::NONE)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .vector_candidates(&heavy, &projects, &noise(7), 64, &Narrow::NONE)
            .await
            .unwrap()
            .len(),
        64
    );
    db.finish().await;
}

#[tokio::test]
async fn lexical_reads_and_as_or_with_coverage_unless_negated() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let work = WorkContext::default();
    let only_drop = store
        .save(&me, save_input(serde_json::json!({"kind":"fact","title":"Drop table policy","summary":"Dropping a table needs two approvals.","reason":"r and the test says why"})), &work)
        .await
        .unwrap()
        .id;
    let guitar = store
        .save(&me, save_input(serde_json::json!({"kind":"fact","title":"Guitar tuning for drop D","summary":"Tune the low string down a whole step.","reason":"r and the test says why"})), &work)
        .await
        .unwrap()
        .id;
    let projects = store.member_projects(&me).await.unwrap();

    let query = "guitar tuning for drop D";
    let found = store
        .lexical_candidates(&me, &projects, query, 10, &Narrow::NONE)
        .await
        .unwrap();
    let ids: Vec<&str> = found.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids.first(),
        Some(&guitar.as_str()),
        "the full match ranks first"
    );
    assert!(
        ids.contains(&only_drop.as_str()),
        "one shared word is a candidate: {ids:?}"
    );

    let loaded = store
        .load_candidates(
            &me,
            &projects,
            &[only_drop.clone(), guitar.clone()],
            query,
            None,
        )
        .await
        .unwrap();
    let coverage = |id: &str| loaded.iter().find(|c| c.memory.id == id).unwrap().coverage;
    // The query's lexemes are guitar, tune, drop and d: one of four. (The spec's 0.33 counted three.)
    assert!(
        (coverage(&only_drop) - 0.25).abs() < 1e-6,
        "{}",
        coverage(&only_drop)
    );
    assert!((coverage(&guitar) - 1.0).abs() < 1e-6);
    assert!(
        loaded.iter().all(|c| c.cosine.is_none()),
        "no query vector, no cosine"
    );

    // A negation keeps AND semantics: both words are required and the excluded one forbidden.
    let negated = store
        .lexical_candidates(&me, &projects, "drop table -guitar", 10, &Narrow::NONE)
        .await
        .unwrap();
    assert_eq!(
        negated
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        [only_drop.as_str()]
    );
    let strict = store
        .lexical_candidates(&me, &projects, "guitar table -tuning", 10, &Narrow::NONE)
        .await
        .unwrap();
    assert!(
        strict.is_empty(),
        "AND of guitar and table matches no memory: {strict:?}"
    );
    db.finish().await;
}

#[tokio::test]
async fn candidate_load_reports_cosine_and_skips_unsearchable() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let work = WorkContext::default();
    let near = store.save(&me, rule("near"), &work).await.unwrap().id;
    let far = store.save(&me, rule("far"), &work).await.unwrap().id;
    let no_vector = store.save(&me, rule("none"), &work).await.unwrap().id;
    let gone = store.save(&me, rule("gone"), &work).await.unwrap().id;
    store
        .upsert_embedding(&near, "h", &tilted(0.9, 1))
        .await
        .unwrap();
    store
        .upsert_embedding(&far, "h", &tilted(0.2, 2))
        .await
        .unwrap();
    store.forget(&me, &gone).await.unwrap();
    let projects = store.member_projects(&me).await.unwrap();
    let ids = [
        near.clone(),
        far.clone(),
        no_vector.clone(),
        gone.clone(),
        "mem_missing".to_owned(),
    ];
    let probe = tilted(1.0, 1);
    let loaded = store
        .load_candidates(&me, &projects, &ids, "unrelated words", Some(&probe))
        .await
        .unwrap();
    let cosine = |id: &str| loaded.iter().find(|c| c.memory.id == id).map(|c| c.cosine);
    assert!((cosine(&near).unwrap().unwrap() - 0.9).abs() < 1e-4);
    assert!((cosine(&far).unwrap().unwrap() - 0.2).abs() < 1e-4);
    assert_eq!(cosine(&no_vector), Some(None));
    assert_eq!(cosine(&gone), None, "forgotten is not loaded");
    assert_eq!(loaded.len(), 3);
    assert!(loaded.iter().all(|c| c.coverage == 0.0));
    assert!(
        loaded
            .iter()
            .all(|c| (c.memory.created_us - c.memory.created_at * 1_000_000).abs() <= 500_000),
        "created_us is created_at in microseconds (created_at rounds to the nearest second)"
    );
    db.finish().await;
}

#[tokio::test]
async fn forgotten_superseded_and_expired_are_never_searchable() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let work = WorkContext {
        files: vec!["src/gone.rs".into()],
        ..WorkContext::default()
    };
    let mut ids = Vec::new();
    for title in [
        "alpha marker",
        "beta marker",
        "gamma marker",
        "delta marker",
    ] {
        ids.push(
            store
                .save(&me, save_input(serde_json::json!({"kind":"fact","title":title,"summary":"marker text is a summary of the test","reason":"r and the test says why"})), &work)
                .await
                .unwrap()
                .id,
        );
        store
            .upsert_embedding(ids.last().unwrap(), "h", &tilted(1.0, ids.len()))
            .await
            .unwrap();
    }
    let projects = store.member_projects(&me).await.unwrap();
    let channels = || async {
        (
            store
                .lexical_candidates(&me, &projects, "marker", 10, &Narrow::NONE)
                .await
                .unwrap()
                .len(),
            store
                .entity_candidates(
                    &me,
                    &projects,
                    &["src/gone.rs".to_owned()],
                    10,
                    &Narrow::NONE,
                )
                .await
                .unwrap()
                .len(),
            store
                .vector_candidates(&me, &projects, &tilted(1.0, 1), 10, &Narrow::NONE)
                .await
                .unwrap()
                .len(),
        )
    };
    assert_eq!(channels().await, (4, 4, 4));
    store.forget(&me, &ids[0]).await.unwrap();
    sqlx::query("UPDATE memories SET status = 'superseded' WHERE memory_id = $1")
        .bind(&ids[1])
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE memories SET expires_at = now() - interval '1 second' WHERE memory_id = $1",
    )
    .bind(&ids[2])
    .execute(store.pool())
    .await
    .unwrap();
    assert_eq!(channels().await, (1, 1, 1), "only the live memory remains");
    sqlx::query("UPDATE memories SET status = 'stale' WHERE memory_id = $1")
        .bind(&ids[3])
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(channels().await, (1, 1, 1), "stale stays searchable");
    db.finish().await;
}

/// Owner, project member, non-member and a global memory, through lexical, entity, vector and
/// neighbour channels.
#[tokio::test]
async fn visibility_matrix_for_all_four_channels() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let owner = db.person("owner").await;
    let member = db.person("member").await;
    let outsider = db.person("outsider").await;
    let (project, _) = store
        .init_project(&owner, "github.com/acme/matrix", None)
        .await
        .unwrap();
    store
        .join_project(&member, &project.project_id, None)
        .await
        .unwrap();
    let work = WorkContext {
        project_id: Some(project.project_id.clone()),
        files: vec!["src/matrix.rs".into()],
        ..WorkContext::default()
    };
    let (owner_ref, work_ref) = (&owner, &work);
    let save = |scope: &'static str, title: &'static str| async move {
        store
            .save(
                owner_ref,
                save_input(serde_json::json!({"kind":"fact","scope":scope,"title":title,"summary":"matrix probe is a summary of the test","reason":"r and the test says why"})),
                work_ref,
            )
            .await
            .unwrap()
            .id
    };
    let personal = save("personal", "personal matrix").await;
    let shared = save("project", "project matrix").await;
    let global = save("global", "global matrix").await;
    let seed = save("global", "link seed").await;
    for id in [&personal, &shared, &global, &seed] {
        store
            .upsert_embedding(id, "h", &tilted(1.0, 1))
            .await
            .unwrap();
    }
    for target in [&personal, &shared, &global] {
        sqlx::query("INSERT INTO links (source_id, target_id, relation, origin) VALUES ($1, $2, 'relates', 'agent')")
            .bind(&seed)
            .bind(target)
            .execute(store.pool())
            .await
            .unwrap();
    }
    let seeds = [(seed.clone(), 1.0_f32)];
    let sorted = |mut ids: Vec<String>| {
        ids.sort();
        ids
    };
    let ids = |rows: Vec<(String, f32)>| sorted(rows.into_iter().map(|(id, _)| id).collect());
    for (person, visible) in [
        (&owner, vec![&personal, &shared, &global]),
        (&member, vec![&shared, &global]),
        (&outsider, vec![&global]),
    ] {
        let projects = store.member_projects(person).await.unwrap();
        // The seed is global, so every channel also finds it; only the neighbour channel skips it.
        let neighbours_want = sorted(visible.iter().map(|id| (*id).clone()).collect());
        let channel_want = sorted(
            visible
                .iter()
                .map(|id| (*id).clone())
                .chain([seed.clone()])
                .collect(),
        );
        let lexical = ids(store
            .lexical_candidates(person, &projects, "matrix", 10, &Narrow::NONE)
            .await
            .unwrap());
        let entity = ids(store
            .entity_candidates(
                person,
                &projects,
                &["src/matrix.rs".to_owned()],
                10,
                &Narrow::NONE,
            )
            .await
            .unwrap());
        let vector = ids(store
            .vector_candidates(person, &projects, &tilted(1.0, 1), 10, &Narrow::NONE)
            .await
            .unwrap());
        let neighbours = ids(store
            .neighbours(person, &projects, &seeds, &Narrow::NONE)
            .await
            .unwrap());
        assert_eq!(lexical, channel_want, "lexical for {person}");
        assert_eq!(entity, channel_want, "entity for {person}");
        assert_eq!(vector, channel_want, "vector for {person}");
        assert_eq!(neighbours, neighbours_want, "neighbours for {person}");
    }
    db.finish().await;
}

#[tokio::test]
async fn search_counts_usage_but_topic_and_brief_do_not() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let work = WorkContext {
        files: vec!["src/usage.rs".into()],
        ..WorkContext::default()
    };
    let id = store
        .save(&me, save_input(serde_json::json!({"kind":"rule","scope":"personal","title":"Usage probe words","summary":"ledger audit trail is a summary of the test","reason":"r and the test says why"})), &work)
        .await
        .unwrap()
        .id;
    let fetched = || async {
        sqlx::query_scalar::<_, i64>("SELECT coalesce(sum(fetched), 0)::bigint FROM usage")
            .fetch_one(store.pool())
            .await
            .unwrap()
    };
    assert_eq!(fetched().await, 0);
    store.topic_notes(&me, "src/usage.rs", &work).await.unwrap();
    store.brief(&me, &work, &[]).await.unwrap();
    assert_eq!(fetched().await, 0, "topic and brief never count");
    let cards = store
        .search(
            &me,
            "ledger audit",
            None,
            5,
            1200,
            &cairn_core::retrieve::filter::Filter::default(),
        )
        .await
        .unwrap();
    assert_eq!(cards.results.first().map(|c| &c.id), Some(&id));
    assert_eq!(fetched().await, 1, "search counts the returned card");
    db.finish().await;
}

#[tokio::test]
async fn migration_requires_pgvector_0_8() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let version: String =
        sqlx::query_scalar("SELECT extversion FROM pg_extension WHERE extname = 'vector'")
            .fetch_one(db.store.pool())
            .await
            .unwrap();
    let parts: Vec<u32> = version.split('.').map(|p| p.parse().unwrap()).collect();
    assert!(
        parts >= vec![0, 8, 0],
        "pgvector {version} is older than the migration requires"
    );
    // The guard raises for an older version: run its test with a lowered bound.
    let guard = include_str!("../../../migrations/0003_retrieval.sql");
    let block = &guard[guard.find("DO $$").unwrap()..guard.find("END $$;").unwrap() + 7];
    let refused = sqlx::raw_sql(&block.replace("'{0,8,0}'", "'{99,0,0}'"))
        .execute(db.store.pool())
        .await;
    assert!(refused.is_err_and(|e| e.to_string().contains("pgvector >= 0.8.0 required")));
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
                serde_json::json!({"kind":"fact","title":"same text","summary":"s is a summary of the test","reason":"r and the test says why"}),
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

fn vector_literal(vector: &[f32]) -> String {
    let items: Vec<String> = vector.iter().map(f32::to_string).collect();
    format!("[{}]", items.join(","))
}

/// `count` bare memories written by SQL, `newer_by` seconds ahead of now so they sort first.
async fn bulk_memories(
    db: &TestDb,
    owner: &str,
    scope: (&str, Option<&str>),
    kind: &str,
    count: i32,
    newer_by: i32,
) {
    sqlx::query(
        "INSERT INTO memories (memory_id, owner_id, project_id, scope, kind, title, summary, evidence_hash,
                               identity_hash, fields, updated_at)
         SELECT 'bulk_' || $1 || '_' || $3 || '_' || i, $1, $4, $2, $3, 'bulk ' || i, 's', 'e',
                'bulk_' || $1 || '_' || $3 || '_' || i,
                CASE WHEN $3 = 'handoff' THEN '{\"next_steps\":[\"x\"]}'::jsonb ELSE '{}'::jsonb END,
                now() + make_interval(secs => $6)
           FROM generate_series(1, $5) i",
    )
    .bind(owner)
    .bind(scope.0)
    .bind(kind)
    .bind(scope.1)
    .bind(count)
    .bind(f64::from(newer_by))
    .execute(db.store.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn consolidate_finds_a_duplicate_among_many_other_peoples_vectors() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let other = db.person("other").await;
    let work = WorkContext::default();
    let a = db.store.save(&me, rule("alpha"), &work).await.unwrap().id;
    let b = db.store.save(&me, rule("beta"), &work).await.unwrap().id;
    db.store
        .upsert_embedding(&a, "h", &tilted(1.0, 1))
        .await
        .unwrap();
    db.store
        .upsert_embedding(&b, "h", &tilted(0.95, 1))
        .await
        .unwrap();
    // 200 rows of another person, all closer to beta than alpha is: more than hnsw's ef_search
    // window of 40, so an index scan that filters afterwards never reaches alpha.
    bulk_memories(&db, &other, ("personal", None), "rule", 200, 0).await;
    bulk_memories(&db, &me, ("personal", None), "rule", 200, 0).await;
    // Distinct vectors, since pgvector stores identical ones once. Others sit closer to beta than
    // alpha does; mine sit far away.
    sqlx::query(
        "UPDATE memories m SET embedding_model = 'm', embedding_source = 'h',
                embedding = (SELECT array_agg(CASE d WHEN 1 THEN 0.97::float4 WHEN 2 THEN 0.243::float4
                                         ELSE (abs(hashtext(m.memory_id || d::text)) % 1000 / 100000.0)::float4 END ORDER BY d)
                   FROM generate_series(1, 384) d)::vector
          WHERE m.owner_id = $1 AND m.memory_id LIKE 'bulk%'",
    )
    .bind(&other)
    .execute(db.store.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE memories SET embedding_model = 'm', embedding_source = 'h', embedding = $1::vector
          WHERE owner_id = $2 AND memory_id LIKE 'bulk%'",
    )
    .bind(vector_literal(&tilted(0.1, 2)))
    .bind(&me)
    .execute(db.store.pool())
    .await
    .unwrap();
    sqlx::query("ANALYZE memories")
        .execute(db.store.pool())
        .await
        .unwrap();
    // On a large table the planner takes the hnsw index for this query. Force that plan here:
    // no other index on memories, and a pool of its own that avoids seq, merge and hash joins.
    sqlx::raw_sql("DROP INDEX memories_owner_idx, memories_project_idx, memories_branch_idx, memories_identity_idx")
        .execute(db.store.pool())
        .await
        .unwrap();
    let url = format!(
        "{}?options=-c%20enable_seqscan%3Doff%20-c%20enable_mergejoin%3Doff%20-c%20enable_hashjoin%3Doff",
        db.url()
    );
    let indexed = cairn_store::Store::connect(&url, db.store.schema())
        .await
        .unwrap();
    let outcome = indexed.consolidate_memory(&b).await.unwrap();
    assert_eq!(outcome.superseded, std::slice::from_ref(&a), "{outcome:?}");
    indexed.pool().close().await;
    db.finish().await;
}

#[tokio::test]
async fn consolidate_does_not_merge_across_projects() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let (one, _) = store
        .init_project(&me, "github.com/acme/one", None)
        .await
        .unwrap();
    let (two, _) = store
        .init_project(&me, "github.com/acme/two", None)
        .await
        .unwrap();
    let in_project = |id: &str| WorkContext {
        project_id: Some(id.into()),
        ..WorkContext::default()
    };
    let a = store
        .save(&me, rule("alpha"), &in_project(&one.project_id))
        .await
        .unwrap()
        .id;
    let b = store
        .save(&me, rule("beta"), &in_project(&two.project_id))
        .await
        .unwrap()
        .id;
    let c = store
        .save(&me, rule("gamma"), &in_project(&one.project_id))
        .await
        .unwrap()
        .id;
    for (id, weight) in [(&a, 1.0), (&b, 0.95), (&c, 0.96)] {
        store
            .upsert_embedding(id, "h", &tilted(weight, 1))
            .await
            .unwrap();
    }
    // Same owner, scope and kind; only the project differs. Beta may not touch alpha or gamma.
    let outcome = store.consolidate_memory(&b).await.unwrap();
    assert!(
        outcome.superseded.is_empty() && outcome.related.is_empty(),
        "{outcome:?}"
    );
    assert_eq!(store.get(&me, &a).await.unwrap().status, Status::Active);
    // Within one project the same pair does merge.
    let outcome = store.consolidate_memory(&c).await.unwrap();
    assert_eq!(outcome.superseded, std::slice::from_ref(&a), "{outcome:?}");
    db.finish().await;
}

#[tokio::test]
async fn brief_keeps_project_rules_among_many_global_rules() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let other = db.person("other").await;
    let (project, _) = store
        .init_project(&me, "github.com/acme/cairn", None)
        .await
        .unwrap();
    let work = WorkContext {
        project_id: Some(project.project_id.clone()),
        ..WorkContext::default()
    };
    store.save(&me, rule("keep me"), &work).await.unwrap();
    bulk_memories(&db, &other, ("global", None), "rule", 520, 60).await;
    let brief = store.brief(&me, &work, &[]).await.unwrap();
    assert!(brief.contains("keep me"), "{brief}");
    db.finish().await;
}

#[tokio::test]
async fn diagnose_counts_only_my_jobs() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let other = db.person("other").await;
    store
        .save(&me, rule("mine"), &WorkContext::default())
        .await
        .unwrap();
    store
        .save(&other, rule("theirs one"), &WorkContext::default())
        .await
        .unwrap();
    store
        .save(&other, rule("theirs two"), &WorkContext::default())
        .await
        .unwrap();
    store
        .enqueue_job(
            JobKind::Distill,
            &cairn_core::process::jobs::distill_key(&other, "s1"),
        )
        .await
        .unwrap();
    assert_eq!(store.diagnose(&me).await.unwrap().jobs_pending, 2);
    assert_eq!(store.diagnose(&other).await.unwrap().jobs_pending, 5);
    db.finish().await;
}

#[tokio::test]
async fn enqueue_missing_embeddings_covers_every_batch() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    bulk_memories(&db, &me, ("personal", None), "rule", 8, 0).await;
    assert_eq!(store.enqueue_missing_embeddings(3).await.unwrap(), 8);
    assert_eq!(
        store.enqueue_missing_embeddings(3).await.unwrap(),
        0,
        "already queued"
    );
    assert_eq!(store.job_snapshot().await.unwrap().pending, 8);
    db.finish().await;
}

#[tokio::test]
async fn concurrent_consolidation_of_a_mutual_pair_never_fails() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    for round in 0..12 {
        let a = store
            .save(&me, rule(&format!("a{round}")), &WorkContext::default())
            .await
            .unwrap()
            .id;
        let b = store
            .save(&me, rule(&format!("b{round}")), &WorkContext::default())
            .await
            .unwrap()
            .id;
        store
            .upsert_embedding(&a, "h", &tilted(1.0, 1))
            .await
            .unwrap();
        store
            .upsert_embedding(&b, "h", &tilted(0.95, 1))
            .await
            .unwrap();
        let (left, right) =
            tokio::join!(store.consolidate_memory(&a), store.consolidate_memory(&b));
        left.unwrap();
        right.unwrap();
        // Keep the next round's pair out of this one's cohort.
        sqlx::query("UPDATE memories SET kind = 'fact' WHERE memory_id = ANY($1)")
            .bind([&a, &b])
            .execute(store.pool())
            .await
            .unwrap();
    }
    db.finish().await;
}

#[tokio::test]
async fn topic_notes_match_the_file_entity_exactly_and_prefer_the_project() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let stranger = db.person("stranger").await;
    let (project, _) = store
        .init_project(&me, "github.com/acme/cairn", None)
        .await
        .unwrap();
    let project_id = project.project_id.clone();
    let on = |file: &str, in_project: bool| WorkContext {
        files: vec![file.to_owned()],
        project_id: in_project.then(|| project_id.clone()),
        ..WorkContext::default()
    };
    let note = |title: &str, scope: &str, kind: &str| {
        save_input(
            serde_json::json!({"kind": kind, "scope": scope, "title": title, "summary": format!("{title} summary for the test"), "reason": "r and the test says why"}),
        )
    };
    let ours = store
        .save(
            &me,
            note("Project rule", "project", "rule"),
            &on("src/a.rs", true),
        )
        .await
        .unwrap()
        .id;
    store
        .save(
            &me,
            note("Personal one", "personal", "fact"),
            &on("src/a.rs", false),
        )
        .await
        .unwrap();
    store
        .save(
            &me,
            note("Personal two", "personal", "fact"),
            &on("src/a.rs", false),
        )
        .await
        .unwrap();
    let newest = store
        .save(
            &me,
            note("Personal three", "personal", "fact"),
            &on("src/a.rs", false),
        )
        .await
        .unwrap()
        .id;
    store
        .save(
            &me,
            note("Other file", "personal", "fact"),
            &on("src/ab.rs", false),
        )
        .await
        .unwrap();

    let context = WorkContext {
        project_id: Some(project.project_id.clone()),
        ..WorkContext::default()
    };
    let notes = store.topic_notes(&me, "src/a.rs", &context).await.unwrap();
    let ids: Vec<_> = notes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(
        ids,
        [ours.as_str(), newest.as_str()],
        "project first, newest next, at most 2"
    );
    assert_eq!(notes[0].summary, "Project rule summary for the test");
    assert!(
        store
            .topic_notes(&me, "src/a", &context)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .topic_notes(&stranger, "src/a.rs", &context)
            .await
            .unwrap()
            .is_empty()
    );

    // Handoffs and inactive notes are never topic notes; asking never counts as a fetch.
    sqlx::query("UPDATE memories SET kind = 'handoff' WHERE memory_id = $1")
        .bind(&newest)
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE memories SET status = 'superseded' WHERE memory_id = $1")
        .bind(&ours)
        .execute(store.pool())
        .await
        .unwrap();
    let notes = store
        .topic_notes(&me, "src/a.rs", &WorkContext::default())
        .await
        .unwrap();
    assert!(notes.iter().all(|n| n.id != newest && n.id != ours));
    let fetched: i64 = sqlx::query_scalar("SELECT coalesce(sum(fetched), 0)::bigint FROM usage")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(fetched, 0);
    db.finish().await;
}

#[tokio::test]
async fn gate_drops_a_one_word_match_and_keeps_a_covered_one() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let work = WorkContext::default();
    let id = store
        .save(
            &me,
            save_input(serde_json::json!({"kind":"rule","scope":"personal","title":"Drop columns slowly","summary":"Drop a column one release after code stops using it.","reason":"r and the test says why"})),
            &work,
        )
        .await
        .unwrap()
        .id;
    let filter = cairn_core::retrieve::filter::Filter::default();
    let search = |query: &'static str| {
        let (store, me, filter) = (store, me.clone(), filter.clone());
        async move {
            store
                .search(&me, query, None, 5, 1200, &filter)
                .await
                .unwrap()
        }
    };
    // One of four query words ("drop") is shared: a candidate, but under the coverage bar.
    assert!(search("guitar tuning for drop D").await.results.is_empty());
    let found = search("drop column release").await;
    assert_eq!(found.results.first().map(|c| &c.id), Some(&id));
    db.finish().await;
}

/// Every scan node's actual row count in an `EXPLAIN (ANALYZE, FORMAT JSON)` plan.
fn scanned_rows(plan: &serde_json::Value, out: &mut Vec<u64>) {
    if let Some(kind) = plan["Node Type"].as_str()
        && kind.ends_with("Scan")
        && plan["Relation Name"] == "memories"
    {
        out.push(plan["Actual Rows"].as_u64().unwrap() * plan["Actual Loops"].as_u64().unwrap());
    }
    for child in plan["Plans"].as_array().into_iter().flatten() {
        scanned_rows(child, out);
    }
}

/// A word that is in every row ("summary") must not make the OR channel rank every row: the
/// candidate set is capped before ranking, whatever the table size.
#[tokio::test]
async fn lexical_or_channel_caps_the_rows_it_ranks() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    synthetic(&db, &[(&me, "global", 10_000)]).await;
    let projects = store.member_projects(&me).await.unwrap();
    let plan: Vec<serde_json::Value> = sqlx::query_scalar(&format!(
        "EXPLAIN (ANALYZE, FORMAT JSON) {}",
        cairn_store::retrieve::lexical_sql(&Narrow::NONE)
    ))
    .bind(&me)
    .bind("summary filler zeta77x")
    .bind(&projects)
    .bind(64_i64)
    .fetch_all(store.pool())
    .await
    .unwrap();
    let mut rows = Vec::new();
    scanned_rows(&plan[0][0]["Plan"], &mut rows);
    let cap = cairn_store::retrieve::LEXICAL_MATCH_CAP as u64;
    assert!(
        rows.iter().all(|r| *r <= cap),
        "a scan read more than {cap} rows of 10,000: {rows:?}"
    );
    // The rare word is still found: the all-terms leg is not starved by the common ones.
    let found = store
        .lexical_candidates(&me, &projects, "summary filler zeta77x", 64, &Narrow::NONE)
        .await
        .unwrap();
    assert_eq!(found.first().map(|(id, _)| id.as_str()), Some("m77"));
    db.finish().await;
}
