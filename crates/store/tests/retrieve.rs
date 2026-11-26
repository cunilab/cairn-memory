use cairn_core::capture::WorkContext;
use cairn_core::memory::{Scope, Status};
use cairn_core::process::jobs::JobKind;
use cairn_core::retrieve::brief::BriefBudget;
use cairn_core::retrieve::hybrid::fuse;
use cairn_store::retrieve::{ALIASES_IN_SEARCH, Narrow};
use cairn_store::testing::{TestDb, rule, save_input};

/// A unit vector with `weight` on axis 0 and the rest on axis `axis`, so cosine to axis 0 is `weight`.
fn tilted(weight: f32, axis: usize) -> Vec<f32> {
    let mut v = vec![0.0; 384];
    v[0] = weight;
    v[axis] = (1.0 - weight * weight).sqrt();
    v
}

async fn current_embedding_hash(store: &cairn_store::Store, id: &str) -> String {
    store
        .embed_items(&[id.to_owned()])
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.alias_ord.is_none())
        .unwrap()
        .source_hash
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
        !store
            .upsert_embedding(&id, "other-hash", &tilted(1.0, 2))
            .await
            .unwrap(),
        "a stale hash cannot replace the vector"
    );
    sqlx::query(
        "UPDATE memories SET summary='An updated summary for embedding' WHERE memory_id=$1",
    )
    .bind(&id)
    .execute(store.pool())
    .await
    .unwrap();
    let new_hash = current_embedding_hash(&db.store, &id).await;
    assert!(
        store
            .upsert_embedding(&id, &new_hash, &tilted(1.0, 2))
            .await
            .unwrap()
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
        store
            .upsert_embedding(id, &current_embedding_hash(&db.store, id).await, &vector)
            .await
            .unwrap();
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
        .upsert_embedding(
            &near,
            &current_embedding_hash(&db.store, &near).await,
            &tilted(0.9, 1),
        )
        .await
        .unwrap();
    store
        .upsert_embedding(
            &far,
            &current_embedding_hash(&db.store, &far).await,
            &tilted(0.2, 2),
        )
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
            .upsert_embedding(
                ids.last().unwrap(),
                &current_embedding_hash(&db.store, ids.last().unwrap()).await,
                &tilted(1.0, ids.len()),
            )
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
    assert_eq!(
        channels().await,
        (3, 3, 3),
        "superseded and old rows stay searchable; forgotten does not"
    );
    sqlx::query("UPDATE memories SET status = 'stale' WHERE memory_id = $1")
        .bind(&ids[3])
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(channels().await, (3, 3, 3), "stale stays searchable");
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
            .upsert_embedding(
                id,
                &current_embedding_hash(&db.store, id).await,
                &tilted(1.0, 1),
            )
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
async fn hook_context_counts_shown_and_search_counts_fetched() {
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
        .save(&me, save_input(serde_json::json!({"kind":"preference","scope":"personal","title":"Usage probe words","summary":"ledger audit trail is a summary of the test","reason":"r and the test says why"})), &work)
        .await
        .unwrap()
        .id;
    let usage = || async {
        sqlx::query_as::<_, (i32, i32, bool, bool)>(
            "SELECT shown, fetched, last_shown_at IS NOT NULL, last_used_at IS NOT NULL
             FROM usage WHERE memory_id = $1",
        )
        .bind(&id)
        .fetch_one(store.pool())
        .await
        .unwrap()
    };
    assert_eq!(usage().await, (0, 0, false, false));
    let brief = store
        .brief(&me, &work, Some("s1"), BriefBudget::Extended)
        .await
        .unwrap();
    assert_eq!(brief.memory_ids, std::slice::from_ref(&id));
    assert!(
        brief.text.contains(r#""why":"personal preference""#),
        "{}",
        brief.text
    );
    assert_eq!(usage().await, (1, 0, true, false));
    let repeat = store
        .brief(&me, &work, Some("s1"), BriefBudget::Extended)
        .await
        .unwrap();
    assert!(repeat.text.is_empty());
    let topic = store
        .topic(&me, "src/usage.rs", &work, Some("s1"))
        .await
        .unwrap();
    assert!(topic.notes.is_empty() && topic.card.is_none());
    assert_eq!(usage().await, (1, 0, true, false));

    let topic = store
        .topic(&me, "src/usage.rs", &work, Some("s2"))
        .await
        .unwrap();
    assert_eq!(topic.memory_ids, std::slice::from_ref(&id));
    assert_eq!(topic.notes[0].why, "file src/usage.rs");
    let repeat = store
        .topic(&me, "src/usage.rs", &work, Some("s2"))
        .await
        .unwrap();
    assert!(repeat.notes.is_empty() && repeat.card.is_none());
    assert_eq!(usage().await, (2, 0, true, false));
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
    assert_eq!(usage().await, (2, 1, true, true));
    sqlx::query(
        "UPDATE session_shown SET shown_at = now() - interval '8 days' WHERE session_id = 's1'",
    )
    .execute(store.pool())
    .await
    .unwrap();
    store.sweep().await.unwrap();
    let retained: Vec<String> =
        sqlx::query_scalar("SELECT session_id FROM session_shown WHERE person_id = $1")
            .bind(&me)
            .fetch_all(store.pool())
            .await
            .unwrap();
    assert_eq!(retained, ["s2"]);
    assert_eq!(usage().await, (2, 1, true, true));
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
        .upsert_embedding(
            &alpha,
            &current_embedding_hash(&db.store, &alpha).await,
            &tilted(1.0, 1),
        )
        .await
        .unwrap();
    store
        .upsert_embedding(
            &beta,
            &current_embedding_hash(&db.store, &beta).await,
            &tilted(0.95, 1),
        )
        .await
        .unwrap(); // cos 0.95 to alpha: supersede
    store
        .upsert_embedding(
            &gamma,
            &current_embedding_hash(&db.store, &gamma).await,
            &tilted(0.80, 2),
        )
        .await
        .unwrap(); // 0.80: relates only
    store
        .upsert_embedding(
            &delta,
            &current_embedding_hash(&db.store, &delta).await,
            &tilted(0.30, 3),
        )
        .await
        .unwrap(); // 0.30: left alone
    for id in [&fact, &theirs, &global] {
        store
            .upsert_embedding(
                id,
                &current_embedding_hash(&db.store, id).await,
                &tilted(1.0, 1),
            )
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
        .upsert_embedding(
            &epsilon,
            &current_embedding_hash(&db.store, &epsilon).await,
            &tilted(0.97, 1),
        )
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
        .upsert_embedding(
            &a,
            &current_embedding_hash(&db.store, &a).await,
            &tilted(1.0, 1),
        )
        .await
        .unwrap();
    store
        .upsert_embedding(
            &b,
            &current_embedding_hash(&db.store, &b).await,
            &tilted(0.80, 2),
        )
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
        .upsert_embedding(
            &a,
            &current_embedding_hash(&db.store, &a).await,
            &tilted(1.0, 1),
        )
        .await
        .unwrap();
    db.store
        .upsert_embedding(
            &b,
            &current_embedding_hash(&db.store, &b).await,
            &tilted(0.95, 1),
        )
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
            .upsert_embedding(
                id,
                &current_embedding_hash(&db.store, id).await,
                &tilted(weight, 1),
            )
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
    let brief = store
        .brief(&me, &work, None, BriefBudget::Extended)
        .await
        .unwrap()
        .text;
    assert!(brief.contains("keep me"), "{brief}");
    db.finish().await;
}

#[tokio::test]
async fn job_counts_only_my_jobs() {
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
    assert_eq!(store.person_job_counts(&me).await.unwrap().0, 2);
    assert_eq!(store.person_job_counts(&other).await.unwrap().0, 5);
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
            .upsert_embedding(
                &a,
                &current_embedding_hash(&db.store, &a).await,
                &tilted(1.0, 1),
            )
            .await
            .unwrap();
        store
            .upsert_embedding(
                &b,
                &current_embedding_hash(&db.store, &b).await,
                &tilted(0.95, 1),
            )
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
    // Four memories name the file, so one card names the first two: project first, newest next.
    let topic = store.topic(&me, "src/a.rs", &context, None).await.unwrap();
    assert!(topic.notes.is_empty());
    let card = topic.card.expect("card");
    assert!(
        card.contains("4 memories \u{2014} rule Project rule (why: file src/a.rs) [0d]; fact Personal three (why: file src/a.rs) [0d]"),
        "{card}"
    );
    for (who, path) in [(&me, "src/a"), (&stranger, "src/a.rs")] {
        let topic = store.topic(who, path, &context, None).await.unwrap();
        assert!(topic.notes.is_empty() && topic.card.is_none());
    }

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
    let topic = store
        .topic(&me, "src/a.rs", &WorkContext::default(), None)
        .await
        .unwrap();
    assert!(topic.card.is_none() && topic.notes.len() == 2);
    assert!(topic.notes.iter().all(|n| n.id != newest && n.id != ours));
    let fetched: i64 = sqlx::query_scalar("SELECT coalesce(sum(fetched), 0)::bigint FROM usage")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(fetched, 0);
    db.finish().await;
}

#[tokio::test]
async fn topic_ranks_kind_then_votes_then_recency_and_matches_symbols() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("topic-ranking").await;
    let work = WorkContext {
        symbols: vec!["queue::take_lock".into()],
        ..WorkContext::default()
    };
    let mut ids = Vec::new();
    for (title, kind) in [
        ("Helpful queue rule", "rule"),
        ("Older queue rule", "rule"),
        ("Newest queue rule", "rule"),
        ("Queue decision", "decision"),
        ("Queue fact", "fact"),
    ] {
        ids.push(store.save(&me, save_input(serde_json::json!({
            "kind":kind, "scope":"personal", "title":title, "summary":"Hold the state lock before claiming a job.",
            "reason":"Concurrent claims share one queue.",
            "fields": if kind == "decision" { serde_json::json!({"context": "Worker lock ordering"}) } else { serde_json::json!({}) }
        })), &work).await.unwrap().id);
    }
    for id in [&ids[0], &ids[4]] {
        store
            .feedback(&me, id, cairn_store::memory::Vote::Helpful)
            .await
            .unwrap();
    }
    let session = Some("symbols");
    let first = store
        .topic(&me, "src/unrelated.rs", &work, session)
        .await
        .unwrap();
    assert_eq!(first.memory_ids, [ids[0].clone(), ids[2].clone()]);
    assert!(first.card.unwrap().contains("why: symbol queue::take_lock"));
    let second = store
        .topic(&me, "src/unrelated.rs", &work, session)
        .await
        .unwrap();
    assert_eq!(second.memory_ids, [ids[1].clone(), ids[3].clone()]);
    let third = store
        .topic(&me, "src/unrelated.rs", &work, session)
        .await
        .unwrap();
    assert_eq!(third.memory_ids, [ids[4].clone()]);
    assert_eq!(third.notes[0].why, "symbol queue::take_lock");
    assert!(
        store
            .topic(&me, "src/unrelated.rs", &WorkContext::default(), None)
            .await
            .unwrap()
            .notes
            .is_empty()
    );
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

#[tokio::test]
async fn brief_skips_candidates() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let (project, _) = store
        .init_project(&me, "github.com/acme/cairn", None)
        .await
        .unwrap();
    let work = WorkContext {
        project_id: Some(project.project_id.clone()),
        repository: Some("github.com/acme/cairn".into()),
        branch: Some("main".into()),
        ..WorkContext::default()
    };
    store
        .save(&me, rule("Keep this rule visible"), &work)
        .await
        .unwrap();
    let gotcha = store
        .save(
            &me,
            save_input(serde_json::json!({
                "kind": "gotcha",
                "scope": "project",
                "title": "Candidate gotcha stays out",
                "summary": "A low-rank gotcha must not enter the session brief.",
                "reason": "Seen once in a session.",
                "fields": {"symptom": "brief showed it", "fix": "leave it candidate"}
            })),
            &work,
        )
        .await
        .unwrap()
        .id;
    sqlx::query("UPDATE memories SET rank = 'candidate' WHERE memory_id = $1")
        .bind(&gotcha)
        .execute(store.pool())
        .await
        .unwrap();
    let brief = store
        .brief(&me, &work, None, BriefBudget::Extended)
        .await
        .unwrap()
        .text;
    assert!(brief.contains("Keep this rule visible"), "{brief}");
    assert!(!brief.contains("Candidate gotcha stays out"), "{brief}");
    db.finish().await;
}

#[tokio::test]
async fn digest_searchable_not_in_brief() {
    use cairn_core::memory::Kind;
    use cairn_core::retrieve::filter::Filter;

    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let person = db.person("a").await;
    let (project, _) = store
        .init_project(&person, "github.com/acme/one", None)
        .await
        .unwrap();
    let pid = project.project_id.clone();
    let take = |event: &str, kind: &str, content: &str, ok: Option<bool>, file: Option<&str>| {
        let mut context = serde_json::json!({
            "repository": "github.com/acme/one",
            "branch": "feat",
            "worktree": "wt-1",
            "project_id": pid,
        });
        if kind == "command" {
            context["commands"] = serde_json::json!([content.lines().next().unwrap_or("")]);
        }
        if let Some(file) = file {
            context["files"] = serde_json::json!([file]);
        }
        serde_json::from_value(serde_json::json!({
            "event_id": event,
            "session_id": "digest",
            "kind": kind,
            "ok": ok,
            "content": content,
            "context": context,
        }))
        .unwrap()
    };
    store
        .capture(
            &person,
            take(
                "p1",
                "prompt",
                "zephyr-quilt: do not copy this prompt into the digest",
                None,
                None,
            ),
        )
        .await
        .unwrap();
    store
        .capture(
            &person,
            take(
                "c1",
                "command",
                "cargo test\nexit 101: error[E0308]: mismatched types",
                Some(false),
                None,
            ),
        )
        .await
        .unwrap();
    // Plain names: the english parser keeps `src/LedControls.tsx` as one lexeme, so
    // `LedControls removed` would not match a path. These files are the words themselves.
    for (event, file) in [("e1", "LedControls"), ("e2", "removed")] {
        store
            .capture(
                &person,
                take(event, "edit", &format!("Edit {file}\nx"), None, Some(file)),
            )
            .await
            .unwrap();
    }
    store
        .capture(
            &person,
            take("c2", "command", "cargo test", Some(true), None),
        )
        .await
        .unwrap();
    store.end_session(&person, "digest").await.unwrap();
    store.distill_session(&person, "digest").await.unwrap();
    let work = WorkContext {
        project_id: Some(pid),
        repository: Some("github.com/acme/one".into()),
        branch: Some("feat".into()),
        worktree: Some("wt-1".into()),
        ..WorkContext::default()
    };
    let brief = store
        .brief(&person, &work, None, BriefBudget::Extended)
        .await
        .unwrap()
        .text;
    assert!(!brief.contains("LedControls"), "{brief}");
    assert!(!brief.contains("cargo test pass"), "{brief}");
    let cards = store
        .search(
            &person,
            "LedControls removed",
            None,
            5,
            1200,
            &Filter::default(),
        )
        .await
        .unwrap();
    let digest = cards
        .results
        .iter()
        .find(|card| card.kind == Kind::Note)
        .expect("search finds the digest");
    assert!(digest.summary.contains("LedControls"), "{}", digest.summary);
    assert!(digest.summary.contains("removed"), "{}", digest.summary);
    assert!(
        digest.summary.contains("cargo test pass"),
        "{}",
        digest.summary
    );
    assert!(
        !digest.summary.contains("zephyr-quilt"),
        "{}",
        digest.summary
    );
    db.finish().await;
}

async fn equalize(store: &cairn_store::Store, ids: &[&str]) {
    let text = "Shared marker wording for the rank test exactly";
    for id in ids {
        sqlx::query("UPDATE memories SET title = $2, summary = $2, reason = $3, updated_at = now() - interval '400 days' WHERE memory_id = $1")
            .bind(id)
            .bind(text)
            .bind("because the test says why this is true")
            .execute(store.pool())
            .await
            .unwrap();
        store
            .upsert_embedding(
                id,
                &current_embedding_hash(store, id).await,
                &tilted(1.0, 1),
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn superseded_is_searchable_ranked_below_and_labelled() {
    use cairn_core::retrieve::filter::Filter;

    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let work = WorkContext::default();
    let old = db
        .store
        .save(&me, rule("Cache sessions in Redis"), &work)
        .await
        .unwrap()
        .id;
    let mut newer = rule("Cache sessions in Postgres");
    newer.supersedes = Some(old.clone());
    let new = db.store.save(&me, newer, &work).await.unwrap().id;
    equalize(&db.store, &[&old, &new]).await;
    let cards = db
        .store
        .search(
            &me,
            "Shared marker wording",
            Some(&tilted(1.0, 1)),
            5,
            1200,
            &Filter::default(),
        )
        .await
        .unwrap();
    let ids: Vec<&str> = cards.results.iter().map(|card| card.id.as_str()).collect();
    assert!(
        ids.contains(&old.as_str()) && ids.contains(&new.as_str()),
        "{ids:?}"
    );
    assert!(
        ids.iter().position(|id| *id == new).unwrap()
            < ids.iter().position(|id| *id == old).unwrap(),
        "replacement ranks first: {ids:?}"
    );
    let label = cards
        .results
        .iter()
        .find(|card| card.id == old)
        .unwrap()
        .label
        .clone()
        .unwrap();
    assert_eq!(label, format!("replaced by {new}"));
    assert!(label.len() <= 42, "{label}");
    assert!(
        cards
            .results
            .iter()
            .find(|card| card.id == new)
            .unwrap()
            .label
            .is_none()
    );
    db.finish().await;
}

#[tokio::test]
async fn stale_labels() {
    use cairn_core::retrieve::filter::Filter;

    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let work = WorkContext::default();
    let live = db
        .store
        .save(&me, rule("Live marker rule"), &work)
        .await
        .unwrap()
        .id;
    let voted = db
        .store
        .save(&me, rule("Voted marker rule"), &work)
        .await
        .unwrap()
        .id;
    let missing = db
        .store
        .save(&me, rule("Missing marker rule"), &work)
        .await
        .unwrap()
        .id;
    sqlx::query(
        "UPDATE memories SET status = 'stale', stale_cause = 'feedback' WHERE memory_id = $1",
    )
    .bind(&voted)
    .execute(db.store.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE usage SET wrong = 2 WHERE memory_id = $1")
        .bind(&voted)
        .execute(db.store.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE memories SET status = 'stale', stale_cause = 'missing' WHERE memory_id = $1",
    )
    .bind(&missing)
    .execute(db.store.pool())
    .await
    .unwrap();
    equalize(&db.store, &[&live, &voted, &missing]).await;
    let cards = db
        .store
        .search(
            &me,
            "Shared marker wording",
            Some(&tilted(1.0, 1)),
            5,
            1200,
            &Filter::default(),
        )
        .await
        .unwrap();
    let pos = |id: &str| cards.results.iter().position(|card| card.id == id).unwrap();
    assert!(
        pos(&live) < pos(&voted) && pos(&live) < pos(&missing),
        "{:?}",
        cards.results
    );
    let label = |id: &str| {
        cards
            .results
            .iter()
            .find(|card| card.id == id)
            .unwrap()
            .label
            .clone()
            .unwrap()
    };
    assert_eq!(label(&voted), "voted wrong 2x");
    assert_eq!(label(&missing), "file deleted");
    assert!(label(&voted).len() <= 16 && label(&missing).len() <= 16);
    db.finish().await;
}

#[tokio::test]
async fn forget_keeps_content_but_hides() {
    use cairn_core::memory::Status;
    use cairn_core::retrieve::filter::Filter;
    use cairn_store::memory::ListFilter;

    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let work = WorkContext {
        files: vec!["src/kept.rs".into()],
        ..WorkContext::default()
    };
    let other = db
        .store
        .save(&me, rule("Other kept rule"), &WorkContext::default())
        .await
        .unwrap()
        .id;
    let mut body = rule("Forget keeps this rule");
    body.supersedes = Some(other);
    let id = db.store.save(&me, body, &work).await.unwrap().id;
    db.store
        .upsert_embedding(
            &id,
            &current_embedding_hash(&db.store, &id).await,
            &tilted(1.0, 1),
        )
        .await
        .unwrap();
    assert!(db.store.forget(&me, &id).await.unwrap());
    let row: (String, String, i32, i64, i64) = sqlx::query_as(
        "SELECT title, summary,
                CASE WHEN embedding IS NOT NULL THEN 1 ELSE 0 END,
                (SELECT count(*) FROM entities WHERE memory_id = $1),
                (SELECT count(*) FROM links WHERE source_id = $1 OR target_id = $1)
           FROM memories WHERE memory_id = $1",
    )
    .bind(&id)
    .fetch_one(db.store.pool())
    .await
    .unwrap();
    assert_eq!(row.0, "Forget keeps this rule");
    assert!(!row.1.is_empty());
    assert_eq!(
        (row.2, row.4),
        (1, 1),
        "embedding and the supersedes link stay"
    );
    assert!(row.3 >= 1, "entities stay");
    let cards = db
        .store
        .search(
            &me,
            "Forget keeps this rule",
            None,
            5,
            1200,
            &Filter::default(),
        )
        .await
        .unwrap();
    assert!(cards.results.iter().all(|card| card.id != id));
    let listed = db.store.list(&me, &ListFilter::default()).await.unwrap();
    assert!(listed.memories.iter().all(|memory| memory.id != id));
    let admin = db
        .store
        .list(
            &me,
            &ListFilter {
                statuses: vec![Status::Forgotten],
                see_forgotten: true,
                ..ListFilter::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(admin.memories.len(), 1);
    assert_eq!(admin.memories[0].status, Status::Forgotten);
    let brief = db
        .store
        .brief(&me, &WorkContext::default(), None, BriefBudget::Extended)
        .await
        .unwrap()
        .text;
    assert!(!brief.contains("Forget keeps this rule"), "{brief}");
    db.finish().await;
}

/// More than 50 handoffs on one checkout. The newest is thin; the rich one is
/// the second newest, so a newest-first limit of 50 still reaches it.
#[tokio::test]
async fn brief_picks_newest_rich_handoff_past_fifty_chain_rows() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    sqlx::query(
        r#"INSERT INTO memories (memory_id, owner_id, scope, kind, title, summary, evidence_hash,
                                 identity_hash, fields, repo, repo_branch, repo_worktree, created_at)
           SELECT CASE WHEN i = 51 THEN 'chain_thin_newest'
                       WHEN i = 50 THEN 'chain_rich'
                       ELSE 'chain_old_' || i END,
                  $1, 'personal', 'handoff', 'Handoff', 'chain row', 'e',
                  CASE WHEN i = 51 THEN 'chain_thin_newest'
                       WHEN i = 50 THEN 'chain_rich'
                       ELSE 'chain_old_' || i END,
                  CASE WHEN i = 50 THEN '{"files":["src/pay.rs"]}'::jsonb ELSE '{}'::jsonb END,
                  'github.com/acme/one', 'feat', 'wt-1',
                  now() + make_interval(secs => i)
             FROM generate_series(1, 51) i"#,
    )
    .bind(&me)
    .execute(db.store.pool())
    .await
    .unwrap();
    let brief = db
        .store
        .brief(
            &me,
            &WorkContext {
                repository: Some("github.com/acme/one".into()),
                branch: Some("feat".into()),
                worktree: Some("wt-1".into()),
                ..WorkContext::default()
            },
            None,
            BriefBudget::Extended,
        )
        .await
        .unwrap()
        .text;
    assert!(
        brief.contains("chain_rich"),
        "brief shows the newest rich handoff: {brief}"
    );
    assert!(
        !brief.contains("chain_thin_newest"),
        "brief skips the thin newest: {brief}"
    );
    db.finish().await;
}

#[tokio::test]
async fn brief_shows_handoff_without_git() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let (project, _) = db
        .store
        .init_project(&me, "local:nongit-project", None)
        .await
        .unwrap();
    let pid = project.project_id;

    // Handoff with repo_branch NULL (non-git project)
    sqlx::query(
        r#"INSERT INTO memories (memory_id, owner_id, project_id, scope, kind, title, summary,
                                 evidence_hash, identity_hash, fields, repo, repo_branch, repo_worktree, created_at)
           VALUES ('nongit_handoff', $1, $2, 'personal', 'handoff', 'Handoff nongit', 'summary of handoff',
                   'e_nongit', 'id_nongit', '{"files":["README.md"]}'::jsonb,
                   'local:nongit-project', NULL, NULL, now())"#,
    )
    .bind(&me)
    .bind(&pid)
    .execute(db.store.pool())
    .await
    .unwrap();

    // Brief for non-git context (branch: None)
    let non_git_work = WorkContext {
        project_id: Some(pid.clone()),
        repository: Some("local:nongit-project".into()),
        branch: None,
        worktree: None,
        ..WorkContext::default()
    };
    let brief = db
        .store
        .brief(&me, &non_git_work, None, BriefBudget::Extended)
        .await
        .unwrap()
        .text;
    assert!(
        brief.contains("nongit_handoff"),
        "non-git brief must show the handoff: {brief}"
    );

    // Git context (branch: Some("main")) does not show the non-git handoff
    let git_work = WorkContext {
        project_id: Some(pid),
        repository: Some("local:nongit-project".into()),
        branch: Some("main".into()),
        worktree: None,
        ..WorkContext::default()
    };
    let git_brief = db
        .store
        .brief(&me, &git_work, None, BriefBudget::Extended)
        .await
        .unwrap()
        .text;
    assert!(
        !git_brief.contains("nongit_handoff"),
        "git brief must only show matching branch's handoff: {git_brief}"
    );

    db.finish().await;
}

/// Alias rows are embedded by the same path as the memory, and a query whose only close vector
/// is an alias returns that memory.
#[tokio::test]
async fn disabled_alias_search_keeps_rows_without_enqueuing_alias_vectors() {
    if ALIASES_IN_SEARCH {
        return;
    }
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("alias-gate").await;
    let store = &db.store;
    let id = store.save(&me, save_input(serde_json::json!({
        "kind":"fact", "title":"Alias pause fixture", "summary":"The search gate stops alias inference", "reason":"Vectors are unnecessary while disabled",
        "aliases":["existing alias vector", "new alias without vector"]
    })), &WorkContext::default()).await.unwrap().id;
    let hash = current_embedding_hash(store, &id).await;
    store
        .upsert_embedding(&id, &hash, &tilted(1.0, 1))
        .await
        .unwrap();
    sqlx::query("UPDATE memory_aliases a SET embedding=m.embedding,embedding_source='fixture' FROM memories m WHERE m.memory_id=a.memory_id AND a.memory_id=$1 AND a.ord=0")
        .bind(&id).execute(store.pool()).await.unwrap();
    sqlx::query("DELETE FROM jobs WHERE kind='embed' AND job_key=$1")
        .bind(&id)
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(store.enqueue_missing_embeddings(10).await.unwrap(), 0);
    assert!(store.pending_embeddings(10).await.unwrap().is_empty());
    assert!(
        store
            .embed_items(std::slice::from_ref(&id))
            .await
            .unwrap()
            .iter()
            .all(|item| item.alias_ord.is_none())
    );
    let rows: Vec<(String, bool)> = sqlx::query_as(
        "SELECT text,embedding IS NOT NULL FROM memory_aliases WHERE memory_id=$1 ORDER BY ord",
    )
    .bind(id)
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            ("existing alias vector".to_owned(), true),
            ("new alias without vector".to_owned(), false)
        ]
    );
    db.finish().await;
}

#[tokio::test]
async fn aliases_embedded_and_searched() {
    if !ALIASES_IN_SEARCH {
        return;
    }
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let id = store
        .save(
            &me,
            save_input(serde_json::json!({
                "kind": "fact",
                "title": "Alias search title",
                "summary": "The body of the memory is about something else entirely.",
                "reason": "because the test needs one",
                "aliases": ["how do I renew the parking permit"],
            })),
            &WorkContext::default(),
        )
        .await
        .unwrap()
        .id;
    let pending = store.pending_embeddings(10).await.unwrap();
    let memory_item = pending
        .iter()
        .find(|item| item.alias_ord.is_none())
        .unwrap()
        .clone();
    assert!(
        store
            .upsert_embedding(&id, &memory_item.source_hash, &tilted(0.2, 2))
            .await
            .unwrap()
    );
    sqlx::query("DELETE FROM jobs WHERE kind = 'embed' AND job_key = $1")
        .bind(&id)
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(store.enqueue_missing_embeddings(10).await.unwrap(), 1);
    let items = store.embed_items(std::slice::from_ref(&id)).await.unwrap();
    let alias = items
        .iter()
        .find(|item| item.alias_ord == Some(0))
        .expect("alias text is embedded with the memory");
    assert_eq!(alias.text, "how do I renew the parking permit");
    assert!(
        store
            .upsert_embedding(&id, &alias.source_hash, &tilted(1.0, 1))
            .await
            .unwrap()
    );
    let nulls: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memory_aliases a
           JOIN memories m ON m.memory_id = a.memory_id
          WHERE a.embedding IS NULL AND m.status <> 'forgotten'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(nulls, 0);
    let projects = store.member_projects(&me).await.unwrap();
    let found = store
        .vector_candidates(&me, &projects, &tilted(1.0, 1), 5, &Narrow::NONE)
        .await
        .unwrap();
    if ALIASES_IN_SEARCH {
        assert_eq!(found[0].0, id);
        assert!(found[0].1 > 0.99, "{found:?}");
    } else {
        assert!(found.iter().all(|(_, score)| *score < 0.5), "{found:?}");
    }
    db.finish().await;
}

/// Forgetting a memory keeps the alias row and hides it from vector search.
#[tokio::test]
async fn forgotten_aliases_not_returned() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let id = store
        .save(
            &me,
            save_input(serde_json::json!({
                "kind": "fact",
                "title": "Forgotten alias title",
                "summary": "The body of the memory is about something else entirely.",
                "reason": "because the test needs one",
                "aliases": ["where is the telescope mirror stored"],
            })),
            &WorkContext::default(),
        )
        .await
        .unwrap()
        .id;
    for item in store.pending_embeddings(10).await.unwrap() {
        let vector = if item.alias_ord.is_some() {
            tilted(1.0, 1)
        } else {
            tilted(0.2, 2)
        };
        store
            .upsert_embedding(&id, &item.source_hash, &vector)
            .await
            .unwrap();
    }
    let projects = store.member_projects(&me).await.unwrap();
    let found = store
        .vector_candidates(&me, &projects, &tilted(1.0, 1), 5, &Narrow::NONE)
        .await
        .unwrap();
    assert!(found.iter().any(|(hit, _)| hit == &id));
    if ALIASES_IN_SEARCH {
        assert_eq!(found[0].0, id);
    }
    assert!(store.forget(&me, &id).await.unwrap());
    let found = store
        .vector_candidates(&me, &projects, &tilted(1.0, 1), 5, &Narrow::NONE)
        .await
        .unwrap();
    assert!(found.iter().all(|(hit, _)| hit != &id), "{found:?}");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM memory_aliases WHERE memory_id = $1")
        .bind(&id)
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(left, 1, "forget hides the row; it does not delete it");
    db.finish().await;
}

/// One memory whose own vector is far from the query axis and whose alias vector is on it;
/// returns its id. The query text shares no word with the memory.
async fn alias_only_memory(db: &TestDb, me: &str) -> String {
    let store = &db.store;
    let id = store
        .save(
            me,
            save_input(serde_json::json!({
                "kind": "fact",
                "title": "Alias gate title",
                "summary": "The body of the memory is about something else entirely.",
                "reason": "because the test needs one",
                "aliases": ["where is the telescope mirror stored"],
            })),
            &WorkContext::default(),
        )
        .await
        .unwrap()
        .id;
    for item in store.pending_embeddings(10).await.unwrap() {
        let vector = if item.alias_ord.is_some() {
            tilted(1.0, 1)
        } else {
            tilted(0.2, 2)
        };
        store
            .upsert_embedding(&id, &item.source_hash, &vector)
            .await
            .unwrap();
    }
    id
}

/// The gate sees the best of the memory and alias cosines: a query that only an alias is close to
/// passes it through `Store::search`, and forgetting the memory hides it again.
#[tokio::test]
async fn alias_only_query_passes_gate_in_search() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let id = alias_only_memory(&db, &me).await;
    let query = tilted(1.0, 1);
    let cards = store
        .search(
            &me,
            "kiwi marmalade",
            Some(&query),
            5,
            1200,
            &cairn_core::retrieve::filter::Filter::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        cards.results.iter().any(|c| c.id == id),
        ALIASES_IN_SEARCH,
        "{cards:?}"
    );
    assert!(store.forget(&me, &id).await.unwrap());
    let cards = store
        .search(
            &me,
            "kiwi marmalade",
            Some(&query),
            5,
            1200,
            &cairn_core::retrieve::filter::Filter::default(),
        )
        .await
        .unwrap();
    assert!(!cards.results.iter().any(|c| c.id == id), "{cards:?}");
    db.finish().await;
}

/// An alias that is only weakly close (cosine 0.3, under the gate) does not make an off-topic
/// query return the memory.
#[tokio::test]
async fn weak_query_returns_nothing_with_aliases_present() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    alias_only_memory(&db, &me).await;
    let query = tilted(0.3, 3);
    let cards = store
        .search(
            &me,
            "kiwi marmalade",
            Some(&query),
            5,
            1200,
            &cairn_core::retrieve::filter::Filter::default(),
        )
        .await
        .unwrap();
    assert!(cards.results.is_empty(), "{cards:?}");
    db.finish().await;
}

/// An alias whose text equals the memory embed text still gets its own vector.
#[tokio::test]
async fn alias_matching_embed_text_keeps_its_own_vector() {
    if !ALIASES_IN_SEARCH {
        return;
    }
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let id = store
        .save(
            &me,
            save_input(serde_json::json!({
                "kind": "fact",
                "title": "Abcdefgh",
                "summary": "0123456789abcdefghij",
                "reason": "0123456789",
            })),
            &WorkContext::default(),
        )
        .await
        .unwrap()
        .id;
    let pending = store.pending_embeddings(10).await.unwrap();
    assert_eq!(pending.len(), 1);
    let text = pending[0].text.clone();
    assert!(
        text.chars().count() <= 120,
        "embed text must fit an alias: {text}"
    );
    sqlx::query(
        "INSERT INTO memory_aliases (memory_id, ord, source, text) VALUES ($1, 0, 'agent', $2)",
    )
    .bind(&id)
    .bind(&text)
    .execute(store.pool())
    .await
    .unwrap();
    let pending = store.pending_embeddings(10).await.unwrap();
    let memory = pending
        .iter()
        .find(|item| item.alias_ord.is_none())
        .unwrap();
    let alias = pending
        .iter()
        .find(|item| item.alias_ord == Some(0))
        .unwrap();
    assert_eq!(alias.text, memory.text);
    assert_ne!(alias.source_hash, memory.source_hash);
    let memory_hash = memory.source_hash.clone();
    let alias_hash = alias.source_hash.clone();
    assert!(
        store
            .upsert_embedding(&id, &memory_hash, &tilted(1.0, 1))
            .await
            .unwrap()
    );
    assert!(
        store
            .upsert_embedding(&id, &alias_hash, &tilted(0.2, 2))
            .await
            .unwrap()
    );
    let memory_dist: f32 = sqlx::query_scalar(
        "SELECT (embedding <=> $2::vector)::float4 FROM memories WHERE memory_id = $1",
    )
    .bind(&id)
    .bind(vector_literal(&tilted(1.0, 1)))
    .fetch_one(store.pool())
    .await
    .unwrap();
    let alias_dist: f32 = sqlx::query_scalar("SELECT (embedding <=> $2::vector)::float4 FROM memory_aliases WHERE memory_id = $1 AND ord = 0")
        .bind(&id)
        .bind(vector_literal(&tilted(0.2, 2)))
        .fetch_one(store.pool())
        .await
        .unwrap();
    let cross: f32 = sqlx::query_scalar("SELECT (embedding <=> $2::vector)::float4 FROM memory_aliases WHERE memory_id = $1 AND ord = 0")
        .bind(&id)
        .bind(vector_literal(&tilted(1.0, 1)))
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert!(memory_dist < 0.01, "memory vector {memory_dist}");
    assert!(alias_dist < 0.01, "alias vector {alias_dist}");
    assert!(cross > 0.5, "alias kept its own vector, cross {cross}");
    db.finish().await;
}

#[tokio::test]
async fn command_recall_uses_strict_matches_and_the_shared_session_ledger() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("recall-owner").await;
    let outsider = db.person("recall-outsider").await;
    let (project, _) = store
        .init_project(&me, "local:e2e-command-recall", Some("E2E command recall"))
        .await
        .unwrap();
    let project = project.project_id;
    let remember = |kind: &str, title: &str, summary: &str, command: &str| {
        let work = WorkContext {
            project_id: Some(project.clone()),
            files: vec!["src/recall.rs".into()],
            commands: vec![command.to_owned()],
            ..WorkContext::default()
        };
        let fields = if kind == "gotcha" {
            serde_json::json!({"fix": "Regenerate build artifacts before retrying"})
        } else {
            serde_json::json!({"steps": ["Remove the old build artifacts", "Run the build again"]})
        };
        let input = save_input(serde_json::json!({
            "kind": kind, "scope": "project", "title": title, "summary": summary,
            "reason": "A specific build failure has a reproducible recovery step", "fields": fields
        }));
        let me = &me;
        async move { store.save(me, input, &work).await.unwrap().id }
    };
    let command = "cargo test --workspace";
    let gotcha = remember(
        "gotcha",
        "Rust target cache",
        "Error: poisoned target directory",
        command,
    )
    .await;
    remember(
        "procedure",
        "Clear target directory",
        "Error: poisoned target directory",
        command,
    )
    .await;
    remember(
        "procedure",
        "Rebuild the target",
        "Remove the build cache before retrying",
        command,
    )
    .await;
    remember(
        "gotcha",
        "Ignore second line",
        "Error: unhappy network proxy",
        command,
    )
    .await;
    let candidate = remember(
        "gotcha",
        "Fresh suggestion",
        "Error: poisoned target directory",
        command,
    )
    .await;
    let doubted = remember(
        "gotcha",
        "Bad suggestion",
        "Error: poisoned target directory",
        command,
    )
    .await;
    sqlx::query("UPDATE memories SET rank = 'candidate' WHERE memory_id = $1")
        .bind(&candidate)
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE usage SET wrong = 1 WHERE memory_id = $1")
        .bind(&doubted)
        .execute(store.pool())
        .await
        .unwrap();

    let recall = async |session: &str, command: &str, failure: Option<&str>| {
        store
            .command_recall(&me, session, Some(&project), command, failure)
            .await
            .unwrap()
    };
    let (one, two) = tokio::join!(
        recall("warning", "cargo  test --workspace && echo done", None),
        recall("warning", command, None),
    );
    assert_ne!(one.is_empty(), two.is_empty(), "{one:?} {two:?}");
    let warning = if one.is_empty() { two } else { one };
    assert!(warning.contains("Rust target cache"), "{warning}");
    assert!(
        warning.contains("why: gotcha for `cargo test --workspace`"),
        "{warning}"
    );
    assert_eq!(warning.matches("\n- ").count(), 2, "{warning}");
    assert!(recall("warning", command, None).await.is_empty());
    assert!(recall("prefix", "cargo test", None).await.is_empty());
    assert!(
        store
            .command_recall(&outsider, "warning", Some(&project), command, None)
            .await
            .unwrap()
            .is_empty()
    );

    let tail = "build starting\nError: poisoned target directory\nError: unhappy network proxy";
    let failed = recall("failed", command, Some(tail)).await;
    assert!(
        failed.contains(
            "Rust target cache: Error: poisoned target directory (why: same error seen before)"
        ),
        "{failed}"
    );
    assert!(failed.contains("Clear target directory"), "{failed}");
    for title in [
        "Ignore second line",
        "Rebuild the target",
        "Fresh suggestion",
        "Bad suggestion",
    ] {
        assert!(!failed.contains(title), "{failed}");
    }
    assert!(recall("failed", command, Some(tail)).await.is_empty());
    assert!(
        recall("wrong command", "npm test", Some(tail))
            .await
            .is_empty()
    );
    assert!(
        recall(
            "second error alone",
            command,
            Some("Error: unhappy network proxy")
        )
        .await
        .contains("Ignore second line")
    );
    assert!(
        recall("weak", command, Some("error: failed"))
            .await
            .is_empty()
    );
    assert!(
        recall(
            "other error",
            command,
            Some("Error: completely unknown failure")
        )
        .await
        .is_empty()
    );
    assert!(
        recall("no output", command, Some(""))
            .await
            .contains("Rust target cache")
    );

    let context = WorkContext {
        project_id: Some(project.clone()),
        ..WorkContext::default()
    };
    let topic = store
        .topic(&me, "src/recall.rs", &context, Some("budget"))
        .await
        .unwrap();
    assert_eq!(topic.memory_ids.len(), 2);
    for i in 0..7 {
        let command = format!("make check-{i}");
        remember(
            "procedure",
            &format!("Check build {i}"),
            "Regenerate dependencies before building",
            &command,
        )
        .await;
        let text = recall("budget", &command, None).await;
        assert_eq!(text.is_empty(), i == 6, "{i}: {text}");
    }
    let used: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM session_shown WHERE person_id = $1 AND session_id = 'budget'",
    )
    .bind(&me)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(used, 8);
    assert!(recall("budget", command, Some(tail)).await.is_empty());

    let shown: i32 = sqlx::query_scalar("SELECT shown FROM usage WHERE memory_id = $1")
        .bind(&gotcha)
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert!(shown >= 3, "{shown}");
    db.finish().await;
}

#[tokio::test]
async fn command_error_uses_settle_and_revoke_without_overwriting_explicit_use() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let person = db.person("error-use").await;
    let (project, _) = store
        .init_project(&person, "local:e2e-error-use", Some("E2E error use"))
        .await
        .unwrap();
    let context = WorkContext {
        project_id: Some(project.project_id.clone()),
        commands: vec!["cargo test --workspace".into()],
        ..WorkContext::default()
    };
    let memory = store
        .save(
            &person,
            save_input(serde_json::json!({
                "kind": "gotcha", "title":"Linker target cache",
                "summary": "Error: poisoned target directory",
                "reason": "A stale locker holds the build directory.",
                "fields":{"fix":"Restart the linker to remove its stale lock."}
            })),
            &context,
        )
        .await
        .unwrap();
    let failure = "Error: poisoned target directory";
    let recall = |session| {
        store.command_recall(
            &person,
            session,
            context.project_id.as_deref(),
            "cargo test --workspace",
            Some(failure),
        )
    };
    let state = |session| {
        sqlx::query_as::<_, (bool, bool, bool)>(
        "SELECT used_at IS NOT NULL, inferred_use, error_repeated FROM session_shown WHERE session_id = $1"
    ).bind(session).fetch_one(store.pool())
    };
    assert!(!recall("repeated").await.unwrap().is_empty());
    assert!(recall("repeated").await.unwrap().is_empty());
    store.end_session(&person, "repeated").await.unwrap();
    assert_eq!(state("repeated").await.unwrap(), (false, false, true));

    assert!(!recall("resolved").await.unwrap().is_empty());
    store.end_session(&person, "resolved").await.unwrap();
    assert_eq!(state("resolved").await.unwrap(), (true, true, false));
    let later_command = store
        .command_recall(
            &person,
            "resolved",
            context.project_id.as_deref(),
            "cargo build",
            Some(failure),
        )
        .await
        .unwrap();
    assert!(later_command.is_empty());
    assert_eq!(state("resolved").await.unwrap(), (false, false, true));

    assert!(!recall("explicit").await.unwrap().is_empty());
    assert!(recall("muted").await.unwrap().is_empty());
    store
        .record_push_use(
            &person,
            Some("explicit"),
            std::slice::from_ref(&memory.id),
            true,
        )
        .await
        .unwrap();
    assert!(recall("explicit").await.unwrap().is_empty());
    store.end_session(&person, "explicit").await.unwrap();
    assert_eq!(state("explicit").await.unwrap(), (true, false, true));
    assert!(!recall("idle").await.unwrap().is_empty());
    sqlx::query("UPDATE session_shown SET shown_at = now() - interval '8 days'")
        .execute(store.pool())
        .await
        .unwrap();
    store.sweep().await.unwrap();
    let counts: (i32, i32, bool) =
        sqlx::query_as("SELECT pushed, used, muted FROM push_usage WHERE memory_id = $1")
            .bind(&memory.id)
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(counts, (4, 2, false));
    db.finish().await;
}

#[tokio::test]
async fn explicit_use_before_muting_counts_without_a_session() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let person = db.person("early-use").await;
    let (project, _) = store
        .init_project(&person, "local:e2e-early-use", Some("E2E early use"))
        .await
        .unwrap();
    let mut ids = Vec::new();
    for (kind, word) in [("search", "alpha"), ("get", "bravo"), ("helpful", "carol")] {
        let command = format!("make {word}");
        let failure = format!("Error: {word} cache poisoned");
        let context = WorkContext {
            project_id: Some(project.project_id.clone()),
            commands: vec![command.clone()],
            ..WorkContext::default()
        };
        let memory = store
            .save(
                &person,
                save_input(serde_json::json!({
                    "kind": "gotcha", "title": format!("Clear the {word} cache"),
                    "summary": failure, "reason": "A stale lock holds the directory.",
                    "fields": {"fix": "Delete the stale lock file."}
                })),
                &context,
            )
            .await
            .unwrap();
        let push = |session: &'static str| {
            store.command_recall(
                &person,
                session,
                context.project_id.as_deref(),
                &command,
                Some(&failure),
            )
        };
        assert!(!push("s1").await.unwrap().is_empty());
        let one = std::slice::from_ref(&memory.id);
        if kind == "search" {
            let hits = store
                .search(
                    &person,
                    word,
                    None,
                    5,
                    1200,
                    &cairn_core::retrieve::filter::Filter::default(),
                )
                .await
                .unwrap();
            assert!(hits.results.iter().any(|c| c.id == memory.id), "{word}");
        } else {
            // Get and helpful votes may carry no session id (a session the ledger never saw too).
            let session = (kind == "get").then_some("elsewhere");
            store
                .record_push_use(&person, session, one, true)
                .await
                .unwrap();
        }
        assert!(!push("s2").await.unwrap().is_empty());
        assert!(!push("s3").await.unwrap().is_empty(), "{kind} was muted");
        ids.push(memory.id);
    }
    let rows: Vec<(i32, bool)> = sqlx::query_as(
        "SELECT used, muted FROM push_usage WHERE memory_id = ANY($1) ORDER BY memory_id",
    )
    .bind(&ids)
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter().all(|&(used, muted)| used == 1 && !muted),
        "{rows:?}"
    );

    // One session's explicit use is credited once, however often the same call repeats.
    let again = std::slice::from_ref(&ids[0]);
    let used = || async {
        sqlx::query_scalar::<_, i32>("SELECT used FROM push_usage WHERE memory_id = $1")
            .bind(&ids[0])
            .fetch_one(store.pool())
            .await
            .unwrap()
    };
    let before = used().await;
    for _ in 0..3 {
        store
            .record_push_use(&person, Some("s1"), again, true)
            .await
            .unwrap();
    }
    assert_eq!(used().await, before + 1);
    db.finish().await;
}

#[tokio::test]
async fn pin_quotas_are_atomic_and_scoped() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let owner = db.person("pins-owner").await;
    let other = db.person("pins-other").await;
    let store = &db.store;
    let (project, _) = store
        .init_project(&owner, "local:e2e-pins", Some("E2E Pin limits"))
        .await
        .unwrap();
    let project_id = &project.project_id;
    sqlx::query("INSERT INTO project_members (project_id, person_id) VALUES ($1, $2)")
        .bind(project_id)
        .bind(&other)
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO memories (memory_id, owner_id, project_id, scope, kind, title, summary,
                               evidence_hash, identity_hash)
         SELECT scope || i, CASE WHEN scope = 'project' AND i % 2 = 0 THEN $2 ELSE $1 END,
                CASE WHEN scope = 'project' THEN $3 END, scope, 'rule', 'Pin fixture', 'Pin summary',
                'e', scope || i
           FROM unnest(ARRAY['personal','project']) scope
           CROSS JOIN generate_series(1, 11) i",
    )
    .bind(&owner)
    .bind(&other)
    .bind(project_id)
    .execute(store.pool())
    .await
    .unwrap();

    for (scope, limit) in [("personal", 5), ("project", 10)] {
        let person = |i| {
            if scope == "project" && i % 2 == 0 {
                &other
            } else {
                &owner
            }
        };
        for i in 1..limit {
            assert!(
                store
                    .set_pinned(person(i), &format!("{scope}{i}"), true)
                    .await
                    .unwrap()
                    .is_some()
            );
        }
        let id1 = format!("{scope}{limit}");
        let id2 = format!("{scope}{}", limit + 1);
        let first = store.set_pinned(person(limit), &id1, true);
        let second = store.set_pinned(person(limit + 1), &id2, true);
        let (first, second) = tokio::join!(first, second);
        assert_ne!(first.is_ok(), second.is_ok());
        let (winner, loser) = if first.is_ok() {
            ((limit, &id1), (limit + 1, &id2))
        } else {
            ((limit + 1, &id2), (limit, &id1))
        };
        let conflict = first.err().or(second.err()).unwrap();
        assert!(
            matches!(conflict, cairn_store::StoreError::Conflict(message) if message.contains(&format!("limit of {limit}")))
        );
        let pinned_at = store
            .set_pinned(person(winner.0), winner.1, true)
            .await
            .unwrap();
        assert!(pinned_at.is_some());
        assert_eq!(
            store
                .set_pinned(person(winner.0), winner.1, true)
                .await
                .unwrap(),
            pinned_at
        );
        assert_eq!(
            store
                .set_pinned(person(winner.0), winner.1, false)
                .await
                .unwrap(),
            None
        );
        assert!(
            store
                .set_pinned(person(loser.0), loser.1, true)
                .await
                .unwrap()
                .is_some()
        );
    }
    db.finish().await;
}
