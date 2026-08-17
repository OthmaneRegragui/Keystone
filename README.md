# Keystone

<p align="center">
  <img src="src/static/logo.svg" alt="Keystone logo" width="120">
</p>

<p align="center">High-performance content-addressable file storage platform built with Rust.</p>

## Features

- **Content-addressable storage** with BLAKE3 hashing for automatic deduplication
- **Multi-bucket storage** with group-based access control (RBAC)
- **Virtual folder system** for organizing files within buckets
- **Drag-and-drop uploads** with real-time progress tracking
- **Secure authentication** with JWT + Argon2 password hashing + refresh token rotation
- **Scoped API keys** for programmatic access (users manage their own from Account → API Keys; admins can create for any user or bot)
- **Bot accounts** with scoped API keys for automated file operations (admins manage all; eligible users manage their own)
- **Admin panel** with user management, group permissions, bucket configuration, and platform settings
- **Modern file explorer** with list/grid views, breadcrumbs, search, and shareable `?dir=` deep links
- **Background workers** for garbage collection, integrity checks, reference counting, and stats
- **Strict OS-safe name validation** — one portable rule set for files, folders, and buckets
- **Rate limiting** and CORS configuration
- **Docker-ready** with multi-stage builds and a fully offline (vendored) UI

## Quick Start

```bash
cp .env.example .env

./run.sh
```

Server starts at `http://localhost:3000`. Navigate to `/setup` to create your first admin user.

## Where Are Files Saved?

Files are stored on disk under paths you configure. Each file is saved using its **BLAKE3 hash** as the filename, so duplicates are automatically detected and deduplicated.

```
Your configured path/
├── ab/
│   └── abc123def456...      <- file stored by hash
├── cd/
│   └── cd789ef0123...      <- another file
└── ...
```

**Configure one or more directories** — the first path is the default. Add more to spread files across multiple disks:

```bash
# Single path (default)
STORAGE_LOCAL_PATHS=./storage

# Multiple paths — files spread across disks
STORAGE_LOCAL_PATHS=/mnt/disk1/storage,/mnt/disk2/storage
```

## Environment

| Value | Database | Description |
|-------|----------|-------------|
| `test` | PostgreSQL (`keystone_test`) | Fast, isolated tests. Requires a running Postgres (see [Database](#database)). |
| `development` | PostgreSQL (`keystone`) | Local dev with persistent data, derived from `POSTGRES_*`. |
| `production` | PostgreSQL | The URL is derived from `POSTGRES_*` too, or set `KEYSTONE__DATABASE__URL` explicitly. |

## Configuration

All settings use the `KEYSTONE__` prefix with `__` as the separator (e.g., `KEYSTONE__SERVER__PORT`).

### Server

| Variable | Description | Default |
|----------|-------------|---------|
| `APP_ENV` | `test`, `development`, `production` | `development` |
| `KEYSTONE__SERVER__HOST` | Bind address | `127.0.0.1` |
| `KEYSTONE__SERVER__PORT` | HTTP port | `3000` |
| `KEYSTONE__SERVER__WORKERS` | Tokio worker threads | CPU count |

### Database

The connection URL is derived from `POSTGRES_*` (percent-encoded automatically, so symbol-laden passwords work):

| Variable | Description | Default |
|----------|-------------|---------|
| `POSTGRES_HOST` | Postgres host. In docker `postgres` (or unset) uses the bundled container; any other host connects to an existing database | bare-metal: `localhost`, docker: `postgres` |
| `POSTGRES_PORT` | "Outside" (host-published) port of the bundled Postgres. The container always listens on 5432 internally and the server connects there; only the host port follows this | `5433` in `.env.example` |
| `POSTGRES_USER` / `POSTGRES_PASSWORD` / `POSTGRES_DB` | Credentials and database name | `keystone` |
| `DATABASE_URL` / `KEYSTONE__DATABASE__URL` | Explicit connection URL — overrides the derivation | Per `APP_ENV` |
| `KEYSTONE__DATABASE__MAX_CONNECTIONS` | Max pool connections | `10` |
| `KEYSTONE__DATABASE__MIN_CONNECTIONS` | Min pool connections | `1` |

### Authentication

| Variable | Description | Default |
|----------|-------------|---------|
| `KEYSTONE__AUTH__JWT_SECRET` | HMAC signing secret | `change-me-in-production` |
| `KEYSTONE__AUTH__JWT_EXPIRATION_SECS` | Access token lifetime (seconds) | `43200` (12 hours) |

### Storage

| Variable | Description | Default |
|----------|-------------|---------|
| `KEYSTONE__STORAGE__BACKEND` | Storage backend type | `local` |
| `STORAGE_LOCAL_PATHS` | Comma-separated storage directories | `./storage` |
| `KEYSTONE__STORAGE__MAX_UPLOAD_SIZE_MB` | Max upload size in MB | `100` |

In the Docker deployment `STORAGE_LOCAL_PATHS` is taken from `.env` (compose default: `/mnt/keystone/data`). Inside the container, bucket files land at:

```
$STORAGE_LOCAL_PATHS/<storage-path-slug>/<bucket-name>/<blake3-shards>
# e.g. /mnt/keystone/data/test1/test1/fc/bd/<blake3-hash>
```

#### Where files live on your PC (`DATA_HOST_PATH`)

The container-side path above never changes. What changes is the *outside* half of the mount — where that folder actually lives on your machine. This is set with `DATA_HOST_PATH` in `.env`:

| `DATA_HOST_PATH` | Where files are stored on the host | Notes |
|---|---|---|
| *(empty)* | Docker named volume `storagedata` → `/var/lib/docker/volumes/keystone_storagedata/_data/` | Default. Zero setup; Docker manages the folder, it is invisible in your project. |
| `./data` | `<project>/data` — e.g. `/home/you/keystone/data` | A real folder you can see, back up and sync. Relative paths are anchored to the project root by `./docker-run.sh`. |
| `/mnt/disk1/keystone` | That exact host path | Any absolute path, e.g. a second disk or a NAS mount. |

Switching modes (for example from the default volume to `./data`) — copy existing files once, then restart:

```bash
mkdir -p data
docker run --rm -v keystone_storagedata:/from -v "$PWD/data":/to alpine \
  sh -c 'cp -a /from/. /to/ && chown -R 1000:1000 /to'
# set DATA_HOST_PATH=./data in .env, then:
./docker-run.sh
```

Ownership is fixed automatically at startup: the container's entrypoint runs as
root, `chown`s `STORAGE_LOCAL_PATHS` (the container-side path) to the non-root
`keystone` user, then drops privileges before starting the server. So the
`1000:1000` in the copy above is only needed if you want the host folder to be
writable by your own user too — the server itself works regardless of who owns
the mount. After verifying uploads work, you can free the old volume with
`docker volume rm keystone_storagedata` — but only after confirming, since until
then it holds the only copy of your files.

### Workers

| Variable | Description | Default |
|----------|-------------|---------|
| `KEYSTONE__WORKER__QUEUE_SIZE` | Worker queue capacity | `1000` |
| `KEYSTONE__WORKER__POLL_INTERVAL_MS` | Polling interval | `500` |
| `KEYSTONE__WORKER__BATCH_SIZE` | Items per batch | `10` |

### Rate Limiting

| Variable | Description | Default |
|----------|-------------|---------|
| `KEYSTONE__RATE_LIMIT__ENABLED` | Enable rate limiting | `true` |
| `KEYSTONE__RATE_LIMIT__REQUESTS_PER_SECOND` | Requests per second | `50` |
| `KEYSTONE__RATE_LIMIT__BURST_SIZE` | Burst capacity | `100` |

### CORS

| Variable | Description | Default |
|----------|-------------|---------|
| `KEYSTONE__CORS__ALLOWED_ORIGINS` | Allowed origins | `http://localhost:3000` |
| `KEYSTONE__CORS__ALLOWED_METHODS` | Allowed methods | `GET,POST,PUT,DELETE,PATCH,OPTIONS` |
| `KEYSTONE__CORS__ALLOWED_HEADERS` | Allowed headers | `authorization,content-type,x-request-id` |

## Technology Stack

| Component       | Technology                          |
|-----------------|-------------------------------------|
| Language        | Rust (edition 2021, MSRV 1.75)     |
| Backend         | Axum 0.7 + Tokio                    |
| Frontend        | Alpine.js + Tailwind CSS            |
| Database        | PostgreSQL (via SQLx)                  |
| Authentication  | JWT (HS256) + Argon2 + Refresh tokens |
| Content Hashing | BLAKE3                              |
| ORM             | SQLx (compile-time checked SQL)     |

## Project Structure

```
keystone/
├── src/
│   ├── main.rs              # Entry point, server setup, route definitions
│   ├── lib.rs               # AppState, module re-exports
│   ├── config.rs            # Configuration structs (KEYSTONE__* env vars)
│   ├── error.rs             # AppError, RFC 7807 Problem Details
│   ├── models/              # Domain models
│   │   ├── user.rs          # User, UserRole
│   │   ├── file.rs          # Physical file (content-addressed)
│   │   ├── user_file.rs     # Per-user file ownership (dedup bridge)
│   │   ├── folder.rs        # Virtual folders
│   │   ├── api_key.rs       # API keys with scopes
│   │   ├── admin.rs         # Bucket, AdminSetting, PlatformSettings
│   │   ├── audit.rs         # Audit log entries
│   │   ├── group.rs         # UserGroup
│   │   └── storage_object.rs # Physical storage references
│   ├── db/
│   │   ├── pool.rs          # PostgreSQL connection pool + migrations
│   │   ├── repos/           # Repository layer (data access)
│   │   │   ├── users.rs, files.rs, user_files.rs, folders.rs
│   │   │   ├── api_keys.rs, settings.rs, buckets.rs
│   │   │   ├── groups.rs, audit.rs, storage_objects.rs
│   │   └── rows/            # Database row structs
│   ├── api/
│   │   ├── routes.rs        # All route definitions
│   │   ├── extractors.rs    # AuthUser JWT/API-key/bot extractor
│   │   ├── validators.rs    # Input validation (scopes, DTOs)
│   │   ├── middleware/      # Rate limiting, request logging, request ID, bot gates
│   │   ├── dto/             # Data transfer objects
│   │   └── controllers/     # Request handlers
│   │       ├── auth.rs      # Login, register, refresh, change-password
│   │       ├── files.rs     # Upload, download, list, rename, move, copy, delete
│   │       ├── health.rs    # Health checks, public settings
│   │       └── admin/       # Admin controllers
│   │           ├── stats.rs, settings.rs, buckets.rs
│   │           ├── users.rs, groups.rs, api_keys.rs, bots.rs
│   ├── storage/
│   │   ├── backend.rs       # StorageBackend trait
│   │   ├── local.rs         # LocalFsBackend implementation
│   │   ├── registry.rs      # StorageRegistry (multi-backend)
│   │   └── health.rs        # Storage health probes
│   ├── utils/
│   │   ├── auth/            # JWT, password hashing, API keys, sessions
│   │   ├── format.rs        # File size formatting
│   │   ├── hashing/         # BLAKE3 content hashing
│   │   ├── names.rs         # OS-safe name validation (files, folders, buckets)
│   │   ├── traits/          # Storage trait
│   │   └── workers/         # Background workers (GC, integrity, refcount, cleanup, stats)
│   └── static/              # HTML pages (compiled in via include_str!)
│       ├── admin.html       # Admin panel
│       ├── docs.html        # Admin-only documentation page (/docs)
│       ├── bots.html        # Bot management page (/bots) — admins all, eligible users own
│       ├── orphans.html     # Admin-only orphaned-files page (/orphans)
│       ├── files.html       # File explorer with drag-and-drop
│       ├── account.html     # User account settings
│       ├── login.html       # Login page
│       ├── register.html    # Registration page
│       ├── logo.svg         # Brand logo (favicon + in-app)
│       └── vendor/          # Self-hosted Alpine.js + Tailwind (offline)
├── migrations/              # SQL migrations (0001-0015)
├── tests/                   # 290 integration tests
└── docker/                  # Docker configuration
```

## Architecture

### Content-Addressed Deduplication

Files are stored by their BLAKE3 hash, not their original name. When two users upload the same file content:

1. A single physical blob is stored (keyed by BLAKE3 hash)
2. A `ref_count` on the `files` record tracks how many users reference it
3. Each user gets their own `user_files` entry with their personal filename
4. Physical blobs are only cleaned up when `ref_count` reaches 0

### Group-Based Access Control (RBAC)

```
Users ──< group_members >── Groups ──< group_buckets >── Buckets
```

- Users belong to groups
- Groups are assigned buckets with per-bucket permissions:
  - `can_upload`: Whether members can upload files to this bucket
  - `can_download`: Whether members can download files from this bucket
  - `user_storage_limit`: Per-user storage quota within this bucket (0 = unlimited)
- Bucket permissions are merged across groups (OR for upload/download, MAX for limits)
- Buckets with `visible_to_users=true` give full access to all users automatically

Groups also carry account-level capability flags, toggled from the admin **Groups** page:

| Flag | Effect |
|------|--------|
| `allow_api_keys` | Whether members may create and manage their own API keys (from the Account → API Keys tab) |
| `allow_bots` | Whether members may create and manage their own bot accounts (from the Bots page) |
| `allow_password_change` | Whether members may change their own password |

These are evaluated with ANY-group-allow semantics: a user may use the capability if **any** of their groups permits it (a single restrictive group cannot block a member who also belongs to an allowed group). Users that belong to **no** group fall back to the global settings below. Admins are always allowed.

### Virtual Folders

Folders are purely database-level constructs. Files are still stored by BLAKE3 hash on disk. The folder hierarchy (`user_folders`) organizes files logically within a bucket using an adjacency list pattern (self-referential `parent_id`).

### Platform Settings

Runtime-configurable settings stored in the `admin_settings` table:

| Key | Default | Description |
|-----|---------|-------------|
| `block_registrations` | `true` | Block new user registrations |
| `allow_user_api_keys` | `false` | Whether users in **no** group may create and manage their own API keys (fallback when a user belongs to no group) |
| `allow_user_bots` | `false` | Whether users in **no** group may create and manage their own bot accounts (fallback when a user belongs to no group) |
| `allow_user_password_change` | `false` | Allow non-admin users to change their password |

## API Reference

**Base URL:** `http://localhost:3000` — **Content-Type:** `application/json` (unless noted)
**Authentication:** `Authorization: Bearer <token>` header (JWT access token).

All endpoints except `/auth/register`, `/auth/login`, `/auth/refresh`, `/api/health/*`, and `/api/public/settings` require a bearer token. Admin endpoints require the `admin` role. The same reference is available in-app at `/docs` (admin only).

### Authentication

#### POST `/auth/register` — Register User
Public endpoint. First user automatically becomes admin.

```json
{ "username": "string (3-50 chars)", "email": "string (valid email)", "password": "string (8-128 chars)" }
```
**Response (201):** `AuthResponse` (below). **Errors:** `400` validation, `409` duplicate username/email, `403` registrations blocked.

```json
{
  "access_token": "eyJ...",
  "refresh_token": "a1b2c3...",
  "token_type": "Bearer",
  "expires_in": 43200,
  "user": { "id": "uuid", "username": "alice", "email": "alice@example.com", "role": "admin", "created_at": "2026-01-01T00:00:00Z" }
}
```

#### POST `/auth/login` — Login
```json
{ "email": "string", "password": "string" }
```
**Response (200):** Same `AuthResponse`. **Errors:** `401` invalid credentials.

#### POST `/auth/refresh` — Refresh Token
Rotates the refresh token — the old one is invalidated.
```json
{ "refresh_token": "string" }
```
**Response (200):** New `AuthResponse`. **Errors:** `401` invalid/expired/revoked.

#### POST `/auth/logout` — Logout
```json
{ "refresh_token": "string" }
```
**Response (200):** `{ "message": "logged out successfully" }`

#### POST `/auth/forgot-password` — Forgot Password
Always returns the same response regardless of email existence (prevents user enumeration).
```json
{ "email": "string" }
```
**Response (200):** `{ "message": "if the email exists, a reset link has been sent" }`
> Note: email sending is not yet implemented.

#### POST `/auth/change-password` — Change Password
Requires JWT. Admins always allowed; non-admins require the `allow_user_password_change` setting.
```json
{ "current_password": "string", "new_password": "string (8+ chars)" }
```
**Response (200):** `{ "message": "password updated successfully" }`

### API Keys (per-user, Account → API Keys)

Users can manage their own scoped API keys when their group has `allow_api_keys` (or the `allow_user_api_keys` setting for users in no group). Admins are always allowed. Creating/revoking keys requires a browser UI session (JWT); listing one's own keys works with a normal API key too. Bot keys are rejected on all of these.

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/api-keys` | List the caller's own API keys |
| POST | `/api/api-keys` | Create a key: `{ "name", "scopes": ["files:read", ...], "expires_in_days" }` — returns `full_key` (shown once) |
| DELETE | `/api/api-keys/:id` | Permanently delete one of the caller's own keys |

### Files

#### POST `/api/files` — Upload File
Multipart form upload with content-addressed deduplication.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `file` | binary | Yes | The file to upload |
| `bucket` | text | No | Target bucket (defaults to user's first accessible bucket) |
| `folder_id` | text | No | Target folder UUID (defaults to root) |

**Response (201):**
```json
{
  "file": { "id": "uuid", "user_file_id": "uuid", "name": "document.pdf", "hash": "abc123...", "size": 1024, "mime_type": "application/pdf", "created_at": "2026-01-01T00:00:00Z", "ref_count": 1, "bucket_name": "default", "folder_id": null },
  "duplicate": false
}
```
When `duplicate: true`, no new blob was written — only a new `user_files` reference. **Errors:** `400` empty file, `413` too large, `403` no upload permission.

#### GET `/api/files` — List Files
Query params: `page` (default `1`), `per_page` (default `20`, max `100`), `search` (filename), `bucket` (name), `folder_id` (UUID).

**Response (200):** `{ "files": [FileDto], "total": 42, "page": 1, "per_page": 20 }`

#### GET `/api/files/:id` — Get File Metadata
**Response (200):** single `FileDto`. **Errors:** `404`.

#### GET `/api/files/:id/download` — Download File
**Response (200):** binary stream. Headers: `content-type` (MIME, fallback `application/octet-stream`), `content-disposition: attachment; filename="<sanitized>"`, `content-length`. **Errors:** `404`, `403` no download permission.

#### GET `/api/files/:id/raw` — Raw File Content (inline)
Same bytes as download but served with `content-disposition: inline`, so the browser renders images, videos, PDFs, etc. instead of saving them. Same auth and permission checks as download.
**Response (200):** binary stream. Headers: `content-type` (MIME, fallback `application/octet-stream`), `content-disposition: inline; filename="<sanitized>"`, `content-length`. **Errors:** `404`, `401`, `403` no download permission.

#### GET `/api/files/:id/verify` — Verify File Integrity
Recomputes the BLAKE3 hash and compares to the stored hash.
**Response (200):** `{ "file_id": "uuid", "user_file_id": "uuid", "expected_hash": "abc123...", "computed_hash": "abc123...", "valid": true }`

#### POST `/api/files/:id/rename` — Rename File
```json
{ "name": "new-name.pdf" }
```
**Response (200):** `{ "message": "file renamed to 'new-name.pdf'" }`

#### POST `/api/files/:id/move` — Move File
```json
{ "folder_id": "uuid | null" }
```
`null` moves to the bucket root. **Response (200):** `{ "message": "file moved" }`

#### DELETE `/api/files/:id` — Delete File
Removes the user's reference; the physical blob is only deleted when `ref_count` reaches 0. **Response (200):** `{ "message": "file 'document.pdf' deleted" }`

### Folders

#### GET `/api/folders` — List Folder Contents
Query params: `bucket` (required), `folder_id` (omit for root).
**Response (200):** `{ "folders": [FolderDto], "files": [FileDto], "path": [ { "id": null, "name": "Root" }, { "id": "uuid", "name": "Documents" } ] }` — `path` provides breadcrumbs.

#### GET `/api/folders/resolve?bucket_id=&path=` — Resolve a Path
Resolves a slash path (e.g. `/test/hello`) to a folder id for deep linking. `/` returns the root.

#### POST `/api/folders` — Create Folder
```json
{ "name": "Documents", "bucket_name": "default", "parent_id": "uuid | null" }
```
**Response (201):** `FolderDto`. **Errors:** `409` duplicate in same parent, `403` no upload permission.

#### POST `/api/folders/:id/rename` — Rename Folder
```json
{ "name": "New Name" }
```

#### DELETE `/api/folders/:id` — Delete Folder
Children (subfolders and files) are moved to the parent (or bucket root). **Response (200):** `{ "message": "folder 'Documents' deleted" }`

### Buckets

#### GET `/api/buckets` — List User's Accessible Buckets
**Response (200):**
```json
[ { "name": "default", "is_default": true, "can_upload": true, "can_download": true, "user_storage_limit": 0 } ]
```
`user_storage_limit: 0` means unlimited.

### Health & Public

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/health` | `{ "status": "ok", "service": "keystone-api" }` (no auth) |
| GET | `/api/health/ready` | Verifies DB connectivity: `{ "message": "ready" }` (no auth) |
| GET | `/api/public/settings` | `{ "block_registrations": true }` (no auth) |

### Bot API (only bot API keys)

Bots are automations that act on behalf of their owner. A bot is limited to **buckets and file/folder operations** — upload, download, edit (rename), create, move, copy, delete, list. It can **never** access accounts, API keys, dashboard stats, health, settings, or admin endpoints.

Bots use their own endpoint namespace **`/api/bot/*`**, which accepts **only bot API keys**. Bot keys are rejected on the regular `/api/*` endpoints, and ordinary users/API keys are rejected on `/api/bot/*` — the two surfaces are fully separated.

Every bot is scoped by its capability flags (`can_upload`, `can_download`, `can_copy`, `can_edit`, `can_delete`, `can_list`) and its **path rules** — a list of `(bucket, path, allow|block)` rows; operations outside those limits fail with `403`. A bucket with no rule is fully accessible; an empty path means the whole bucket; `block` always wins over `allow`; a bucket that has rules is fail-closed (a path is allowed only when an `allow` rule covers it and no `block` rule does).

```http
Authorization: Bearer ks_...
```

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/bot/buckets` | Buckets the bot can access (honors its path rules) |
| POST | `/api/bot/files` | Upload file (multipart) |
| GET | `/api/bot/files?bucket=&folder_id=&page=` | List files (paginated) |
| POST | `/api/bot/files/batch-move` | Move multiple files at once |
| POST | `/api/bot/files/batch-copy` | Copy multiple files at once |
| POST | `/api/bot/files/batch-delete` | Delete multiple files at once |
| GET | `/api/bot/files/:id` | File metadata |
| GET | `/api/bot/files/:id/download` | Download bytes (attachment) |
| GET | `/api/bot/files/:id/raw` | Raw bytes inline (images/videos/PDFs render in the browser) |
| GET | `/api/bot/files/:id/verify` | Verify stored bytes match the recorded hash |
| POST | `/api/bot/files/:id/rename` | Rename file (edit) |
| POST | `/api/bot/files/:id/move` | Move file between folders |
| POST | `/api/bot/files/:id/copy` | Copy file (content-addressed dedup) |
| DELETE | `/api/bot/files/:id` | Delete file |
| GET | `/api/bot/folders?bucket=&parent_id=` | List folder contents (files + folders) |
| GET | `/api/bot/folders/all` | List all folders in a bucket (for building a tree) |
| GET | `/api/bot/folders/resolve?bucket_id=&path=` | Resolve a path (e.g. `/test/hello`) to a folder id |
| POST | `/api/bot/folders` | Create folder |
| POST | `/api/bot/folders/:id/rename` | Rename folder |
| POST | `/api/bot/folders/:id/move` | Move folder to a new parent |
| DELETE | `/api/bot/folders/:id` | Delete folder |

Requests and responses are identical to the corresponding `/api/*` endpoints above. Bot capabilities and path rules are enforced on every call — for example `can_upload` gates `POST /api/bot/files` and `POST /api/bot/folders`, and the bucket's path rules restrict which paths a bot may upload into, list, or read.

### Admin (all require `admin` role)

#### Stats & Settings
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/admin/stats` | `{ total_users, total_files, total_buckets, total_groups, block_registrations, default_bucket }` |
| GET | `/api/admin/settings` | `{ block_registrations, allow_user_api_keys, allow_user_bots, allow_user_password_change }` |
| PUT | `/api/admin/settings` | Update setting: `{ "key": "allow_user_api_keys", "value": "true" }` — keys: `block_registrations`, `allow_user_api_keys`, `allow_user_bots`, `allow_user_password_change` (`"true"`/`"false"`), `default_bucket` (bucket name) |

#### Buckets
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/admin/buckets` | List all buckets (with path, visibility, storage used/limit) |
| POST | `/api/admin/buckets` | Create: `{ "name": "archive", "path": "/mnt/archive/storage" }` (name validated) |
| POST | `/api/admin/buckets/delete` | `{ "name": "archive" }` |
| PUT | `/api/admin/buckets/visible` | `{ "name": "archive", "visible": true }` — when `visible_to_users`, bucket is accessible to all users |
| PUT | `/api/admin/buckets/edit` | `{ "original_name", "name", "path", "visible_to_users", "is_active", "storage_limit" }` (name validated) |

#### Users
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/admin/users` | List up to 200 users with group memberships |
| GET | `/api/admin/users/single?id=<uuid>` | Single user |
| POST | `/api/admin/users` | Create: `{ "username", "email", "password", "role", "group_ids" }` |
| PUT | `/api/admin/users/update` | Update: `{ "id", "email"?, "role"?, "password"?, "group_ids"? }` — `group_ids` replaces all memberships |
| PUT | `/api/admin/users/quota` | `{ "user_id", "storage_quota" }` (bytes; `0` = unlimited) |

#### Groups
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/admin/groups` | List groups with member/bucket counts |
| GET | `/api/admin/groups/detail?id=<string>` | Members + linked buckets with permissions |
| POST | `/api/admin/groups` | Create: `{ "name", "buckets"? }` |
| DELETE | `/api/admin/groups/delete` | `{ "id" }` |
| POST | `/api/admin/groups/members` | Add member: `{ "group_id", "user_id" }` |
| DELETE | `/api/admin/groups/members/remove` | Remove member: `{ "group_id", "user_id" }` |
| POST | `/api/admin/groups/buckets` | Link bucket: `{ "group_id", "bucket_name", "user_storage_limit" }` (`0` = unlimited per-user) |
| DELETE | `/api/admin/groups/buckets/remove` | Unlink bucket: `{ "group_id", "bucket_name" }` |
| PATCH | `/api/admin/groups/buckets/permissions` | `{ "group_id", "bucket_name", "can_upload", "can_download" }` |
| PUT | `/api/admin/groups/buckets/user-limit` | `{ "group_id", "bucket_name", "user_storage_limit" }` |

#### Admin API Keys
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/admin/api-keys` | List all user and bot keys (`user_id`/`username` are `null` for bots) |
| POST | `/api/admin/api-keys` | Create for any user or a bot: `{ "user_id": "uuid \| null", "name", "scopes", "expires_in_days" }` — bots get a `bot_` prefix |
| DELETE | `/api/admin/api-keys/revoke` | Revoke any key: `{ "id" }` |

#### Admin Bots
Bots are managed through the UI at `/bots`. Admins see and manage **all** bots; a non-admin user may create and manage **their own** bots when their group has `allow_bots`, or (for users in no group) the `allow_user_bots` setting is enabled. These endpoints require a browser UI session (JWT) — they cannot be used with an API key. Once a bot exists, it talks to the platform exclusively through the dedicated [`/api/bot/*`](#bot-api-only-bot-api-keys) endpoints; it can never use these admin endpoints.

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/admin/bots` | List all bot accounts (admin) or the caller's own bots (eligible user) |
| POST | `/api/admin/bots` | Create a bot: `{ "user_id", "name", "can_upload", "can_download", "can_copy", "can_edit", "can_delete", "can_list", "allowed_buckets", "allowed_folder_ids", "allowed_file_ids", "upload_limit_bytes", "expires_in_days" }` — creates a scoped API key for the bot. Admins may pick any owner; an eligible user always gets a bot for themself |
| PUT | `/api/admin/bots/:id` | Update bot permissions (all fields optional) — admin any bot, eligible user own bots only |
| DELETE | `/api/admin/bots/:id` | Delete a bot and revoke its key — admin any bot, eligible user own bots only |

### Errors

All errors follow [RFC 7807 Problem Details](https://datatracker.ietf.org/doc/html/rfc7807):
```json
{ "type": "/errors/NOT_FOUND", "title": "Resource Not Found", "status": 404, "detail": "resource not found: file not found" }
```

| Code | HTTP | Description |
|------|------|-------------|
| `NOT_FOUND` | 404 | Resource does not exist |
| `UNAUTHORIZED` | 401 | Authentication required or invalid |
| `FORBIDDEN` | 403 | Insufficient permissions |
| `BAD_REQUEST` | 400 | Invalid request body or parameters |
| `CONFLICT` | 409 | Resource already exists (duplicate) |
| `INTERNAL_ERROR` | 500 | Unexpected server error |
| `STORAGE_ERROR` | 500 | Storage backend failure |
| `VALIDATION_FAILED` | 422 | Input validation failed |

Every response carries `X-Request-ID` (UUID) for tracing and the CORS origin header.

### Name Validation Rules

Files, folders, and bucket names are validated server-side (`src/utils/names.rs`) so every name is safe on all major OS filesystems. A name is rejected if it:

- is empty, or equals `.` or `..`
- contains control characters (newline, tab, etc.)
- contains any of `\ / : * ? " < > |`
- starts with a space, or ends with a space or a dot
- exceeds 255 bytes
- is a Windows reserved device name, case-insensitive (with or without extension): `CON PRN AUX NUL COM1-9 LPT1-9`

The same rules are mirrored in the browser UI, so invalid names are caught before upload. Admin imports validate per-item and skip invalid entries rather than failing wholesale.

### URL Deep Links

The Files UI keeps the current location in the URL, so pages can be bookmarked and shared:

```
/files?dir=/                Root of your current bucket
/files?dir=/test            Folder "test" at the root
/files?dir=/test/hello      Nested folder "hello"
```

`dir=/` means the bucket root; the effective bucket comes from the in-app selector (persisted per browser, or forced with the legacy `?bucket_id=&path=` params). Breadcrumb clicks, folder opens, and bucket switches rewrite the URL via `history.replaceState` — no reload. Refreshing or pasting a deep link re-resolves the path server-side; if the folder no longer exists, the UI falls back to the root.

## Web Pages

| Route | Page | Description |
|-------|------|-------------|
| `/`, `/dashboard` | Dashboard | Overview and quick actions |
| `/files` | File Explorer | Browse, upload, organize files with folders |
| `/account` | Account | Profile, security settings, and per-user API key management |
| `/admin` | Admin Panel | Users, groups, buckets, settings (admin only) |
| `/bots` | Bots | Bot management — admins see all, eligible users manage their own (scoped API-key accounts) |
| `/orphans` | Orphaned Files | Admin-only orphaned file reclamation |
| `/docs` | Documentation | Admin-only ops + API reference (mirrors this README) |
| `/login` | Login | User authentication |
| `/register` | Register | New user registration |
| `/setup` | Setup | First-time admin user creation |

## Development

```bash
# Run tests (requires a running PostgreSQL).
# Each test binary uses its own database (created on demand, named after the
# binary, e.g. keystone_test_api_auth_tests). Tests within a binary are
# serialized because some truncate the shared tables to reset state.
RUST_TEST_THREADS=1 cargo test

# Run with verbose output
RUST_TEST_THREADS=1 cargo test -- --nocapture

# Lint
cargo clippy -- -D warnings

# Format
cargo fmt --all
```

## Docker

```bash
# Edit .env with your settings, then:
docker compose --env-file .env -f docker/docker-compose.yml up --build
# Or use the helper script (same thing, plus convenience commands):
./docker-run.sh
```

### Docker Setup

- Multi-stage build (builder: `rust:1.75-slim`, runtime: `debian:bookworm-slim`)
- Runs as non-root `keystone` user
- Exposes port 3000
- Containers: `keystone-server` (app) and, when using the bundled database, `keystone-postgres` — prefixed names avoid collisions with existing containers
- Volumes: `pgdata` (bundled PostgreSQL data). File storage is either the `storagedata` volume or a host folder you choose with `DATA_HOST_PATH` in `.env` (see [Storage](#storage))
- All database settings come from `POSTGRES_*` in `.env`. `POSTGRES_HOST` decides the target: unset or `postgres` uses the bundled container (enabled via the `db` compose profile); any other host connects to an existing Postgres and the bundled one is not started. The server percent-encodes every URL component, so symbol-laden passwords (`@ : / %`) just work
- The bundled PostgreSQL runs on `postgres:16-alpine`. It always listens on 5432 inside the compose network; `POSTGRES_PORT` in `.env` (default 5433 in `.env.example`) is only the host-published port it is mapped to

### Reset the Database

To delete ALL data in the PostgreSQL database and start from scratch (all 14
migrations are re-run on the next start, so the schema is rebuilt exactly as on a
fresh install):

```bash
./docker-run.sh db-reset
```

This stops the server, drops every table in the database (including the migration
history), and restarts the server so the schema is recreated from the first
migration. Uploaded files on disk (the `storagedata` volume, or your
`DATA_HOST_PATH` folder if you set one) are kept — use
`./docker-run.sh reset` if you also want to destroy the storage and database
volumes.

## Database

PostgreSQL, automatic migrations on startup. Schema managed via 18 numbered migrations:

- **Development** (default): derived from `POSTGRES_*` (or `postgres://keystone:keystone@localhost:5432/keystone`) — also set by `run.sh`
- **Tests**: need a running Postgres. Each test binary creates its own database on demand, named after the binary (`keystone_test_<binary>`). Override the server with `TEST_DATABASE_BASE_URL` (default: `postgres://keystone:keystone@localhost:5432/postgres`), or set `TEST_DATABASE_URL` to use one pre-provisioned database verbatim.
- **Production**: requires `KEYSTONE__DATABASE__URL` to be set, e.g. `postgres://<user>:<pass>@host:5432/<db>`
- **Docker**: `docker-compose` runs the full stack — `keystone-server` (app) + `keystone-postgres` — and points the server at the Postgres container automatically

1. Core tables (users, files, api_keys, storage_objects, audit_logs)
2. Admin settings and buckets
3. Groups and bucket access (RBAC)
4. Bot API keys (nullable user_id)
5. Bucket storage limits
6. Per-user file ownership (dedup bridge)
7. Group bucket permissions (upload/download)
8. Group bucket user storage limits
9. User file bucket name
10. Virtual folders
11. Soft-deleted user files
12. Storage paths
13. Group bucket id
14. Integer columns → bigint
15. Storage root mount
16. Group capability flags (`allow_api_keys`, `allow_bots`, `allow_password_change`)
17. Bots table
18. Bot capabilities (copy and edit permissions)

## License

Apache License 2.0 — see [LICENSE](LICENSE) for details.
