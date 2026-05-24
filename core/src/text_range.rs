use serde::{Deserialize, Serialize};

use crate::op::{OpId, TextOp};

/// Stable logical identity for one character within a canonical text range.
///
/// The persisted range is the canonical unit, but each character still gets a
/// deterministic identity derived from the range's transaction identity plus
/// its offset. That keeps anchoring, cursors, and tombstones stable without
/// generating runtime-local IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextCharId {
    pub base: OpId,
    pub offset: u32,
}

/// Derive the stable identity of the character at `offset` within a persisted
/// text range anchored at `base`.
pub fn derive_text_char_id(base: OpId, offset: u32) -> TextCharId {
    TextCharId { base, offset }
}

/// Phase 2 scaffolding: anchor model for range text operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextRangeAnchor {
    Start,
    End,
    /// Position after an existing character id.
    After(OpId),
    /// Character offset in the visible sequence.
    Offset(usize),
}

/// Phase 2 scaffolding: high-level range operation shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextRangeOp {
    Insert {
        key: String,
        anchor: TextRangeAnchor,
        text: String,
    },
    Delete {
        key: String,
        anchor: TextRangeAnchor,
        len_chars: usize,
    },
}

/// Canonical lowered edit stream before concrete op-id materialization.
///
/// Rule:
/// - first insert in a range is anchored to existing sequence context
/// - subsequent inserts are anchored to the previous inserted char
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoweredTextEdit {
    InsertFirst {
        key: String,
        after_existing: Option<OpId>,
        ch: char,
    },
    InsertNext {
        key: String,
        ch: char,
    },
    Delete {
        key: String,
        target: OpId,
    },
}

/// Canonical lowering rules from phase-2 range shape to deterministic edit stream.
pub fn lower_text_range_op(seq: &[(OpId, char)], op: &TextRangeOp) -> Vec<LoweredTextEdit> {
    match op {
        TextRangeOp::Insert { key, anchor, text } => lower_insert(seq, key, anchor, text),
        TextRangeOp::Delete {
            key,
            anchor,
            len_chars,
        } => lower_delete(seq, key, anchor, *len_chars),
    }
}

/// Materialize canonical lowered edits into concrete char-level `TextOp`s.
///
/// Callers pass the author + next lamport so this function can deterministically
/// chain `InsertNext` operations to the prior inserted op-id.
pub fn materialize_lowered_edits(
    edits: &[LoweredTextEdit],
    author: [u8; 32],
    mut next_lamport: u64,
) -> Vec<TextOp> {
    let mut out = Vec::with_capacity(edits.len());
    let mut previous_inserted: Option<OpId> = None;

    for edit in edits {
        match edit {
            LoweredTextEdit::InsertFirst {
                key,
                after_existing,
                ch,
            } => {
                out.push(TextOp::Insert {
                    key: key.clone(),
                    after: *after_existing,
                    ch: *ch,
                });
                previous_inserted = Some(OpId {
                    lamport: next_lamport,
                    author,
                });
                next_lamport = next_lamport.saturating_add(1);
            }
            LoweredTextEdit::InsertNext { key, ch } => {
                out.push(TextOp::Insert {
                    key: key.clone(),
                    after: previous_inserted,
                    ch: *ch,
                });
                previous_inserted = Some(OpId {
                    lamport: next_lamport,
                    author,
                });
                next_lamport = next_lamport.saturating_add(1);
            }
            LoweredTextEdit::Delete { key, target } => out.push(TextOp::Delete {
                key: key.clone(),
                target: *target,
            }),
        }
    }

    out
}

fn lower_insert(
    seq: &[(OpId, char)],
    key: &str,
    anchor: &TextRangeAnchor,
    text: &str,
) -> Vec<LoweredTextEdit> {
    let mut chars = text.chars();
    let Some(first_ch) = chars.next() else {
        return Vec::new();
    };

    let insertion_index = resolve_anchor_index(seq, anchor);
    let after_existing = if insertion_index == 0 {
        None
    } else {
        Some(seq[insertion_index - 1].0)
    };

    let mut out = Vec::with_capacity(text.chars().count());
    out.push(LoweredTextEdit::InsertFirst {
        key: key.to_string(),
        after_existing,
        ch: first_ch,
    });

    for ch in chars {
        out.push(LoweredTextEdit::InsertNext {
            key: key.to_string(),
            ch,
        });
    }
    out
}

fn lower_delete(
    seq: &[(OpId, char)],
    key: &str,
    anchor: &TextRangeAnchor,
    len_chars: usize,
) -> Vec<LoweredTextEdit> {
    if len_chars == 0 {
        return Vec::new();
    }
    let start = resolve_anchor_index(seq, anchor);
    let end = start.saturating_add(len_chars).min(seq.len());

    seq[start..end]
        .iter()
        .map(|(id, _)| LoweredTextEdit::Delete {
            key: key.to_string(),
            target: *id,
        })
        .collect()
}

fn resolve_anchor_index(seq: &[(OpId, char)], anchor: &TextRangeAnchor) -> usize {
    match anchor {
        TextRangeAnchor::Start => 0,
        TextRangeAnchor::End => seq.len(),
        TextRangeAnchor::Offset(i) => (*i).min(seq.len()),
        TextRangeAnchor::After(id) => seq
            .iter()
            .position(|(existing, _)| existing == id)
            .map(|i| i + 1)
            .unwrap_or(seq.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_id(lamport: u64) -> OpId {
        OpId {
            lamport,
            author: [0x11; 32],
        }
    }

    #[test]
    fn lower_insert_anchors_first_then_chains() {
        let seq = vec![(mk_id(1), 'a'), (mk_id(2), 'b')];
        let op = TextRangeOp::Insert {
            key: "doc".into(),
            anchor: TextRangeAnchor::Offset(1),
            text: "xy".into(),
        };

        let lowered = lower_text_range_op(&seq, &op);
        assert_eq!(
            lowered,
            vec![
                LoweredTextEdit::InsertFirst {
                    key: "doc".into(),
                    after_existing: Some(mk_id(1)),
                    ch: 'x',
                },
                LoweredTextEdit::InsertNext {
                    key: "doc".into(),
                    ch: 'y',
                }
            ]
        );
    }

    #[test]
    fn lower_delete_is_left_to_right_window() {
        let seq = vec![(mk_id(1), 'a'), (mk_id(2), 'b'), (mk_id(3), 'c')];
        let op = TextRangeOp::Delete {
            key: "doc".into(),
            anchor: TextRangeAnchor::Offset(1),
            len_chars: 2,
        };
        let lowered = lower_text_range_op(&seq, &op);
        assert_eq!(
            lowered,
            vec![
                LoweredTextEdit::Delete {
                    key: "doc".into(),
                    target: mk_id(2),
                },
                LoweredTextEdit::Delete {
                    key: "doc".into(),
                    target: mk_id(3),
                }
            ]
        );
    }

    #[test]
    fn materialize_insert_chain_uses_previous_generated_id() {
        let edits = vec![
            LoweredTextEdit::InsertFirst {
                key: "doc".into(),
                after_existing: Some(mk_id(7)),
                ch: 'x',
            },
            LoweredTextEdit::InsertNext {
                key: "doc".into(),
                ch: 'y',
            },
        ];

        let ops = materialize_lowered_edits(&edits, [0xAB; 32], 100);
        assert_eq!(ops.len(), 2);

        match &ops[0] {
            TextOp::Insert { after, ch, .. } => {
                assert_eq!(*after, Some(mk_id(7)));
                assert_eq!(*ch, 'x');
            }
            _ => panic!("expected insert"),
        }

        match &ops[1] {
            TextOp::Insert { after, ch, .. } => {
                assert_eq!(
                    *after,
                    Some(OpId {
                        lamport: 100,
                        author: [0xAB; 32],
                    })
                );
                assert_eq!(*ch, 'y');
            }
            _ => panic!("expected insert"),
        }
    }

    #[test]
    fn derive_text_char_id_is_deterministic_and_offset_based() {
        let base = mk_id(9);
        assert_eq!(derive_text_char_id(base, 0), TextCharId { base, offset: 0 });
        assert_eq!(derive_text_char_id(base, 4), TextCharId { base, offset: 4 });
        assert_ne!(derive_text_char_id(base, 0), derive_text_char_id(base, 1));
    }
}
