//! Real-PostgreSQL checks for sign-in identity: a person who signs in again
//! is the same user. Run with `bash scripts/test-plugin-runtime.sh`.

use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn signing_in_again_links_the_same_user() {
    let url =
        std::env::var("PLATFORM_TEST_DATABASE_URL").expect("PLATFORM_TEST_DATABASE_URL is set");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect");
    let issuer = "https://issuer.example/realms/test";
    let subject = Uuid::now_v7().to_string();
    let first = platform_db::find_or_create_external_user(
        &pool,
        issuer,
        &subject,
        "Pat",
        Some("pat@example.test"),
    )
    .await
    .expect("first sign-in");
    let again = platform_db::find_or_create_external_user(
        &pool,
        issuer,
        &subject,
        "Pat",
        Some("pat@example.test"),
    )
    .await
    .expect("second sign-in");
    assert_eq!(first.id, again.id);
    let other = platform_db::find_or_create_external_user(
        &pool,
        "https://issuer.example/realms",
        &format!("test{subject}"),
        "Someone else",
        None,
    )
    .await
    .expect("another identity");
    assert_ne!(first.id, other.id);
}
