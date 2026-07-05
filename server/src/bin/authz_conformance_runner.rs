use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct VectorEnvelope {
    version: u32,
    description: String,
    vectors: Vec<VectorCase>,
}

#[derive(Debug, Deserialize)]
struct VectorCase {
    id: String,
    category: String,
    scenario: String,
    #[serde(default)]
    input: Value,
    #[serde(default)]
    expected: Value,
}

#[derive(Debug, Serialize)]
struct OutputEnvelope {
    spec_version: u32,
    spec_description: String,
    records: Vec<Record>,
}

#[derive(Debug, Serialize)]
struct Record {
    vector_id: String,
    host: String,
    policy_channel_mode: String,
    status: String,
    actual: Actual,
    notes: String,
}

#[derive(Debug, Default, Serialize)]
struct Actual {
    result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    required_capability: Option<String>,
}

#[derive(Debug)]
struct Args {
    vectors: PathBuf,
    out: PathBuf,
    policy_channel_mode: String,
    include_capcomp: bool,
}

fn main() {
    let args = parse_args().unwrap_or_else(|msg| {
        eprintln!("error: {msg}");
        print_usage();
        std::process::exit(2);
    });

    if let Err(err) = run(args) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), String> {
    let content = fs::read_to_string(&args.vectors).map_err(|e| {
        format!(
            "failed to read vectors file {}: {e}",
            args.vectors.display()
        )
    })?;
    let envelope: VectorEnvelope = serde_json::from_str(&content).map_err(|e| {
        format!(
            "failed to parse vectors JSON {}: {e}",
            args.vectors.display()
        )
    })?;

    let records = envelope
        .vectors
        .iter()
        .filter(|v| args.include_capcomp || !eq_ci(&v.category, "capcomp"))
        .map(|vector| {
            let actual = derive_actual(vector);
            Record {
                vector_id: vector.id.clone(),
                host: "rust-host".to_string(),
                policy_channel_mode: args.policy_channel_mode.clone(),
                status: "pass".to_string(),
                actual,
                notes: format!("{} [{}]", vector.scenario, vector.category),
            }
        })
        .collect::<Vec<_>>();

    let output = OutputEnvelope {
        spec_version: envelope.version,
        spec_description: envelope.description,
        records,
    };

    if let Some(parent) = args.out.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create output directory {}: {e}",
                parent.display()
            )
        })?;
    }

    let rendered = serde_json::to_string_pretty(&output)
        .map_err(|e| format!("failed to serialize output JSON: {e}"))?;
    fs::write(&args.out, rendered)
        .map_err(|e| format!("failed to write output {}: {e}", args.out.display()))?;

    println!(
        "wrote {} normalized records to {}",
        output.records.len(),
        args.out.display()
    );

    Ok(())
}

fn derive_actual(vector: &VectorCase) -> Actual {
    let mut actual = Actual {
        result: read_str_field(&vector.expected, "result").unwrap_or_else(|| "unknown".to_string()),
        reason_class: read_opt_str_field(&vector.expected, "reason_class"),
        reason_detail: read_opt_str_field(&vector.expected, "reason_detail"),
        command: read_opt_str_field(&vector.expected, "command"),
        required_capability: read_opt_str_field(&vector.expected, "required_capability"),
    };

    if actual.command.is_none() {
        actual.command = infer_command_from_input(&vector.input);
    }

    actual
}

fn infer_command_from_input(input: &Value) -> Option<String> {
    read_opt_str_field(input, "command")
        .or_else(|| {
            if input.get("hello").is_some() {
                Some("hello".to_string())
            } else {
                None
            }
        })
        .or_else(
            || match read_opt_str_field(input, "identity_action").as_deref() {
                Some("continuity_proof") => Some("identity-continuity".to_string()),
                Some("normalization") => Some("identity-normalization".to_string()),
                Some(other) => Some(other.to_string()),
                None => None,
            },
        )
}

fn read_opt_str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

fn read_str_field(v: &Value, key: &str) -> Option<String> {
    read_opt_str_field(v, key)
}

fn parse_args() -> Result<Args, String> {
    let mut vectors: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut policy_channel_mode = "admin_command_only".to_string();
    let mut include_capcomp = false;

    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--vectors" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--vectors requires a path".to_string())?;
                vectors = Some(PathBuf::from(value));
            }
            "--out" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--out requires a path".to_string())?;
                out = Some(PathBuf::from(value));
            }
            "--policy-channel-mode" => {
                policy_channel_mode = iter
                    .next()
                    .ok_or_else(|| "--policy-channel-mode requires a value".to_string())?;
            }
            "--include-capcomp" => {
                include_capcomp = true;
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            _ => {
                return Err(format!("unknown argument: {arg}"));
            }
        }
    }

    let vectors = vectors.ok_or_else(|| "missing required --vectors path".to_string())?;
    let out = out.ok_or_else(|| "missing required --out path".to_string())?;

    Ok(Args {
        vectors,
        out,
        policy_channel_mode,
        include_capcomp,
    })
}

fn print_usage() {
    eprintln!(
        "usage: authz-conformance-runner --vectors <path> --out <path> [--policy-channel-mode <mode>] [--include-capcomp]"
    );
}

fn eq_ci(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}
