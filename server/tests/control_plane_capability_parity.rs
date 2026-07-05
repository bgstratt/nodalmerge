use nodalmerge_server::ws_handler::required_capability_for_control_plane_command;

/// Canonical control-plane command -> required-capability table.
///
/// This is one half of a cross-runtime parity check: the same table (as data,
/// not code) is asserted against the .NET host's real message-handling path in
/// `nodalmerge-host/tests/NodalMerge.DotNetHost.Tests/ControlPlaneCapabilityParityTests.cs`.
/// If you add, remove, or re-gate a control-plane command in either runtime,
/// update the table in *both* places in the same change, or these two tests
/// will disagree about what "correct" means without either one failing.
///
/// `None` rows are asserted, not skipped: they pin down commands that are
/// deliberately ungated in this runtime today (including ones only wired up
/// as a gated WS command on the .NET side so far). If a future change makes
/// Rust gate one of these, flip the expectation here *and* in the .NET table
/// in the same commit.
///
/// Note this table only covers *gating* (is the right capability required),
/// not whether the command actually does the real thing once authorized.
/// `archive.import`/`archive.export` are a known gap there: gating is correct
/// on both runtimes, but the .NET path (host-core's `HostCommand::ImportArchive`)
/// is a stub, and `archive.export` has no host-core/.NET path at all. See the
/// comments on `HostCommand::ImportArchive` in host-core/src/engine.rs and on
/// the `archive.import` handler in RuntimeProtocolMapper.cs.
const TABLE: &[(&str, Option<&str>)] = &[
    ("set-policy", Some("policy.admin")),
    ("set-room-key", Some("room.admin")),
    ("start-tick", Some("tick.admin")),
    ("stop-tick", Some("tick.admin")),
    ("archive.describe", Some("archive.read")),
    ("archive.validate", Some("archive.admin")),
    ("archive.import", Some("archive.admin")),
    ("archive.export", Some("archive.admin")),
    ("query.register", Some("query.admin")),
    ("projection.build", Some("query.admin")),
    ("projection.invalidate", Some("query.admin")),
    ("projection.read", Some("query.read")),
    ("projection.list", Some("query.read")),
    ("replay.read-range", Some("query.read")),
    ("topology.create-child", Some("topology.admin")),
    ("topology.describe-lineage", Some("topology.admin")),
    ("topology.list-children", Some("topology.admin")),
    ("topology.propose-promotion", Some("topology.admin")),
    ("topology.validate-promotion", Some("topology.admin")),
    ("topology.apply-promotion", Some("topology.admin")),
    // .NET-only today: host-core already has these HostCommand variants, but
    // nodalmerge-server has no WS route for them yet, so they fall through to
    // `None` here. Once a Rust WS route is added for one of these, this is
    // the line to change (and to move out of this comment block).
    ("checkpoint.promote", None),
    ("graph.get-frontier", None),
    ("graph.get-causal-parents", None),
    ("graph.get-canonical-resolution", None),
    ("graph.compute-sync-diff", None),
    // Sanity check: an unrecognized command must never be gated.
    ("not-a-real-command", None),
];

#[test]
fn control_plane_capability_table_matches_rust_implementation() {
    let mut failures = Vec::new();

    for (command, expected) in TABLE {
        let actual = required_capability_for_control_plane_command(command);
        if actual != *expected {
            failures.push(format!(
                "command `{command}`: rust returned {actual:?}, expected {expected:?}"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "control-plane capability table mismatch(es):\n{}\n\nKeep this table and \
         NodalMerge.DotNetHost.Tests/ControlPlaneCapabilityParityTests.cs in sync.",
        failures.join("\n")
    );
}
