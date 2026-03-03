use std::process::Command;

use anyhow::{Context, Result, anyhow};

pub struct PrMetadata {
    pub number: u64,
    pub title: String,
    pub repo: String,
    pub head_sha: String,
}

pub fn fetch_pr_diff(pr_number: u64, repo: Option<&str>) -> Result<String> {
    let mut cmd = Command::new("gh");
    cmd.args(["pr", "diff", &pr_number.to_string()]);

    if let Some(r) = repo {
        cmd.args(["--repo", r]);
    }

    let output = cmd
        .output()
        .context("Failed to execute `gh pr diff`. Is the GitHub CLI (`gh`) installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("gh pr diff failed: {}", stderr.trim()));
    }

    String::from_utf8(output.stdout).context("PR diff is not valid UTF-8")
}

pub fn fetch_pr_metadata(pr_number: u64, repo: Option<&str>) -> Result<PrMetadata> {
    let mut cmd = Command::new("gh");
    cmd.args([
        "pr",
        "view",
        &pr_number.to_string(),
        "--json",
        "number,title,url,headRefOid",
    ]);

    if let Some(r) = repo {
        cmd.args(["--repo", r]);
    }

    let output = cmd
        .output()
        .context("Failed to execute `gh pr view`. Is the GitHub CLI (`gh`) installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("gh pr view failed: {}", stderr.trim()));
    }

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse gh pr view JSON")?;

    let url = json["url"].as_str().unwrap_or("").to_string();

    // Extract repo slug from URL: https://github.com/owner/repo/pull/N
    let repo_slug = extract_repo_from_url(&url).unwrap_or_default();

    Ok(PrMetadata {
        number: json["number"].as_u64().unwrap_or(pr_number),
        title: json["title"].as_str().unwrap_or("").to_string(),
        repo: repo_slug,
        head_sha: json["headRefOid"].as_str().unwrap_or("").to_string(),
    })
}

fn extract_repo_from_url(url: &str) -> Option<String> {
    // https://github.com/owner/repo/pull/N -> owner/repo
    let url = url.strip_prefix("https://github.com/")?;
    let parts: Vec<&str> = url.splitn(4, '/').collect();
    if parts.len() >= 2 {
        Some(format!("{}/{}", parts[0], parts[1]))
    } else {
        None
    }
}

pub fn file_url(repo_slug: &str, pr: u64, path: &str, line: usize) -> String {
    format!(
        "https://github.com/{}/pull/{}/files#diff-{}R{}",
        repo_slug,
        pr,
        github_path_hash(path),
        line
    )
}

fn github_path_hash(path: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(path.as_bytes());
    format!("{:x}", hash)
}

/// Return the repository workspace root directory.
/// Tries `jj workspace root` first, then falls back to `git rev-parse --show-toplevel`.
pub fn repo_root() -> Option<String> {
    if let Ok(output) = Command::new("jj").args(["workspace", "root"]).output()
        && output.status.success()
    {
        let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !root.is_empty() {
            return Some(root);
        }
    }

    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        && output.status.success()
    {
        let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !root.is_empty() {
            return Some(root);
        }
    }

    None
}

pub fn fetch_local_diff() -> Result<String> {
    let output = Command::new("jj")
        .args(["diff", "--git"])
        .output()
        .context("Failed to execute `jj diff --git`. Is `jj` installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("jj diff failed: {}", stderr.trim()));
    }

    String::from_utf8(output.stdout).context("jj diff output is not valid UTF-8")
}

pub fn local_metadata() -> Result<PrMetadata> {
    let output = Command::new("jj")
        .args(["log", "-r", "@", "--no-graph", "-T", "description"])
        .output()
        .context("Failed to execute `jj log`. Is `jj` installed?")?;

    let title = if output.status.success() {
        let desc = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if desc.is_empty() {
            "(no description)".to_string()
        } else {
            desc.lines()
                .next()
                .unwrap_or("(no description)")
                .to_string()
        }
    } else {
        "(no description)".to_string()
    };

    Ok(PrMetadata {
        number: 0,
        title,
        repo: String::new(),
        head_sha: String::new(),
    })
}

pub fn current_jj_bookmark() -> Result<String> {
    let output = Command::new("jj")
        .args(["bookmark", "list", "-r", "@-"])
        .output()
        .context("Failed to execute `jj bookmark list`. Is `jj` installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("jj bookmark list failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout
        .lines()
        .next()
        .ok_or_else(|| anyhow!("No bookmarks on @-"))?;

    let bookmark = first_line.split(':').next().unwrap_or("").trim();
    if bookmark.is_empty() {
        return Err(anyhow!("No bookmarks on @-"));
    }

    Ok(bookmark.to_string())
}

/// Infer GitHub repo slug (owner/repo) from jj git remotes.
fn infer_repo_from_jj() -> Option<String> {
    let output = Command::new("jj")
        .args(["git", "remote", "list"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Prefer "origin", fall back to first remote
    let mut first_url = None;
    for line in stdout.lines() {
        // Format: "remote_name url"
        let mut parts = line.split_whitespace();
        let name = parts.next()?;
        let url = parts.next()?;
        if name == "origin" {
            return extract_repo_from_remote_url(url);
        }
        if first_url.is_none() {
            first_url = Some(url.to_string());
        }
    }

    first_url.and_then(|url| extract_repo_from_remote_url(&url))
}

fn extract_repo_from_remote_url(url: &str) -> Option<String> {
    // SSH: git@github.com:owner/repo.git
    if let Some(path) = url.strip_prefix("git@github.com:") {
        let slug = path.strip_suffix(".git").unwrap_or(path);
        return Some(slug.to_string());
    }
    // HTTPS: https://github.com/owner/repo.git
    if let Some(path) = url.strip_prefix("https://github.com/") {
        let slug = path.strip_suffix(".git").unwrap_or(path);
        // Strip trailing slash if any
        let slug = slug.strip_suffix('/').unwrap_or(slug);
        return Some(slug.to_string());
    }
    None
}

pub fn pr_number_for_current_bookmark(repo: Option<&str>) -> Result<(u64, Option<String>)> {
    let bookmark = current_jj_bookmark()?;

    // If no --repo provided, infer from jj git remotes (gh needs this in non-git dirs)
    let inferred_repo = if repo.is_none() {
        infer_repo_from_jj()
    } else {
        None
    };
    let effective_repo = repo.or(inferred_repo.as_deref());

    let mut cmd = Command::new("gh");
    cmd.args(["pr", "view", &bookmark, "--json", "number"]);

    if let Some(r) = effective_repo {
        cmd.args(["--repo", r]);
    }

    let output = cmd
        .output()
        .context("Failed to execute `gh pr view`. Is the GitHub CLI (`gh`) installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "gh pr view failed for bookmark '{}': {}",
            bookmark,
            stderr.trim()
        ));
    }

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse gh pr view JSON")?;

    let number = json["number"]
        .as_u64()
        .ok_or_else(|| anyhow!("No PR number found for bookmark '{}'", bookmark))?;

    Ok((number, inferred_repo))
}

pub fn create_pr_review(
    repo: &str,
    pr_number: u64,
    head_sha: &str,
    comments: &[super::app::PendingComment],
) -> Result<String> {
    let review_comments: Vec<serde_json::Value> = comments
        .iter()
        .map(|c| {
            serde_json::json!({
                "path": c.path,
                "line": c.line,
                "side": "RIGHT",
                "body": c.body,
            })
        })
        .collect();

    let body = serde_json::json!({
        "commit_id": head_sha,
        "event": "COMMENT",
        "comments": review_comments,
    });

    let output = Command::new("gh")
        .args([
            "api",
            &format!("repos/{}/pulls/{}/reviews", repo, pr_number),
            "--method",
            "POST",
            "--input",
            "-",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                stdin.write_all(body.to_string().as_bytes())?;
            }
            child.wait_with_output()
        })
        .context("Failed to execute `gh api`. Is the GitHub CLI (`gh`) installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("gh api review failed: {}", stderr.trim()));
    }

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse review response JSON")?;

    let url = json["html_url"]
        .as_str()
        .ok_or_else(|| anyhow!("No html_url in review response"))?
        .to_string();

    Ok(url)
}

pub fn open_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";
    #[cfg(target_os = "windows")]
    let cmd = "start";
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let cmd = "xdg-open";

    Command::new(cmd)
        .arg(url)
        .spawn()
        .context("Failed to open browser")?;

    Ok(())
}
