use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use libcrucible::config::CrucibleConfig;
use libcrucible::consensus::{ConsensusTaskRequest, TaskAttachment};
use libcrucible::task_pack::{AttachmentKind, list_task_packs};
use std::path::PathBuf;

#[derive(Args)]
pub struct ConsensusArgs {
    #[command(subcommand)]
    pub command: ConsensusCommand,
}

#[derive(Subcommand)]
pub enum ConsensusCommand {
    Run(RunArgs),
    Reply(ReplyArgs),
    Packs(PacksArgs),
}

#[derive(Args)]
pub struct RunArgs {
    #[arg(long)]
    pub pack: String,
    #[arg(long)]
    pub prompt: String,
    #[arg(long = "task-path")]
    pub task_paths: Vec<PathBuf>,
    #[arg(long = "attach")]
    pub attachments: Vec<PathBuf>,
    #[arg(long = "attach-text")]
    pub inline_attachments: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,
}

#[derive(Args)]
pub struct ReplyArgs {
    #[arg(long)]
    pub session: String,
    #[arg(long)]
    pub message: String,
    #[arg(long = "task-path")]
    pub task_paths: Vec<PathBuf>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,
}

#[derive(Args)]
pub struct PacksArgs {
    #[arg(long = "task-path")]
    pub task_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

impl Default for OutputFormat {
    fn default() -> Self {
        Self::Text
    }
}

pub async fn run(args: ConsensusArgs) -> Result<()> {
    match args.command {
        ConsensusCommand::Run(args) => run_task(args).await,
        ConsensusCommand::Reply(args) => reply_task(args).await,
        ConsensusCommand::Packs(args) => list_packs(args),
    }
}

async fn run_task(args: RunArgs) -> Result<()> {
    let cfg = CrucibleConfig::load()?;
    let request = ConsensusTaskRequest {
        pack_id: args.pack,
        prompt: args.prompt,
        attachments: build_attachments(args.attachments, args.inline_attachments),
        task_paths: args.task_paths,
        clarification_history: Vec::new(),
    };
    let report = libcrucible::run_consensus(&cfg, request).await?;
    render_report(&report, args.format)
}

async fn reply_task(args: ReplyArgs) -> Result<()> {
    let cfg = CrucibleConfig::load()?;
    let cwd = std::env::current_dir()?;
    let session_dir = cwd.join(".crucible/sessions").join(&args.session);
    let request_raw = std::fs::read_to_string(session_dir.join("request.json"))
        .with_context(|| format!("read session {}", args.session))?;
    let mut request: ConsensusTaskRequest =
        serde_json::from_str(&request_raw).context("parse session request")?;
    request.clarification_history.push(args.message);
    request.task_paths.extend(args.task_paths);
    let report = libcrucible::run_consensus(&cfg, request).await?;
    render_report(&report, args.format)
}

fn list_packs(args: PacksArgs) -> Result<()> {
    let cfg = CrucibleConfig::load()?;
    let cwd = std::env::current_dir().ok();
    let packs = list_task_packs(&cfg, cwd.as_deref(), &args.task_paths)?;
    for pack in packs {
        println!(
            "{}\t{}\t{}",
            pack.manifest.id, pack.manifest.version, pack.manifest.description
        );
    }
    Ok(())
}

fn build_attachments(files: Vec<PathBuf>, inline_attachments: Vec<String>) -> Vec<TaskAttachment> {
    let mut attachments = Vec::new();
    for path in files {
        let id = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("attachment")
            .to_string();
        attachments.push(TaskAttachment {
            id,
            kind: guess_attachment_kind(&path),
            path: Some(path),
            inline: None,
        });
    }
    for (idx, inline) in inline_attachments.into_iter().enumerate() {
        attachments.push(TaskAttachment {
            id: format!("inline-{}", idx + 1),
            kind: AttachmentKind::Text,
            path: None,
            inline: Some(inline),
        });
    }
    attachments
}

fn guess_attachment_kind(path: &std::path::Path) -> AttachmentKind {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("md") => AttachmentKind::Markdown,
        Some("diff") | Some("patch") => AttachmentKind::Diff,
        Some("rs") | Some("ts") | Some("tsx") | Some("js") | Some("py") | Some("go") => {
            AttachmentKind::SourceFile
        }
        _ => AttachmentKind::Text,
    }
}

fn render_report(
    report: &libcrucible::consensus::ConsensusReport,
    format: OutputFormat,
) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(report)?);
        }
        OutputFormat::Text => {
            println!("Crucible Consensus — {}", report.title);
            println!("Pack: {}", report.pack_id);
            println!("\n{}", report.summary_markdown);
            if !report.clarification_requests.is_empty() {
                println!("\nClarifications:");
                for question in &report.clarification_requests {
                    println!("- {}", question);
                }
                println!("\nReply with:");
                println!(
                    "  crucible consensus reply --session {} --message \"...\"",
                    report.session_id
                );
            }
        }
    }
    Ok(())
}
