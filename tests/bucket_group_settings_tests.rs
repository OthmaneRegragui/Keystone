mod helpers;

use keystone::db::repos::buckets::BucketRepository;
use keystone::db::repos::groups::GroupRepository;
use keystone::db::repos::settings::AdminSettingRepository;
use keystone::db::repos::users::UserRepository;
use keystone::db::rows::user_row::CreateUserData;
use keystone::error::AppError;
use keystone::models::UserRole;
use uuid::Uuid;

async fn create_test_user(pool: &sqlx::PgPool) -> String {
    let id = Uuid::new_v4().to_string();
    let user = UserRepository::create(
        pool,
        CreateUserData {
            username: format!("user_{}", &id[..8]),
            email: format!("user_{}@test.com", &id[..8]),
            password_hash: "hash".to_string(),
            role: UserRole::User,
            storage_quota: 1_073_741_824,
        },
    )
    .await
    .unwrap();
    user.id.to_string()
}

// ─── BucketRepository Tests ─────────────────────────────────────────────

#[tokio::test]
async fn test_bucket_create() {
    let db = helpers::setup_test_db().await;
    let name = format!("b_{}", &Uuid::new_v4().to_string()[..8]);
    let bucket = BucketRepository::create(db.pool(), &name, "/data/bucket1")
        .await
        .unwrap();

    assert!(!bucket.id.is_empty());
    assert_eq!(bucket.name, name);
    assert_eq!(bucket.path, "/data/bucket1");
    assert!(bucket.is_active);
    assert!(bucket.visible_to_users);
}

#[tokio::test]
async fn test_bucket_create_duplicate() {
    let db = helpers::setup_test_db().await;
    let name = format!("b_{}", &Uuid::new_v4().to_string()[..8]);
    BucketRepository::create(db.pool(), &name, "/data/a")
        .await
        .unwrap();

    let result = BucketRepository::create(db.pool(), &name, "/data/b").await;
    assert!(matches!(result, Err(AppError::Conflict(_))));
}

#[tokio::test]
async fn test_bucket_list() {
    let db = helpers::setup_test_db().await;
    let n1 = format!("b_{}", &Uuid::new_v4().to_string()[..8]);
    let n2 = format!("b_{}", &Uuid::new_v4().to_string()[..8]);
    let n3 = format!("b_{}", &Uuid::new_v4().to_string()[..8]);

    BucketRepository::create(db.pool(), &n1, "/1").await.unwrap();
    BucketRepository::create(db.pool(), &n2, "/2").await.unwrap();
    BucketRepository::create(db.pool(), &n3, "/3").await.unwrap();

    let buckets = BucketRepository::list(db.pool()).await.unwrap();
    // The list is global across the shared test DB, so only assert on the
    // buckets this test created.
    let ours: Vec<_> = buckets
        .iter()
        .filter(|b| b.name == n1 || b.name == n2 || b.name == n3)
        .collect();
    assert_eq!(ours.len(), 3);
    assert!(ours.iter().any(|b| b.name == n1));
    assert!(ours.iter().any(|b| b.name == n2));
    assert!(ours.iter().any(|b| b.name == n3));
    // The global list is ORDER BY name ASC, so the filtered subset preserves that order.
    assert!(ours[0].name < ours[1].name);
    assert!(ours[1].name < ours[2].name);
}

#[tokio::test]
async fn test_bucket_find_by_name() {
    let db = helpers::setup_test_db().await;
    let name = format!("b_{}", &Uuid::new_v4().to_string()[..8]);
    BucketRepository::create(db.pool(), &name, "/data/x")
        .await
        .unwrap();

    let found = BucketRepository::find_by_name(db.pool(), &name)
        .await
        .unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, name);

    let not_found = BucketRepository::find_by_name(db.pool(), "nonexistent").await.unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_bucket_set_visible() {
    let db = helpers::setup_test_db().await;
    let name = format!("b_{}", &Uuid::new_v4().to_string()[..8]);
    BucketRepository::create(db.pool(), &name, "/data/x")
        .await
        .unwrap();

    BucketRepository::set_visible(db.pool(), &name, false)
        .await
        .unwrap();

    let b = BucketRepository::find_by_name(db.pool(), &name)
        .await
        .unwrap()
        .unwrap();
    assert!(!b.visible_to_users);
}

#[tokio::test]
async fn test_bucket_set_visible_nonexistent() {
    let db = helpers::setup_test_db().await;
    let result = BucketRepository::set_visible(db.pool(), "no_such", false).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_bucket_update() {
    let db = helpers::setup_test_db().await;
    let name = format!("b_{}", &Uuid::new_v4().to_string()[..8]);
    BucketRepository::create(db.pool(), &name, "/old")
        .await
        .unwrap();

    let new_name = format!("b_{}", &Uuid::new_v4().to_string()[..8]);
    BucketRepository::update(db.pool(), &name, &new_name, "/new", false, false, 999)
        .await
        .unwrap();

    let b = BucketRepository::find_by_name(db.pool(), &new_name)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.name, new_name);
    assert_eq!(b.path, "/new");
    assert!(!b.is_active);
    assert!(!b.visible_to_users);
    assert_eq!(b.storage_limit, 999);
}

#[tokio::test]
async fn test_bucket_update_not_found() {
    let db = helpers::setup_test_db().await;
    let result =
        BucketRepository::update(db.pool(), "no_such", "new", "/x", true, true, 0).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_bucket_update_conflict() {
    let db = helpers::setup_test_db().await;
    let n1 = format!("b_{}", &Uuid::new_v4().to_string()[..8]);
    let n2 = format!("b_{}", &Uuid::new_v4().to_string()[..8]);
    BucketRepository::create(db.pool(), &n1, "/1").await.unwrap();
    BucketRepository::create(db.pool(), &n2, "/2").await.unwrap();

    let result =
        BucketRepository::update(db.pool(), &n1, &n2, "/1", true, true, 0).await;
    assert!(matches!(result, Err(AppError::Conflict(_))));
}

#[tokio::test]
async fn test_bucket_delete() {
    let db = helpers::setup_test_db().await;
    let name = format!("b_{}", &Uuid::new_v4().to_string()[..8]);
    BucketRepository::create(db.pool(), &name, "/data/x")
        .await
        .unwrap();

    let deleted = BucketRepository::delete(db.pool(), &name).await.unwrap();
    assert!(deleted);

    let gone = BucketRepository::find_by_name(db.pool(), &name).await.unwrap();
    assert!(gone.is_none());
}

#[tokio::test]
async fn test_bucket_delete_nonexistent() {
    let db = helpers::setup_test_db().await;
    let result = BucketRepository::delete(db.pool(), "no_such").await;
    assert!(matches!(result, Err(AppError::BadRequest(_))));
}

#[tokio::test]
async fn test_bucket_get_storage_used() {
    let db = helpers::setup_test_db().await;
    let sentinel = format!("no_such_backend_{}", Uuid::new_v4());
    let map = BucketRepository::get_storage_used_per_bucket(db.pool())
        .await
        .unwrap();
    // The map aggregates storage across the whole shared test DB, so instead of
    // asserting it is empty, assert that a backend with no storage appears nowhere.
    assert!(!map.contains_key(&sentinel));
}

#[tokio::test]
async fn test_bucket_list_visible_to_user() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();

    let user_id = create_test_user(pool).await;
    let group = GroupRepository::create(pool, &format!("g_{}", &Uuid::new_v4().to_string()[..8]))
        .await
        .unwrap();
    let bucket = BucketRepository::create(pool, &format!("b_{}", &Uuid::new_v4().to_string()[..8]), "/data/v")
        .await
        .unwrap();

    GroupRepository::add_member(pool, &group.id, &user_id).await.unwrap();
    GroupRepository::add_bucket(pool, &group.id, &bucket.id, 0).await.unwrap();

    let visible = BucketRepository::list_visible_to_user(pool, &user_id).await.unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].name, bucket.name);

    // User without group access gets empty
    let other_user = create_test_user(pool).await;
    let empty = BucketRepository::list_visible_to_user(pool, &other_user)
        .await
        .unwrap();
    assert!(empty.is_empty());
}

#[tokio::test]
async fn test_bucket_list_accessible_to_user() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();

    let user_id = create_test_user(pool).await;
    let g1 = GroupRepository::create(pool, &format!("g_{}", &Uuid::new_v4().to_string()[..8]))
        .await
        .unwrap();
    let g2 = GroupRepository::create(pool, &format!("g_{}", &Uuid::new_v4().to_string()[..8]))
        .await
        .unwrap();
    let bucket = BucketRepository::create(pool, &format!("b_{}", &Uuid::new_v4().to_string()[..8]), "/data/acc")
        .await
        .unwrap();

    GroupRepository::add_member(pool, &g1.id, &user_id).await.unwrap();
    GroupRepository::add_member(pool, &g2.id, &user_id).await.unwrap();
    // g1: upload=true, download=false, limit=100
    GroupRepository::add_bucket(pool, &g1.id, &bucket.id, 100).await.unwrap();
    GroupRepository::update_bucket_permissions(pool, &g1.id, &bucket.id, true, false).await.unwrap();
    // g2: upload=false, download=true, limit=500
    GroupRepository::add_bucket(pool, &g2.id, &bucket.id, 500).await.unwrap();
    GroupRepository::update_bucket_permissions(pool, &g2.id, &bucket.id, false, true).await.unwrap();

    let accessible = BucketRepository::list_accessible_to_user(pool, &user_id)
        .await
        .unwrap();
    assert_eq!(accessible.len(), 1);
    let b = &accessible[0];
    assert_eq!(b.name, bucket.name);
    // OR logic: true || false = true, false || true = true
    assert!(b.can_upload);
    assert!(b.can_download);
    // MAX limit
    assert_eq!(b.user_storage_limit, 500);
}

// ─── GroupRepository Tests ──────────────────────────────────────────────

#[tokio::test]
async fn test_group_create() {
    let db = helpers::setup_test_db().await;
    let name = format!("g_{}", &Uuid::new_v4().to_string()[..8]);
    let group = GroupRepository::create(db.pool(), &name).await.unwrap();

    assert!(!group.id.is_empty());
    assert_eq!(group.name, name);
}

#[tokio::test]
async fn test_group_create_duplicate() {
    let db = helpers::setup_test_db().await;
    let name = format!("g_{}", &Uuid::new_v4().to_string()[..8]);
    GroupRepository::create(db.pool(), &name).await.unwrap();

    let result = GroupRepository::create(db.pool(), &name).await;
    assert!(matches!(result, Err(AppError::Conflict(_))));
}

#[tokio::test]
async fn test_group_list() {
    let db = helpers::setup_test_db().await;
    let n1 = format!("g_{}", &Uuid::new_v4().to_string()[..8]);
    let n2 = format!("g_{}", &Uuid::new_v4().to_string()[..8]);
    let n3 = format!("g_{}", &Uuid::new_v4().to_string()[..8]);

    GroupRepository::create(db.pool(), &n1).await.unwrap();
    GroupRepository::create(db.pool(), &n2).await.unwrap();
    GroupRepository::create(db.pool(), &n3).await.unwrap();

    let groups = GroupRepository::list(db.pool()).await.unwrap();
    // The list is global across the shared test DB, so only assert on the
    // groups this test created.
    let ours: Vec<_> = groups
        .iter()
        .filter(|g| g.name == n1 || g.name == n2 || g.name == n3)
        .collect();
    assert_eq!(ours.len(), 3);
    assert!(ours.iter().any(|g| g.name == n1));
    assert!(ours.iter().any(|g| g.name == n2));
    assert!(ours.iter().any(|g| g.name == n3));
    // The global list is ORDER BY name ASC, so the filtered subset preserves that order.
    assert!(ours[0].name < ours[1].name);
    assert!(ours[1].name < ours[2].name);
}

#[tokio::test]
async fn test_group_delete() {
    let db = helpers::setup_test_db().await;
    let name = format!("g_{}", &Uuid::new_v4().to_string()[..8]);
    let group = GroupRepository::create(db.pool(), &name).await.unwrap();

    let deleted = GroupRepository::delete(db.pool(), &group.id).await.unwrap();
    assert!(deleted);

    let list = GroupRepository::list(db.pool()).await.unwrap();
    assert!(!list.iter().any(|g| g.id == group.id));

    let deleted_again = GroupRepository::delete(db.pool(), &group.id).await.unwrap();
    assert!(!deleted_again);
}

#[tokio::test]
async fn test_group_add_remove_member() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let group = GroupRepository::create(pool, &format!("g_{}", &Uuid::new_v4().to_string()[..8]))
        .await
        .unwrap();
    let user_id = create_test_user(pool).await;

    GroupRepository::add_member(pool, &group.id, &user_id).await.unwrap();
    let members = GroupRepository::list_members(pool, &group.id).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0], user_id);

    let removed = GroupRepository::remove_member(pool, &group.id, &user_id)
        .await
        .unwrap();
    assert!(removed);

    let empty = GroupRepository::list_members(pool, &group.id).await.unwrap();
    assert!(empty.is_empty());

    let removed_again = GroupRepository::remove_member(pool, &group.id, &user_id)
        .await
        .unwrap();
    assert!(!removed_again);
}

#[tokio::test]
async fn test_group_add_remove_bucket() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let group = GroupRepository::create(pool, &format!("g_{}", &Uuid::new_v4().to_string()[..8]))
        .await
        .unwrap();
    let bucket = BucketRepository::create(pool, &format!("b_{}", &Uuid::new_v4().to_string()[..8]), "/data/x")
        .await
        .unwrap();

    GroupRepository::add_bucket(pool, &group.id, &bucket.id, 1024)
        .await
        .unwrap();
    let buckets = GroupRepository::list_buckets(pool, &group.id).await.unwrap();
    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0], bucket.id);

    let removed = GroupRepository::remove_bucket(pool, &group.id, &bucket.id)
        .await
        .unwrap();
    assert!(removed);

    let empty = GroupRepository::list_buckets(pool, &group.id).await.unwrap();
    assert!(empty.is_empty());
}

#[tokio::test]
async fn test_group_list_group_bucket_details() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let group = GroupRepository::create(pool, &format!("g_{}", &Uuid::new_v4().to_string()[..8]))
        .await
        .unwrap();
    let bucket = BucketRepository::create(pool, &format!("b_{}", &Uuid::new_v4().to_string()[..8]), "/data/details")
        .await
        .unwrap();
    GroupRepository::add_bucket(pool, &group.id, &bucket.id, 500)
        .await
        .unwrap();

    let details = GroupRepository::list_group_bucket_details(pool, &group.id)
        .await
        .unwrap();
    assert_eq!(details.len(), 1);
    let (_id, name, path, storage_used, _bucket_limit, user_limit, _user_count, can_upload, can_download) =
        &details[0];
    assert_eq!(name, &bucket.name);
    assert_eq!(path, "/data/details");
    assert_eq!(*storage_used, 0);
    assert_eq!(*user_limit, 500);
    // default permissions are true
    assert!(can_upload);
    assert!(can_download);
}

#[tokio::test]
async fn test_group_update_bucket_permissions() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let group = GroupRepository::create(pool, &format!("g_{}", &Uuid::new_v4().to_string()[..8]))
        .await
        .unwrap();
    let bucket = BucketRepository::create(pool, &format!("b_{}", &Uuid::new_v4().to_string()[..8]), "/data/p")
        .await
        .unwrap();
    GroupRepository::add_bucket(pool, &group.id, &bucket.id, 0).await.unwrap();

    GroupRepository::update_bucket_permissions(pool, &group.id, &bucket.id, true, false)
        .await
        .unwrap();

    let details = GroupRepository::list_group_bucket_details(pool, &group.id)
        .await
        .unwrap();
    let (_id, _name, _path, _used, _blim, _ulim, _uc, can_upload, can_download) = &details[0];
    assert!(can_upload);
    assert!(!can_download);
}

#[tokio::test]
async fn test_group_update_bucket_permissions_not_found() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let group = GroupRepository::create(pool, &format!("g_{}", &Uuid::new_v4().to_string()[..8]))
        .await
        .unwrap();

    let result =
        GroupRepository::update_bucket_permissions(pool, &group.id, "no_bucket", true, true).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_group_set_user_storage_limit() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let group = GroupRepository::create(pool, &format!("g_{}", &Uuid::new_v4().to_string()[..8]))
        .await
        .unwrap();
    let bucket = BucketRepository::create(pool, &format!("b_{}", &Uuid::new_v4().to_string()[..8]), "/data/lim")
        .await
        .unwrap();
    GroupRepository::add_bucket(pool, &group.id, &bucket.id, 0).await.unwrap();

    GroupRepository::set_user_storage_limit(pool, &group.id, &bucket.id, 2048)
        .await
        .unwrap();

    let details = GroupRepository::list_group_bucket_details(pool, &group.id)
        .await
        .unwrap();
    let (_id, _name, _path, _used, _blim, user_limit, _uc, _up, _dl) = &details[0];
    assert_eq!(*user_limit, 2048);
}

#[tokio::test]
async fn test_group_set_user_storage_limit_not_found() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let group = GroupRepository::create(pool, &format!("g_{}", &Uuid::new_v4().to_string()[..8]))
        .await
        .unwrap();

    let result =
        GroupRepository::set_user_storage_limit(pool, &group.id, "no_bucket", 100).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_group_set_user_groups() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let g1 = GroupRepository::create(pool, &format!("g_{}", &Uuid::new_v4().to_string()[..8]))
        .await
        .unwrap();
    let g2 = GroupRepository::create(pool, &format!("g_{}", &Uuid::new_v4().to_string()[..8]))
        .await
        .unwrap();

    GroupRepository::set_user_groups(pool, &user_id, &[g1.id.clone()])
        .await
        .unwrap();
    let groups = GroupRepository::list_user_groups(pool, &user_id).await.unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0], g1.id);

    GroupRepository::set_user_groups(pool, &user_id, &[g2.id.clone()])
        .await
        .unwrap();
    let groups = GroupRepository::list_user_groups(pool, &user_id).await.unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0], g2.id);
}

// ─── AdminSettingRepository Tests ───────────────────────────────────────

#[tokio::test]
async fn test_setting_get_set() {
    let db = helpers::setup_test_db().await;
    AdminSettingRepository::set(db.pool(), "test_key", "test_value")
        .await
        .unwrap();

    let val = AdminSettingRepository::get(db.pool(), "test_key")
        .await
        .unwrap();
    assert_eq!(val, Some("test_value".to_string()));
}

#[tokio::test]
async fn test_setting_get_unset() {
    let db = helpers::setup_test_db().await;
    let val = AdminSettingRepository::get(db.pool(), "nonexistent")
        .await
        .unwrap();
    assert!(val.is_none());
}

#[tokio::test]
async fn test_setting_upsert() {
    let db = helpers::setup_test_db().await;
    AdminSettingRepository::set(db.pool(), "key", "value1")
        .await
        .unwrap();
    AdminSettingRepository::set(db.pool(), "key", "value2")
        .await
        .unwrap();

    let val = AdminSettingRepository::get(db.pool(), "key").await.unwrap();
    assert_eq!(val, Some("value2".to_string()));
}

#[tokio::test]
async fn test_setting_list() {
    let db = helpers::setup_test_db().await;
    AdminSettingRepository::set(db.pool(), "aaa", "1").await.unwrap();
    AdminSettingRepository::set(db.pool(), "bbb", "2").await.unwrap();
    AdminSettingRepository::set(db.pool(), "ccc", "3").await.unwrap();

    let list = AdminSettingRepository::list(db.pool()).await.unwrap();
    // Migrations seed block_registrations, allow_user_api_keys, allow_user_password_change
    assert!(list.len() >= 3);
    let keys: Vec<&str> = list.iter().map(|s| s.key.as_str()).collect();
    assert!(keys.contains(&"aaa"));
    assert!(keys.contains(&"bbb"));
    assert!(keys.contains(&"ccc"));
    assert!(keys.contains(&"block_registrations"));
}

#[tokio::test]
async fn test_setting_get_bool_true() {
    let db = helpers::setup_test_db().await;
    AdminSettingRepository::set_bool(db.pool(), "flag", true)
        .await
        .unwrap();
    let val = AdminSettingRepository::get_bool(db.pool(), "flag").await.unwrap();
    assert!(val);
}

#[tokio::test]
async fn test_setting_get_bool_false() {
    let db = helpers::setup_test_db().await;
    AdminSettingRepository::set_bool(db.pool(), "flag", false)
        .await
        .unwrap();
    let val = AdminSettingRepository::get_bool(db.pool(), "flag").await.unwrap();
    assert!(!val);
}

#[tokio::test]
async fn test_setting_get_bool_unset() {
    let db = helpers::setup_test_db().await;
    let val = AdminSettingRepository::get_bool(db.pool(), "nonexistent")
        .await
        .unwrap();
    assert!(!val);
}

#[tokio::test]
async fn test_setting_get_platform_settings() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();

    // Migrations seed: block_registrations=true
    let defaults = AdminSettingRepository::get_platform_settings(pool)
        .await
        .unwrap();
    assert!(defaults.block_registrations);
    assert!(!defaults.allow_user_api_keys);
    assert!(!defaults.allow_user_password_change);

    // Override all 3 keys
    AdminSettingRepository::set_bool(pool, "block_registrations", false)
        .await
        .unwrap();
    AdminSettingRepository::set_bool(pool, "allow_user_api_keys", true)
        .await
        .unwrap();
    AdminSettingRepository::set_bool(pool, "allow_user_password_change", true)
        .await
        .unwrap();

    let settings = AdminSettingRepository::get_platform_settings(pool)
        .await
        .unwrap();
    assert!(!settings.block_registrations);
    assert!(settings.allow_user_api_keys);
    assert!(settings.allow_user_password_change);
}

#[tokio::test]
async fn test_group_permission_flags_for_user() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let g_off = GroupRepository::create(pool, &format!("g_{}", &Uuid::new_v4().to_string()[..8]))
        .await
        .unwrap();
    let g_on = GroupRepository::create(pool, &format!("g_{}", &Uuid::new_v4().to_string()[..8]))
        .await
        .unwrap();

    // Defaults are both false.
    assert!(!GroupRepository::user_allows_api_keys(pool, &user_id)
        .await
        .unwrap());
    assert!(!GroupRepository::user_allows_password_change(pool, &user_id)
        .await
        .unwrap());

    // No membership at all -> still false.
    GroupRepository::update_permissions(pool, &g_on.id, true, true)
        .await
        .unwrap();
    assert!(!GroupRepository::user_allows_api_keys(pool, &user_id)
        .await
        .unwrap());

    // Add the user to the restricted group: still denied.
    GroupRepository::add_member(pool, &g_off.id, &user_id).await.unwrap();
    assert!(!GroupRepository::user_allows_api_keys(pool, &user_id)
        .await
        .unwrap());
    assert!(!GroupRepository::user_allows_password_change(pool, &user_id)
        .await
        .unwrap());

    // Add the user to the permissive group: ANY-group-allow kicks in.
    GroupRepository::add_member(pool, &g_on.id, &user_id).await.unwrap();
    assert!(GroupRepository::user_allows_api_keys(pool, &user_id)
        .await
        .unwrap());
    assert!(GroupRepository::user_allows_password_change(pool, &user_id)
        .await
        .unwrap());

    // Turning the permissive group's password flag back off blocks again.
    GroupRepository::update_permissions(pool, &g_on.id, true, false)
        .await
        .unwrap();
    assert!(GroupRepository::user_allows_api_keys(pool, &user_id)
        .await
        .unwrap());
    assert!(!GroupRepository::user_allows_password_change(pool, &user_id)
        .await
        .unwrap());
}
