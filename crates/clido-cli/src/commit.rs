//! `clido commit` subcommand: generate an AI commit message and optionally commit.

use std::path::Path;
use std::process::Command;

/// Run `clido commit`.
///
/// 1. Check `git diff --staged` — exit with clear error if nothing staged.
/// 2. Build an LLM prompt containing the diff.
/// 3. Use `AgentSetup` to call the LLM (no tools, no agent loop).
/// 4. Show the generated message.
/// 5. If `yes` or `dry_run`, skip interactive confirmation.
/// 6. Prompt user: Accept / Cancel.
/// 7. On Accept, run `git commit -m "<message>"`.
pub async fn run_commit(
    workspace_root: &Path,
    yes: bool,
    dry_run: bool,
    cli: &crate::cli::Cli,
) -> Result<(), anyhow::Error> {
    // Step 1: Check for staged changes.
    let staged_diff = run_git_command(workspace_root, &["diff", "--staged"])?;
    if staged_diff.trim().is_empty() {
        return Err(anyhow::anyhow!(
            "Nothing staged. Stage changes with `git add` first."
        ));
    }

    // Step 2: Build the LLM prompt.
    let system_prompt = "You are a commit message generator. \
        Write a concise, accurate git commit message following Conventional Commits style \
        (e.g. feat:, fix:, chore:, docs:, refactor:, test:). \
        Output ONLY the commit message — no explanation, no code blocks, no quotes. \
        The subject line must be 72 characters or fewer.";

    let user_prompt = format!(
        "Write a git commit message for the following staged diff:\n\n```diff\n{}\n```",
        staged_diff.trim()
    );

    // Step 3: Call the LLM via a minimal agent setup.
    let setup = crate::agent_setup::AgentSetup::build(cli, workspace_root)?;
    // Use fast provider for commit message generation if configured.
    let commit_provider = setup
        .fast_provider
        .clone()
        .unwrap_or_else(|| setup.provider.clone());
    let commit_model = setup
        .fast_config
        .as_ref()
        .map(|c| c.model.clone())
        .unwrap_or_else(|| setup.config.model.clone());
    let agent = clido_agent::AgentLoop::new(
        commit_provider,
        setup.registry,
        clido_core::AgentConfig {
            model: commit_model,
            system_prompt: Some(system_prompt.to_string()),
            max_turns: 1,
            ..setup.config
        },
        None, // no ask_user for commit
    );

    let commit_message = agent
        .complete_simple(&user_prompt)
        .await
        .map_err(|e| anyhow::anyhow!("LLM request failed: {}", e))?
        .trim()
        .to_string();

    // Step 4: Show the generated message.
    println!("\nGenerated commit message:\n\n  {}\n", commit_message);

    if dry_run {
        println!("Dry run: skipping git commit.");
        return Ok(());
    }

    // Step 5: Accept immediately if --yes.
    let do_commit = if yes {
        true
    } else {
        // Step 6: Interactive confirmation.
        use inquire::Confirm;
        Confirm::new("Commit with this message?")
            .with_default(true)
            .prompt()
            .unwrap_or(false)
    };

    if !do_commit {
        println!("Commit cancelled.");
        return Ok(());
    }

    // Step 7: Run git commit.
    let output = Command::new("git")
        .args(["commit", "-m", &commit_message])
        .current_dir(workspace_root)
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run git commit: {}", e))?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("{}", stdout.trim());
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("git commit failed: {}", stderr.trim()));
    }

    Ok(())
}

fn run_git_command(cwd: &Path, args: &[&str]) -> Result<String, anyhow::Error> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run git: {}", e))?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
