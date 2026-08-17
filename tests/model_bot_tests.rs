use chrono::Utc;
use keystone::models::bot::{Bot, BotPathRule, BotRuleStatus};
use uuid::Uuid;

fn make_bot(path_rules: Option<Vec<BotPathRule>>) -> Bot {
    Bot {
        id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        key_id: Uuid::new_v4(),
        name: "test-bot".to_string(),
        can_upload: false,
        can_download: false,
        can_copy: false,
        can_edit: false,
        can_delete: false,
        can_list: false,
        path_rules,
        upload_limit_bytes: 0,
        uploaded_bytes: 0,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

// ============================================================
// path_allowed
// ============================================================

#[test]
fn test_no_rules_allows_all() {
    let bot = make_bot(None);
    assert!(bot.path_allowed("data", ""));
    assert!(bot.path_allowed("data", "/anything"));
}

#[test]
fn test_allow_root_bucket() {
    let bot = make_bot(Some(vec![BotPathRule {
        bucket: "data".into(),
        path: "".into(),
        status: BotRuleStatus::Allow,
    }]));
    assert!(bot.path_allowed("data", ""));
    assert!(bot.path_allowed("data", "/anything"));
}

#[test]
fn test_allow_subpath() {
    let bot = make_bot(Some(vec![BotPathRule {
        bucket: "data".into(),
        path: "/docs".into(),
        status: BotRuleStatus::Allow,
    }]));
    assert!(bot.path_allowed("data", "/docs"));
    assert!(bot.path_allowed("data", "/docs/report.pdf"));
    assert!(!bot.path_allowed("data", "/other"));
    assert!(!bot.path_allowed("data", "/documents"));
}

#[test]
fn test_block_overrides_allow() {
    let bot = make_bot(Some(vec![
        BotPathRule {
            bucket: "data".into(),
            path: "".into(),
            status: BotRuleStatus::Allow,
        },
        BotPathRule {
            bucket: "data".into(),
            path: "/secret".into(),
            status: BotRuleStatus::Block,
        },
    ]));
    assert!(!bot.path_allowed("data", "/secret"));
    assert!(bot.path_allowed("data", "/public"));
}

#[test]
fn test_block_all() {
    let bot = make_bot(Some(vec![BotPathRule {
        bucket: "data".into(),
        path: "".into(),
        status: BotRuleStatus::Block,
    }]));
    assert!(!bot.path_allowed("data", ""));
    assert!(!bot.path_allowed("data", "/anything"));
}

#[test]
fn test_empty_path_covers_bucket_root() {
    let bot = make_bot(Some(vec![BotPathRule {
        bucket: "data".into(),
        path: "".into(),
        status: BotRuleStatus::Allow,
    }]));
    assert!(bot.path_allowed("data", ""));
}

#[test]
fn test_trailing_slash_normalized() {
    let bot = make_bot(Some(vec![BotPathRule {
        bucket: "data".into(),
        path: "/docs".into(),
        status: BotRuleStatus::Allow,
    }]));
    assert!(bot.path_allowed("data", "/docs/"));
}

#[test]
fn test_different_bucket_not_affected() {
    let bot = make_bot(Some(vec![BotPathRule {
        bucket: "data".into(),
        path: "".into(),
        status: BotRuleStatus::Allow,
    }]));
    assert!(bot.path_allowed("data", ""));
    assert!(bot.path_allowed("other", ""));
}

// ============================================================
// bucket_allowed
// ============================================================

#[test]
fn test_no_rules_bucket_allowed() {
    let bot = make_bot(None);
    assert!(bot.bucket_allowed("data"));
}

#[test]
fn test_allow_rule_bucket_allowed() {
    let bot = make_bot(Some(vec![BotPathRule {
        bucket: "data".into(),
        path: "/docs".into(),
        status: BotRuleStatus::Allow,
    }]));
    assert!(bot.bucket_allowed("data"));
}

#[test]
fn test_only_block_rules_bucket_not_allowed() {
    let bot = make_bot(Some(vec![BotPathRule {
        bucket: "data".into(),
        path: "".into(),
        status: BotRuleStatus::Block,
    }]));
    assert!(!bot.bucket_allowed("data"));
}

// ============================================================
// scopes
// ============================================================

#[test]
fn test_scopes_from_capabilities() {
    let mut bot = make_bot(None);
    bot.can_upload = true;
    bot.can_download = true;
    bot.can_delete = true;
    bot.can_list = true;
    let scopes = bot.scopes();
    assert!(scopes.contains(&"files:read".to_string()));
    assert!(scopes.contains(&"files:write".to_string()));
    assert!(scopes.contains(&"files:delete".to_string()));
}

#[test]
fn test_scopes_read_only() {
    let mut bot = make_bot(None);
    bot.can_list = true;
    bot.can_download = true;
    let scopes = bot.scopes();
    assert!(scopes.contains(&"files:read".to_string()));
    assert!(!scopes.contains(&"files:write".to_string()));
    assert!(!scopes.contains(&"files:delete".to_string()));
}

#[test]
fn test_scopes_no_delete() {
    let mut bot = make_bot(None);
    bot.can_upload = true;
    bot.can_list = true;
    bot.can_delete = false;
    let scopes = bot.scopes();
    assert!(scopes.contains(&"files:read".to_string()));
    assert!(scopes.contains(&"files:write".to_string()));
    assert!(!scopes.contains(&"files:delete".to_string()));
}

// ============================================================
// upload_capacity_remaining
// ============================================================

#[test]
fn test_unlimited_when_zero() {
    let mut bot = make_bot(None);
    bot.upload_limit_bytes = 0;
    assert_eq!(bot.upload_capacity_remaining(), i64::MAX);
}

#[test]
fn test_remaining_calculation() {
    let mut bot = make_bot(None);
    bot.upload_limit_bytes = 1000;
    bot.uploaded_bytes = 300;
    assert_eq!(bot.upload_capacity_remaining(), 700);
}

#[test]
fn test_exhausted() {
    let mut bot = make_bot(None);
    bot.upload_limit_bytes = 100;
    bot.uploaded_bytes = 200;
    assert_eq!(bot.upload_capacity_remaining(), 0);
}
