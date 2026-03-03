mod align;
mod ansi;
mod cli;
mod color;
mod colors;
mod config;
mod delta;
mod edits;
mod env;
mod features;
mod format;
mod git_config;
mod handlers;
mod minusplus;
mod options;
mod paint;
mod parse_style;
mod parse_styles;
mod review;
mod style;
mod utils;
mod wrapping;

mod tests;

use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::process;

pub fn fatal<T>(errmsg: T) -> !
where
    T: AsRef<str> + std::fmt::Display,
{
    #[cfg(not(test))]
    {
        eprintln!("{errmsg}");
        // As in Config::error_exit_code: use 2 for error
        // because diff uses 0 and 1 for non-error.
        process::exit(2);
    }
    #[cfg(test)]
    panic!("{}\n", errmsg);
}

pub mod errors {
    pub use anyhow::{Context, Error, Result, anyhow};
}

#[cfg(not(tarpaulin_include))]
fn main() -> std::io::Result<()> {
    let args: Vec<OsString> = std::env::args_os().collect();

    // Check if invoked with a PR number or --pr flag.
    // If so, launch the interactive review TUI instead of the normal delta pipeline.
    match parse_review_args(&args) {
        Some(ReviewArgs::Pr {
            number,
            repo,
            dry_run,
        }) => return run_review(number, repo.as_deref(), dry_run, &args),
        Some(ReviewArgs::PrFromBookmark { repo, dry_run }) => {
            return run_pr_from_bookmark(repo.as_deref(), dry_run, &args);
        }
        None => {}
    }

    // No PR number: if stdin is a terminal and no delta-specific flags, launch local review.
    if io::stdin().is_terminal()
        && let Some(dry_run) = parse_local_review_args(&args)
    {
        return run_local_review(dry_run, &args);
    }

    eprintln!("Usage: drev [PR_NUMBER] [--pr] [--repo owner/repo] [--dry-run]");
    process::exit(2);
}

enum ReviewArgs {
    Pr {
        number: u64,
        repo: Option<String>,
        dry_run: bool,
    },
    PrFromBookmark {
        repo: Option<String>,
        dry_run: bool,
    },
}

/// Check if the invocation is `delta <PR_NUMBER> [--repo owner/repo] [--dry-run]`
/// or `delta --pr [--repo owner/repo] [--dry-run]`.
fn parse_review_args(args: &[OsString]) -> Option<ReviewArgs> {
    let mut pr_number = None;
    let mut repo = None;
    let mut dry_run = false;
    let mut pr_flag = false;
    let mut i = 1; // skip argv[0]

    while i < args.len() {
        let arg = args[i].to_string_lossy();
        if arg == "--dry-run" {
            dry_run = true;
        } else if arg == "--pr" {
            pr_flag = true;
        } else if arg == "--repo" {
            i += 1;
            if i < args.len() {
                repo = Some(args[i].to_string_lossy().to_string());
            }
        } else if !arg.starts_with('-') && pr_number.is_none() {
            // First non-flag arg: check if it's a number
            if let Ok(n) = arg.parse::<u64>() {
                pr_number = Some(n);
            } else {
                // Not a number — this is a file path or subcommand, fall through to normal delta
                return None;
            }
        }
        i += 1;
    }

    if let Some(n) = pr_number {
        Some(ReviewArgs::Pr {
            number: n,
            repo,
            dry_run,
        })
    } else if pr_flag {
        Some(ReviewArgs::PrFromBookmark { repo, dry_run })
    } else {
        None
    }
}

/// Check if invocation has only review-mode flags (--dry-run) and no delta-specific args.
/// Returns Some(dry_run) if so.
fn parse_local_review_args(args: &[OsString]) -> Option<bool> {
    let mut dry_run = false;
    for arg in &args[1..] {
        let s = arg.to_string_lossy();
        if s == "--dry-run" {
            dry_run = true;
        } else {
            return None;
        }
    }
    Some(dry_run)
}

#[cfg(not(tarpaulin_include))]
fn run_local_review(dry_run: bool, args: &[OsString]) -> std::io::Result<()> {
    let filtered_args: Vec<OsString> = {
        let mut filtered = vec![args[0].clone()];
        filtered.push("--detect-dark-light".into());
        filtered.push("never".into());
        for arg in &args[1..] {
            let s = arg.to_string_lossy();
            if s == "--dry-run" {
                continue;
            }
            filtered.push(arg.clone());
        }
        filtered
    };

    utils::process::start_determining_calling_process_in_thread();

    let env = env::DeltaEnv::init();
    let assets = utils::bat::assets::load_highlighting_assets();
    let (_call, opt) = cli::Opt::from_args_and_git_config(filtered_args, &env, assets);

    let opt = match opt {
        Some(opt) => opt,
        None => {
            eprintln!("Failed to parse delta options");
            process::exit(2);
        }
    };

    let config = config::Config::from(opt);

    if let Err(e) = review::run_local(&config, dry_run) {
        eprintln!("Error: {:#}", e);
        process::exit(2);
    }

    Ok(())
}

#[cfg(not(tarpaulin_include))]
fn run_pr_from_bookmark(
    repo: Option<&str>,
    dry_run: bool,
    args: &[OsString],
) -> std::io::Result<()> {
    let (pr_number, inferred_repo) = match review::github::pr_number_for_current_bookmark(repo) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Error resolving PR from bookmark: {:#}", e);
            process::exit(2);
        }
    };
    let effective_repo = repo.or(inferred_repo.as_deref());
    run_review(pr_number, effective_repo, dry_run, args)
}

#[cfg(not(tarpaulin_include))]
fn run_review(
    pr_number: u64,
    repo: Option<&str>,
    dry_run: bool,
    args: &[OsString],
) -> std::io::Result<()> {
    // Build a delta Config for rendering (reuse theming, line numbers, etc.)
    // We pass the args through but strip the PR number and --repo flag so clap doesn't choke.
    let filtered_args: Vec<OsString> = {
        let mut filtered = vec![args[0].clone()];
        // Disable terminal color detection — it blocks when we're about to enter a TUI.
        filtered.push("--detect-dark-light".into());
        filtered.push("never".into());
        let mut i = 1;
        let pr_str = pr_number.to_string();
        while i < args.len() {
            let arg = args[i].to_string_lossy();
            if arg == "--repo" {
                i += 2; // skip --repo and its value
                continue;
            }
            if arg == "--dry-run" || arg == "--pr" {
                i += 1;
                continue;
            }
            if arg == pr_str {
                i += 1;
                continue;
            }
            filtered.push(args[i].clone());
            i += 1;
        }
        filtered
    };

    // Must be called before Config::from, which calls calling_process() via a condvar.
    utils::process::start_determining_calling_process_in_thread();

    let env = env::DeltaEnv::init();
    let assets = utils::bat::assets::load_highlighting_assets();
    let (_call, opt) = cli::Opt::from_args_and_git_config(filtered_args, &env, assets);

    let opt = match opt {
        Some(opt) => opt,
        None => {
            eprintln!("Failed to parse delta options");
            process::exit(2);
        }
    };

    let config = config::Config::from(opt);

    if let Err(e) = review::run(pr_number, repo, &config, dry_run) {
        eprintln!("Error: {:#}", e);
        process::exit(2);
    }

    Ok(())
}

