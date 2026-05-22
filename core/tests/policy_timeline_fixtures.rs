use std::collections::BTreeMap;

use activesync_core::{
    MapOp, Op, Policy, PolicyDefault, PolicyRule, PolicyTimelineEntry, SyncError, SyncNode,
    Transaction, replay_with_policy_timeline,
};
use ed25519_dalek::SigningKey;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Fixture {
    name: String,
    timeline: Vec<FixtureTimelineEntry>,
    nodes: Vec<FixtureNode>,
    expect: FixtureExpect,
}

#[derive(Debug, Deserialize)]
struct FixtureTimelineEntry {
    effective_lamport: u64,
    default: FixtureDefault,
    rules: Vec<FixtureRule>,
}

#[derive(Debug, Deserialize)]
enum FixtureDefault {
    AllowAll,
    DenyAll,
}

#[derive(Debug, Deserialize)]
struct FixtureRule {
    path_glob: String,
    can_write_authors: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct FixtureNode {
    author: String,
    lamport: u64,
    parents: Vec<usize>,
    ops: Vec<FixtureSetOp>,
}

#[derive(Debug, Deserialize)]
struct FixtureSetOp {
    key: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct FixtureExpect {
    map: Option<BTreeMap<String, String>>,
    error: Option<String>,
}

fn key_a() -> SigningKey {
    SigningKey::from_bytes(&[0x0Au8; 32])
}

fn key_b() -> SigningKey {
    SigningKey::from_bytes(&[0x0Bu8; 32])
}

fn signing_key_for_label(label: &str) -> SigningKey {
    match label {
        "A" => key_a(),
        "B" => key_b(),
        other => panic!("unsupported author label in fixture: {other}"),
    }
}

fn public_key_for_label(label: &str) -> [u8; 32] {
    signing_key_for_label(label).verifying_key().to_bytes()
}

fn parse_fixture(json: &str) -> Fixture {
    serde_json::from_str(json).expect("fixture JSON must be valid")
}

fn build_timeline(entries: &[FixtureTimelineEntry]) -> Vec<PolicyTimelineEntry> {
    entries
        .iter()
        .map(|entry| {
            let default = match entry.default {
                FixtureDefault::AllowAll => PolicyDefault::AllowAll,
                FixtureDefault::DenyAll => PolicyDefault::DenyAll,
            };

            let rules = entry
                .rules
                .iter()
                .map(|rule| PolicyRule {
                    path_glob: rule.path_glob.clone(),
                    can_write: rule
                        .can_write_authors
                        .iter()
                        .map(|author| public_key_for_label(author))
                        .collect(),
                    can_read: Vec::new(),
                    can_derive: Vec::new(),
                })
                .collect();

            PolicyTimelineEntry {
                effective_lamport: entry.effective_lamport,
                policy: Policy { rules, default },
            }
        })
        .collect()
}

fn build_nodes(nodes: &[FixtureNode]) -> Vec<SyncNode> {
    let mut built: Vec<SyncNode> = Vec::with_capacity(nodes.len());

    for node in nodes {
        let key = signing_key_for_label(&node.author);
        let parents = node
            .parents
            .iter()
            .map(|idx| built[*idx].id)
            .collect::<Vec<_>>();
        let ops = node
            .ops
            .iter()
            .map(|op| Op::Map(MapOp::Set {
                key: op.key.clone(),
                value: op.value.as_bytes().to_vec(),
            }))
            .collect();

        let tx = Transaction {
            author: key.verifying_key().to_bytes(),
            lamport: node.lamport,
            wall_ms: 0,
            ops,
            parents,
        };
        built.push(SyncNode::new_signed(tx, &key));
    }

    built
}

fn run_fixture(json: &str) {
    let fixture = parse_fixture(json);
    let timeline = build_timeline(&fixture.timeline);
    let nodes = build_nodes(&fixture.nodes);
    let replay_result = replay_with_policy_timeline(&nodes, &timeline);

    if let Some(expected_error) = fixture.expect.error {
        assert!(
            replay_result.is_err(),
            "fixture '{}' expected an error result",
            fixture.name
        );

        if expected_error == "PolicyViolation" {
            assert!(
                matches!(replay_result, Err(SyncError::PolicyViolation { .. })),
                "fixture '{}' expected PolicyViolation",
                fixture.name
            );
        }
        return;
    }

    let resolved = replay_result.unwrap_or_else(|err| {
        panic!("fixture '{}' expected success, got error: {err}", fixture.name)
    });

    let expected_map = fixture.expect.map.expect("fixture success case must include map");
    for (key, value) in expected_map {
        assert_eq!(
            resolved.map.get(&key).map(|v| String::from_utf8_lossy(v).to_string()),
            Some(value),
            "fixture '{}' map mismatch for key '{}'",
            fixture.name,
            key
        );
    }
}

#[test]
fn fixture_deny_after_cutover() {
    run_fixture(include_str!("fixtures/policy_timeline/deny_after_cutover.json"));
}

#[test]
fn fixture_allow_authorized_after_cutover() {
    run_fixture(include_str!("fixtures/policy_timeline/allow_authorized_after_cutover.json"));
}

#[test]
fn fixture_multi_transition_order_independent() {
    run_fixture(include_str!("fixtures/policy_timeline/multi_transition_order_independent.json"));
}
