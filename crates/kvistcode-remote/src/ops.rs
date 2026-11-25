//! Agent operations the desktop and the remote server share: committing, merging and
//! shipping an idle agent's work, and the recent-projects list.

use kvistcode_core::{Agent, AgentStatus, Core};
use serde::Serialize;

use crate::git_review::{self, CommitSummary};

const MAX_RECENT_PROJECTS: usize = 10;

pub fn remember_project(core: &Core, project: &str) -> anyhow::Result<()> {
    // One locked read-modify-write, so a concurrent settings save can't drop it.
    core.update_settings(|settings| {
        settings.recent_projects.retain(|p| p != project);
        settings.recent_projects.insert(0, project.to_string());
        settings.recent_projects.truncate(MAX_RECENT_PROJECTS);
    })?;
    Ok(())
}

/// Refuses to change the agent's files or branch while its turn is running: it may be
/// writing files at that very moment (or resume as soon as an approval is answered).
pub fn ensure_not_working(agent: &Agent, doing: &str) -> anyhow::Result<()> {
    match agent.status {
        AgentStatus::Working | AgentStatus::NeedsPermission => {
            anyhow::bail!("The agent is still working; stop it or wait before {doing}")
        }
        _ => Ok(()),
    }
}

/// Takes the agent's git lock, then reads the agent (so the status check sees the
/// state after any operation that held the lock) and checks it isn't mid-turn.
pub fn lock_idle_agent(
    core: &Core,
    agent_id: &str,
    doing: &str,
) -> anyhow::Result<(tokio::sync::OwnedMutexGuard<()>, Agent)> {
    let guard = git_review::try_lock_agent(agent_id)?;
    let agent = core.agent(agent_id)?;
    ensure_not_working(&agent, doing)?;
    Ok((guard, agent))
}

/// Commit for the desktop and the phone remote, leaving `exclude` (repository-relative paths)
/// out of the commit. Fails with [`git_review::NothingToCommit`] when there's nothing to take.
pub async fn commit_excluding_impl(
    core: &Core,
    agent_id: &str,
    message: &str,
    exclude: &[String],
) -> anyhow::Result<CommitSummary> {
    let (_guard, agent) = lock_idle_agent(core, agent_id, "committing")?;
    let touched = if agent.worktree {
        Vec::new()
    } else {
        core.touched_files(agent_id)
    };
    git_review::commit_excluding(&agent, message, &touched, exclude).await
}

/// Secret-looking files a commit would add, for the desktop and the phone remote.
pub async fn secret_files_impl(core: &Core, agent_id: &str) -> anyhow::Result<Vec<String>> {
    let agent = core.agent(agent_id)?;
    let touched = if agent.worktree {
        Vec::new()
    } else {
        core.touched_files(agent_id)
    };
    git_review::pending_secret_files(&agent, &touched).await
}

/// Outcome of "Commit & merge". `Err` from [`ship_excluding_impl`] means nothing changed; `Ok`
/// with `error` set means the commit landed but the merge didn't.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShipResult {
    /// Short hash of the commit made first, if there was anything to commit.
    pub committed: Option<String>,
    /// The merge's summary ("Merged x into main: 3 files changed…" or "Nothing to
    /// merge: …"), if the merge ran.
    pub merged: Option<String>,
    /// Set when the commit succeeded but the merge failed:
    /// "Committed <hash> on <branch>, but the merge failed: …".
    pub error: Option<String>,
    /// The base branch's upstream state after a successful merge: the merge is local
    /// until it is pushed.
    pub base_status: Option<git_review::BaseBranchStatus>,
}

/// "Commit & merge" for the desktop and the phone remote: commits what's left in the
/// agent's worktree (if anything), then merges its branch into the base branch.
pub async fn ship_excluding_impl(
    core: &Core,
    agent_id: &str,
    message: &str,
    exclude: &[String],
) -> anyhow::Result<ShipResult> {
    let (_guard, agent) = lock_idle_agent(core, agent_id, "committing")?;
    if !agent.worktree {
        anyhow::bail!("Only agents running in their own worktree branch can be merged");
    }
    // Everything that would stop the merge, checked before committing anything.
    git_review::merge_preflight(&agent).await?;
    let message = if message.trim().is_empty() {
        agent.title.as_str()
    } else {
        message
    };
    let committed = match git_review::commit_excluding(&agent, message, &[], exclude).await {
        Ok(summary) => Some(summary.hash),
        Err(e) if e.is::<git_review::NothingToCommit>() => None,
        Err(e) => return Err(e),
    };
    match git_review::merge(&agent).await {
        Ok(merged) => Ok(ShipResult {
            committed,
            merged: Some(merged),
            error: None,
            base_status: git_review::base_branch_status(&agent).await.ok(),
        }),
        Err(e) => match committed {
            Some(hash) => Ok(ShipResult {
                error: Some(format!(
                    "Committed {hash} on {}, but the merge failed: {e:#}",
                    agent.branch.as_deref().unwrap_or("the agent's branch")
                )),
                committed: Some(hash),
                merged: None,
                base_status: None,
            }),
            None => Err(e),
        },
    }
}
