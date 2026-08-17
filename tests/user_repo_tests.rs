mod helpers;

use keystone::db::Database;
use keystone::db::repos::UserRepository;
use keystone::db::rows::user_row::CreateUserData;
use keystone::error::AppError;
use keystone::models::UserRole;
use uuid::Uuid;

async fn create_test_user(db: &Database) -> CreateUserData {
    let data = CreateUserData {
        username: format!("user_{}", &Uuid::new_v4().to_string()[..8]),
        email: format!("user_{}@test.com", &Uuid::new_v4().to_string()[..8]),
        password_hash: "hashed_password".to_string(),
        role: UserRole::User,
        storage_quota: 1_073_741_824,
    };
    UserRepository::create(db.pool(), CreateUserData {
        username: data.username.clone(),
        email: data.email.clone(),
        password_hash: data.password_hash.clone(),
        role: data.role,
        storage_quota: data.storage_quota,
    })
    .await
    .unwrap();
    data
}

#[tokio::test]
async fn test_create_user() {
    let db = helpers::setup_reset_db().await;
    let data = create_test_user(&db).await;

    let user = UserRepository::find_by_username(db.pool(), &data.username)
        .await
        .unwrap()
        .expect("user should exist");

    assert!(!user.id.is_nil());
    assert_eq!(user.username, data.username);
    assert_eq!(user.email, data.email);
    assert_eq!(user.password_hash, "hashed_password");
    assert_eq!(user.role, UserRole::User);
    assert_eq!(user.storage_quota, 1_073_741_824);
    assert_eq!(user.storage_used, 0);
    assert!(user.last_login_at.is_none());
}

#[tokio::test]
async fn test_create_admin_user() {
    let db = helpers::setup_reset_db().await;
    let username = format!("admin_{}", &Uuid::new_v4().to_string()[..8]);
    let email = format!("admin_{}@test.com", &Uuid::new_v4().to_string()[..8]);

    let created = UserRepository::create(
        db.pool(),
        CreateUserData {
            username: username.clone(),
            email: email.clone(),
            password_hash: "hashed_password".to_string(),
            role: UserRole::Admin,
            storage_quota: 1_073_741_824,
        },
    )
    .await
    .unwrap();

    assert_eq!(created.role, UserRole::Admin);

    let fetched = UserRepository::find_by_id(db.pool(), created.id)
        .await
        .unwrap()
        .expect("user should exist");
    assert_eq!(fetched.role, UserRole::Admin);
}

#[tokio::test]
async fn test_create_duplicate_username() {
    let db = helpers::setup_reset_db().await;
    let data = create_test_user(&db).await;

    let result = UserRepository::create(
        db.pool(),
        CreateUserData {
            username: data.username,
            email: format!("different_{}@test.com", &Uuid::new_v4().to_string()[..8]),
            password_hash: "hashed_password".to_string(),
            role: UserRole::User,
            storage_quota: 1_073_741_824,
        },
    )
    .await;

    assert!(matches!(result, Err(AppError::Conflict(_))));
}

#[tokio::test]
async fn test_create_duplicate_email() {
    let db = helpers::setup_reset_db().await;
    let data = create_test_user(&db).await;

    let result = UserRepository::create(
        db.pool(),
        CreateUserData {
            username: format!("different_{}", &Uuid::new_v4().to_string()[..8]),
            email: data.email,
            password_hash: "hashed_password".to_string(),
            role: UserRole::User,
            storage_quota: 1_073_741_824,
        },
    )
    .await;

    assert!(matches!(result, Err(AppError::Conflict(_))));
}

#[tokio::test]
async fn test_find_by_id() {
    let db = helpers::setup_reset_db().await;
    let data = create_test_user(&db).await;

    let user = UserRepository::find_by_username(db.pool(), &data.username)
        .await
        .unwrap()
        .expect("user should exist");

    let found = UserRepository::find_by_id(db.pool(), user.id)
        .await
        .unwrap()
        .expect("should find by id");
    assert_eq!(found.id, user.id);
    assert_eq!(found.username, data.username);

    let nonexistent = Uuid::new_v4();
    let not_found = UserRepository::find_by_id(db.pool(), nonexistent)
        .await
        .unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_find_by_email() {
    let db = helpers::setup_reset_db().await;
    let data = create_test_user(&db).await;

    let found = UserRepository::find_by_email(db.pool(), &data.email)
        .await
        .unwrap()
        .expect("should find by email");
    assert_eq!(found.email, data.email);
    assert_eq!(found.username, data.username);

    let not_found = UserRepository::find_by_email(db.pool(), "nonexistent@test.com")
        .await
        .unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_find_by_username() {
    let db = helpers::setup_reset_db().await;
    let data = create_test_user(&db).await;

    let found = UserRepository::find_by_username(db.pool(), &data.username)
        .await
        .unwrap()
        .expect("should find by username");
    assert_eq!(found.username, data.username);
    assert_eq!(found.email, data.email);

    let not_found = UserRepository::find_by_username(db.pool(), "nonexistent_user")
        .await
        .unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_list_users() {
    let db = helpers::setup_reset_db().await;

    let mut usernames = Vec::new();
    for _ in 0..5 {
        let data = create_test_user(&db).await;
        usernames.push(data.username);
    }

    let page1 = UserRepository::list(db.pool(), 0, 3).await.unwrap();
    assert_eq!(page1.len(), 3);

    let page2 = UserRepository::list(db.pool(), 3, 3).await.unwrap();
    assert_eq!(page2.len(), 2);

    let all = UserRepository::list(db.pool(), 0, 100).await.unwrap();
    assert_eq!(all.len(), 5);

    for window in all.windows(2) {
        assert!(window[0].created_at >= window[1].created_at);
    }
}

#[tokio::test]
async fn test_count_users() {
    let db = helpers::setup_reset_db().await;

    let count = UserRepository::count(db.pool()).await.unwrap();
    assert_eq!(count, 0);

    create_test_user(&db).await;
    create_test_user(&db).await;
    create_test_user(&db).await;

    let count = UserRepository::count(db.pool()).await.unwrap();
    assert_eq!(count, 3);
}

#[tokio::test]
async fn test_update_last_login() {
    let db = helpers::setup_reset_db().await;
    let data = create_test_user(&db).await;

    let user = UserRepository::find_by_username(db.pool(), &data.username)
        .await
        .unwrap()
        .expect("user should exist");
    assert!(user.last_login_at.is_none());

    UserRepository::update_last_login(db.pool(), user.id)
        .await
        .unwrap();

    let updated = UserRepository::find_by_id(db.pool(), user.id)
        .await
        .unwrap()
        .expect("user should exist");
    assert!(updated.last_login_at.is_some());

    let nonexistent = Uuid::new_v4();
    let result = UserRepository::update_last_login(db.pool(), nonexistent).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_update_storage_used() {
    let db = helpers::setup_reset_db().await;
    let data = create_test_user(&db).await;

    let user = UserRepository::find_by_username(db.pool(), &data.username)
        .await
        .unwrap()
        .expect("user should exist");
    assert_eq!(user.storage_used, 0);

    UserRepository::update_storage_used(db.pool(), user.id, 500)
        .await
        .unwrap();

    let updated = UserRepository::find_by_id(db.pool(), user.id)
        .await
        .unwrap()
        .expect("user should exist");
    assert_eq!(updated.storage_used, 500);

    let nonexistent = Uuid::new_v4();
    let result = UserRepository::update_storage_used(db.pool(), nonexistent, 500).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_delete_user() {
    let db = helpers::setup_reset_db().await;
    let data = create_test_user(&db).await;

    let user = UserRepository::find_by_username(db.pool(), &data.username)
        .await
        .unwrap()
        .expect("user should exist");

    let deleted = UserRepository::delete(db.pool(), user.id).await.unwrap();
    assert!(deleted);

    let gone = UserRepository::find_by_id(db.pool(), user.id).await.unwrap();
    assert!(gone.is_none());

    let deleted_again = UserRepository::delete(db.pool(), user.id).await.unwrap();
    assert!(!deleted_again);
}

#[tokio::test]
async fn test_update_user_email() {
    let db = helpers::setup_reset_db().await;
    let data = create_test_user(&db).await;

    let user = UserRepository::find_by_username(db.pool(), &data.username)
        .await
        .unwrap()
        .expect("user should exist");

    let new_email = "updated@test.com";
    UserRepository::update_user(db.pool(), user.id, Some(new_email), None, None)
        .await
        .unwrap();

    let updated = UserRepository::find_by_id(db.pool(), user.id)
        .await
        .unwrap()
        .expect("user should exist");
    assert_eq!(updated.email, new_email);
}

#[tokio::test]
async fn test_update_user_role() {
    let db = helpers::setup_reset_db().await;
    let data = create_test_user(&db).await;

    let user = UserRepository::find_by_username(db.pool(), &data.username)
        .await
        .unwrap()
        .expect("user should exist");
    assert_eq!(user.role, UserRole::User);

    UserRepository::update_user(db.pool(), user.id, None, Some("admin"), None)
        .await
        .unwrap();

    let updated = UserRepository::find_by_id(db.pool(), user.id)
        .await
        .unwrap()
        .expect("user should exist");
    assert_eq!(updated.role, UserRole::Admin);
}

#[tokio::test]
async fn test_update_user_password_hash() {
    let db = helpers::setup_reset_db().await;
    let data = create_test_user(&db).await;

    let user = UserRepository::find_by_username(db.pool(), &data.username)
        .await
        .unwrap()
        .expect("user should exist");
    assert_eq!(user.password_hash, "hashed_password");

    UserRepository::update_user(db.pool(), user.id, None, None, Some("new_hash"))
        .await
        .unwrap();

    let updated = UserRepository::find_by_id(db.pool(), user.id)
        .await
        .unwrap()
        .expect("user should exist");
    assert_eq!(updated.password_hash, "new_hash");
}

#[tokio::test]
async fn test_update_user_not_found() {
    let db = helpers::setup_reset_db().await;
    let nonexistent = Uuid::new_v4();

    let result = UserRepository::update_user(db.pool(), nonexistent, Some("a@b.com"), None, None).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_update_storage_quota() {
    let db = helpers::setup_reset_db().await;
    let data = create_test_user(&db).await;

    let user = UserRepository::find_by_username(db.pool(), &data.username)
        .await
        .unwrap()
        .expect("user should exist");

    UserRepository::update_storage_quota(db.pool(), user.id, 2_000_000)
        .await
        .unwrap();

    let updated = UserRepository::find_by_id(db.pool(), user.id)
        .await
        .unwrap()
        .expect("user should exist");
    assert_eq!(updated.storage_quota, 2_000_000);
}

#[tokio::test]
async fn test_update_storage_quota_not_found() {
    let db = helpers::setup_reset_db().await;
    let nonexistent = Uuid::new_v4();

    let result = UserRepository::update_storage_quota(db.pool(), nonexistent, 500).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_update_password_hash_not_found() {
    let db = helpers::setup_reset_db().await;
    let nonexistent = Uuid::new_v4();

    let result = UserRepository::update_password_hash(db.pool(), nonexistent, "new_hash").await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}
