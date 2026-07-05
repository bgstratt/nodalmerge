//! Asserts the Rust server's live control-plane capability gating against the
//! canonical registry (`engine/commands/registry.json`, via the
//! `nodalmerge-command-registry` crate). The .NET mirror is
//! `hosts/dotnet/tests/NodalMerge.DotNetHost.Tests/ControlPlaneCapabilityParityTests.cs`,
//! which reads the same file. To add/remove/re-gate a command, change the
//! registry and the implementation together — this test fails on any drift.
//!
//! Registry semantics for this surface: rows with `rust_server: absent` must
//! return `None` (ungated because unrouted); everything else must require
//! exactly the registry's capability. The registry covers *gating* only, not
//! whether a command does the real thing once authorized — see each row's
//! `surfaces`/`notes` for stub status.

use nodalmerge_command_registry::{rows, SurfaceStatus};
use nodalmerge_server::ws_handler::required_capability_for_control_plane_command;

#[test]
fn rust_server_gating_matches_command_registry() {
    let mut failures = Vec::new();

    for row in rows() {
        let actual = required_capability_for_control_plane_command(&row.command);
        let expected = match row.surfaces.rust_server {
            SurfaceStatus::Absent => None,
            SurfaceStatus::Real | SurfaceStatus::Stub => Some(row.required_capability.as_str()),
        };
        if actual != expected {
            failures.push(format!(
                "command `{}`: rust returned {actual:?}, registry expects {expected:?}",
                row.command
            ));
        }
    }

    // Unregistered commands must never be gated.
    if required_capability_for_control_plane_command("not-a-real-command").is_some() {
        failures.push("unknown command `not-a-real-command` is unexpectedly gated".to_string());
    }

    assert!(
        failures.is_empty(),
        "control-plane gating drifted from engine/commands/registry.json:\n{}",
        failures.join("\n")
    );
}
