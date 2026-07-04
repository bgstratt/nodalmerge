//! Debug helper: import a base64 catch-up pack (as dumped from a browser/Node
//! SDK store) into a fresh StateGraph and print every rejection reason.
//!
//! Usage: cargo run -p nodalmerge-host-core --example import_debug -- <pack.b64>

use nodalmerge_core::{StateGraph, SyncNode};

fn main() {
    let path = std::env::args().nth(1).expect("usage: import_debug <pack.b64>");
    let b64 = std::fs::read_to_string(&path).expect("read pack file");
    let bytes = base64_decode(b64.trim()).expect("base64 decode");
    let nodes: Vec<SyncNode> = nodalmerge_core::unpack_nodes(&bytes).expect("unpack nodes");
    println!("unpacked {} nodes", nodes.len());
    for n in &nodes {
        println!(
            "  node {} lamport={} parents={:?} ops={}",
            n.id.to_hex(),
            n.lamport(),
            n.transaction.parents.iter().map(|p| p.to_hex()[..8].to_string()).collect::<Vec<_>>(),
            n.transaction.ops.len()
        );
    }

    let mut graph = StateGraph::new();
    let result = graph.apply_remote_batch(nodes);
    println!("accepted: {}", result.accepted.len());
    println!("rejected: {}", result.rejected.len());
    for (id, err) in &result.rejected {
        println!("  reject {}: {:?}", &id.to_hex()[..12], err);
    }
}

fn base64_decode(s: &str) -> Result<Vec<u8>, ()> {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255u8; 256];
    for (i, &c) in TABLE.iter().enumerate() {
        lookup[c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &c in s.as_bytes() {
        if c == b'=' || c == b'\n' || c == b'\r' {
            continue;
        }
        let v = lookup[c as usize];
        if v == 255 {
            return Err(());
        }
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Ok(out)
}
