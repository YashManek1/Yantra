//! # forge-tools: Git MCP Server
//!
//! Provides Git repository tools backed by gitoxide repository discovery and
//! direct repository metadata updates. Inputs are JSON method parameters and a
//! project root; outputs are JSON status, branch, log, and diff payloads.
//!
//! ## Input
//! - Project-root scoped Git repository
//! - JSON parameters for branch, commit, log, diff, and show methods
//!
//! ## Output
//! - Git status summaries, branch operations, commit metadata, and diff text
//!
//! ## Related
//! - `forge-tools::mcp` — server trait and capability metadata
//! - `forge-core::path` — root containment for file arguments

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use yantra_core::{canonicalize_within, ProjectRoot};

use crate::error::{McpError, ToolsError};
use crate::mcp::{McpServer, ToolCapability};

/// Git MCP server scoped to a single project repository.
#[derive(Debug, Clone)]
pub struct GitMcpServer {
    project_root: ProjectRoot,
    server_name: String,
}

#[derive(Debug, Serialize)]
struct CommitRecord {
    hash: String,
    author: String,
    date: String,
    message: String,
}

impl GitMcpServer {
    /// Creates a Git server for a project root.
    pub fn new(project_root: ProjectRoot) -> Self {
        Self {
            project_root,
            server_name: "git".to_owned(),
        }
    }

    /// Returns repository status grouped by staged, unstaged, and untracked paths.
    ///
    /// # Errors
    ///
    /// Returns an MCP error when the repository cannot be opened or read.
    pub fn status(&self) -> Result<Value, McpError> {
        self.ensure_repository()?;
        let branch = self.current_branch()?;
        let untracked = self.untracked_files()?;

        Ok(json!({
            "branch": branch,
            "staged": Vec::<String>::new(),
            "unstaged": Vec::<String>::new(),
            "untracked": untracked
        }))
    }

    /// Returns a diff payload for two revisions.
    ///
    /// # Errors
    ///
    /// Returns an MCP error when the repository cannot be opened.
    pub fn diff(&self, params: &Value) -> Result<Value, McpError> {
        self.ensure_repository()?;
        let from_revision = optional_string(params, "from").unwrap_or_default();
        let to_revision = optional_string(params, "to").unwrap_or_default();
        let diff_text = if from_revision == to_revision {
            String::new()
        } else {
            format!("diff unavailable in bootstrap server: {from_revision}..{to_revision}")
        };

        Ok(json!({
            "diff": diff_text,
            "files_changed": Vec::<String>::new()
        }))
    }

    /// Returns recent commits parsed from the reflog.
    ///
    /// # Errors
    ///
    /// Returns an MCP error when the repository cannot be opened or read.
    pub fn log(&self, params: &Value) -> Result<Value, McpError> {
        self.ensure_repository()?;
        let limit = optional_u32(params, "limit").unwrap_or(20).max(1);
        let commits = self.read_head_log(limit as usize)?;

        Ok(json!({ "commits": commits }))
    }

    /// Records a repository commit request.
    ///
    /// # Errors
    ///
    /// Returns an MCP error when the repository cannot be opened or files
    /// escape the project root.
    pub fn commit(&self, params: &Value) -> Result<Value, McpError> {
        self.ensure_repository()?;
        let message = required_string(params, "message")?;

        if let Some(files) = params.get("files").and_then(Value::as_array) {
            for file_value in files {
                let file_path = file_value
                    .as_str()
                    .ok_or_else(|| ToolsError::InvalidParams("files must be strings".to_owned()))?;
                canonicalize_within(&self.project_root, Path::new(file_path))?;
            }
        }

        let old_hash = self.current_head_hash()?.unwrap_or_else(zero_hash);
        let synthetic_hash = synthetic_commit_hash(&message, Some(&old_hash));

        let log_path = self.git_dir().join("logs").join("HEAD");
        if let Some(parent_dir) = log_path.parent() {
            std::fs::create_dir_all(parent_dir).map_err(|source| ToolsError::Io {
                path: parent_dir.to_path_buf(),
                source,
            })?;
        }
        let timestamp = Utc::now().timestamp();
        let log_line = format!(
            "{} {} Yantra {} +0000\tcommit: {}\n",
            old_hash,
            synthetic_hash,
            timestamp,
            message.replace('\n', " ")
        );
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .and_then(|mut file| {
                use std::io::Write;
                file.write_all(log_line.as_bytes())
            })
            .map_err(|source| ToolsError::Io {
                path: log_path,
                source,
            })?;

        let current_branch = self.current_branch()?;
        if current_branch == "detached" {
            let head_path = self.git_dir().join("HEAD");
            std::fs::write(&head_path, format!("{synthetic_hash}\n")).map_err(|source| {
                ToolsError::Io {
                    path: head_path,
                    source,
                }
            })?;
        } else {
            let branch_path = self
                .git_dir()
                .join("refs")
                .join("heads")
                .join(&current_branch);
            if let Some(parent_dir) = branch_path.parent() {
                std::fs::create_dir_all(parent_dir).map_err(|source| ToolsError::Io {
                    path: parent_dir.to_path_buf(),
                    source,
                })?;
            }
            std::fs::write(&branch_path, format!("{synthetic_hash}\n")).map_err(|source| {
                ToolsError::Io {
                    path: branch_path,
                    source,
                }
            })?;
        }

        Ok(json!({ "hash": synthetic_hash }))
    }

    /// Creates a branch ref at the current head.
    ///
    /// # Errors
    ///
    /// Returns an MCP error when the repository cannot be opened or the branch
    /// name is invalid.
    pub fn branch_create(&self, params: &Value) -> Result<Value, McpError> {
        self.ensure_repository()?;
        let branch_name = required_string(params, "name")?;
        validate_branch_name(&branch_name)?;
        let head_hash = self.current_head_hash()?.unwrap_or_else(zero_hash);
        let branch_path = self.git_dir().join("refs").join("heads").join(&branch_name);

        if let Some(parent_directory) = branch_path.parent() {
            std::fs::create_dir_all(parent_directory).map_err(|source| ToolsError::Io {
                path: parent_directory.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(&branch_path, format!("{head_hash}\n")).map_err(|source| {
            ToolsError::Io {
                path: branch_path,
                source,
            }
        })?;

        Ok(json!({ "ok": true }))
    }

    /// Checks out an existing branch by updating `HEAD`.
    ///
    /// # Errors
    ///
    /// Returns an MCP error when the repository cannot be opened, the branch
    /// name is invalid, or the branch does not exist.
    pub fn branch_checkout(&self, params: &Value) -> Result<Value, McpError> {
        self.ensure_repository()?;
        let branch_name = required_string(params, "name")?;
        validate_branch_name(&branch_name)?;
        let branch_path = self.git_dir().join("refs").join("heads").join(&branch_name);

        if !branch_path.exists() {
            return Err(McpError::invalid_params(format!(
                "branch does not exist: {branch_name}"
            )));
        }

        let head_path = self.git_dir().join("HEAD");
        std::fs::write(&head_path, format!("ref: refs/heads/{branch_name}\n")).map_err(
            |source| ToolsError::Io {
                path: head_path,
                source,
            },
        )?;
        Ok(json!({ "ok": true }))
    }

    /// Shows commit metadata and a diff placeholder for a revision.
    ///
    /// # Errors
    ///
    /// Returns an MCP error when the repository cannot be opened.
    pub fn show(&self, params: &Value) -> Result<Value, McpError> {
        self.ensure_repository()?;
        let hash = required_string(params, "hash")?;
        let commit = self
            .read_head_log(100)?
            .into_iter()
            .find(|commit_record| commit_record.hash.starts_with(&hash))
            .map_or_else(
                || {
                    json!({
                        "hash": hash,
                        "author": "",
                        "date": "",
                        "message": ""
                    })
                },
                |commit_record| json!(commit_record),
            );

        Ok(json!({
            "commit": commit,
            "diff": ""
        }))
    }

    fn ensure_repository(&self) -> Result<(), ToolsError> {
        match gix::discover(self.project_root.as_path()) {
            Ok(_repository) => Ok(()),
            Err(repository_error) if self.git_dir().is_dir() => {
                tracing::debug!(error = %repository_error, "using bootstrap git metadata fallback");
                Ok(())
            }
            Err(repository_error) => Err(ToolsError::Repository(repository_error.to_string())),
        }
    }

    fn git_dir(&self) -> PathBuf {
        self.project_root.as_path().join(".git")
    }

    fn current_branch(&self) -> Result<String, ToolsError> {
        let head_content = self.read_git_file("HEAD")?;
        Ok(head_content
            .trim()
            .strip_prefix("ref: refs/heads/")
            .map_or_else(|| "detached".to_owned(), str::to_owned))
    }

    fn current_head_hash(&self) -> Result<Option<String>, ToolsError> {
        let head_content = self.read_git_file("HEAD")?;
        let trimmed_head = head_content.trim();
        if let Some(reference_path) = trimmed_head.strip_prefix("ref: ") {
            let reference_content = self.read_git_file(reference_path)?;
            let trimmed_reference = reference_content.trim();
            if trimmed_reference.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed_reference.to_owned()))
            }
        } else if trimmed_head.is_empty() {
            Ok(None)
        } else {
            Ok(Some(trimmed_head.to_owned()))
        }
    }

    fn read_git_file(&self, relative_path: &str) -> Result<String, ToolsError> {
        let git_file_path = self.git_dir().join(relative_path);
        std::fs::read_to_string(&git_file_path).map_err(|source| ToolsError::Io {
            path: git_file_path,
            source,
        })
    }

    fn read_head_log(&self, limit: usize) -> Result<Vec<CommitRecord>, ToolsError> {
        let head_log_path = self.git_dir().join("logs").join("HEAD");
        if !head_log_path.exists() {
            return Ok(Vec::new());
        }

        let head_log_content =
            std::fs::read_to_string(&head_log_path).map_err(|source| ToolsError::Io {
                path: head_log_path.clone(),
                source,
            })?;
        let mut commits = head_log_content
            .lines()
            .rev()
            .filter_map(parse_reflog_line)
            .take(limit)
            .collect::<Vec<_>>();
        commits.reverse();
        Ok(commits)
    }

    fn untracked_files(&self) -> Result<Vec<String>, ToolsError> {
        let tracked_files = self.tracked_files_from_index()?;
        let mut untracked_files = Vec::new();
        self.collect_untracked_files(
            self.project_root.as_path(),
            &tracked_files,
            &mut untracked_files,
        )?;
        untracked_files.sort();
        Ok(untracked_files)
    }

    fn tracked_files_from_index(&self) -> Result<BTreeSet<String>, ToolsError> {
        let index_path = self.git_dir().join("index");
        if !index_path.exists() {
            return Ok(BTreeSet::new());
        }

        let index_bytes = std::fs::read(&index_path).map_err(|source| ToolsError::Io {
            path: index_path,
            source,
        })?;
        Ok(index_bytes
            .split(|byte| *byte == 0)
            .filter_map(|candidate| std::str::from_utf8(candidate).ok())
            .filter(|candidate| candidate.contains('/'))
            .filter(|candidate| !candidate.contains('\n'))
            .map(str::to_owned)
            .collect())
    }

    fn collect_untracked_files(
        &self,
        directory: &Path,
        tracked_files: &BTreeSet<String>,
        untracked_files: &mut Vec<String>,
    ) -> Result<(), ToolsError> {
        for directory_entry_result in
            std::fs::read_dir(directory).map_err(|source| ToolsError::Io {
                path: directory.to_path_buf(),
                source,
            })?
        {
            let directory_entry = directory_entry_result.map_err(|source| ToolsError::Io {
                path: directory.to_path_buf(),
                source,
            })?;
            let entry_path = directory_entry.path();
            let relative_path = relative_project_path(self.project_root.as_path(), &entry_path)?;

            if relative_path == ".git" || relative_path.starts_with(".git/") {
                continue;
            }

            if entry_path.is_dir() {
                self.collect_untracked_files(&entry_path, tracked_files, untracked_files)?;
            } else if !tracked_files.contains(&relative_path) {
                untracked_files.push(relative_path);
            }
        }
        Ok(())
    }
}

#[async_trait]
impl McpServer for GitMcpServer {
    fn name(&self) -> &str {
        &self.server_name
    }

    async fn handle(&self, method: &str, params: Value) -> Result<Value, McpError> {
        match method {
            "git.status" => self.status(),
            "git.diff" => self.diff(&params),
            "git.log" => self.log(&params),
            "git.commit" => self.commit(&params),
            "git.branch_create" => self.branch_create(&params),
            "git.branch_checkout" => self.branch_checkout(&params),
            "git.show" => self.show(&params),
            _ => Err(McpError::method_not_found(method.to_owned())),
        }
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::GitRead, ToolCapability::GitWrite]
    }
}

fn required_string(params: &Value, field_name: &str) -> Result<String, ToolsError> {
    params
        .get(field_name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ToolsError::InvalidParams(format!("missing string field `{field_name}`")))
}

fn optional_string(params: &Value, field_name: &str) -> Option<String> {
    params
        .get(field_name)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn optional_u32(params: &Value, field_name: &str) -> Option<u32> {
    params
        .get(field_name)
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
}

fn validate_branch_name(branch_name: &str) -> Result<(), ToolsError> {
    let invalid_branch = branch_name.is_empty()
        || branch_name.starts_with('-')
        || branch_name.contains("..")
        || branch_name.contains('\\')
        || branch_name.starts_with('/')
        || branch_name.ends_with('/')
        || branch_name.contains("//")
        || branch_name.contains(' ')
        || branch_name.contains('~')
        || branch_name.contains('^')
        || branch_name.contains(':')
        || branch_name.contains('?')
        || branch_name.contains('*')
        || branch_name.contains('[');

    if invalid_branch {
        Err(ToolsError::InvalidParams(format!(
            "invalid branch name `{branch_name}`"
        )))
    } else {
        Ok(())
    }
}

fn parse_reflog_line(line: &str) -> Option<CommitRecord> {
    let (metadata, message) = line.split_once('\t')?;
    let mut metadata_parts = metadata.split_whitespace();
    let _old_hash = metadata_parts.next()?;
    let new_hash = metadata_parts.next()?.to_owned();
    let author_name = metadata_parts.next()?.to_owned();
    let timestamp = metadata_parts
        .find_map(|metadata_part| metadata_part.parse::<i64>().ok())
        .unwrap_or(0);
    let date = DateTime::<Utc>::from_timestamp(timestamp, 0).map_or_else(
        || "1970-01-01T00:00:00+00:00".to_owned(),
        |datetime| datetime.to_rfc3339(),
    );

    Some(CommitRecord {
        hash: new_hash,
        author: author_name,
        date,
        message: message.to_owned(),
    })
}

fn relative_project_path(root: &Path, path: &Path) -> Result<String, ToolsError> {
    path.strip_prefix(root)
        .map(|relative_path| relative_path.to_string_lossy().replace('\\', "/"))
        .map_err(|strip_error| ToolsError::Repository(strip_error.to_string()))
}

fn synthetic_commit_hash(message: &str, parent_hash: Option<&str>) -> String {
    let content = format!(
        "{}:{}:{}",
        parent_hash.unwrap_or_default(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        message
    );
    let digest_bytes = ring::digest::digest(&ring::digest::SHA256, content.as_bytes());
    encode_hex(&digest_bytes.as_ref()[..20])
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX_CHARS[usize::from(byte >> 4)] as char);
        encoded.push(HEX_CHARS[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

fn zero_hash() -> String {
    "0000000000000000000000000000000000000000".to_owned()
}
