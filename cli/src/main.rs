use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use nodalmerge_cli::{
    run_archive_command, run_query_command, run_topology_command, run_worker, ArchiveCommand,
    ArchiveCliError, QueryCommand, QueryCliError, RunWorkerOpts, TopologyCommand,
    TopologyCliError, TopologyGlobalOpts,
};
use nodalmerge_cli::ws_session::build_ws_url;

#[derive(Parser)]
#[command(name = "nodalmerge", about = "NodalMerge operator CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Headless peer worker: WS sync + peer-local persistence (wraps nodalmerge-headless).
    Run {
        #[arg(long, env = "NODALMERGE_HEADLESS_SERVER_URL")]
        server_url: Option<String>,
        #[arg(long, env = "NODALMERGE_HEADLESS_ROOM")]
        room: Option<String>,
        #[arg(long, env = "NODALMERGE_HEADLESS_BACKEND", default_value = "memory")]
        backend: String,
        #[arg(long, env = "NODALMERGE_HEADLESS_DATA_DIR")]
        data_dir: Option<PathBuf>,
        #[arg(long, env = "NODALMERGE_HEADLESS_RUN_SECS", default_value_t = 10)]
        run_secs: u64,
        #[arg(long, env = "NODALMERGE_HEADLESS_NEGOTIATE_IBF", default_value_t = true)]
        negotiate_ibf: bool,
        #[arg(long, env = "NODALMERGE_HEADLESS_NEGOTIATE_MST", default_value_t = true)]
        negotiate_mst: bool,
        #[arg(long, env = "NODALMERGE_HEADLESS_REPORT_JSON")]
        report_json: Option<String>,
    },
    /// Room-family topology workflows (connects to nodalmerge-server over WebSocket).
    Topology {
        #[command(subcommand)]
        command: TopologySub,
    },
    /// Archive export/import control plane (describe, validate, export).
    Archive {
        #[command(subcommand)]
        command: ArchiveSub,
    },
    /// Query / projection control plane.
    Query {
        #[command(subcommand)]
        command: QuerySub,
    },
}

#[derive(Subcommand)]
enum ArchiveSub {
    /// Describe an archive reference (`room://…` or `file://…`).
    Describe {
        #[arg(long)]
        archive_ref: String,
        #[arg(long, env = "NODALMERGE_SERVER_URL")]
        server: Option<String>,
        #[arg(long, env = "NODALMERGE_ROOM")]
        room: Option<String>,
        #[arg(long, env = "NODALMERGE_TOKEN_JSON")]
        token_json: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
    /// Validate archive integrity.
    Validate {
        #[arg(long)]
        archive_ref: String,
        #[arg(long, default_value = "full_integrity")]
        mode: String,
        #[arg(long, env = "NODALMERGE_SERVER_URL")]
        server: Option<String>,
        #[arg(long, env = "NODALMERGE_ROOM")]
        room: Option<String>,
        #[arg(long, env = "NODALMERGE_TOKEN_JSON")]
        token_json: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
    /// Export a room to an archive reference.
    Export {
        #[arg(long)]
        source_room: String,
        #[arg(long)]
        archive_ref: String,
        #[arg(long, env = "NODALMERGE_SERVER_URL")]
        server: Option<String>,
        #[arg(long, env = "NODALMERGE_ROOM")]
        room: Option<String>,
        #[arg(long, env = "NODALMERGE_TOKEN_JSON")]
        token_json: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
    /// Import an archive into the session room.
    Import {
        #[arg(long)]
        archive_ref: String,
        #[arg(long, default_value = "full_apply")]
        import_mode: String,
        #[arg(long)]
        expected_checkpoint_file: Option<PathBuf>,
        #[arg(long, env = "NODALMERGE_SERVER_URL")]
        server: Option<String>,
        #[arg(long, env = "NODALMERGE_ROOM")]
        room: Option<String>,
        #[arg(long, env = "NODALMERGE_TOKEN_JSON")]
        token_json: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
}

#[derive(Subcommand)]
enum QuerySub {
    /// Register a query spec version.
    RegisterSpec {
        #[arg(long)]
        query_spec_id: String,
        #[arg(long)]
        version: String,
        #[arg(long)]
        descriptor_file: PathBuf,
        #[arg(long, env = "NODALMERGE_SERVER_URL")]
        server: Option<String>,
        #[arg(long, env = "NODALMERGE_ROOM")]
        room: Option<String>,
        #[arg(long, env = "NODALMERGE_TOKEN_JSON")]
        token_json: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
    /// Build a projection at a checkpoint selector.
    BuildProjection {
        #[arg(long)]
        projection_id: String,
        #[arg(long)]
        query_spec_id: String,
        #[arg(long, default_value = "latest")]
        selector: String,
        #[arg(long)]
        canonical_seq: Option<u64>,
        #[arg(long, env = "NODALMERGE_SERVER_URL")]
        server: Option<String>,
        #[arg(long, env = "NODALMERGE_ROOM")]
        room: Option<String>,
        #[arg(long, env = "NODALMERGE_TOKEN_JSON")]
        token_json: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
    /// List projections (optional filter by query spec).
    ListProjections {
        #[arg(long)]
        query_spec_id: Option<String>,
        #[arg(long, env = "NODALMERGE_SERVER_URL")]
        server: Option<String>,
        #[arg(long, env = "NODALMERGE_ROOM")]
        room: Option<String>,
        #[arg(long, env = "NODALMERGE_TOKEN_JSON")]
        token_json: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
    /// Read paginated projection rows.
    ReadProjection {
        #[arg(long)]
        projection_id: String,
        #[arg(long, default_value_t = 50)]
        limit: u64,
        #[arg(long)]
        page_token: Option<String>,
        #[arg(long, env = "NODALMERGE_SERVER_URL")]
        server: Option<String>,
        #[arg(long, env = "NODALMERGE_ROOM")]
        room: Option<String>,
        #[arg(long, env = "NODALMERGE_TOKEN_JSON")]
        token_json: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
    /// Invalidate a projection.
    InvalidateProjection {
        #[arg(long)]
        projection_id: String,
        #[arg(long, default_value = "manual")]
        reason: String,
        #[arg(long, env = "NODALMERGE_SERVER_URL")]
        server: Option<String>,
        #[arg(long, env = "NODALMERGE_ROOM")]
        room: Option<String>,
        #[arg(long, env = "NODALMERGE_TOKEN_JSON")]
        token_json: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
}

#[derive(Subcommand)]
enum TopologySub {
    /// Create a child room bound to a parent checkpoint.
    CreateChild {
        #[arg(long)]
        parent_room: String,
        #[arg(long)]
        child_room: String,
        #[arg(long)]
        purpose: String,
        #[arg(long)]
        policy: String,
        #[arg(long)]
        created_by: Option<String>,
        #[arg(long)]
        parent_checkpoint_file: PathBuf,
        #[arg(long, env = "NODALMERGE_SERVER_URL")]
        server: Option<String>,
        #[arg(long, env = "NODALMERGE_ROOM")]
        room: Option<String>,
        #[arg(long, env = "NODALMERGE_TOKEN_JSON")]
        token_json: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
    /// List child rooms for a parent.
    ListChildren {
        #[arg(long)]
        parent_room: String,
        #[arg(long, env = "NODALMERGE_SERVER_URL")]
        server: Option<String>,
        #[arg(long, env = "NODALMERGE_ROOM")]
        room: Option<String>,
        #[arg(long, env = "NODALMERGE_TOKEN_JSON")]
        token_json: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
    /// Show lineage for a room (describe-lineage).
    ShowLineage {
        #[arg(long)]
        room: String,
        #[arg(long, env = "NODALMERGE_SERVER_URL")]
        server: Option<String>,
        #[arg(long, env = "NODALMERGE_TOKEN_JSON")]
        token_json: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
    /// Submit a promotion proposal.
    ProposePromotion {
        #[arg(long)]
        parent_room: String,
        #[arg(long)]
        child_room: String,
        #[arg(long)]
        child_checkpoint: String,
        #[arg(long)]
        payload_ref: String,
        #[arg(long)]
        idempotency_key: Option<String>,
        #[arg(long, env = "NODALMERGE_SERVER_URL")]
        server: Option<String>,
        #[arg(long, env = "NODALMERGE_ROOM")]
        room: Option<String>,
        #[arg(long, env = "NODALMERGE_TOKEN_JSON")]
        token_json: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
    /// Validate a promotion proposal.
    ValidatePromotion {
        #[arg(long)]
        proposal_id: String,
        #[arg(long, env = "NODALMERGE_SERVER_URL")]
        server: Option<String>,
        #[arg(long, env = "NODALMERGE_ROOM")]
        room: Option<String>,
        #[arg(long, env = "NODALMERGE_TOKEN_JSON")]
        token_json: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
    /// Apply a validated promotion to the parent room.
    ApplyPromotion {
        #[arg(long)]
        proposal_id: String,
        #[arg(long, env = "NODALMERGE_SERVER_URL")]
        server: Option<String>,
        #[arg(long, env = "NODALMERGE_ROOM")]
        room: Option<String>,
        #[arg(long, env = "NODALMERGE_TOKEN_JSON")]
        token_json: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let peer_seed = [0xC1u8; 32];

    match cli.command {
        Commands::Run {
            server_url,
            room,
            backend,
            data_dir,
            run_secs,
            negotiate_ibf,
            negotiate_mst,
            report_json,
        } => {
            let server_ws_url = server_url
                .or_else(|| std::env::var("NODALMERGE_HEADLESS_SERVER_URL").ok())
                .ok_or_else(|| "server URL required (--server-url or NODALMERGE_HEADLESS_SERVER_URL)")
                .map_err(run_err);
            let room_id = room
                .or_else(|| std::env::var("NODALMERGE_HEADLESS_ROOM").ok())
                .ok_or_else(|| "room required (--room or NODALMERGE_HEADLESS_ROOM)")
                .map_err(run_err);
            match (server_ws_url, room_id) {
                (Ok(url), Ok(rid)) => match run_worker(RunWorkerOpts {
                    server_ws_url: build_ws_url(&url, &rid),
                    room_id: rid,
                    backend,
                    data_dir,
                    run_secs,
                    negotiate_ibf,
                    negotiate_mst,
                    report_json,
                })
                .await
                {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("{e}");
                        ExitCode::FAILURE
                    }
                },
                (Err(e), _) | (_, Err(e)) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        Commands::Topology { command } => {
            let (globals, cmd) = map_topology(command, peer_seed);
            print_json_result(run_topology_command(&globals, cmd).await.map_err(topology_err))
        }
        Commands::Archive { command } => {
            let (globals, cmd) = map_archive(command, peer_seed);
            print_json_result(run_archive_command(&globals, cmd).await.map_err(archive_err))
        }
        Commands::Query { command } => {
            let (globals, cmd) = map_query(command, peer_seed);
            print_json_result(run_query_command(&globals, cmd).await.map_err(query_err))
        }
    }
}

fn print_json_result(result: Result<serde_json::Value, String>) -> ExitCode {
    match result {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run_err(msg: &'static str) -> String {
    msg.to_string()
}

fn topology_err(e: TopologyCliError) -> String {
    e.to_string()
}

fn archive_err(e: ArchiveCliError) -> String {
    e.to_string()
}

fn query_err(e: QueryCliError) -> String {
    e.to_string()
}

fn map_archive(sub: ArchiveSub, peer_seed: [u8; 32]) -> (TopologyGlobalOpts, ArchiveCommand) {
    match sub {
        ArchiveSub::Describe {
            archive_ref,
            server,
            room,
            token_json,
            timeout_secs,
        } => (
            TopologyGlobalOpts {
                server,
                room,
                token_json,
                timeout_secs,
                peer_seed,
            },
            ArchiveCommand::Describe { archive_ref },
        ),
        ArchiveSub::Validate {
            archive_ref,
            mode,
            server,
            room,
            token_json,
            timeout_secs,
        } => (
            TopologyGlobalOpts {
                server,
                room,
                token_json,
                timeout_secs,
                peer_seed,
            },
            ArchiveCommand::Validate { archive_ref, mode },
        ),
        ArchiveSub::Export {
            source_room,
            archive_ref,
            server,
            room,
            token_json,
            timeout_secs,
        } => (
            TopologyGlobalOpts {
                server,
                room,
                token_json,
                timeout_secs,
                peer_seed,
            },
            ArchiveCommand::Export {
                source_room,
                archive_ref,
            },
        ),
        ArchiveSub::Import {
            archive_ref,
            import_mode,
            expected_checkpoint_file,
            server,
            room,
            token_json,
            timeout_secs,
        } => (
            TopologyGlobalOpts {
                server,
                room,
                token_json,
                timeout_secs,
                peer_seed,
            },
            ArchiveCommand::Import {
                archive_ref,
                import_mode,
                expected_checkpoint_file,
            },
        ),
    }
}

fn map_query(sub: QuerySub, peer_seed: [u8; 32]) -> (TopologyGlobalOpts, QueryCommand) {
    match sub {
        QuerySub::RegisterSpec {
            query_spec_id,
            version,
            descriptor_file,
            server,
            room,
            token_json,
            timeout_secs,
        } => (
            TopologyGlobalOpts {
                server,
                room,
                token_json,
                timeout_secs,
                peer_seed,
            },
            QueryCommand::RegisterSpec {
                query_spec_id,
                version,
                descriptor_file,
            },
        ),
        QuerySub::BuildProjection {
            projection_id,
            query_spec_id,
            selector,
            canonical_seq,
            server,
            room,
            token_json,
            timeout_secs,
        } => (
            TopologyGlobalOpts {
                server,
                room,
                token_json,
                timeout_secs,
                peer_seed,
            },
            QueryCommand::BuildProjection {
                projection_id,
                query_spec_id,
                selector,
                canonical_seq,
            },
        ),
        QuerySub::ListProjections {
            query_spec_id,
            server,
            room,
            token_json,
            timeout_secs,
        } => (
            TopologyGlobalOpts {
                server,
                room,
                token_json,
                timeout_secs,
                peer_seed,
            },
            QueryCommand::ListProjections { query_spec_id },
        ),
        QuerySub::ReadProjection {
            projection_id,
            limit,
            page_token,
            server,
            room,
            token_json,
            timeout_secs,
        } => (
            TopologyGlobalOpts {
                server,
                room,
                token_json,
                timeout_secs,
                peer_seed,
            },
            QueryCommand::ReadProjection {
                projection_id,
                limit,
                page_token,
            },
        ),
        QuerySub::InvalidateProjection {
            projection_id,
            reason,
            server,
            room,
            token_json,
            timeout_secs,
        } => (
            TopologyGlobalOpts {
                server,
                room,
                token_json,
                timeout_secs,
                peer_seed,
            },
            QueryCommand::InvalidateProjection {
                projection_id,
                reason,
            },
        ),
    }
}

fn map_topology(
    sub: TopologySub,
    peer_seed: [u8; 32],
) -> (TopologyGlobalOpts, TopologyCommand) {
    match sub {
        TopologySub::CreateChild {
            parent_room,
            child_room,
            purpose,
            policy,
            created_by,
            parent_checkpoint_file,
            server,
            room,
            token_json,
            timeout_secs,
        } => (
            TopologyGlobalOpts {
                server,
                room,
                token_json,
                timeout_secs,
                peer_seed,
            },
            TopologyCommand::CreateChild {
                parent_room,
                child_room,
                purpose,
                policy,
                created_by,
                parent_checkpoint_file,
            },
        ),
        TopologySub::ListChildren {
            parent_room,
            server,
            room,
            token_json,
            timeout_secs,
        } => (
            TopologyGlobalOpts {
                server,
                room,
                token_json,
                timeout_secs,
                peer_seed,
            },
            TopologyCommand::ListChildren { parent_room },
        ),
        TopologySub::ShowLineage {
            room,
            server,
            token_json,
            timeout_secs,
        } => (
            TopologyGlobalOpts {
                server,
                room: Some(room.clone()),
                token_json,
                timeout_secs,
                peer_seed,
            },
            TopologyCommand::ShowLineage { room },
        ),
        TopologySub::ProposePromotion {
            parent_room,
            child_room,
            child_checkpoint,
            payload_ref,
            idempotency_key,
            server,
            room,
            token_json,
            timeout_secs,
        } => (
            TopologyGlobalOpts {
                server,
                room,
                token_json,
                timeout_secs,
                peer_seed,
            },
            TopologyCommand::ProposePromotion {
                parent_room,
                child_room,
                child_checkpoint,
                payload_ref,
                idempotency_key,
            },
        ),
        TopologySub::ValidatePromotion {
            proposal_id,
            server,
            room,
            token_json,
            timeout_secs,
        } => (
            TopologyGlobalOpts {
                server,
                room,
                token_json,
                timeout_secs,
                peer_seed,
            },
            TopologyCommand::ValidatePromotion { proposal_id },
        ),
        TopologySub::ApplyPromotion {
            proposal_id,
            server,
            room,
            token_json,
            timeout_secs,
        } => (
            TopologyGlobalOpts {
                server,
                room,
                token_json,
                timeout_secs,
                peer_seed,
            },
            TopologyCommand::ApplyPromotion { proposal_id },
        ),
    }
}
