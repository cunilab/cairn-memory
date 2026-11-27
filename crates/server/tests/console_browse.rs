//! Console browse: literal search, numbered pages, and pin quotas.

mod common;
use common::App;
use serde_json::{Value, json};

fn rule(title: &str, summary: &str) -> Value {
    json!({
        "kind": "rule",
        "title": title,
        "summary": summary,
        "reason": "the test needs a reason that is long enough"
    })
}

fn ids(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn exact_search_is_literal_and_related_still_stems() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let member = app.member(&admin, "member").await;
    let device = app.device(&admin, "laptop").await;
    let member_device = app.device(&member, "member-laptop").await;

    let worker = app
        .save(
            &device,
            rule(
                "retry worker budget",
                "A singular worker budget stays stored.",
            ),
        )
        .await;
    let workers = app
        .save(
            &device,
            rule(
                "retry workers budget",
                "The plural form is a different phrase.",
            ),
        )
        .await;
    app.save(
        &device,
        json!({
            "kind": "fact",
            "title": "Percent stored literally",
            "summary": "The gauge reads 100% after deploy.",
            "reason": "operators asked for the raw figure",
            "fields": {"source": "the run book"}
        }),
    )
    .await;
    let cafe = app
        .save(
            &device,
            rule("Café menu item", "The café keeps its accent in the title."),
        )
        .await;
    app.save(
        &device,
        rule(
            "keep  two spaces",
            "Internal whitespace is part of the phrase.",
        ),
    )
    .await;
    let secret = app
        .save(
            &member_device,
            rule(
                "member private budget",
                "Only the member should see this note.",
            ),
        )
        .await;
    let forgotten = app
        .save(
            &device,
            rule(
                "forgotten budget phrase",
                "This one gets forgotten after the save.",
            ),
        )
        .await;
    app.post("forget", json!({"id": forgotten}), &admin).await;
    let steps = app
        .save(
            &device,
            json!({
                "kind": "procedure",
                "title": "Release steps stay separate",
                "summary": "Each step is its own field value.",
                "reason": "a joined string would be a false match",
                "fields": {"steps": ["alpha", "beta"], "when": "during release"}
            }),
        )
        .await;
    let detail = app
        .save(
            &device,
            json!({
                "kind": "note",
                "title": "Detail only phrase holder",
                "summary": "The summary does not contain the marker.",
                "detail": "needle lives in the detail field only"
            }),
        )
        .await;

    let exact = |q: &str| format!("console/memories?match=exact&q={}", urlencoding(q));
    let workers_page = app.get(&exact("retry workers budget"), &admin).await;
    assert_eq!(ids(&workers_page), vec![workers.clone()], "{workers_page}");
    assert_eq!(workers_page["items"][0]["matched"]["field"], "title");
    assert!(
        workers_page["items"][0]["matched"]["segments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|segment| segment["hit"] == true)
    );

    let stemmed = app
        .get(
            &format!(
                "console/memories?match=related&q={}",
                urlencoding("retry workers budget")
            ),
            &admin,
        )
        .await;
    let stemmed_ids = ids(&stemmed);
    assert!(
        stemmed_ids.contains(&worker),
        "related keeps the english stem: {stemmed}"
    );
    assert!(stemmed_ids.contains(&workers));

    let percent = app.get(&exact("%"), &admin).await;
    assert!(percent["total"].as_i64().unwrap() >= 1, "{percent}");
    assert!(
        percent["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["title"] == "Percent stored literally")
    );

    let accent = app.get(&exact("café"), &admin).await;
    assert_eq!(ids(&accent), vec![cafe], "{accent}");
    assert!(
        app.get(&exact("cafe"), &admin).await["total"]
            .as_i64()
            .unwrap()
            == 0
    );

    let spaces = app.get(&exact("  two"), &admin).await;
    assert_eq!(spaces["items"][0]["title"], "keep  two spaces");
    assert_eq!(
        app.get(&exact("keep two"), &admin).await["total"]
            .as_i64()
            .unwrap(),
        0
    );

    let joined = app.get(&exact("alphabeta"), &admin).await;
    assert_eq!(joined["total"].as_i64().unwrap(), 0, "{joined}");
    let step = app.get(&exact("beta"), &admin).await;
    assert_eq!(ids(&step), vec![steps], "{step}");
    assert_eq!(step["items"][0]["matched"]["label"], "Steps");

    let in_detail = app.get(&exact("needle lives"), &admin).await;
    assert_eq!(ids(&in_detail), vec![detail], "{in_detail}");
    assert_eq!(in_detail["items"][0]["matched"]["field"], "detail");
    assert!(
        in_detail["items"][0].get("detail").is_none(),
        "list rows omit full detail"
    );

    let stop = app.get(&exact("the"), &admin).await;
    assert!(
        stop["total"].as_i64().unwrap() >= 1,
        "exact keeps stop words: {stop}"
    );

    let admin_ids = ids(&app.get("console/memories?pageSize=100", &admin).await);
    assert!(
        !admin_ids.contains(&secret),
        "a personal memory stays with its owner"
    );
    assert!(!admin_ids.contains(&forgotten));
    let member_ids = ids(&app.get("console/memories?pageSize=100", &member).await);
    assert!(member_ids.contains(&secret));
    assert!(!member_ids.contains(&worker));
    let forgotten_page = app.get("console/memories?status=forgotten", &admin).await;
    assert_eq!(ids(&forgotten_page), vec![forgotten]);
    assert_eq!(
        app.get("console/memories?status=forgotten", &member).await["total"]
            .as_i64()
            .unwrap(),
        0
    );

    let (status, body) = app
        .send(
            "GET",
            "console/memories?pageSize=10",
            json!(null),
            Some(&admin),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    let (status, _) = app
        .send("GET", "console/memories?page=0", json!(null), Some(&admin))
        .await;
    assert_eq!(status, 400);
    let (status, _) = app
        .send(
            "GET",
            "console/memories?sort=title",
            json!(null),
            Some(&admin),
        )
        .await;
    assert_eq!(status, 400);
    let (status, _) = app
        .send(
            "GET",
            "console/memories?match=fuzzy",
            json!(null),
            Some(&admin),
        )
        .await;
    assert_eq!(status, 400);
    let (status, body) = app
        .send("GET", "console/memories", json!(null), Some(&device))
        .await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["code"], "session_required");

    app.finish().await;
}

#[tokio::test]
async fn pages_cover_the_filtered_set_once() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "laptop").await;
    let mut saved = Vec::new();
    for n in 0..30 {
        saved.push(
            app.save(
                &device,
                rule(
                    &format!("Paged memory number {n:02} rule"),
                    &format!("Summary for paged memory number {n:02} only."),
                ),
            )
            .await,
        );
    }
    let older = &saved[0];
    let newer = &saved[1];
    sql(
        &app,
        &format!("UPDATE memories SET title = 'Exact title phrase' WHERE memory_id = '{older}'"),
    )
    .await;
    sql(
        &app,
        &format!("UPDATE memories SET summary = 'mentions Exact title phrase inside', updated_at = now() WHERE memory_id = '{newer}'"),
    )
    .await;
    sql(
        &app,
        &format!(
            "UPDATE memories SET updated_at = now() - interval '2 days' WHERE memory_id = '{older}'"
        ),
    )
    .await;

    let first = app.get("console/memories?page=1&pageSize=25", &admin).await;
    let second = app.get("console/memories?page=2&pageSize=25", &admin).await;
    assert_eq!(first["total"].as_i64().unwrap(), 30);
    assert_eq!(first["page"].as_i64().unwrap(), 1);
    assert_eq!(first["page_size"].as_i64().unwrap(), 25);
    assert_eq!(ids(&first).len(), 25);
    assert_eq!(ids(&second).len(), 5);
    assert_eq!(second["page"].as_i64().unwrap(), 2);
    let mut all = ids(&first);
    all.extend(ids(&second));
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 30);

    let clamped = app.get("console/memories?page=9&pageSize=25", &admin).await;
    assert_eq!(clamped["page"].as_i64().unwrap(), 2);
    assert_eq!(ids(&clamped), ids(&second));

    let ordered = app
        .get(
            &format!(
                "console/memories?match=exact&q={}",
                urlencoding("Exact title phrase")
            ),
            &admin,
        )
        .await;
    assert_eq!(
        ordered["items"][0]["id"], *older,
        "exact title leads: {ordered}"
    );
    assert_eq!(ordered["total"].as_i64().unwrap(), 2);

    app.finish().await;
}

#[tokio::test]
async fn pinned_tab_reports_quotas_without_changing_filters() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let member = app.member(&admin, "member").await;
    let device = app.device(&admin, "laptop").await;
    let member_device = app.device(&member, "member-laptop").await;
    let project = app.project(&device, "github.com/acme/widgets").await;
    app.post(
        "projects/init",
        json!({"project_id": project, "repository": "x"}),
        &member_device,
    )
    .await;

    let personal_a = app
        .save(
            &device,
            rule(
                "Personal pin alpha rule",
                "First personal pin for the quota.",
            ),
        )
        .await;
    let personal_b = app
        .save(
            &device,
            rule(
                "Personal pin bravo rule",
                "Second personal pin for the quota.",
            ),
        )
        .await;
    let project_pin = app
        .save(
            &device,
            json!({
                "kind": "rule",
                "title": "Project pin cache rule",
                "summary": "Shared with the project members.",
                "reason": "the team needs this one pinned",
                "context": {"project_id": project, "repository": "github.com/acme/widgets", "branch": "main"}
            }),
        )
        .await;
    let fact = app
        .save(
            &device,
            json!({
                "kind": "fact",
                "title": "Unpinned fact stays out",
                "summary": "This fact is not pinned.",
                "reason": "it only checks the pinned filter",
                "fields": {"source": "notes"}
            }),
        )
        .await;
    for id in [&personal_a, &personal_b, &project_pin] {
        app.post(
            &format!("memories/{id}/pin"),
            json!({"pinned": true}),
            &admin,
        )
        .await;
    }
    sql(&app, &format!("UPDATE memories SET pinned_at = now() - interval '1 hour' WHERE memory_id = '{personal_a}'")).await;
    sql(
        &app,
        &format!("UPDATE memories SET status = 'superseded' WHERE memory_id = '{personal_b}'"),
    )
    .await;

    let pinned = app
        .get("console/memories?pinned=true&pageSize=25", &admin)
        .await;
    assert_eq!(
        ids(&pinned),
        vec![personal_b.clone(), personal_a.clone(), project_pin.clone()],
        "{pinned}"
    );
    assert!(!ids(&pinned).contains(&fact));
    let quotas = pinned["quotas"].as_array().unwrap();
    assert_eq!(quotas[0]["name"], "Personal");
    assert_eq!(quotas[0]["used"].as_i64().unwrap(), 2);
    assert_eq!(quotas[0]["limit"].as_i64().unwrap(), 3);
    let project_quota = quotas
        .iter()
        .find(|quota| quota["project_id"] == project)
        .unwrap();
    assert_eq!(project_quota["used"].as_i64().unwrap(), 1);
    assert_eq!(project_quota["limit"].as_i64().unwrap(), 4);

    let narrowed = app
        .get("console/memories?pinned=true&kind=fact", &admin)
        .await;
    assert!(ids(&narrowed).is_empty(), "{narrowed}");
    assert_eq!(
        narrowed["quotas"][0]["used"].as_i64().unwrap(),
        2,
        "filters do not shrink the quota"
    );

    let seen = app.get("console/memories?pinned=true", &member).await;
    assert_eq!(ids(&seen), vec![project_pin.clone()], "{seen}");
    assert_eq!(seen["items"][0]["owner_is_me"], false);
    let (status, _) = app
        .send(
            "POST",
            &format!("memories/{project_pin}/pin"),
            json!({"pinned": false}),
            Some(&member),
        )
        .await;
    assert_eq!(status, 404);

    app.post(&format!("projects/{project}/archive"), json!({}), &admin)
        .await;
    let (status, body) = app
        .send(
            "POST",
            &format!("memories/{project_pin}/pin"),
            json!({"pinned": false}),
            Some(&admin),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    let still = app
        .get(
            &format!("console/memories?pinned=true&projectId={project}"),
            &admin,
        )
        .await;
    assert_eq!(
        ids(&still),
        vec![project_pin],
        "archived pins stay explainable: {still}"
    );
    assert_eq!(still["quotas"][0]["limit"].as_i64().unwrap(), 4);

    app.finish().await;
}

fn urlencoding(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

async fn sql(app: &App, query: &str) {
    sqlx::query(query)
        .execute(app.db.store.pool())
        .await
        .unwrap();
}
