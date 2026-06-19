use trpg_db::Db;
use trpg_runtime::RuntimeEngine;

const RULESET: &str = "call_of_cthulhu_7e";
const SESSION: &str = "sess_live_chargen_explicit_session";
const ACTOR: &str = "pc.live_chargen_explicit_session";

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Db::connect(&url).await.ok()
}

async fn reset(db: &Db) {
    sqlx::query("delete from runtime_actor_parameters where session_id=$1")
        .bind(SESSION)
        .execute(&db.pool)
        .await
        .ok();
    sqlx::query("delete from sessions where session_id=$1")
        .bind(SESSION)
        .execute(&db.pool)
        .await
        .ok();
}

#[tokio::test]
async fn create_and_bind_character_with_explicit_session_initializes_session_row() {
    let Some(db) = connect().await else { return };
    if db
        .load_character_onboarding_pack(RULESET)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        eprintln!("SKIP: no {RULESET} character onboarding pack");
        return;
    }
    reset(&db).await;

    let created = RuntimeEngine::new(db.clone())
        .create_and_bind_character(
            &trpg_llm::MockLlmClient,
            RULESET,
            Some("document"),
            Some(SESSION),
            ACTOR,
            "live regression",
        )
        .await
        .expect("create and bind character");
    assert_eq!(created.session_id, SESSION);

    let row: Option<(String, Option<String>)> =
        sqlx::query_as("select ruleset_id, module_id from sessions where session_id=$1")
            .bind(SESSION)
            .fetch_optional(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        row,
        Some((RULESET.to_string(), Some("document".to_string()))),
        "explicit --session-id must still create the sessions row used by turns/memory_events FKs"
    );
    let actor_count: i64 = sqlx::query_scalar(
        "select count(*) from runtime_actor_parameters where session_id=$1 and actor_id=$2",
    )
    .bind(SESSION)
    .bind(ACTOR)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(actor_count, 1);

    sqlx::query("delete from characters where character_id=$1")
        .bind(&created.character_id)
        .execute(&db.pool)
        .await
        .ok();
    reset(&db).await;
}
