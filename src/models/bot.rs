use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Whether a path rule allows or blocks access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BotRuleStatus {
    Allow,
    Block,
}

/// One row of a bot's path-rule table: a (bucket, path) pair with an
/// allow/block status.
///
/// Path semantics:
///   - `""` (empty path) applies to the entire bucket.
///   - Otherwise the path is slash-separated starting with `/` (e.g.
///     `/Documents/Work`); a rule covers a target when the target equals the
///     rule path or lives beneath it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotPathRule {
    pub bucket: String,
    pub path: String,
    pub status: BotRuleStatus,
}

/// A bot is a user-scoped API key with granular access restrictions.
///
/// The bot authenticates as its owner (`user_id`), sharing the owner's
/// storage, quota and bucket permissions. This row only narrows that access:
/// it can never widen it.
///
/// `path_rules` semantics:
///   - `None` or `Some(vec![])`  → unrestricted (every bucket/folder/file the
///                                  owner can reach)
///   - `Some(vec![..])`          → per-bucket allow/block path rules
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bot {
    pub id: Uuid,
    pub user_id: Uuid,
    pub key_id: Uuid,
    pub name: String,
    pub can_upload: bool,
    pub can_download: bool,
    pub can_copy: bool,
    pub can_edit: bool,
    pub can_delete: bool,
    pub can_list: bool,
    pub path_rules: Option<Vec<BotPathRule>>,
    pub upload_limit_bytes: i64,
    pub uploaded_bytes: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Bot {
    /// The rules that apply to a given bucket.
    fn rules_for(&self, bucket: &str) -> Vec<&BotPathRule> {
        match &self.path_rules {
            Some(rules) => rules.iter().filter(|r| r.bucket == bucket).collect(),
            None => Vec::new(),
        }
    }

    /// Whether a rule path covers a (normalized, `/`-prefixed) target path.
    /// An empty rule path covers everything; otherwise the target must equal
    /// the rule path or sit beneath it (`/a` covers `/a` and `/a/b`, but not
    /// `/ab`).
    fn covers(rule: &str, target: &str) -> bool {
        if rule.is_empty() {
            return true;
        }
        if target == rule {
            return true;
        }
        target.starts_with(rule) && target.as_bytes().get(rule.len()) == Some(&b'/')
    }

    /// Whether the bot may access `path` inside `bucket`. A bucket with no
    /// rules is fully accessible. A bucket with rules is fail-closed: a path is
    /// allowed only when an allow rule covers it and no block rule does.
    /// `""` is the bucket root.
    pub fn path_allowed(&self, bucket: &str, path: &str) -> bool {
        let rules = self.rules_for(bucket);
        if rules.is_empty() {
            return true;
        }
        let target = path.trim_end_matches('/');
        if rules
            .iter()
            .any(|r| r.status == BotRuleStatus::Block && Self::covers(&r.path, target))
        {
            return false;
        }
        rules
            .iter()
            .any(|r| r.status == BotRuleStatus::Allow && Self::covers(&r.path, target))
    }

    /// Whether the bot may operate inside `bucket` at all. A bucket with no
    /// rules (or only block rules) is not listed; a bucket with an allow rule
    /// (even for a sub-path) is reachable.
    pub fn bucket_allowed(&self, bucket: &str) -> bool {
        let rules = self.rules_for(bucket);
        if rules.is_empty() {
            return true;
        }
        rules
            .iter()
            .any(|r| r.status == BotRuleStatus::Allow)
    }

    /// Whether the bot's lifetime upload cap is already exhausted (0 = unlimited).
    pub fn upload_capacity_remaining(&self) -> i64 {
        if self.upload_limit_bytes <= 0 {
            i64::MAX
        } else {
            (self.upload_limit_bytes - self.uploaded_bytes).max(0)
        }
    }

    /// Scopes carried by the bot's underlying API key, derived from its
    /// capabilities. This is the single point that bridges a bot's
    /// capabilities to the scope checks the file endpoints run.
    pub fn scopes(&self) -> Vec<String> {
        let mut scopes = Vec::new();
        if self.can_list || self.can_download {
            scopes.push("files:read".to_string());
        }
        // Copy and edit (rename/move) both mutate the user's file tree, so they
        // need write scope just like upload.
        if self.can_upload || self.can_copy || self.can_edit {
            scopes.push("files:write".to_string());
        }
        if self.can_delete {
            scopes.push("files:delete".to_string());
        }
        scopes
    }
}
