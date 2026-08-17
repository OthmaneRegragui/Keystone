use std::sync::Arc;
use axum::Router;
use crate::AppState;
use axum::middleware::from_fn;
use crate::api::middleware::{bot_only, reject_bots};

pub fn api_routes() -> Router<Arc<AppState>> {
    // Bot-only API namespace. Bots authenticate with a bot API key and are
    // restricted to buckets plus file/folder operations (upload, download,
    // edit, create, move, copy, delete, list). The handlers here are the same
    // ones used by the user-facing API — they already enforce the bot's
    // capability flags and bucket/file/folder allow-lists — but this namespace
    // is gated so only bot API keys can reach it.
    let bot_routes = axum::Router::new()
        .route("/buckets", axum::routing::get(crate::api::controllers::files::list_user_buckets))
        .nest("/files", axum::Router::new()
            .route("/", axum::routing::post(crate::api::controllers::files::upload).get(crate::api::controllers::files::list_files))
            .route("/batch-move", axum::routing::post(crate::api::controllers::files::batch_move))
            .route("/batch-copy", axum::routing::post(crate::api::controllers::files::batch_copy))
            .route("/batch-delete", axum::routing::post(crate::api::controllers::files::batch_delete))
            .route("/:id", axum::routing::get(crate::api::controllers::files::get_file).delete(crate::api::controllers::files::delete_file))
            .route("/:id/download", axum::routing::get(crate::api::controllers::files::download_file))
            .route("/:id/raw", axum::routing::get(crate::api::controllers::files::raw_file))
            .route("/:id/verify", axum::routing::get(crate::api::controllers::files::verify_file))
            .route("/:id/rename", axum::routing::post(crate::api::controllers::files::rename_file))
            .route("/:id/move", axum::routing::post(crate::api::controllers::files::move_file))
            .route("/:id/copy", axum::routing::post(crate::api::controllers::files::copy_file)),
        )
        .nest("/folders", axum::Router::new()
            .route("/", axum::routing::get(crate::api::controllers::files::list_folder_contents).post(crate::api::controllers::files::create_folder))
            .route("/all", axum::routing::get(crate::api::controllers::files::list_all_folders))
            .route("/resolve", axum::routing::get(crate::api::controllers::files::resolve_folder_path))
            .route("/:id/rename", axum::routing::post(crate::api::controllers::files::rename_folder))
            .route("/:id/move", axum::routing::post(crate::api::controllers::files::move_folder))
            .route("/:id", axum::routing::delete(crate::api::controllers::files::delete_folder)),
        )
        .layer(from_fn(bot_only));

    Router::new()
        .nest(
            "/auth",
            axum::Router::new()
                .route("/register", axum::routing::post(crate::api::controllers::auth::register))
                .route("/login", axum::routing::post(crate::api::controllers::auth::login))
                .route("/refresh", axum::routing::post(crate::api::controllers::auth::refresh))
                .route("/logout", axum::routing::post(crate::api::controllers::auth::logout))
                .route("/forgot-password", axum::routing::post(crate::api::controllers::auth::forgot_password))
                .route("/reset-password", axum::routing::post(crate::api::controllers::auth::reset_password))
                .route("/change-password", axum::routing::post(crate::api::controllers::auth::change_password)),
        )
        .nest("/api/bot", bot_routes)
        .nest(
            "/api",
            axum::Router::new()
                .route("/public/settings", axum::routing::get(crate::api::controllers::health::public_settings))
                .route("/me/permissions", axum::routing::get(crate::api::controllers::auth::account_permissions))
                .route("/api-keys", axum::routing::get(crate::api::controllers::account::list_my_api_keys).post(crate::api::controllers::account::create_user_api_key))
                .route("/api-keys/:id", axum::routing::delete(crate::api::controllers::account::delete_user_api_key))
                .route("/dashboard/stats", axum::routing::get(crate::api::controllers::dashboard::stats))
                .route("/buckets", axum::routing::get(crate::api::controllers::files::list_user_buckets))
                .nest("/files", axum::Router::new()
                    .route("/", axum::routing::post(crate::api::controllers::files::upload).get(crate::api::controllers::files::list_files))
                    .route("/batch-move", axum::routing::post(crate::api::controllers::files::batch_move))
                    .route("/batch-copy", axum::routing::post(crate::api::controllers::files::batch_copy))
                    .route("/batch-delete", axum::routing::post(crate::api::controllers::files::batch_delete))
                    .route("/:id", axum::routing::get(crate::api::controllers::files::get_file).delete(crate::api::controllers::files::delete_file))
                    .route("/:id/download", axum::routing::get(crate::api::controllers::files::download_file))
                    .route("/:id/raw", axum::routing::get(crate::api::controllers::files::raw_file))
                    .route("/:id/verify", axum::routing::get(crate::api::controllers::files::verify_file))
                    .route("/:id/rename", axum::routing::post(crate::api::controllers::files::rename_file))
                    .route("/:id/move", axum::routing::post(crate::api::controllers::files::move_file))
                    .route("/:id/copy", axum::routing::post(crate::api::controllers::files::copy_file)),
                )
                .nest("/folders", axum::Router::new()
                    .route("/", axum::routing::get(crate::api::controllers::files::list_folder_contents).post(crate::api::controllers::files::create_folder))
                    .route("/all", axum::routing::get(crate::api::controllers::files::list_all_folders))
                    .route("/resolve", axum::routing::get(crate::api::controllers::files::resolve_folder_path))
                    .route("/:id/rename", axum::routing::post(crate::api::controllers::files::rename_folder))
                    .route("/:id/move", axum::routing::post(crate::api::controllers::files::move_folder))
                    .route("/:id", axum::routing::delete(crate::api::controllers::files::delete_folder)),
                )
                .nest("/health", axum::Router::new()
                    .route("/", axum::routing::get(crate::api::controllers::health::health))
                    .route("/ready", axum::routing::get(crate::api::controllers::health::ready)),
                )
                .nest("/admin", axum::Router::new()
                    .route("/stats", axum::routing::get(crate::api::controllers::admin::get_stats))
                    .route("/orphaned-files", axum::routing::get(crate::api::controllers::admin::list_orphaned_files).delete(crate::api::controllers::admin::delete_all_orphaned_files))
                    .route("/orphaned-files/:id", axum::routing::delete(crate::api::controllers::admin::delete_orphaned_file))
                    .route("/settings", axum::routing::get(crate::api::controllers::admin::get_settings))
                    .route("/settings", axum::routing::put(crate::api::controllers::admin::update_setting))
                    .route("/backends", axum::routing::get(crate::api::controllers::admin::list_storage_backends))
                    .route("/buckets", axum::routing::get(crate::api::controllers::admin::list_buckets))
                    .route("/buckets", axum::routing::post(crate::api::controllers::admin::create_bucket))
                    .route("/buckets/delete", axum::routing::post(crate::api::controllers::admin::delete_bucket))
                    .route("/buckets/visible", axum::routing::put(crate::api::controllers::admin::set_bucket_visible))
                    .route("/buckets/edit", axum::routing::put(crate::api::controllers::admin::update_bucket))
                    .route("/buckets/change-path", axum::routing::post(crate::api::controllers::admin::change_bucket_path))
                    .route("/paths", axum::routing::get(crate::api::controllers::admin::list_storage_paths))
                    .route("/paths", axum::routing::post(crate::api::controllers::admin::create_storage_path))
                    .route("/paths", axum::routing::delete(crate::api::controllers::admin::delete_storage_path))
                    .route("/storage-base", axum::routing::get(crate::api::controllers::admin::get_storage_base))
                    .route("/users", axum::routing::get(crate::api::controllers::admin::list_users))
                    .route("/users", axum::routing::post(crate::api::controllers::admin::create_user))
                    .route("/users/single", axum::routing::get(crate::api::controllers::admin::get_user))
                    .route("/users/update", axum::routing::put(crate::api::controllers::admin::update_user))
                    .route("/users/quota", axum::routing::put(crate::api::controllers::admin::update_user_quota))
                    .route("/groups", axum::routing::get(crate::api::controllers::admin::list_groups))
                    .route("/groups", axum::routing::post(crate::api::controllers::admin::create_group))
                    .route("/groups/detail", axum::routing::get(crate::api::controllers::admin::get_group_detail))
                    .route("/groups/delete", axum::routing::delete(crate::api::controllers::admin::delete_group))
                    .route("/groups/members", axum::routing::post(crate::api::controllers::admin::add_group_member))
                    .route("/groups/members/bulk", axum::routing::post(crate::api::controllers::admin::add_bulk_group_members))
                    .route("/groups/members/remove", axum::routing::delete(crate::api::controllers::admin::remove_group_member))
                    .route("/groups/buckets", axum::routing::post(crate::api::controllers::admin::add_group_bucket))
                    .route("/groups/buckets/remove", axum::routing::delete(crate::api::controllers::admin::remove_group_bucket))
                    .route("/groups/buckets/permissions", axum::routing::patch(crate::api::controllers::admin::update_group_bucket_permissions))
                    .route("/groups/buckets/user-limit", axum::routing::put(crate::api::controllers::admin::set_group_bucket_user_limit))
                    .route("/groups/permissions", axum::routing::put(crate::api::controllers::admin::update_group_permissions))
                    .route("/buckets/:name/export-index", axum::routing::get(crate::api::controllers::admin::export_bucket_index))
                    .route("/buckets/:name/export-zip", axum::routing::get(crate::api::controllers::admin::export_bucket_zip))
                    .route("/buckets/:name/import-zip", axum::routing::post(crate::api::controllers::admin::import_bucket_zip))
                    .route("/buckets/:name/import-file", axum::routing::post(crate::api::controllers::admin::import_bucket_file))
                    .route("/buckets/:name/import-index", axum::routing::post(crate::api::controllers::admin::import_bucket_index))
                    .route("/buckets/:name/import-combined", axum::routing::post(crate::api::controllers::admin::import_bucket_combined))
                    .route("/api-keys", axum::routing::get(crate::api::controllers::admin::list_all_api_keys))
                    .route("/api-keys", axum::routing::post(crate::api::controllers::admin::create_admin_api_key))
                    .route("/api-keys/revoke", axum::routing::delete(crate::api::controllers::admin::revoke_api_key))
                    .route("/bots", axum::routing::get(crate::api::controllers::admin::bots::list_bots))
                    .route("/bots", axum::routing::post(crate::api::controllers::admin::bots::create_bot))
                    .route("/bots/:id", axum::routing::put(crate::api::controllers::admin::bots::update_bot).delete(crate::api::controllers::admin::bots::delete_bot)),
                )
                .layer(from_fn(reject_bots)),
        )
}
