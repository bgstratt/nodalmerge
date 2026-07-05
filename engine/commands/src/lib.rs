//! Canonical control-plane command-surface registry.
//!
//! `registry.json` (embedded at compile time) is the single source of truth
//! for which WS control-plane commands exist, what capability each requires,
//! and how trustworthy each surface's implementation is (`real`/`stub`/
//! `absent`). The per-runtime parity tests assert their host's *live*
//! behavior against this data:
//!
//! - `server/server/tests/control_plane_capability_parity.rs`
//! - `hosts/dotnet/tests/NodalMerge.DotNetHost.Tests/ControlPlaneCapabilityParityTests.cs`
//!   (reads the same registry.json, linked into the test project)
//!
//! [`ws_command_name`] is an exhaustive match over [`HostCommand`]: adding an
//! engine command without deciding its registry status is a compile error,
//! not a silent gap.

use nodalmerge_host_core::api::HostCommand;
use serde::Deserialize;
use std::sync::OnceLock;

pub const REGISTRY_JSON: &str = include_str!("../registry.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SurfaceStatus {
    /// Implemented and trustworthy on this surface.
    Real,
    /// Wire-compatible placeholder that fakes success — do not rely on it.
    Stub,
    /// No route/implementation on this surface.
    Absent,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Surfaces {
    pub rust_server: SurfaceStatus,
    pub host_core: SurfaceStatus,
    pub dotnet_host: SurfaceStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Gaps on this row (if any) are pending work.
    #[default]
    Active,
    /// Gaps on this row are a recorded decision (plan S3), not an accident.
    Deferred,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommandRow {
    pub command: String,
    pub required_capability: String,
    #[serde(default)]
    pub scope: Scope,
    pub surfaces: Surfaces,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Registry {
    version: u32,
    commands: Vec<CommandRow>,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        serde_json::from_str(REGISTRY_JSON).expect("engine/commands/registry.json must parse")
    })
}

/// All registry rows, in file order.
pub fn rows() -> &'static [CommandRow] {
    &registry().commands
}

/// Registry schema version.
pub fn version() -> u32 {
    registry().version
}

/// Look up a row by WS command name.
pub fn row(command: &str) -> Option<&'static CommandRow> {
    rows().iter().find(|r| r.command == command)
}

/// Map an engine command to its WS control-plane command name, if it has one.
///
/// This match is deliberately exhaustive with no wildcard arm: adding a
/// `HostCommand` variant fails compilation here until you decide whether it
/// is a control-plane command (add a registry row and map it) or data-plane
/// (add it to the `None` arm below). Either way the decision is recorded.
pub fn ws_command_name(command: &HostCommand) -> Option<&'static str> {
    match command {
        HostCommand::SetPolicy { .. } => Some("set-policy"),
        HostCommand::SetRoomKey { .. } => Some("set-room-key"),
        HostCommand::DescribeArchive { .. } => Some("archive.describe"),
        HostCommand::ValidateArchive { .. } => Some("archive.validate"),
        HostCommand::ImportArchive { .. } => Some("archive.import"),
        HostCommand::RegisterQuerySpec { .. } => Some("query.register"),
        HostCommand::BuildProjection { .. } => Some("projection.build"),
        HostCommand::InvalidateProjection { .. } => Some("projection.invalidate"),
        HostCommand::ReadProjection { .. } => Some("projection.read"),
        HostCommand::ListProjections { .. } => Some("projection.list"),
        HostCommand::CreateTopologyChild { .. } => Some("topology.create-child"),
        HostCommand::DescribeRoomLineage { .. } => Some("topology.describe-lineage"),
        HostCommand::ListTopologyChildren { .. } => Some("topology.list-children"),
        HostCommand::ProposeTopologyPromotion { .. } => Some("topology.propose-promotion"),
        HostCommand::ValidateTopologyPromotion { .. } => Some("topology.validate-promotion"),
        HostCommand::ApplyTopologyPromotion { .. } => Some("topology.apply-promotion"),
        HostCommand::ReplayReadRange { .. } => Some("replay.read-range"),
        HostCommand::PromoteCheckpointToGraph { .. } => Some("checkpoint.promote"),
        HostCommand::GetFrontier => Some("graph.get-frontier"),
        HostCommand::GetCausalParents { .. } => Some("graph.get-causal-parents"),
        HostCommand::GetCanonicalResolution => Some("graph.get-canonical-resolution"),
        HostCommand::ComputeSyncDiff { .. } => Some("graph.compute-sync-diff"),
        // Data-plane / session-plane commands: ungated by design.
        HostCommand::EnsureRoom
        | HostCommand::OpenSession { .. }
        | HostCommand::CloseSession { .. }
        | HostCommand::ClientHello { .. }
        | HostCommand::MapSet { .. }
        | HostCommand::MapDelete { .. }
        | HostCommand::MapGet { .. }
        | HostCommand::MapAll { .. }
        | HostCommand::TextInsert { .. }
        | HostCommand::TextDelete { .. }
        | HostCommand::TextGet { .. }
        | HostCommand::TextGetCanonical { .. }
        | HostCommand::ListPush { .. }
        | HostCommand::ListInsert { .. }
        | HostCommand::ListDelete { .. }
        | HostCommand::ListMove { .. }
        | HostCommand::ListUpdate { .. }
        | HostCommand::ListGet { .. }
        | HostCommand::BlobSet { .. }
        | HostCommand::BlobGet { .. }
        | HostCommand::BlobGetMany { .. }
        | HostCommand::RequestUpload { .. }
        | HostCommand::BlobRequest { .. }
        | HostCommand::PresenceSet { .. }
        | HostCommand::PresenceGetAll
        | HostCommand::PresenceSweep { .. }
        | HostCommand::Subscribe { .. }
        | HostCommand::ImportPack { .. }
        | HostCommand::RequestServerPack { .. }
        | HostCommand::MstRequest { .. }
        | HostCommand::MstDone { .. }
        | HostCommand::GetRecentConflicts { .. }
        | HostCommand::RelayPeerSignal { .. }
        | HostCommand::InspectPack { .. }
        | HostCommand::Noop => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_parses_and_is_v1() {
        assert_eq!(version(), 1);
        assert!(!rows().is_empty());
    }

    #[test]
    fn no_duplicate_command_names() {
        let mut seen = std::collections::HashSet::new();
        for r in rows() {
            assert!(seen.insert(r.command.as_str()), "duplicate row: {}", r.command);
        }
    }

    #[test]
    fn every_row_has_nonempty_capability() {
        for r in rows() {
            assert!(
                !r.required_capability.is_empty(),
                "row {} has empty required_capability",
                r.command
            );
        }
    }

    #[test]
    fn every_stub_or_asymmetric_row_documents_why() {
        for r in rows() {
            let has_gap = [r.surfaces.rust_server, r.surfaces.host_core, r.surfaces.dotnet_host]
                .iter()
                .any(|s| *s != SurfaceStatus::Real);
            if has_gap {
                assert!(
                    r.notes.as_deref().is_some_and(|n| !n.is_empty()),
                    "row {} has a stub/absent surface but no notes explaining it",
                    r.command
                );
            }
        }
    }

    #[test]
    fn every_mapped_host_command_has_a_registry_row() {
        // Sample one value per control-plane-mapped variant to exercise the
        // mapping. (The compile-time exhaustiveness of ws_command_name is the
        // real guard; this checks the mapped names resolve to rows.)
        let mapped = [
            "set-policy",
            "set-room-key",
            "archive.describe",
            "archive.validate",
            "archive.import",
            "query.register",
            "projection.build",
            "projection.invalidate",
            "projection.read",
            "projection.list",
            "topology.create-child",
            "topology.describe-lineage",
            "topology.list-children",
            "topology.propose-promotion",
            "topology.validate-promotion",
            "topology.apply-promotion",
            "checkpoint.promote",
            "graph.get-frontier",
            "graph.get-causal-parents",
            "graph.get-canonical-resolution",
            "graph.compute-sync-diff",
        ];
        for name in mapped {
            assert!(row(name).is_some(), "ws_command_name maps to unregistered command: {name}");
        }
    }

    #[test]
    fn host_core_absent_rows_have_no_host_command_mapping() {
        // Rows marked host_core: absent must not claim an engine mapping.
        for r in rows() {
            if r.surfaces.host_core == SurfaceStatus::Absent {
                assert!(
                    ["archive.export", "replay.read-range"].contains(&r.command.as_str()),
                    "unexpected host_core-absent row: {} (add it here and to the docs if intentional)",
                    r.command
                );
            }
        }
    }
}
