/// Escape a room id into a filesystem-safe directory name.
pub(crate) fn sanitize_room_id(room_id: &str) -> String {
    let mut out = String::with_capacity(room_id.len());
    for b in room_id.as_bytes() {
        if b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-' {
            out.push(*b as char);
        } else {
            out.push('_');
            out.push_str(&format!("{:02X}", b));
        }
    }
    out
}
